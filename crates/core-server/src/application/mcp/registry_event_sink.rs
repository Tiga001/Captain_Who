use std::collections::VecDeque;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use mycopilot_core::storage::service::McpStartupActionTerminalOutcome;
use mycopilot_core::AgentMcpServerScope;
use mycopilot_mcp_client::{McpEvent, McpEventSink, McpRegistryChangeKind, McpServerScope};
use tokio::sync::broadcast;

use crate::adapters::mcp_runtime::McpRegistrySecurityGate;
use crate::application::agent::{
    AgentService, McpActionInvalidationSummary, McpActionInvalidationTarget,
};

const MAX_DEFERRED_INVALIDATIONS: usize = 128;
const MCP_SAFE_EVENT_CAPACITY: usize = 256;
const REGISTRY_SECURITY_GATE_ERROR: &str =
    "MCP Registry approval state requires fail-closed reconciliation";

trait McpApprovalInvalidator: Send + Sync {
    fn invalidate_server(
        &self,
        target: &McpActionInvalidationTarget,
    ) -> Result<McpActionInvalidationSummary, String>;

    fn invalidate_all(&self) -> Result<McpActionInvalidationSummary, String>;
}

impl McpApprovalInvalidator for AgentService {
    fn invalidate_server(
        &self,
        target: &McpActionInvalidationTarget,
    ) -> Result<McpActionInvalidationSummary, String> {
        self.invalidate_mcp_actions_for_server(
            target,
            McpStartupActionTerminalOutcome::PolicyDenied,
        )
    }

    fn invalidate_all(&self) -> Result<McpActionInvalidationSummary, String> {
        self.invalidate_all_mcp_actions(McpStartupActionTerminalOutcome::PolicyDenied)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RegistryInvalidation {
    Server(McpActionInvalidationTarget),
    All,
}

struct SinkState {
    invalidator: Option<Arc<dyn McpApprovalInvalidator>>,
    deferred: VecDeque<RegistryInvalidation>,
}

impl fmt::Debug for SinkState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SinkState")
            .field("bound", &self.invalidator.is_some())
            .field("deferred_count", &self.deferred.len())
            .finish()
    }
}

/// Bridges safe MCP Registry lifecycle events into durable Agent approval invalidation.
///
/// Bootstrap can construct this sink before it has built the final `AgentService`. Relevant
/// mutations received before `bind` are retained as typed, secret-free identities. A bounded
/// overflow collapses to a fail-closed global invalidation.
#[derive(Debug)]
pub(crate) struct McpAgentRegistryEventSink {
    state: Mutex<SinkState>,
    event_watermark: AtomicU64,
    failed_watermark: AtomicU64,
    safe_events: broadcast::Sender<McpEvent>,
}

impl McpAgentRegistryEventSink {
    pub(crate) fn new() -> Self {
        let (safe_events, _) = broadcast::channel(MCP_SAFE_EVENT_CAPACITY);
        Self {
            state: Mutex::new(SinkState {
                invalidator: None,
                deferred: VecDeque::new(),
            }),
            event_watermark: AtomicU64::new(0),
            failed_watermark: AtomicU64::new(0),
            safe_events,
        }
    }

    pub(crate) fn subscribe_safe_events(&self) -> broadcast::Receiver<McpEvent> {
        self.safe_events.subscribe()
    }

    pub(crate) fn bind(&self, agent_service: AgentService) -> Result<(), String> {
        self.bind_invalidator(Arc::new(agent_service))
    }

    fn bind_invalidator(&self, invalidator: Arc<dyn McpApprovalInvalidator>) -> Result<(), String> {
        let deferred = {
            let mut state = self.state.lock().map_err(|_| {
                let watermark = self.next_watermark();
                self.trip(watermark);
                "MCP Registry event sink lock is unavailable".to_string()
            })?;
            if state.invalidator.is_some() {
                return Err("MCP Registry event sink is already bound".to_string());
            }
            state.invalidator = Some(Arc::clone(&invalidator));
            std::mem::take(&mut state.deferred)
        };

        let mut failed = false;
        for invalidation in deferred {
            let watermark = self.next_watermark();
            if apply_invalidation(invalidator.as_ref(), &invalidation).is_err() {
                self.trip(watermark);
                failed = true;
            }
        }
        if failed {
            Err("one or more deferred MCP approval invalidations failed".to_string())
        } else {
            Ok(())
        }
    }

    fn dispatch(&self, invalidation: RegistryInvalidation) {
        let watermark = self.next_watermark();
        let invalidator = {
            let Ok(mut state) = self.state.lock() else {
                self.trip(watermark);
                eprintln!("MCP Registry event could not be applied safely");
                return;
            };
            if let Some(invalidator) = &state.invalidator {
                Arc::clone(invalidator)
            } else {
                enqueue_deferred(&mut state.deferred, invalidation);
                return;
            }
        };
        if apply_invalidation(invalidator.as_ref(), &invalidation).is_err() {
            self.trip(watermark);
            eprintln!("MCP Registry approval invalidation failed");
        }
    }

    fn next_watermark(&self) -> u64 {
        self.event_watermark
            .fetch_add(1, Ordering::AcqRel)
            .saturating_add(1)
    }

    fn trip(&self, watermark: u64) {
        self.failed_watermark
            .fetch_max(watermark.max(1), Ordering::AcqRel);
    }

    /// Explicitly performs a full durable approval reconciliation and clears
    /// the sticky gate only if no newer Registry event or invalidation failure
    /// raced with that reconciliation.
    #[allow(dead_code)]
    pub(crate) fn reconcile_fail_closed(&self) -> Result<(), String> {
        let failed = self.failed_watermark.load(Ordering::Acquire);
        if failed == 0 {
            return Ok(());
        }
        let target_watermark = self.event_watermark.load(Ordering::Acquire);
        let invalidator = {
            let state = self.state.lock().map_err(|_| {
                self.trip(target_watermark.max(1));
                REGISTRY_SECURITY_GATE_ERROR.to_string()
            })?;
            state
                .invalidator
                .as_ref()
                .cloned()
                .ok_or_else(|| REGISTRY_SECURITY_GATE_ERROR.to_string())?
        };
        if invalidator.invalidate_all().is_err() {
            self.trip(target_watermark.max(1));
            return Err(REGISTRY_SECURITY_GATE_ERROR.to_string());
        }
        if self.event_watermark.load(Ordering::Acquire) != target_watermark {
            self.trip(self.event_watermark.load(Ordering::Acquire).max(1));
            return Err(REGISTRY_SECURITY_GATE_ERROR.to_string());
        }
        self.failed_watermark
            .compare_exchange(failed, 0, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| ())
            .map_err(|_| REGISTRY_SECURITY_GATE_ERROR.to_string())
    }
}

impl Default for McpAgentRegistryEventSink {
    fn default() -> Self {
        Self::new()
    }
}

impl McpEventSink for McpAgentRegistryEventSink {
    fn emit(&self, event: McpEvent) {
        if let Some(invalidation) = registry_invalidation(event.clone()) {
            self.dispatch(invalidation);
        }
        // `McpEvent` is already a protocol-owned safe projection. A bounded
        // broadcast channel lets the management notification adapter coalesce
        // status invalidations without coupling approval invalidation to an
        // unbounded Renderer queue.
        let _ = self.safe_events.send(event);
    }
}

impl McpRegistrySecurityGate for McpAgentRegistryEventSink {
    fn ensure_reconciled(&self) -> Result<(), String> {
        if self.failed_watermark.load(Ordering::Acquire) == 0 {
            Ok(())
        } else {
            Err(REGISTRY_SECURITY_GATE_ERROR.to_string())
        }
    }
}

fn enqueue_deferred(
    deferred: &mut VecDeque<RegistryInvalidation>,
    invalidation: RegistryInvalidation,
) {
    if matches!(invalidation, RegistryInvalidation::All)
        || deferred.len() >= MAX_DEFERRED_INVALIDATIONS
    {
        deferred.clear();
        deferred.push_back(RegistryInvalidation::All);
        return;
    }
    if deferred
        .iter()
        .any(|queued| matches!(queued, RegistryInvalidation::All))
    {
        return;
    }
    deferred.push_back(invalidation);
}

fn registry_invalidation(event: McpEvent) -> Option<RegistryInvalidation> {
    match event {
        McpEvent::RegistryChanged {
            kind: McpRegistryChangeKind::Added,
            ..
        } => None,
        McpEvent::RegistryChanged {
            kind: McpRegistryChangeKind::Updated,
            revision,
            server_id,
            ..
        } => Some(RegistryInvalidation::Server(
            McpActionInvalidationTarget::server(server_id).prior_to_registry_revision(revision),
        )),
        McpEvent::RegistryChanged {
            kind: McpRegistryChangeKind::Removed,
            revision,
            server_id,
            scope,
            config_digest,
            ..
        } => {
            let target = McpActionInvalidationTarget::server(server_id)
                .with_source_config_digest(config_digest)
                .prior_to_registry_revision(revision);
            let target = agent_scope(scope)
                .map(|scope| target.clone().with_scope(scope))
                .unwrap_or(target);
            Some(RegistryInvalidation::Server(target))
        }
        McpEvent::RegistryReconciliationRequired { .. } => Some(RegistryInvalidation::All),
        McpEvent::CatalogChanged {
            server_id,
            generation,
            config_epoch,
            config_digest,
            ..
        } => Some(RegistryInvalidation::Server(
            McpActionInvalidationTarget::server(server_id)
                .with_source_config_digest(config_digest)
                .with_source_config_epoch(config_epoch)
                .prior_to_catalog_generation(generation),
        )),
        McpEvent::ServerStateChanged { .. }
        | McpEvent::ServerError { .. }
        | McpEvent::ServerExited { .. } => None,
        _ => None,
    }
}

fn agent_scope(scope: McpServerScope) -> Option<AgentMcpServerScope> {
    match scope {
        McpServerScope::Builtin => Some(AgentMcpServerScope::Builtin),
        McpServerScope::User => Some(AgentMcpServerScope::User),
        McpServerScope::Project { project_id } => Some(AgentMcpServerScope::Project { project_id }),
        McpServerScope::Plugin { plugin_id } => Some(AgentMcpServerScope::Plugin { plugin_id }),
        McpServerScope::Managed => Some(AgentMcpServerScope::Managed),
        _ => None,
    }
}

fn apply_invalidation(
    invalidator: &dyn McpApprovalInvalidator,
    invalidation: &RegistryInvalidation,
) -> Result<McpActionInvalidationSummary, String> {
    match invalidation {
        RegistryInvalidation::Server(target) => invalidator.invalidate_server(target),
        RegistryInvalidation::All => invalidator.invalidate_all(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Mutex;

    use mycopilot_mcp_client::{McpConfigDigest, McpConfigEpoch, McpServerId};

    #[derive(Default)]
    struct RecordingInvalidator {
        calls: Mutex<Vec<RegistryInvalidation>>,
    }

    impl RecordingInvalidator {
        fn calls(&self) -> Vec<RegistryInvalidation> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl McpApprovalInvalidator for RecordingInvalidator {
        fn invalidate_server(
            &self,
            target: &McpActionInvalidationTarget,
        ) -> Result<McpActionInvalidationSummary, String> {
            self.calls
                .lock()
                .unwrap()
                .push(RegistryInvalidation::Server(target.clone()));
            Ok(McpActionInvalidationSummary::default())
        }

        fn invalidate_all(&self) -> Result<McpActionInvalidationSummary, String> {
            self.calls.lock().unwrap().push(RegistryInvalidation::All);
            Ok(McpActionInvalidationSummary::default())
        }
    }

    fn digest(value: char) -> McpConfigDigest {
        value.to_string().repeat(64).parse().unwrap()
    }

    #[test]
    fn updated_before_bind_invalidates_the_whole_server() {
        let sink = McpAgentRegistryEventSink::new();
        let server_id = McpServerId::new();
        sink.emit(McpEvent::RegistryChanged {
            revision: 2,
            kind: McpRegistryChangeKind::Updated,
            server_id,
            scope: McpServerScope::Plugin {
                plugin_id: "plugin-safe-id".to_string(),
            },
            config_digest: digest('a'),
            config_epoch: McpConfigEpoch::new(),
        });
        let invalidator = Arc::new(RecordingInvalidator::default());
        sink.bind_invalidator(invalidator.clone()).unwrap();

        assert_eq!(
            invalidator.calls(),
            vec![RegistryInvalidation::Server(
                McpActionInvalidationTarget::server(server_id).prior_to_registry_revision(2)
            )]
        );
    }

    #[test]
    fn removed_after_bind_uses_the_old_scope_and_digest() {
        let sink = McpAgentRegistryEventSink::new();
        let invalidator = Arc::new(RecordingInvalidator::default());
        sink.bind_invalidator(invalidator.clone()).unwrap();
        let server_id = McpServerId::new();
        let config_digest = digest('b');
        sink.emit(McpEvent::RegistryChanged {
            revision: 3,
            kind: McpRegistryChangeKind::Removed,
            server_id,
            scope: McpServerScope::Project {
                project_id: "project-safe-id".to_string(),
            },
            config_digest: config_digest.clone(),
            config_epoch: McpConfigEpoch::new(),
        });

        assert_eq!(
            invalidator.calls(),
            vec![RegistryInvalidation::Server(
                McpActionInvalidationTarget::server(server_id)
                    .with_scope(AgentMcpServerScope::Project {
                        project_id: "project-safe-id".to_string()
                    })
                    .with_source_config_digest(config_digest)
                    .prior_to_registry_revision(3)
            )]
        );
    }

    #[test]
    fn catalog_change_invalidates_only_the_prior_generation_of_the_same_config_epoch() {
        let sink = McpAgentRegistryEventSink::new();
        let invalidator = Arc::new(RecordingInvalidator::default());
        sink.bind_invalidator(invalidator.clone()).unwrap();
        let server_id = McpServerId::new();
        let config_digest = digest('e');
        let config_epoch = McpConfigEpoch::new();
        sink.emit(McpEvent::CatalogChanged {
            server_id,
            sequence: 7,
            generation: 4,
            config_epoch,
            registry_revision: 9,
            config_digest: config_digest.clone(),
            completeness: mycopilot_mcp_client::McpCatalogCompleteness::Complete,
            tool_count: 3,
        });

        assert_eq!(
            invalidator.calls(),
            vec![RegistryInvalidation::Server(
                McpActionInvalidationTarget::server(server_id)
                    .with_source_config_digest(config_digest)
                    .with_source_config_epoch(config_epoch)
                    .prior_to_catalog_generation(4)
            )]
        );
    }

    #[test]
    fn registry_gap_fails_closed_for_all_mcp_approvals() {
        let sink = McpAgentRegistryEventSink::new();
        sink.emit(McpEvent::RegistryReconciliationRequired {
            skipped_changes: 129,
        });
        let invalidator = Arc::new(RecordingInvalidator::default());
        sink.bind_invalidator(invalidator.clone()).unwrap();
        assert_eq!(invalidator.calls(), vec![RegistryInvalidation::All]);
    }

    struct ToggleInvalidator {
        fail: AtomicBool,
        calls: Mutex<Vec<RegistryInvalidation>>,
    }

    impl ToggleInvalidator {
        fn new(fail: bool) -> Self {
            Self {
                fail: AtomicBool::new(fail),
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    impl McpApprovalInvalidator for ToggleInvalidator {
        fn invalidate_server(
            &self,
            target: &McpActionInvalidationTarget,
        ) -> Result<McpActionInvalidationSummary, String> {
            self.calls
                .lock()
                .unwrap()
                .push(RegistryInvalidation::Server(target.clone()));
            if self.fail.load(Ordering::Acquire) {
                Err("fixed fixture failure".to_string())
            } else {
                Ok(McpActionInvalidationSummary::default())
            }
        }

        fn invalidate_all(&self) -> Result<McpActionInvalidationSummary, String> {
            self.calls.lock().unwrap().push(RegistryInvalidation::All);
            if self.fail.load(Ordering::Acquire) {
                Err("fixed fixture failure".to_string())
            } else {
                Ok(McpActionInvalidationSummary::default())
            }
        }
    }

    #[test]
    fn invalidation_failure_trips_a_sticky_gate_until_explicit_full_reconciliation() {
        let sink = McpAgentRegistryEventSink::new();
        let invalidator = Arc::new(ToggleInvalidator::new(true));
        sink.bind_invalidator(invalidator.clone()).unwrap();
        let server_id = McpServerId::new();
        let updated = || McpEvent::RegistryChanged {
            revision: 4,
            kind: McpRegistryChangeKind::Updated,
            server_id,
            scope: McpServerScope::User,
            config_digest: digest('c'),
            config_epoch: McpConfigEpoch::new(),
        };

        sink.emit(updated());
        assert!(sink.ensure_reconciled().is_err());

        invalidator.fail.store(false, Ordering::Release);
        sink.emit(updated());
        assert!(
            sink.ensure_reconciled().is_err(),
            "an ordinary successful event must not clear the sticky gate"
        );

        sink.reconcile_fail_closed().unwrap();
        sink.ensure_reconciled().unwrap();
        assert!(matches!(
            invalidator.calls.lock().unwrap().last(),
            Some(RegistryInvalidation::All)
        ));
    }

    #[test]
    fn deferred_invalidation_failure_trips_gate_and_makes_bind_fail_closed() {
        let sink = McpAgentRegistryEventSink::new();
        sink.emit(McpEvent::RegistryChanged {
            revision: 9,
            kind: McpRegistryChangeKind::Updated,
            server_id: McpServerId::new(),
            scope: McpServerScope::User,
            config_digest: digest('d'),
            config_epoch: McpConfigEpoch::new(),
        });
        let invalidator = Arc::new(ToggleInvalidator::new(true));

        assert!(sink.bind_invalidator(invalidator).is_err());
        assert!(sink.ensure_reconciled().is_err());
    }
}
