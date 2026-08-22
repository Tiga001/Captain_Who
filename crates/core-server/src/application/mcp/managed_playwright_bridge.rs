use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use mycopilot_mcp_client::{
    mcp_schema_digest, BoxMcpFuture, InMemoryMcpRegistry, McpActiveCallId, McpApprovalMode,
    McpCancellationToken, McpCatalogCompleteness, McpCatalogToolCall, McpCatalogToolCallIdentity,
    McpConnectionManager, McpConnectionState, McpConnector, McpDispatchCertainty,
    McpDispatchTracker, McpError, McpHostBridgeConfig, McpInvocationId, McpLifecycleKind,
    McpManagerPolicy, McpModelCallId, McpOutcomeUnknownReason, McpPeer, McpProtocolSnapshot,
    McpRegistry, McpServerConfig, McpServerId, McpServerScope, McpToolCall, McpToolId, McpToolPage,
    McpToolResult, McpTransportConfig, McpTrustLevel,
};
use mycopilot_protocol_rs::{
    ManagedPlaywrightAuthorizationContext, ManagedPlaywrightBridgeErrorCode,
    ManagedPlaywrightCancelNotification, ManagedPlaywrightCancelReason, ManagedPlaywrightCommand,
    ManagedPlaywrightCommandNotification, ManagedPlaywrightCompletionInput,
    ManagedPlaywrightCompletionOutcome, ManagedPlaywrightDispatchCertainty,
    ManagedPlaywrightDispatchPhase, ManagedPlaywrightDispatchPhaseInput,
    ManagedPlaywrightPrepareSensitiveToolInput, ManagedPlaywrightSensitiveBindingReleaseReason,
    MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION, MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
    MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::task::{JoinHandle, JoinSet};
use uuid::{Uuid, Version};

#[cfg(test)]
use super::playwright_manifest::load_playwright_browser_manifest;
use super::playwright_manifest::{
    load_playwright_browser_contract, BROWSER_AUTOMATION_MANAGED_SERVER_ID,
};

pub(crate) const MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL: &str = "builtin.playwright.v1";
const MAX_PENDING_REQUESTS: usize = 8;
const MANAGED_PLAYWRIGHT_COMPLETION_GRACE: Duration = Duration::from_millis(100);
pub(crate) const DEFAULT_MANAGED_PLAYWRIGHT_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const STATE_CONNECTING: u8 = 0;
const STATE_READY: u8 = 1;
const STATE_CLOSING: u8 = 2;
const STATE_CLOSED: u8 = 3;
const STATE_FAILED: u8 = 4;

type ManagedPlaywrightIdleFuture = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;
type ManagedPlaywrightIdleSleeper =
    Arc<dyn Fn(Duration) -> ManagedPlaywrightIdleFuture + Send + Sync>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PendingOperation {
    Connect,
    ListTools,
    PrepareSensitiveTool,
    CallTool,
    Close,
}

struct PendingRequest {
    operation: PendingOperation,
    completion: oneshot::Sender<ManagedPlaywrightCompletionOutcome>,
    manager_dispatch: Option<McpDispatchTracker>,
    main_dispatch: Option<McpDispatchTracker>,
}

impl PendingRequest {
    fn dispatch_certainty(&self) -> ManagedPlaywrightDispatchCertainty {
        self.main_dispatch
            .as_ref()
            .map(McpDispatchTracker::certainty)
            .map(managed_dispatch_certainty)
            .unwrap_or(ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched)
    }

    fn acknowledge_phase(&self, phase: ManagedPlaywrightDispatchPhase) {
        let Some(main_dispatch) = self.main_dispatch.as_ref() else {
            return;
        };
        match phase {
            ManagedPlaywrightDispatchPhase::PreDispatch => main_dispatch.mark_dispatching(),
            ManagedPlaywrightDispatchPhase::PossiblyDispatched => {
                main_dispatch.mark_request_queued()
            }
            ManagedPlaywrightDispatchPhase::ResponseReceived => {
                main_dispatch.mark_response_received();
                if let Some(manager_dispatch) = self.manager_dispatch.as_ref() {
                    manager_dispatch.mark_response_received();
                }
            }
        }
    }

    fn acknowledge_completion(&self, outcome: &ManagedPlaywrightCompletionOutcome) {
        if self.operation != PendingOperation::CallTool {
            return;
        }
        let phase = match outcome {
            ManagedPlaywrightCompletionOutcome::ToolCalled { .. } => {
                Some(ManagedPlaywrightDispatchPhase::ResponseReceived)
            }
            ManagedPlaywrightCompletionOutcome::Error {
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::PossiblyDispatched,
                ..
            } => Some(ManagedPlaywrightDispatchPhase::PossiblyDispatched),
            ManagedPlaywrightCompletionOutcome::Error {
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::ResponseReceived,
                ..
            } => Some(ManagedPlaywrightDispatchPhase::ResponseReceived),
            _ => None,
        };
        if let Some(phase) = phase {
            self.acknowledge_phase(phase);
        }
    }

    fn completion_is_monotonic(&self, outcome: &ManagedPlaywrightCompletionOutcome) -> bool {
        if self.operation != PendingOperation::CallTool {
            return true;
        }
        let completion_certainty = match outcome {
            ManagedPlaywrightCompletionOutcome::ToolCalled { .. } => {
                ManagedPlaywrightDispatchCertainty::ResponseReceived
            }
            ManagedPlaywrightCompletionOutcome::Error {
                dispatch_certainty, ..
            } => *dispatch_certainty,
            _ => return false,
        };
        dispatch_certainty_rank(completion_certainty)
            >= dispatch_certainty_rank(self.dispatch_certainty())
    }
}

/// Strict, bounded reverse command bus from the Rust Core to Electron Main.
///
/// The bus emits only the reviewed managed-Playwright methods. It owns no browser/CDP identity and
/// it never logs command arguments or results. Completion is one-shot and keyed by canonical UUIDv4.
pub(crate) struct ManagedPlaywrightHostBridge {
    server_id: McpServerId,
    outbound: StdMutex<Option<mpsc::UnboundedSender<Value>>>,
    pending: StdMutex<HashMap<Uuid, PendingRequest>>,
    authorization_contexts:
        StdMutex<HashMap<McpInvocationId, ManagedPlaywrightAuthorizationContext>>,
    host_image_publish_paths: StdMutex<HashMap<String, String>>,
    closed: AtomicBool,
}

/// Last-resort cleanup when the async request future itself is dropped (for example, when an outer
/// Manager settlement budget is shorter than this bridge's completion grace). Normal settlement
/// disarms the guard after Main completion or explicit pending removal.
struct PendingRequestCleanup<'a> {
    bridge: &'a ManagedPlaywrightHostBridge,
    request_id: Uuid,
    armed: bool,
}

impl PendingRequestCleanup<'_> {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for PendingRequestCleanup<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.bridge.remove_pending(self.request_id);
            self.bridge
                .send_cancel(self.request_id, ManagedPlaywrightCancelReason::Shutdown);
        }
    }
}

impl std::fmt::Debug for ManagedPlaywrightHostBridge {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let pending_count = self
            .pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0);
        formatter
            .debug_struct("ManagedPlaywrightHostBridge")
            .field("server_id", &self.server_id)
            .field("pending_count", &pending_count)
            .field("closed", &self.closed.load(Ordering::Acquire))
            .finish_non_exhaustive()
    }
}

impl ManagedPlaywrightHostBridge {
    pub(crate) fn new(server_id: McpServerId) -> Arc<Self> {
        Arc::new(Self {
            server_id,
            outbound: StdMutex::new(None),
            pending: StdMutex::new(HashMap::new()),
            authorization_contexts: StdMutex::new(HashMap::new()),
            host_image_publish_paths: StdMutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
        })
    }

    pub(crate) fn attach_outbound(
        &self,
        outbound: mpsc::UnboundedSender<Value>,
    ) -> Result<(), McpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(McpError::shutdown("managed Playwright bridge is closed"));
        }
        let mut slot = self
            .outbound
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright outbound lock is unavailable"))?;
        *slot = Some(outbound);
        Ok(())
    }

    pub(crate) fn complete(
        &self,
        input: ManagedPlaywrightCompletionInput,
    ) -> Result<bool, McpError> {
        if input.schema_version != MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION {
            return Err(McpError::protocol(
                "managed Playwright completion schema is unsupported",
            ));
        }
        let request_id = parse_uuid_v4(&input.request_id)?;
        let mut requests = self
            .pending
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright pending lock is unavailable"))?;
        let Some(pending) = requests.remove(&request_id) else {
            return Ok(false);
        };
        if !outcome_matches(pending.operation, &input.outcome) {
            let dispatch_certainty = pending.dispatch_certainty();
            let _ = pending
                .completion
                .send(ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::ProtocolError,
                    dispatch_certainty,
                });
            return Ok(false);
        }
        if !pending.completion_is_monotonic(&input.outcome) {
            let dispatch_certainty = pending.dispatch_certainty();
            let _ = pending
                .completion
                .send(ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::ProtocolError,
                    dispatch_certainty,
                });
            return Ok(false);
        }
        pending.acknowledge_completion(&input.outcome);
        Ok(pending.completion.send(input.outcome).is_ok())
    }

    pub(crate) fn acknowledge_dispatch_phase(
        &self,
        input: ManagedPlaywrightDispatchPhaseInput,
    ) -> Result<bool, McpError> {
        if input.schema_version != MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION {
            return Err(McpError::protocol(
                "managed Playwright dispatch phase schema is unsupported",
            ));
        }
        let request_id = parse_uuid_v4(&input.request_id)?;
        let requests = self
            .pending
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright pending lock is unavailable"))?;
        let Some(request) = requests.get(&request_id) else {
            return Ok(false);
        };
        if request.operation != PendingOperation::CallTool {
            return Ok(false);
        }
        request.acknowledge_phase(input.phase);
        Ok(true)
    }

    pub(crate) fn close_now(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            for (request_id, request) in pending.drain() {
                self.send_cancel(request_id, ManagedPlaywrightCancelReason::Shutdown);
                let dispatch_certainty = request.dispatch_certainty();
                let _ = request
                    .completion
                    .send(ManagedPlaywrightCompletionOutcome::Error {
                        code: ManagedPlaywrightBridgeErrorCode::Closed,
                        dispatch_certainty,
                    });
            }
        }
        if let Ok(mut outbound) = self.outbound.lock() {
            outbound.take();
        }
        if let Ok(mut contexts) = self.authorization_contexts.lock() {
            contexts.clear();
        }
    }

    pub(crate) async fn prepare_sensitive_tool(
        &self,
        input: ManagedPlaywrightPrepareSensitiveToolInput,
    ) -> Result<ManagedPlaywrightCompletionOutcome, McpError> {
        self.request(
            ManagedPlaywrightCommand::PrepareSensitiveTool {
                input: Box::new(input),
            },
            PendingOperation::PrepareSensitiveTool,
            Duration::from_secs(10),
            // Proposal-time target freezing is deliberately drained to a terminal Main outcome.
            // Cancelling this bridge request could discard a late `sensitive_tool_prepared`
            // completion after Main created the binding, leaving that authority alive until TTL.
            None,
            None,
        )
        .await
    }

    /// Best-effort process-only cleanup from synchronous reject/cancel/revoke paths. Main performs
    /// the deletion before attempting its completion, so an untracked completion cannot restore
    /// or retain the authority.
    pub(crate) fn release_sensitive_tool_binding_now(
        &self,
        binding_id: String,
        run_id: String,
        activation_id: String,
        call_id: String,
        reason: ManagedPlaywrightSensitiveBindingReleaseReason,
    ) -> Result<(), McpError> {
        if self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let request_id = Uuid::new_v4();
        let notification = json!({
            "jsonrpc": "2.0",
            "method": MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
            "params": ManagedPlaywrightCommandNotification {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: request_id.to_string(),
                server_id: self.server_id.to_string(),
                deadline_ms: unix_millis().saturating_add(10_000),
                command: ManagedPlaywrightCommand::ReleaseSensitiveToolBinding {
                    binding_id,
                    run_id,
                    activation_id,
                    call_id,
                    reason,
                },
            },
        });
        let sent = self
            .outbound
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright outbound lock is unavailable"))?
            .as_ref()
            .is_some_and(|outbound| outbound.send(notification).is_ok());
        if sent {
            Ok(())
        } else {
            Err(McpError::shutdown(
                "managed Playwright Host bridge is unavailable",
            ))
        }
    }

    #[cfg(test)]
    pub(crate) fn pending_request_count(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or_default()
    }

    fn abort_pending(&self, reason: ManagedPlaywrightCancelReason) {
        if let Ok(mut pending) = self.pending.lock() {
            for (request_id, request) in pending.drain() {
                self.send_cancel(request_id, reason);
                let dispatch_certainty = request.dispatch_certainty();
                let _ = request
                    .completion
                    .send(ManagedPlaywrightCompletionOutcome::Error {
                        code: ManagedPlaywrightBridgeErrorCode::Closed,
                        dispatch_certainty,
                    });
            }
        }
    }

    fn abort_startup_pending(&self, reason: ManagedPlaywrightCancelReason) {
        if let Ok(mut pending) = self.pending.lock() {
            let request_ids = pending
                .iter()
                .filter_map(|(request_id, request)| {
                    matches!(
                        request.operation,
                        PendingOperation::Connect | PendingOperation::ListTools
                    )
                    .then_some(*request_id)
                })
                .collect::<Vec<_>>();
            for request_id in request_ids {
                if let Some(request) = pending.remove(&request_id) {
                    self.send_cancel(request_id, reason);
                    let _ = request
                        .completion
                        .send(ManagedPlaywrightCompletionOutcome::Error {
                            code: ManagedPlaywrightBridgeErrorCode::Cancelled,
                            dispatch_certainty:
                                ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
                        });
                }
            }
        }
    }

    async fn request(
        &self,
        command: ManagedPlaywrightCommand,
        operation: PendingOperation,
        timeout: Duration,
        cancellation: Option<McpCancellationToken>,
        manager_dispatch: Option<McpDispatchTracker>,
    ) -> Result<ManagedPlaywrightCompletionOutcome, McpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(authoritative_pre_dispatch_error(McpError::shutdown(
                "managed Playwright bridge is closed",
            )));
        }
        if (operation == PendingOperation::CallTool) != manager_dispatch.is_some() {
            return Err(authoritative_pre_dispatch_error(McpError::protocol(
                "managed Playwright dispatch tracker is inconsistent",
            )));
        }
        if cancellation
            .as_ref()
            .is_some_and(McpCancellationToken::is_cancelled)
        {
            return Err(authoritative_pre_dispatch_error(McpError::cancelled(
                "managed Playwright tools/call",
            )));
        }
        let request_id = Uuid::new_v4();
        let (completion, mut receiver) = oneshot::channel();
        let main_dispatch = manager_dispatch.as_ref().map(|_| McpDispatchTracker::new());
        let notification = json!({
            "jsonrpc": "2.0",
            "method": MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
            "params": ManagedPlaywrightCommandNotification {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: request_id.to_string(),
                server_id: self.server_id.to_string(),
                deadline_ms: unix_millis().saturating_add(
                    u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX)
                ),
                command,
            },
        });
        {
            // Keep the pending lock across the non-blocking send. A completion can therefore
            // never observe a request before its Manager tracker crosses the exact send boundary,
            // while `close_now` uses the same lock order (pending -> outbound).
            let mut pending = self.pending.lock().map_err(|_| {
                authoritative_pre_dispatch_error(McpError::protocol(
                    "managed Playwright pending lock is unavailable",
                ))
            })?;
            if self.closed.load(Ordering::Acquire) {
                return Err(authoritative_pre_dispatch_error(McpError::shutdown(
                    "managed Playwright bridge is closed",
                )));
            }
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(authoritative_pre_dispatch_error(McpError::capacity(
                    "managed Playwright bridge request limit was reached",
                )));
            }
            let outbound = self.outbound.lock().map_err(|_| {
                authoritative_pre_dispatch_error(McpError::protocol(
                    "managed Playwright outbound lock is unavailable",
                ))
            })?;
            let Some(outbound) = outbound.as_ref() else {
                return Err(authoritative_pre_dispatch_error(McpError::shutdown(
                    "managed Playwright Host bridge is unavailable",
                )));
            };
            if cancellation
                .as_ref()
                .is_some_and(McpCancellationToken::is_cancelled)
            {
                return Err(authoritative_pre_dispatch_error(McpError::cancelled(
                    "managed Playwright tools/call",
                )));
            }
            pending.insert(
                request_id,
                PendingRequest {
                    operation,
                    completion,
                    manager_dispatch: manager_dispatch.clone(),
                    main_dispatch: main_dispatch.clone(),
                },
            );
            if outbound.send(notification).is_err() {
                pending.remove(&request_id);
                return Err(authoritative_pre_dispatch_error(McpError::shutdown(
                    "managed Playwright Host bridge is unavailable",
                )));
            }
            // This is the sole reverse-peer transition to possibly dispatched. It is deliberately
            // adjacent to a successful outbound send and runs before `pending` can be completed.
            if let Some(dispatch) = manager_dispatch.as_ref() {
                dispatch.mark_request_queued();
            }
        }
        let mut cleanup = PendingRequestCleanup {
            bridge: self,
            request_id,
            armed: true,
        };

        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        enum WaitOutcome {
            Completed(Result<ManagedPlaywrightCompletionOutcome, oneshot::error::RecvError>),
            Cancelled,
            TimedOut,
        }
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let wait = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => WaitOutcome::Cancelled,
                result = &mut receiver => WaitOutcome::Completed(result),
                _ = &mut deadline => WaitOutcome::TimedOut,
            }
        } else {
            tokio::select! {
                result = &mut receiver => WaitOutcome::Completed(result),
                _ = &mut deadline => WaitOutcome::TimedOut,
            }
        };
        let result = match wait {
            WaitOutcome::Completed(Ok(outcome)) => Ok(outcome),
            WaitOutcome::Completed(Err(_)) => {
                self.remove_pending(request_id);
                Err(interrupted_error(
                    operation,
                    McpOutcomeUnknownReason::Shutdown,
                    dispatch_certainty_for(operation, main_dispatch.as_ref()),
                    timeout_ms,
                ))
            }
            WaitOutcome::Cancelled => {
                self.settle_interrupted_request(
                    request_id,
                    &mut receiver,
                    operation,
                    ManagedPlaywrightCancelReason::Cancelled,
                    McpOutcomeUnknownReason::Cancelled,
                    main_dispatch.as_ref(),
                    timeout_ms,
                )
                .await
            }
            WaitOutcome::TimedOut => {
                self.settle_interrupted_request(
                    request_id,
                    &mut receiver,
                    operation,
                    ManagedPlaywrightCancelReason::Timeout,
                    McpOutcomeUnknownReason::TimedOut,
                    main_dispatch.as_ref(),
                    timeout_ms,
                )
                .await
            }
        };
        cleanup.disarm();
        result
    }

    async fn settle_interrupted_request(
        &self,
        request_id: Uuid,
        receiver: &mut oneshot::Receiver<ManagedPlaywrightCompletionOutcome>,
        operation: PendingOperation,
        cancel_reason: ManagedPlaywrightCancelReason,
        unknown_reason: McpOutcomeUnknownReason,
        main_dispatch: Option<&McpDispatchTracker>,
        timeout_ms: u64,
    ) -> Result<ManagedPlaywrightCompletionOutcome, McpError> {
        self.send_cancel(request_id, cancel_reason);
        match tokio::time::timeout(MANAGED_PLAYWRIGHT_COMPLETION_GRACE, &mut *receiver).await {
            Ok(Ok(outcome)) => return Ok(outcome),
            Ok(Err(_)) => {}
            Err(_) => {}
        }
        self.remove_pending(request_id);
        if let Ok(outcome) = receiver.try_recv() {
            return Ok(outcome);
        }
        Err(interrupted_error(
            operation,
            unknown_reason,
            dispatch_certainty_for(operation, main_dispatch),
            timeout_ms,
        ))
    }

    fn remove_pending(&self, request_id: Uuid) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(&request_id);
        }
    }

    fn register_authorization_context(
        &self,
        invocation_id: McpInvocationId,
        context: ManagedPlaywrightAuthorizationContext,
    ) -> Result<(), McpError> {
        let mut contexts = self.authorization_contexts.lock().map_err(|_| {
            authoritative_pre_dispatch_error(McpError::protocol(
                "managed Playwright authorization context is unavailable",
            ))
        })?;
        if contexts.len() >= MAX_PENDING_REQUESTS || contexts.contains_key(&invocation_id) {
            return Err(authoritative_pre_dispatch_error(McpError::capacity(
                "managed Playwright authorization context limit was reached",
            )));
        }
        contexts.insert(invocation_id, context);
        Ok(())
    }

    fn take_authorization_context(
        &self,
        invocation_id: McpInvocationId,
    ) -> Result<ManagedPlaywrightAuthorizationContext, McpError> {
        self.authorization_contexts
            .lock()
            .map_err(|_| {
                authoritative_pre_dispatch_error(McpError::protocol(
                    "managed Playwright authorization context is unavailable",
                ))
            })?
            .remove(&invocation_id)
            .ok_or_else(|| {
                authoritative_pre_dispatch_error(McpError::protocol(
                    "managed Playwright authorization context is missing",
                ))
            })
    }

    fn remove_authorization_context(&self, invocation_id: McpInvocationId) {
        if let Ok(mut contexts) = self.authorization_contexts.lock() {
            contexts.remove(&invocation_id);
        }
    }

    fn stash_host_image_publish_path(&self, call_id: String, path: String) {
        if call_id.trim().is_empty()
            || path.trim().is_empty()
            || !std::path::Path::new(&path).is_absolute()
        {
            return;
        }
        if let Ok(mut paths) = self.host_image_publish_paths.lock() {
            if paths.len() >= MAX_PENDING_REQUESTS {
                paths.clear();
            }
            paths.insert(call_id, path);
        }
    }

    fn take_host_image_publish_path(&self, call_id: &str) -> Option<String> {
        self.host_image_publish_paths.lock().ok()?.remove(call_id)
    }

    fn send_cancel(&self, request_id: Uuid, reason: ManagedPlaywrightCancelReason) {
        let notification = json!({
            "jsonrpc": "2.0",
            "method": MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
            "params": ManagedPlaywrightCancelNotification {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: request_id.to_string(),
                reason,
            },
        });
        if let Ok(outbound) = self.outbound.lock() {
            let _ = outbound
                .as_ref()
                .map(|outbound| outbound.send(notification));
        }
    }
}

#[derive(Clone)]
pub(crate) struct ManagedPlaywrightHostBridgeConnector {
    bridge: Arc<ManagedPlaywrightHostBridge>,
}

impl ManagedPlaywrightHostBridgeConnector {
    pub(crate) fn new(bridge: Arc<ManagedPlaywrightHostBridge>) -> Self {
        Self { bridge }
    }
}

impl McpConnector for ManagedPlaywrightHostBridgeConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async move {
            let McpTransportConfig::HostBridge(bridge_config) = &config.transport else {
                return Err(McpError::config(
                    "managed Playwright connector requires HostBridge transport",
                ));
            };
            if bridge_config.channel != MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL
                || config.id != self.bridge.server_id
            {
                return Err(McpError::config(
                    "managed Playwright bridge identity is not registered",
                ));
            }
            let outcome = self
                .bridge
                .request(
                    ManagedPlaywrightCommand::Connect,
                    PendingOperation::Connect,
                    Duration::from_millis(config.connect_timeout_ms),
                    None,
                    None,
                )
                .await?;
            let ManagedPlaywrightCompletionOutcome::Connected { protocol } = outcome else {
                return Err(error_from_outcome(outcome, PendingOperation::Connect));
            };
            let protocol = serde_json::from_value::<McpProtocolSnapshot>(protocol)
                .map_err(|_| McpError::negotiation("managed Playwright protocol is invalid"))?;
            if protocol.negotiated_version != "2025-11-25"
                || protocol.lifecycle != McpLifecycleKind::InitializeFallback
                || !protocol.capabilities.tools
                || protocol.server.as_ref().is_none_or(|server| {
                    server.name != "@playwright/mcp" || server.version != "0.0.79"
                })
            {
                return Err(McpError::negotiation(
                    "managed Playwright protocol identity is incompatible",
                ));
            }
            Ok(Arc::new(ManagedPlaywrightHostBridgePeer {
                server_id: config.id,
                protocol,
                bridge: Arc::clone(&self.bridge),
                request_timeout: Duration::from_millis(config.request_timeout_ms),
                shutdown_timeout: Duration::from_millis(config.shutdown_timeout_ms),
                state: AtomicU8::new(STATE_READY),
            }) as Arc<dyn McpPeer>)
        })
    }
}

struct ManagedPlaywrightHostBridgePeer {
    server_id: McpServerId,
    protocol: McpProtocolSnapshot,
    bridge: Arc<ManagedPlaywrightHostBridge>,
    request_timeout: Duration,
    shutdown_timeout: Duration,
    state: AtomicU8,
}

impl ManagedPlaywrightHostBridgePeer {
    fn call_tool_with_dispatch<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
        dispatch: McpDispatchTracker,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        dispatch.mark_dispatching();
        Box::pin(async move {
            if self.connection_state() != McpConnectionState::Ready {
                return Err(authoritative_pre_dispatch_error(McpError::shutdown(
                    "managed Playwright peer is not ready",
                )));
            }
            let timeout = call
                .timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(self.request_timeout)
                .min(Duration::from_secs(300));
            let invocation_id = call.invocation_id.ok_or_else(|| {
                authoritative_pre_dispatch_error(McpError::protocol(
                    "managed Playwright active-call identity is missing",
                ))
            })?;
            let authorization_context = self
                .bridge
                .take_authorization_context(invocation_id)
                .map_err(authoritative_pre_dispatch_error)?;
            let call_id = authorization_context.call_id.clone();
            let outcome = self
                .bridge
                .request(
                    ManagedPlaywrightCommand::CallTool {
                        name: call.name,
                        arguments: call.arguments,
                        timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
                        authorization_context: Box::new(authorization_context),
                    },
                    PendingOperation::CallTool,
                    timeout,
                    Some(cancellation),
                    Some(dispatch),
                )
                .await?;
            let ManagedPlaywrightCompletionOutcome::ToolCalled {
                result,
                host_image_publish_path,
            } = outcome
            else {
                return Err(error_from_outcome(outcome, PendingOperation::CallTool));
            };
            if let Some(path) = host_image_publish_path {
                self.bridge.stash_host_image_publish_path(call_id, path);
            }
            serde_json::from_value(result).map_err(|_| {
                McpError::protocol("managed Playwright tools/call result is invalid")
                    .with_authoritative_dispatch_certainty(McpDispatchCertainty::ResponseReceived)
            })
        })
    }
}

impl McpPeer for ManagedPlaywrightHostBridgePeer {
    fn server_id(&self) -> McpServerId {
        self.server_id
    }

    fn connection_state(&self) -> McpConnectionState {
        match self.state.load(Ordering::Acquire) {
            STATE_CONNECTING => McpConnectionState::Connecting,
            STATE_READY => McpConnectionState::Ready,
            STATE_CLOSING => McpConnectionState::Closing,
            STATE_CLOSED => McpConnectionState::Closed,
            _ => McpConnectionState::Failed,
        }
    }

    fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        &self.protocol
    }

    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
        Box::pin(async move {
            if self.connection_state() != McpConnectionState::Ready {
                return Err(McpError::shutdown("managed Playwright peer is not ready"));
            }
            let outcome = self
                .bridge
                .request(
                    ManagedPlaywrightCommand::ListTools { cursor },
                    PendingOperation::ListTools,
                    self.request_timeout,
                    None,
                    None,
                )
                .await?;
            let ManagedPlaywrightCompletionOutcome::ToolsListed { page } = outcome else {
                return Err(error_from_outcome(outcome, PendingOperation::ListTools));
            };
            serde_json::from_value(page)
                .map_err(|_| McpError::protocol("managed Playwright tools/list is invalid"))
        })
    }

    fn call_tool<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        self.call_tool_with_dispatch(call, cancellation, McpDispatchTracker::new())
    }

    fn call_tool_tracked<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
        dispatch: McpDispatchTracker,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        self.call_tool_with_dispatch(call, cancellation, dispatch)
    }

    fn force_close(&self) -> bool {
        self.state.store(STATE_FAILED, Ordering::Release);
        // Manager stop/restart escalation must not permanently destroy the process-wide reverse
        // bus. Abort this peer's in-flight work; app shutdown owns `close_now`.
        self.bridge
            .abort_pending(ManagedPlaywrightCancelReason::Shutdown);
        true
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async move {
            if self
                .state
                .compare_exchange(
                    STATE_READY,
                    STATE_CLOSING,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_err()
            {
                return Ok(());
            }
            let outcome = self
                .bridge
                .request(
                    ManagedPlaywrightCommand::Close,
                    PendingOperation::Close,
                    self.shutdown_timeout,
                    None,
                    None,
                )
                .await;
            match outcome {
                Ok(ManagedPlaywrightCompletionOutcome::Closed) => {
                    self.state.store(STATE_CLOSED, Ordering::Release);
                    Ok(())
                }
                Ok(other) => {
                    self.state.store(STATE_FAILED, Ordering::Release);
                    Err(error_from_outcome(other, PendingOperation::Close))
                }
                Err(error) => {
                    self.state.store(STATE_FAILED, Ordering::Release);
                    Err(error)
                }
            }
        })
    }
}

/// Managed MCP runtime that deliberately reuses the normal Manager, Catalog and typed invocation
/// path while keeping its Host bridge separate from the persistent external Registry.
pub(crate) struct ManagedPlaywrightMcpRuntime {
    server_id: McpServerId,
    manager: Arc<McpConnectionManager>,
    bridge: Arc<ManagedPlaywrightHostBridge>,
    desired_active: AtomicBool,
    lifecycle_epoch: AtomicU64,
    lifecycle_tasks: StdMutex<JoinSet<Result<(), McpError>>>,
    transition: AsyncMutex<()>,
    idle_timeout: Duration,
    idle_sleeper: ManagedPlaywrightIdleSleeper,
    idle_epoch: AtomicU64,
    active_invocations: AtomicUsize,
    idle_task: StdMutex<Option<JoinHandle<()>>>,
}

impl std::fmt::Debug for ManagedPlaywrightMcpRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedPlaywrightMcpRuntime")
            .field("server_id", &self.server_id)
            .finish_non_exhaustive()
    }
}

impl ManagedPlaywrightMcpRuntime {
    pub(crate) fn new() -> Result<Arc<Self>, McpError> {
        Self::with_idle_timeout(DEFAULT_MANAGED_PLAYWRIGHT_IDLE_TIMEOUT)
    }

    fn with_idle_timeout(idle_timeout: Duration) -> Result<Arc<Self>, McpError> {
        Self::with_idle_policy(
            idle_timeout,
            Arc::new(|timeout| Box::pin(tokio::time::sleep(timeout))),
        )
    }

    fn with_idle_policy(
        idle_timeout: Duration,
        idle_sleeper: ManagedPlaywrightIdleSleeper,
    ) -> Result<Arc<Self>, McpError> {
        if idle_timeout.is_zero() {
            return Err(McpError::config(
                "managed Playwright idle timeout must be positive",
            ));
        }
        let server_id = managed_playwright_server_id()?;
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let registry = InMemoryMcpRegistry::shared();
        registry.add(McpServerConfig {
            id: server_id,
            display_name: "Browser automation".to_string(),
            scope: McpServerScope::Managed,
            trust: McpTrustLevel::Managed,
            approval_mode: McpApprovalMode::Auto,
            enabled: true,
            transport: McpTransportConfig::HostBridge(McpHostBridgeConfig::new(
                MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL,
            )?),
            connect_timeout_ms: 10_000,
            request_timeout_ms: 60_000,
            shutdown_timeout_ms: 2_000,
        })?;
        let manager = Arc::new(McpConnectionManager::without_events(
            registry,
            Arc::new(ManagedPlaywrightHostBridgeConnector::new(Arc::clone(
                &bridge,
            ))),
            McpManagerPolicy::default(),
        )?);
        Ok(Arc::new(Self {
            server_id,
            manager,
            bridge,
            desired_active: AtomicBool::new(false),
            lifecycle_epoch: AtomicU64::new(0),
            lifecycle_tasks: StdMutex::new(JoinSet::new()),
            transition: AsyncMutex::new(()),
            idle_timeout,
            idle_sleeper,
            idle_epoch: AtomicU64::new(0),
            active_invocations: AtomicUsize::new(0),
            idle_task: StdMutex::new(None),
        }))
    }

    pub(crate) fn bridge(&self) -> Arc<ManagedPlaywrightHostBridge> {
        Arc::clone(&self.bridge)
    }

    pub(crate) fn take_host_image_publish_path(&self, call_id: &str) -> Option<String> {
        self.bridge.take_host_image_publish_path(call_id)
    }

    #[cfg(test)]
    pub(crate) fn is_ready(&self) -> bool {
        self.manager
            .get_status(self.server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == mycopilot_mcp_client::McpServerState::Ready)
    }

    #[cfg(test)]
    pub(crate) fn lifecycle_task_count(&self) -> usize {
        self.lifecycle_tasks
            .lock()
            .map(|tasks| tasks.len())
            .unwrap_or_default()
    }

    /// Records activation intent synchronously and owns the resulting task until shutdown.
    /// A generation check after discovery prevents a stale slow start from reopening after deny.
    pub(crate) fn request_start(self: &Arc<Self>) -> Result<(), McpError> {
        self.cancel_idle_timer();
        self.desired_active.store(true, Ordering::Release);
        let epoch = self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let runtime = Arc::clone(self);
        let mut tasks = self
            .lifecycle_tasks
            .lock()
            .map_err(|_| McpError::shutdown("managed Playwright lifecycle owner is unavailable"))?;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move {
            let result = runtime.start_for_epoch(epoch).await;
            if result.is_ok() {
                runtime.schedule_idle_timer();
            }
            result
        });
        Ok(())
    }

    pub(crate) fn request_stop(self: &Arc<Self>) -> Result<(), McpError> {
        self.cancel_idle_timer();
        self.desired_active.store(false, Ordering::Release);
        let epoch = self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.bridge
            .abort_pending(ManagedPlaywrightCancelReason::Shutdown);
        let runtime = Arc::clone(self);
        let mut tasks = self
            .lifecycle_tasks
            .lock()
            .map_err(|_| McpError::shutdown("managed Playwright lifecycle owner is unavailable"))?;
        while tasks.try_join_next().is_some() {}
        tasks.spawn(async move { runtime.stop_for_epoch(epoch).await });
        Ok(())
    }

    pub(crate) async fn start(&self) -> Result<(), McpError> {
        if !self.desired_active.load(Ordering::Acquire) {
            return Err(McpError::config(
                "managed Playwright capability is not active",
            ));
        }
        let epoch = self.lifecycle_epoch.load(Ordering::Acquire);
        self.start_for_epoch(epoch).await
    }

    async fn start_for_epoch(&self, epoch: u64) -> Result<(), McpError> {
        if !self.desired_active.load(Ordering::Acquire) {
            return Err(McpError::cancelled(
                "managed Playwright activation was superseded",
            ));
        }
        let _transition = self.transition.lock().await;
        if !self.desired_active.load(Ordering::Acquire)
            || self.lifecycle_epoch.load(Ordering::Acquire) != epoch
        {
            return Err(McpError::cancelled(
                "managed Playwright activation was superseded",
            ));
        }
        self.manager.start(self.server_id).await?;
        let validation = self.validate_reviewed_catalog();
        if validation.is_err() || !self.desired_active.load(Ordering::Acquire) {
            let _ = self.manager.stop(self.server_id).await;
        }
        validation?;
        if !self.desired_active.load(Ordering::Acquire) {
            return Err(McpError::cancelled(
                "managed Playwright activation was superseded",
            ));
        }
        if self.lifecycle_epoch.load(Ordering::Acquire) != epoch {
            // A newer activation generation owns the already de-duplicated Manager start. Never
            // let this stale task stop a Ready peer while desired state remains active.
            return Err(McpError::cancelled(
                "managed Playwright activation was superseded",
            ));
        }
        Ok(())
    }

    pub(crate) async fn stop(&self) -> Result<(), McpError> {
        self.cancel_idle_timer();
        self.desired_active.store(false, Ordering::Release);
        let epoch = self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.bridge
            .abort_pending(ManagedPlaywrightCancelReason::Shutdown);
        self.stop_for_epoch(epoch).await
    }

    async fn stop_for_epoch(&self, epoch: u64) -> Result<(), McpError> {
        if self.desired_active.load(Ordering::Acquire)
            || self.lifecycle_epoch.load(Ordering::Acquire) != epoch
        {
            return Err(McpError::cancelled(
                "managed Playwright stop was superseded",
            ));
        }
        let _transition = self.transition.lock().await;
        if self.desired_active.load(Ordering::Acquire)
            || self.lifecycle_epoch.load(Ordering::Acquire) != epoch
        {
            return Err(McpError::cancelled(
                "managed Playwright stop was superseded",
            ));
        }
        self.manager.stop(self.server_id).await?;
        // A newer start may have raced after the pre-check but before Manager mutation. Reconcile
        // to the latest desired generation so a stale stop can never be the final state.
        let current_epoch = self.lifecycle_epoch.load(Ordering::Acquire);
        if self.desired_active.load(Ordering::Acquire) && current_epoch != epoch {
            drop(_transition);
            return self.start_for_epoch(current_epoch).await;
        }
        Ok(())
    }

    pub(crate) async fn shutdown(&self) {
        self.cancel_idle_timer();
        self.desired_active.store(false, Ordering::Release);
        self.lifecycle_epoch.fetch_add(1, Ordering::AcqRel);
        self.bridge
            .abort_pending(ManagedPlaywrightCancelReason::Shutdown);
        let idle_task = self
            .idle_task
            .lock()
            .map(|mut task| task.take())
            .unwrap_or_default();
        if let Some(task) = idle_task {
            task.abort();
            let _ = task.await;
        }
        let _transition = self.transition.lock().await;
        let _ = self.manager.shutdown(Duration::from_secs(2)).await;
        let mut tasks = self
            .lifecycle_tasks
            .lock()
            .map(|mut tasks| std::mem::replace(&mut *tasks, JoinSet::new()))
            .unwrap_or_default();
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
        self.bridge.close_now();
    }

    pub(crate) async fn invoke(
        self: &Arc<Self>,
        raw_name: &str,
        model_call_id: &str,
        arguments: Value,
        authorization_context: ManagedPlaywrightAuthorizationContext,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        self.cancel_idle_timer();
        self.active_invocations.fetch_add(1, Ordering::AcqRel);
        let result = self
            .invoke_inner(
                raw_name,
                model_call_id,
                arguments,
                authorization_context,
                cancellation,
            )
            .await;
        if self.active_invocations.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.schedule_idle_timer();
        }
        result
    }

    async fn invoke_inner(
        &self,
        raw_name: &str,
        model_call_id: &str,
        arguments: Value,
        authorization_context: ManagedPlaywrightAuthorizationContext,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        let start = self.start();
        tokio::pin!(start);
        tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.bridge.abort_startup_pending(ManagedPlaywrightCancelReason::Cancelled);
                let _ = start.await;
                return Err(McpError::cancelled("managed Playwright startup"));
            }
            result = &mut start => result?,
        }
        let catalog = self
            .manager
            .catalog(self.server_id)?
            .ok_or_else(|| McpError::config("managed Playwright catalog is unavailable"))?;
        let status = self
            .manager
            .get_status(self.server_id)?
            .ok_or_else(|| McpError::config("managed Playwright status is unavailable"))?;
        let tool = catalog
            .tools
            .iter()
            .find(|tool| tool.raw_name == raw_name && tool.routable)
            .ok_or_else(|| McpError::config("managed Playwright tool is not reviewed"))?;
        let catalog_digest = catalog
            .content_digest
            .clone()
            .ok_or_else(|| McpError::config("managed Playwright catalog digest is unavailable"))?;
        let request = McpCatalogToolCall::new(
            McpCatalogToolCallIdentity {
                tool_id: McpToolId {
                    server_id: self.server_id,
                    raw_name: raw_name.to_string(),
                },
                expected_config_epoch: status.config_epoch,
                expected_registry_revision: status.registry_revision,
                expected_config_digest: status.config_digest,
                expected_catalog_generation: catalog.generation,
                expected_catalog_digest: catalog_digest,
                expected_schema_digest: tool.schema_digest.clone(),
                expected_model_name: tool.model_name.clone(),
            },
            arguments,
        );
        let invocation_id = McpInvocationId::from_str(&authorization_context.invocation_id)
            .map_err(|_| McpError::config("managed Playwright invocation identity is invalid"))?;
        let active_id = McpActiveCallId::new(
            self.server_id,
            invocation_id,
            McpModelCallId::new(model_call_id)?,
        );
        self.bridge
            .register_authorization_context(invocation_id, authorization_context)?;
        let result = self
            .manager
            .call_catalog_tool_identified(active_id, request, cancellation)
            .await;
        self.bridge.remove_authorization_context(invocation_id);
        result
    }

    fn cancel_idle_timer(&self) {
        self.idle_epoch.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut task) = self.idle_task.lock() {
            if let Some(task) = task.take() {
                task.abort();
            }
        }
    }

    fn schedule_idle_timer(self: &Arc<Self>) {
        if !self.desired_active.load(Ordering::Acquire)
            || self.active_invocations.load(Ordering::Acquire) != 0
        {
            return;
        }
        let epoch = self.idle_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let timeout = self.idle_timeout;
        let sleeper = Arc::clone(&self.idle_sleeper);
        let runtime = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            sleeper(timeout).await;
            if let Some(runtime) = runtime.upgrade() {
                runtime.stop_automation_when_idle(epoch).await;
            }
        });
        if let Ok(mut current) = self.idle_task.lock() {
            if let Some(previous) = current.replace(task) {
                previous.abort();
            }
        }
    }

    async fn stop_automation_when_idle(&self, epoch: u64) {
        if !self.desired_active.load(Ordering::Acquire)
            || self.active_invocations.load(Ordering::Acquire) != 0
            || self.idle_epoch.load(Ordering::Acquire) != epoch
        {
            return;
        }
        let _transition = self.transition.lock().await;
        if !self.desired_active.load(Ordering::Acquire)
            || self.active_invocations.load(Ordering::Acquire) != 0
            || self.idle_epoch.load(Ordering::Acquire) != epoch
        {
            return;
        }
        // Manager stop sends the managed `close` command, which detaches Playwright/CDP while
        // BrowserSurfaceManager deliberately keeps the user-visible guest page alive.
        let _ = self.manager.stop(self.server_id).await;
    }

    fn validate_reviewed_catalog(&self) -> Result<(), McpError> {
        let reviewed_contract = load_playwright_browser_contract()
            .map_err(|_| McpError::config("managed Playwright manifest is invalid"))?;
        let reviewed = &reviewed_contract.manifest;
        if reviewed.provider_contract.upstream_catalog_digest
            != reviewed_contract.upstream_catalog_digest
            || reviewed.provider_contract.policy_digest != reviewed_contract.policy_digest
        {
            return Err(McpError::config(
                "managed Playwright provider contract identity drifted",
            ));
        }
        let catalog = self
            .manager
            .catalog(self.server_id)?
            .ok_or_else(|| McpError::config("managed Playwright catalog is unavailable"))?;
        if catalog.completeness != McpCatalogCompleteness::Complete
            || catalog.tools.len() != reviewed.tools.len()
            || catalog.content_digest.is_none()
        {
            return Err(McpError::config(
                "managed Playwright catalog does not match the reviewed manifest",
            ));
        }
        for reviewed_tool in &reviewed.tools {
            let Some(contract_tool) = reviewed_contract.tool(&reviewed_tool.raw_name) else {
                return Err(McpError::config(
                    "managed Playwright typed tool contract is unavailable",
                ));
            };
            let Some(tool) = catalog
                .tools
                .iter()
                .find(|tool| tool.raw_name == reviewed_tool.tool_id && tool.routable)
            else {
                return Err(McpError::config(
                    "managed Playwright reviewed tool is unavailable",
                ));
            };
            if !contract_tool.is_runtime_exposable()
                || contract_tool.upstream_schema_digest != reviewed_tool.upstream_schema_digest
                || contract_tool.host_overlay_digest != reviewed_tool.host_overlay_digest
                || contract_tool.host_schema_digest != reviewed_tool.schema_digest
                || contract_tool.host_input_schema != reviewed_tool.input_schema
                || tool.descriptor.input_schema != reviewed_tool.input_schema
                || tool.raw_name != reviewed_tool.tool_id
                || tool.descriptor.name != reviewed_tool.model_name
                || tool.schema_digest != mcp_schema_digest(&reviewed_tool.input_schema, None)?
            {
                return Err(McpError::config(
                    "managed Playwright reviewed tool schema drifted",
                ));
            }
        }
        Ok(())
    }
}

fn managed_playwright_server_id() -> Result<McpServerId, McpError> {
    let uuid = Uuid::parse_str(BROWSER_AUTOMATION_MANAGED_SERVER_ID)
        .map_err(|_| McpError::config("managed Playwright server id is invalid"))?;
    Ok(McpServerId::from_uuid(uuid))
}

fn outcome_matches(
    operation: PendingOperation,
    outcome: &ManagedPlaywrightCompletionOutcome,
) -> bool {
    matches!(
        (operation, outcome),
        (_, ManagedPlaywrightCompletionOutcome::Error { .. })
            | (
                PendingOperation::Connect,
                ManagedPlaywrightCompletionOutcome::Connected { .. }
            )
            | (
                PendingOperation::ListTools,
                ManagedPlaywrightCompletionOutcome::ToolsListed { .. }
            )
            | (
                PendingOperation::PrepareSensitiveTool,
                ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared { .. }
            )
            | (
                PendingOperation::CallTool,
                ManagedPlaywrightCompletionOutcome::ToolCalled { .. }
            )
            | (
                PendingOperation::Close,
                ManagedPlaywrightCompletionOutcome::Closed
            )
    )
}

fn error_from_outcome(
    outcome: ManagedPlaywrightCompletionOutcome,
    operation: PendingOperation,
) -> McpError {
    let ManagedPlaywrightCompletionOutcome::Error {
        code,
        dispatch_certainty,
    } = outcome
    else {
        return McpError::protocol("managed Playwright completion operation mismatch");
    };
    let certainty = match dispatch_certainty {
        ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched => {
            McpDispatchCertainty::DefinitelyNotDispatched
        }
        ManagedPlaywrightDispatchCertainty::PossiblyDispatched => {
            McpDispatchCertainty::PossiblyDispatched
        }
        ManagedPlaywrightDispatchCertainty::ResponseReceived => {
            McpDispatchCertainty::ResponseReceived
        }
    };
    if operation == PendingOperation::CallTool
        && code == ManagedPlaywrightBridgeErrorCode::OutcomeUnknown
    {
        return McpError::outcome_unknown(
            "managed Playwright tools/call",
            McpOutcomeUnknownReason::ProtocolFailure,
            McpDispatchCertainty::PossiblyDispatched,
        );
    }
    if operation == PendingOperation::CallTool
        && certainty == McpDispatchCertainty::PossiblyDispatched
    {
        let reason = match code {
            ManagedPlaywrightBridgeErrorCode::Cancelled => McpOutcomeUnknownReason::Cancelled,
            ManagedPlaywrightBridgeErrorCode::Timeout => McpOutcomeUnknownReason::TimedOut,
            ManagedPlaywrightBridgeErrorCode::TargetClosed => {
                McpOutcomeUnknownReason::TransportClosed
            }
            _ => McpOutcomeUnknownReason::ProtocolFailure,
        };
        return McpError::outcome_unknown("managed Playwright tools/call", reason, certainty);
    }
    let message = safe_error_message(code);
    let error = match code {
        ManagedPlaywrightBridgeErrorCode::Cancelled => {
            McpError::cancelled("managed Playwright operation")
        }
        ManagedPlaywrightBridgeErrorCode::Timeout => {
            McpError::timeout("managed Playwright operation", 0)
        }
        ManagedPlaywrightBridgeErrorCode::Busy
        | ManagedPlaywrightBridgeErrorCode::SurfaceCapacityExceeded => McpError::capacity(message),
        ManagedPlaywrightBridgeErrorCode::Closed => McpError::shutdown(message),
        ManagedPlaywrightBridgeErrorCode::OutputTooLarge => {
            McpError::output_too_large("managed Playwright tools/call", message)
        }
        ManagedPlaywrightBridgeErrorCode::SurfaceUnavailable
        | ManagedPlaywrightBridgeErrorCode::TargetClosed
        | ManagedPlaywrightBridgeErrorCode::ToolNotReviewed
        | ManagedPlaywrightBridgeErrorCode::InvalidArguments
        | ManagedPlaywrightBridgeErrorCode::CatalogDrift
        | ManagedPlaywrightBridgeErrorCode::ProtocolError
        | ManagedPlaywrightBridgeErrorCode::OutcomeUnknown
        | ManagedPlaywrightBridgeErrorCode::InternalSafeError => McpError::protocol(message),
    };
    // This is a trusted Main completion, not an untrusted MCP wire error. Main's Tool lifecycle
    // knows whether it dispatched the fixed official handler, so Manager must preserve this
    // evidence instead of widening an acknowledged pre-dispatch rejection to OutcomeUnknown.
    error.with_authoritative_dispatch_certainty(certainty)
}

fn authoritative_pre_dispatch_error(error: McpError) -> McpError {
    error.with_authoritative_dispatch_certainty(McpDispatchCertainty::DefinitelyNotDispatched)
}

fn interrupted_error(
    operation: PendingOperation,
    reason: McpOutcomeUnknownReason,
    certainty: ManagedPlaywrightDispatchCertainty,
    timeout_ms: u64,
) -> McpError {
    if operation != PendingOperation::CallTool {
        return if reason == McpOutcomeUnknownReason::TimedOut {
            McpError::timeout("managed Playwright Host bridge", timeout_ms)
        } else if reason == McpOutcomeUnknownReason::Shutdown {
            McpError::shutdown("managed Playwright Host bridge stopped")
        } else {
            McpError::cancelled("managed Playwright Host bridge")
        };
    }

    let certainty = mcp_dispatch_certainty(certainty);
    if certainty != McpDispatchCertainty::DefinitelyNotDispatched {
        return McpError::outcome_unknown("managed Playwright tools/call", reason, certainty);
    }

    let error = if reason == McpOutcomeUnknownReason::TimedOut {
        McpError::timeout("managed Playwright tools/call", timeout_ms)
    } else if reason == McpOutcomeUnknownReason::Shutdown {
        McpError::shutdown("managed Playwright Host bridge stopped before Tool dispatch")
    } else {
        McpError::cancelled("managed Playwright tools/call")
    };
    authoritative_pre_dispatch_error(error)
}

fn dispatch_certainty_for(
    operation: PendingOperation,
    dispatch: Option<&McpDispatchTracker>,
) -> ManagedPlaywrightDispatchCertainty {
    if operation != PendingOperation::CallTool {
        return ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched;
    }
    dispatch
        .map(McpDispatchTracker::certainty)
        .map(managed_dispatch_certainty)
        .unwrap_or(ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched)
}

fn managed_dispatch_certainty(
    certainty: McpDispatchCertainty,
) -> ManagedPlaywrightDispatchCertainty {
    match certainty {
        McpDispatchCertainty::DefinitelyNotDispatched => {
            ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched
        }
        McpDispatchCertainty::PossiblyDispatched => {
            ManagedPlaywrightDispatchCertainty::PossiblyDispatched
        }
        McpDispatchCertainty::ResponseReceived => {
            ManagedPlaywrightDispatchCertainty::ResponseReceived
        }
        _ => ManagedPlaywrightDispatchCertainty::PossiblyDispatched,
    }
}

fn mcp_dispatch_certainty(certainty: ManagedPlaywrightDispatchCertainty) -> McpDispatchCertainty {
    match certainty {
        ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched => {
            McpDispatchCertainty::DefinitelyNotDispatched
        }
        ManagedPlaywrightDispatchCertainty::PossiblyDispatched => {
            McpDispatchCertainty::PossiblyDispatched
        }
        ManagedPlaywrightDispatchCertainty::ResponseReceived => {
            McpDispatchCertainty::ResponseReceived
        }
    }
}

fn dispatch_certainty_rank(certainty: ManagedPlaywrightDispatchCertainty) -> u8 {
    match certainty {
        ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched => 0,
        ManagedPlaywrightDispatchCertainty::PossiblyDispatched => 1,
        ManagedPlaywrightDispatchCertainty::ResponseReceived => 2,
    }
}

fn safe_error_message(code: ManagedPlaywrightBridgeErrorCode) -> &'static str {
    match code {
        ManagedPlaywrightBridgeErrorCode::Cancelled => "managed Playwright operation cancelled",
        ManagedPlaywrightBridgeErrorCode::Timeout => "managed Playwright operation timed out",
        ManagedPlaywrightBridgeErrorCode::Closed => "managed Playwright Host is closed",
        ManagedPlaywrightBridgeErrorCode::Busy => "managed Playwright Host is busy",
        ManagedPlaywrightBridgeErrorCode::SurfaceUnavailable => "browser surface is unavailable",
        ManagedPlaywrightBridgeErrorCode::SurfaceCapacityExceeded => {
            "browser surface capacity was exceeded"
        }
        ManagedPlaywrightBridgeErrorCode::TargetClosed => "browser target was closed",
        ManagedPlaywrightBridgeErrorCode::ToolNotReviewed => "browser tool is not reviewed",
        ManagedPlaywrightBridgeErrorCode::InvalidArguments => "browser tool arguments are invalid",
        ManagedPlaywrightBridgeErrorCode::CatalogDrift => "browser tool catalog changed",
        ManagedPlaywrightBridgeErrorCode::OutputTooLarge => "browser tool output is too large",
        ManagedPlaywrightBridgeErrorCode::ProtocolError => "managed Playwright protocol failed",
        ManagedPlaywrightBridgeErrorCode::OutcomeUnknown => {
            "managed Playwright operation outcome is unknown"
        }
        ManagedPlaywrightBridgeErrorCode::InternalSafeError => {
            "managed Playwright Host operation failed"
        }
    }
}

fn parse_uuid_v4(value: &str) -> Result<Uuid, McpError> {
    let parsed = Uuid::from_str(value)
        .map_err(|_| McpError::protocol("managed Playwright request id is invalid"))?;
    if parsed.is_nil()
        || parsed.get_version() != Some(Version::Random)
        || parsed.to_string() != value
    {
        return Err(McpError::protocol(
            "managed Playwright request id is invalid",
        ));
    }
    Ok(parsed)
}

fn unix_millis() -> u64 {
    u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_mcp_client::{
        InMemoryMcpRegistry, McpApprovalMode, McpContentBlock, McpHostBridgeConfig,
        McpManagerPolicy, McpRegistry, McpServerScope, McpTrustLevel,
    };
    use std::collections::{BTreeSet, VecDeque};

    #[cfg(target_os = "macos")]
    use crate::application::mcp::browser_risk::BrowserRiskCoordinator;
    #[cfg(target_os = "macos")]
    use crate::application::mcp::builtin_capability_policy::{
        BuiltinCapabilityId as StoredCapabilityId, SqliteBuiltinCapabilityPolicyStore,
    };
    #[cfg(target_os = "macos")]
    use crate::application::mcp::builtin_capability_runtime::HostBuiltinCapabilityProvider;
    #[cfg(target_os = "macos")]
    use crate::application::mcp::playwright_manifest::BROWSER_AUTOMATION_CAPABILITY_ID;
    #[cfg(target_os = "macos")]
    use mycopilot_core::{
        AgentApprovalStatus, AgentBuiltinCapabilityActivationApproval, AgentEvent,
        AgentProposedAction, AgentRunStatus, BuiltinCapabilityRuntime, BuiltinMcpToolRiskKind,
        CapabilityGrant, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    };
    #[cfg(target_os = "macos")]
    use mycopilot_protocol_rs::{
        BrowserRiskAuthorizeInput, BrowserRiskCancelInput, BuiltinMcpToolRiskKindDto,
        ManagedPlaywrightBuiltinToolGrantContext, ManagedPlaywrightSensitiveBindingScopeDto,
        ManagedPlaywrightSensitiveFilePreparation, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
    };
    #[cfg(target_os = "macos")]
    use std::process::Stdio;
    #[cfg(target_os = "macos")]
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    #[cfg(target_os = "macos")]
    use tokio::process::Command;

    #[cfg(target_os = "macos")]
    const FIXTURE_AGENT_EVENT_METHOD: &str = "fixture.agentEvent";

    fn test_authorization_context() -> ManagedPlaywrightAuthorizationContext {
        ManagedPlaywrightAuthorizationContext {
            run_id: "run-test".to_string(),
            capability_id: "browser_automation".to_string(),
            activation_id: Uuid::new_v4().to_string(),
            manifest_digest: format!("sha256:{}", "a".repeat(64)),
            policy_revision: 1,
            grant_expires_at_ms: 2_000_000_000_000,
            invocation_id: Uuid::new_v4().to_string(),
            call_id: "call-test".to_string(),
            trigger_tool_name: "browser_snapshot".to_string(),
            call_reason: "Exercise the reverse bridge dispatch boundary.".to_string(),
            builtin_tool_grant: None,
        }
    }

    fn test_call_command() -> ManagedPlaywrightCommand {
        ManagedPlaywrightCommand::CallTool {
            name: "browser_snapshot".to_string(),
            arguments: json!({
                "call_reason": "Exercise the reverse bridge dispatch boundary."
            }),
            timeout_ms: 1_000,
            authorization_context: Box::new(test_authorization_context()),
        }
    }

    fn test_protocol_snapshot() -> McpProtocolSnapshot {
        serde_json::from_value(json!({
            "negotiatedVersion": "2025-11-25",
            "lifecycle": "initialize_fallback",
            "server": {"name": "@playwright/mcp", "version": "0.0.79"},
            "capabilities": {
                "tools": true,
                "toolsListChanged": false,
                "resources": false,
                "resourcesListChanged": false,
                "resourcesSubscribe": false,
                "prompts": false,
                "promptsListChanged": false,
                "logging": false,
                "completions": false,
                "tasks": false,
                "extensions": []
            }
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn reverse_peer_pre_dispatch_rejections_never_cross_the_queue_boundary() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let error = bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                None,
                Some(dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(bridge.pending_request_count(), 0);

        let cancelled_bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        cancelled_bridge.attach_outbound(outbound).unwrap();
        let cancellation = McpCancellationToken::new();
        cancellation.cancel();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let error = cancelled_bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                Some(cancellation),
                Some(dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Cancelled);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(cancelled_bridge.pending_request_count(), 0);

        let (outbound, commands) = mpsc::unbounded_channel();
        drop(commands);
        bridge.attach_outbound(outbound).unwrap();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let error = bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                None,
                Some(dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(bridge.pending_request_count(), 0);

        let poisoned_bridge = ManagedPlaywrightHostBridge::new(server_id);
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe({
            let poisoned_bridge = Arc::clone(&poisoned_bridge);
            move || {
                let _outbound = poisoned_bridge.outbound.lock().unwrap();
                panic!("poison the outbound lock");
            }
        }))
        .is_err());
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let error = poisoned_bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                None,
                Some(dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(poisoned_bridge.pending_request_count(), 0);

        bridge.close_now();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let error = bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                None,
                Some(dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn reverse_bridge_capacity_rejection_does_not_queue_or_leak_the_rejected_request() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let mut tasks = Vec::new();
        for _ in 0..MAX_PENDING_REQUESTS {
            let bridge = Arc::clone(&bridge);
            tasks.push(tokio::spawn(async move {
                let dispatch = McpDispatchTracker::new();
                dispatch.mark_dispatching();
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(10),
                        None,
                        Some(dispatch),
                    )
                    .await
            }));
            let command = commands.recv().await.unwrap();
            assert_eq!(
                command["method"],
                json!(MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD)
            );
        }
        assert_eq!(bridge.pending_request_count(), MAX_PENDING_REQUESTS);

        let rejected_dispatch = McpDispatchTracker::new();
        rejected_dispatch.mark_dispatching();
        let error = bridge
            .request(
                test_call_command(),
                PendingOperation::CallTool,
                Duration::from_secs(1),
                None,
                Some(rejected_dispatch.clone()),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Capacity);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(
            rejected_dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(bridge.pending_request_count(), MAX_PENDING_REQUESTS);

        bridge.abort_pending(ManagedPlaywrightCancelReason::Shutdown);
        for task in tasks {
            assert!(matches!(
                task.await.unwrap().unwrap(),
                ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::Closed,
                    dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
                }
            ));
        }
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn dropping_a_reverse_request_future_removes_pending_and_notifies_main() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let task = {
            let bridge = Arc::clone(&bridge);
            tokio::spawn(async move {
                let dispatch = McpDispatchTracker::new();
                dispatch.mark_dispatching();
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(10),
                        None,
                        Some(dispatch),
                    )
                    .await
            })
        };
        let command = commands.recv().await.unwrap();
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert_eq!(bridge.pending_request_count(), 1);

        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        assert_eq!(bridge.pending_request_count(), 0);
        let cancel = commands.recv().await.unwrap();
        let cancel_params: ManagedPlaywrightCancelNotification =
            serde_json::from_value(cancel["params"].clone()).unwrap();
        assert_eq!(cancel_params.request_id, params.request_id);
        assert_eq!(
            cancel_params.reason,
            ManagedPlaywrightCancelReason::Shutdown
        );
    }

    #[tokio::test]
    async fn successful_reverse_send_is_the_only_manager_queue_boundary() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let task = {
            let bridge = Arc::clone(&bridge);
            let dispatch = dispatch.clone();
            tokio::spawn(async move {
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(1),
                        None,
                        Some(dispatch),
                    )
                    .await
            })
        };
        let command = commands.recv().await.unwrap();
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::PossiblyDispatched
        );
        assert_eq!(bridge.pending_request_count(), 1);
        assert!(bridge
            .complete(ManagedPlaywrightCompletionInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id,
                outcome: ManagedPlaywrightCompletionOutcome::ToolCalled {
                    result: json!({"content": [], "structuredContent": null, "isError": false}),
                    host_image_publish_path: None,
                },
            })
            .unwrap());
        assert!(matches!(
            task.await.unwrap().unwrap(),
            ManagedPlaywrightCompletionOutcome::ToolCalled { .. }
        ));
        assert_eq!(dispatch.certainty(), McpDispatchCertainty::ResponseReceived);
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn cancellation_grace_accepts_exact_pre_dispatch_completion_without_leaking_pending() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let cancellation = McpCancellationToken::new();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let task = {
            let bridge = Arc::clone(&bridge);
            let cancellation = cancellation.clone();
            let dispatch = dispatch.clone();
            tokio::spawn(async move {
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(10),
                        Some(cancellation),
                        Some(dispatch),
                    )
                    .await
            })
        };
        let command = commands.recv().await.unwrap();
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert!(bridge
            .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id.clone(),
                phase: ManagedPlaywrightDispatchPhase::PreDispatch,
            })
            .unwrap());
        cancellation.cancel();
        let cancel = commands.recv().await.unwrap();
        assert_eq!(
            cancel["method"],
            json!(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD)
        );
        assert_eq!(bridge.pending_request_count(), 1);
        assert!(bridge
            .complete(ManagedPlaywrightCompletionInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id,
                outcome: ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::Cancelled,
                    dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
                },
            })
            .unwrap());
        let outcome = task.await.unwrap().unwrap();
        let error = error_from_outcome(outcome, PendingOperation::CallTool);
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Cancelled);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::DefinitelyNotDispatched)
        );
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn cancellation_after_main_dispatch_is_unknown_and_cleans_pending_after_grace() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let cancellation = McpCancellationToken::new();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let task = {
            let bridge = Arc::clone(&bridge);
            let cancellation = cancellation.clone();
            let dispatch = dispatch.clone();
            tokio::spawn(async move {
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(10),
                        Some(cancellation),
                        Some(dispatch),
                    )
                    .await
            })
        };
        let command = commands.recv().await.unwrap();
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert!(bridge
            .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id.clone(),
                phase: ManagedPlaywrightDispatchPhase::PossiblyDispatched,
            })
            .unwrap());
        assert!(bridge
            .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id,
                phase: ManagedPlaywrightDispatchPhase::PreDispatch,
            })
            .unwrap());
        cancellation.cancel();
        let _cancel = commands.recv().await.unwrap();
        assert_eq!(bridge.pending_request_count(), 1);
        let error = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(
            error.kind,
            mycopilot_mcp_client::McpErrorKind::OutcomeUnknown
        );
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::PossiblyDispatched)
        );
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn main_dispatch_completion_cannot_regress_acknowledged_certainty() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let dispatch = McpDispatchTracker::new();
        dispatch.mark_dispatching();
        let task = {
            let bridge = Arc::clone(&bridge);
            let dispatch = dispatch.clone();
            tokio::spawn(async move {
                bridge
                    .request(
                        test_call_command(),
                        PendingOperation::CallTool,
                        Duration::from_secs(1),
                        None,
                        Some(dispatch),
                    )
                    .await
            })
        };
        let command = commands.recv().await.unwrap();
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert!(bridge
            .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id.clone(),
                phase: ManagedPlaywrightDispatchPhase::PossiblyDispatched,
            })
            .unwrap());
        assert!(!bridge
            .complete(ManagedPlaywrightCompletionInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: params.request_id,
                outcome: ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::InvalidArguments,
                    dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
                },
            })
            .unwrap());
        let outcome = task.await.unwrap().unwrap();
        assert!(matches!(
            outcome,
            ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::ProtocolError,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::PossiblyDispatched,
            }
        ));
        assert_eq!(bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn managed_peer_rejects_not_ready_and_missing_invocation_or_authorization_pre_dispatch() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let peer = ManagedPlaywrightHostBridgePeer {
            server_id,
            protocol: test_protocol_snapshot(),
            bridge,
            request_timeout: Duration::from_secs(1),
            shutdown_timeout: Duration::from_secs(1),
            state: AtomicU8::new(STATE_CLOSED),
        };
        let dispatch = McpDispatchTracker::new();
        let error = peer
            .call_tool_tracked(
                McpToolCall {
                    name: "browser_snapshot".to_string(),
                    arguments: json!({}),
                    timeout_ms: None,
                    invocation_id: None,
                },
                McpCancellationToken::new(),
                dispatch.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Shutdown);
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );

        peer.state.store(STATE_READY, Ordering::Release);
        let dispatch = McpDispatchTracker::new();
        let error = peer
            .call_tool_tracked(
                McpToolCall {
                    name: "browser_snapshot".to_string(),
                    arguments: json!({}),
                    timeout_ms: None,
                    invocation_id: None,
                },
                McpCancellationToken::new(),
                dispatch.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol);
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );

        let dispatch = McpDispatchTracker::new();
        let error = peer
            .call_tool_tracked(
                McpToolCall {
                    name: "browser_snapshot".to_string(),
                    arguments: json!({}),
                    timeout_ms: None,
                    invocation_id: Some(McpInvocationId::new()),
                },
                McpCancellationToken::new(),
                dispatch.clone(),
            )
            .await
            .unwrap_err();
        assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol);
        assert_eq!(
            dispatch.certainty(),
            McpDispatchCertainty::DefinitelyNotDispatched
        );
        assert_eq!(peer.bridge.pending_request_count(), 0);
    }

    #[tokio::test]
    async fn connector_flows_through_manager_catalog_without_surface_identity() {
        let server_id = McpServerId::new();
        let bridge = ManagedPlaywrightHostBridge::new(server_id);
        let (outbound, mut commands) = mpsc::unbounded_channel();
        bridge.attach_outbound(outbound).unwrap();
        let registry = InMemoryMcpRegistry::shared();
        registry
            .add(McpServerConfig {
                id: server_id,
                display_name: "Managed browser automation".to_string(),
                scope: McpServerScope::Managed,
                trust: McpTrustLevel::Managed,
                approval_mode: McpApprovalMode::Auto,
                enabled: true,
                transport: McpTransportConfig::HostBridge(
                    McpHostBridgeConfig::new(MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL).unwrap(),
                ),
                connect_timeout_ms: 1_000,
                request_timeout_ms: 1_000,
                shutdown_timeout_ms: 1_000,
            })
            .unwrap();
        let manager = mycopilot_mcp_client::McpConnectionManager::without_events(
            registry,
            Arc::new(ManagedPlaywrightHostBridgeConnector::new(Arc::clone(
                &bridge,
            ))),
            McpManagerPolicy::default(),
        )
        .unwrap();
        let responder = tokio::spawn(async move {
            for _ in 0..2 {
                let command = commands.recv().await.unwrap();
                let params: ManagedPlaywrightCommandNotification =
                    serde_json::from_value(command["params"].clone()).unwrap();
                let outcome = match params.command {
                    ManagedPlaywrightCommand::Connect => {
                        ManagedPlaywrightCompletionOutcome::Connected {
                            protocol: json!({
                                "negotiatedVersion": "2025-11-25",
                                "lifecycle": "initialize_fallback",
                                "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                                "capabilities": {"tools": true, "toolsListChanged": false,
                                  "resources": false, "resourcesListChanged": false,
                                  "resourcesSubscribe": false, "prompts": false,
                                  "promptsListChanged": false, "logging": false,
                                  "completions": false, "tasks": false, "extensions": []}
                            }),
                        }
                    }
                    ManagedPlaywrightCommand::ListTools { cursor: None } => {
                        ManagedPlaywrightCompletionOutcome::ToolsListed {
                            page: json!({
                                "tools": [{"name":"browser_snapshot","title":null,
                                  "description":"Snapshot","inputSchema":{"type":"object",
                                  "properties":{"call_reason":{"type":"string"}},
                                  "required":["call_reason"],"additionalProperties":false},
                                  "outputSchema":null,"annotations":{"readOnlyHint":true,
                                  "destructiveHint":false,"idempotentHint":null,"openWorldHint":null,
                                  "title":null}}],
                                "nextCursor":null,"ttlMs":null,"cacheScope":null
                            }),
                        }
                    }
                    _ => panic!("unexpected command"),
                };
                bridge
                    .complete(ManagedPlaywrightCompletionInput {
                        schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                        request_id: params.request_id,
                        outcome,
                    })
                    .unwrap();
            }
        });

        let status = manager.start(server_id).await.unwrap();
        assert_eq!(status.state, mycopilot_mcp_client::McpServerState::Ready);
        let catalog = manager.catalog(server_id).unwrap().unwrap();
        assert_eq!(catalog.tools.len(), 1);
        assert!(catalog.tools[0].descriptor.input_schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("call_reason")));
        responder.await.unwrap();
    }

    fn attach_reviewed_runtime_responder(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    ) -> tokio::task::JoinHandle<()> {
        attach_reviewed_runtime_responder_with_call_error(runtime, None)
    }

    fn attach_reviewed_runtime_responder_with_call_error(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        call_error: Option<(
            ManagedPlaywrightBridgeErrorCode,
            ManagedPlaywrightDispatchCertainty,
        )>,
    ) -> tokio::task::JoinHandle<()> {
        let (outbound, mut commands) = mpsc::unbounded_channel();
        runtime.bridge.attach_outbound(outbound).unwrap();
        let bridge = Arc::clone(&runtime.bridge);
        tokio::spawn(async move {
            while let Some(command) = commands.recv().await {
                let params: ManagedPlaywrightCommandNotification =
                    serde_json::from_value(command["params"].clone()).unwrap();
                let outcome = match params.command {
                    ManagedPlaywrightCommand::Connect => {
                        ManagedPlaywrightCompletionOutcome::Connected {
                            protocol: json!({
                                "negotiatedVersion": "2025-11-25",
                                "lifecycle": "initialize_fallback",
                                "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                                "capabilities": {"tools": true, "toolsListChanged": false,
                                  "resources": false, "resourcesListChanged": false,
                                  "resourcesSubscribe": false, "prompts": false,
                                  "promptsListChanged": false, "logging": false,
                                  "completions": false, "tasks": false, "extensions": []}
                            }),
                        }
                    }
                    ManagedPlaywrightCommand::ListTools { cursor: None } => {
                        let manifest = load_playwright_browser_manifest().unwrap();
                        ManagedPlaywrightCompletionOutcome::ToolsListed {
                            page: json!({
                                "tools": manifest.tools.into_iter().map(|tool| json!({
                                    "name": tool.tool_id,
                                    "title": null,
                                    "description": tool.description,
                                    "inputSchema": tool.input_schema,
                                    "outputSchema": null,
                                    "annotations": {"readOnlyHint": false,
                                      "destructiveHint": false,
                                      "idempotentHint": null, "openWorldHint": null, "title": null}
                                })).collect::<Vec<_>>(),
                                "nextCursor": null,
                                "ttlMs": null,
                                "cacheScope": null
                            }),
                        }
                    }
                    ManagedPlaywrightCommand::CallTool { .. } => {
                        if let Some((code, dispatch_certainty)) = call_error {
                            ManagedPlaywrightCompletionOutcome::Error {
                                code,
                                dispatch_certainty,
                            }
                        } else {
                            ManagedPlaywrightCompletionOutcome::ToolCalled {
                                result: json!({"content":[{"type":"text","text":"ok"}],
                                  "structuredContent":null,"isError":false}),
                                host_image_publish_path: None,
                            }
                        }
                    }
                    ManagedPlaywrightCommand::PrepareSensitiveTool { input } => {
                        ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                            binding_id: Uuid::new_v4().to_string(),
                            target_binding_digest: format!("sha256:{}", "7".repeat(64)),
                            origin: Some("https://mail.example.test".to_string()),
                            created_at_ms: input.created_at_ms,
                            expires_at_ms: input.expires_at_ms,
                            file_basenames: Vec::new(),
                            file_revision_digest: None,
                        }
                    }
                    ManagedPlaywrightCommand::ReleaseSensitiveToolBinding { .. } => {
                        ManagedPlaywrightCompletionOutcome::SensitiveToolBindingReleased {
                            released: true,
                        }
                    }
                    ManagedPlaywrightCommand::Close => ManagedPlaywrightCompletionOutcome::Closed,
                    ManagedPlaywrightCommand::ListTools { cursor: Some(_) } => {
                        panic!("reviewed catalog is not paginated")
                    }
                };
                bridge
                    .complete(ManagedPlaywrightCompletionInput {
                        schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                        request_id: params.request_id,
                        outcome,
                    })
                    .unwrap();
            }
        })
    }

    fn deterministic_idle_sleeper() -> (
        ManagedPlaywrightIdleSleeper,
        Arc<StdMutex<VecDeque<oneshot::Sender<()>>>>,
    ) {
        let pending = Arc::new(StdMutex::new(VecDeque::new()));
        let pending_for_sleep = Arc::clone(&pending);
        let sleeper: ManagedPlaywrightIdleSleeper = Arc::new(move |_| {
            let (release, wait) = oneshot::channel();
            pending_for_sleep.lock().unwrap().push_back(release);
            Box::pin(async move {
                let _ = wait.await;
            })
        });
        (sleeper, pending)
    }

    async fn release_next_idle_timer(pending: &Arc<StdMutex<VecDeque<oneshot::Sender<()>>>>) {
        for _ in 0..100 {
            if let Some(release) = pending.lock().unwrap().pop_front() {
                let _ = release.send(());
                return;
            }
            tokio::task::yield_now().await;
        }
        panic!("managed Playwright idle timer was not registered");
    }

    async fn join_owned_lifecycle_tasks(runtime: &ManagedPlaywrightMcpRuntime) {
        let mut tasks = runtime
            .lifecycle_tasks
            .lock()
            .map(|mut tasks| std::mem::replace(&mut *tasks, JoinSet::new()))
            .unwrap();
        while let Some(result) = tasks.join_next().await {
            if let Err(error) = result.unwrap() {
                assert!(
                    matches!(
                        error.kind,
                        mycopilot_mcp_client::McpErrorKind::Cancelled
                            | mycopilot_mcp_client::McpErrorKind::Shutdown
                    ),
                    "unexpected lifecycle error: {error:?}"
                );
                assert_eq!(
                    error.dispatch_certainty,
                    Some(mycopilot_mcp_client::McpDispatchCertainty::DefinitelyNotDispatched),
                    "lifecycle cancellation before Tool dispatch must remain authoritative"
                );
            }
        }
    }

    #[tokio::test]
    async fn overlapping_start_generations_leave_reviewed_runtime_ready() {
        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let responder = attach_reviewed_runtime_responder(&runtime);
        runtime.request_start().unwrap();
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        assert_eq!(
            runtime
                .manager
                .get_status(runtime.server_id)
                .unwrap()
                .unwrap()
                .state,
            mycopilot_mcp_client::McpServerState::Ready
        );
        runtime.shutdown().await;
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn stale_stop_generation_cannot_close_newer_start() {
        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let responder = attach_reviewed_runtime_responder(&runtime);
        runtime.request_start().unwrap();
        runtime.request_stop().unwrap();
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        assert_eq!(
            runtime
                .manager
                .get_status(runtime.server_id)
                .unwrap()
                .unwrap()
                .state,
            mycopilot_mcp_client::McpServerState::Ready
        );
        runtime.shutdown().await;
        responder.await.unwrap();
    }

    #[tokio::test]
    async fn trusted_main_pre_dispatch_rejection_stays_definite_through_runtime_manager() {
        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let responder = attach_reviewed_runtime_responder_with_call_error(
            &runtime,
            Some((
                ManagedPlaywrightBridgeErrorCode::InvalidArguments,
                ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            )),
        );
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;

        let error = invoke_browser_tool_result(
            &runtime,
            "browser_snapshot",
            json!({"call_reason": "Exercise an authoritative pre-dispatch rejection."}),
        )
        .await
        .expect_err("trusted Main rejection must remain an error");
        assert!(
            matches!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol),
            "unexpected trusted Main rejection: {error:?}"
        );
        assert_eq!(
            error.dispatch_certainty,
            Some(mycopilot_mcp_client::McpDispatchCertainty::DefinitelyNotDispatched)
        );

        runtime.shutdown().await;
        responder.await.unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_tab_count_accepts_the_fixed_official_text_result() {
        let result = McpToolResult {
            content: vec![McpContentBlock::Text {
                text: "### Open tabs\n- 0: Fixture (current)\n- 1: Secondary Fixture".to_string(),
            }],
            structured_content: None,
            is_error: false,
        };
        assert_eq!(managed_tab_count(&result), 2);
    }

    #[tokio::test]
    async fn idle_policy_reuses_one_timer_and_reactivates_for_one_hundred_cycles() {
        let (sleeper, pending_idle) = deterministic_idle_sleeper();
        let runtime =
            ManagedPlaywrightMcpRuntime::with_idle_policy(Duration::from_secs(10 * 60), sleeper)
                .unwrap();
        let responder = attach_reviewed_runtime_responder(&runtime);
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        assert!(runtime.is_ready());

        for cycle in 0..100 {
            release_next_idle_timer(&pending_idle).await;
            for _ in 0..100 {
                if !runtime.is_ready() {
                    break;
                }
                tokio::task::yield_now().await;
            }
            assert!(!runtime.is_ready(), "idle stop failed at cycle {cycle}");
            let result = invoke_browser_tool(
                &runtime,
                "browser_snapshot",
                json!({"call_reason": format!("idle cycle {cycle}")}),
            )
            .await;
            assert!(!result.is_error, "reactivation failed at cycle {cycle}");
            assert!(runtime.is_ready());
            assert_eq!(runtime.lifecycle_task_count(), 0);
            assert!(pending_idle.lock().unwrap().len() <= 1);
        }

        runtime.shutdown().await;
        responder.await.unwrap();
        assert!(pending_idle.lock().unwrap().len() <= 1);
    }

    #[tokio::test]
    async fn stop_aborts_a_hung_connect_within_the_shutdown_budget() {
        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let (outbound, mut commands) = mpsc::unbounded_channel();
        runtime.bridge.attach_outbound(outbound).unwrap();
        runtime.request_start().unwrap();
        let command = tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("connect notification timeout")
            .expect("connect notification");
        let params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(command["params"].clone()).unwrap();
        assert!(matches!(params.command, ManagedPlaywrightCommand::Connect));

        tokio::time::timeout(Duration::from_secs(2), runtime.stop())
            .await
            .expect("managed runtime stop exceeded its bounded budget")
            .unwrap();
        let cancel = tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("cancel notification timeout")
            .expect("cancel notification");
        assert_eq!(
            cancel["method"],
            json!(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD)
        );
        let cancel_params: ManagedPlaywrightCancelNotification =
            serde_json::from_value(cancel["params"].clone()).unwrap();
        assert_eq!(cancel_params.request_id, params.request_id);
        assert_eq!(
            cancel_params.reason,
            ManagedPlaywrightCancelReason::Shutdown
        );
        join_owned_lifecycle_tasks(&runtime).await;
        assert_ne!(
            runtime
                .manager
                .get_status(runtime.server_id)
                .unwrap()
                .unwrap()
                .state,
            mycopilot_mcp_client::McpServerState::Ready
        );
        runtime.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_aborts_a_hung_catalog_discovery_within_the_shutdown_budget() {
        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let (outbound, mut commands) = mpsc::unbounded_channel();
        runtime.bridge.attach_outbound(outbound).unwrap();
        runtime.request_start().unwrap();
        let connect = tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("connect notification timeout")
            .expect("connect notification");
        let connect_params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(connect["params"].clone()).unwrap();
        runtime
            .bridge
            .complete(ManagedPlaywrightCompletionInput {
                schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                request_id: connect_params.request_id,
                outcome: ManagedPlaywrightCompletionOutcome::Connected {
                    protocol: json!({
                        "negotiatedVersion": "2025-11-25",
                        "lifecycle": "initialize_fallback",
                        "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                        "capabilities": {"tools": true, "toolsListChanged": false,
                          "resources": false, "resourcesListChanged": false,
                          "resourcesSubscribe": false, "prompts": false,
                          "promptsListChanged": false, "logging": false,
                          "completions": false, "tasks": false, "extensions": []}
                    }),
                },
            })
            .unwrap();
        let list = tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("list notification timeout")
            .expect("list notification");
        let list_params: ManagedPlaywrightCommandNotification =
            serde_json::from_value(list["params"].clone()).unwrap();
        assert!(matches!(
            list_params.command,
            ManagedPlaywrightCommand::ListTools { .. }
        ));

        tokio::time::timeout(Duration::from_secs(3), runtime.shutdown())
            .await
            .expect("managed runtime shutdown exceeded its bounded budget");
        let cancel = tokio::time::timeout(Duration::from_secs(1), commands.recv())
            .await
            .expect("cancel notification timeout")
            .expect("cancel notification");
        let cancel_params: ManagedPlaywrightCancelNotification =
            serde_json::from_value(cancel["params"].clone()).unwrap();
        assert_eq!(cancel_params.request_id, list_params.request_id);
        assert_eq!(
            cancel_params.reason,
            ManagedPlaywrightCancelReason::Shutdown
        );
    }

    /// Cross-language release gate for the production managed-browser stack.
    ///
    /// The normal Vitest wrapper builds the repository-owned Electron helper and supplies these
    /// two paths. Keeping this test ignored avoids starting Electron from an ordinary Rust-only
    /// test run while still making the full gate explicit and reproducible.
    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "run through the managed Playwright Electron fixture wrapper"]
    async fn managed_playwright_official_electron_e2e() {
        const READY_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_READY=";
        const COMPLETION_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_COMPLETION=";
        const DISPATCH_PHASE_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_DISPATCH_PHASE=";
        const RESULT_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_RESULT=";
        const RISK_AUTHORIZE_MARKER: &str = "MYCOPILOT_BROWSER_RISK_AUTHORIZE=";
        const RISK_CANCEL_MARKER: &str = "MYCOPILOT_BROWSER_RISK_CANCEL=";

        let electron = std::env::var("MYCOPILOT_MANAGED_PLAYWRIGHT_ELECTRON")
            .expect("Electron fixture executable must be supplied by the Vitest wrapper");
        let fixture = std::env::var("MYCOPILOT_MANAGED_PLAYWRIGHT_FIXTURE")
            .expect("managed Playwright fixture bundle must be supplied by the Vitest wrapper");
        let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("resolve workspace root");

        let mut child = Command::new(electron)
            .arg(fixture)
            .current_dir(workspace)
            .env_remove("ELECTRON_RUN_AS_NODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("start repository-owned managed Playwright Electron fixture");
        let mut child_stdin = child.stdin.take().expect("fixture stdin");
        let child_stdout = child.stdout.take().expect("fixture stdout");
        let child_stderr = child.stderr.take().expect("fixture stderr");

        let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
        let bridge = runtime.bridge();
        let (outbound, mut outbound_commands) = mpsc::unbounded_channel::<Value>();
        let fixture_events = outbound.clone();
        // The reader must be able to answer Main's reverse Browser-risk request on the same
        // newline-framed stdin without retaining a Sender forever (which would deadlock fixture
        // shutdown). The slot is explicitly cleared after managed runtime shutdown.
        let fixture_input = Arc::new(StdMutex::new(Some(outbound.clone())));
        bridge.attach_outbound(outbound).unwrap();

        let (
            _risk_directory,
            _risk_storage,
            capability_runtime,
            risk_coordinator,
            capability_grant,
        ) = managed_playwright_e2e_risk_authority();
        let (risk_notifications, mut risk_approval_events) = mpsc::unbounded_channel::<Value>();
        let approval_coordinator = Arc::clone(&risk_coordinator);
        let approval_count = Arc::new(AtomicUsize::new(0));
        let observed_approval_count = Arc::clone(&approval_count);
        let risk_approver = tokio::spawn(async move {
            while let Some(notification) = risk_approval_events.recv().await {
                let action: AgentProposedAction =
                    serde_json::from_value(notification["params"]["action"].clone())
                        .expect("parse typed Browser-risk approval notification");
                let AgentProposedAction::BrowserRiskApproval { approval } = action else {
                    panic!("managed fixture emitted a non-Browser risk approval");
                };
                assert!(approval.trigger_tool_name.starts_with("browser_"));
                match observed_approval_count.fetch_add(1, Ordering::AcqRel) {
                    0 => assert!(approval_coordinator
                        .approve(&approval.run_id, &approval.action_id)
                        .expect("approve fixture Browser risk")
                        .is_some()),
                    1 => assert!(approval_coordinator
                        .reject(
                            &approval.run_id,
                            &approval.action_id,
                            Some("The fixture user declined this destination.".to_string()),
                        )
                        .expect("reject fixture Browser risk")
                        .is_some()),
                    2 => {
                        assert_eq!(
                            approval.trigger,
                            mycopilot_core::BrowserRiskTrigger::Redirect
                        );
                        assert!(approval_coordinator
                            .reject(
                                &approval.run_id,
                                &approval.action_id,
                                Some("The fixture user declined this redirect.".to_string()),
                            )
                            .expect("reject fixture Browser redirect risk")
                            .is_some());
                    }
                    unexpected => panic!("unexpected Browser-risk approval #{unexpected}"),
                }
            }
        });

        let writer = tokio::spawn(async move {
            while let Some(command) = outbound_commands.recv().await {
                eprintln!(
                    "managed Playwright bridge outbound: {} {}",
                    command["method"].as_str().unwrap_or("invalid"),
                    command["params"]["command"]["type"]
                        .as_str()
                        .unwrap_or("cancel")
                );
                let encoded = serde_json::to_vec(&command).expect("serialize bridge notification");
                child_stdin
                    .write_all(&encoded)
                    .await
                    .expect("write fixture command");
                child_stdin
                    .write_all(b"\n")
                    .await
                    .expect("frame fixture command");
                child_stdin.flush().await.expect("flush fixture command");
            }
        });

        let (ready_sender, ready_receiver) = oneshot::channel::<Value>();
        let (result_sender, result_receiver) = oneshot::channel::<Value>();
        let reader_bridge = Arc::clone(&bridge);
        let reader_risk_coordinator = Arc::clone(&risk_coordinator);
        let reader_fixture_input = Arc::clone(&fixture_input);
        let risk_authorize_count = Arc::new(AtomicUsize::new(0));
        let observed_authorize_count = Arc::clone(&risk_authorize_count);
        let reader = tokio::spawn(async move {
            let mut ready_sender = Some(ready_sender);
            let mut result_sender = Some(result_sender);
            let mut lines = BufReader::new(child_stdout).lines();
            while let Some(line) = lines.next_line().await.expect("read fixture protocol line") {
                if let Some(payload) = line.strip_prefix(READY_MARKER) {
                    let value: Value = serde_json::from_str(payload).expect("parse fixture ready");
                    if let Some(sender) = ready_sender.take() {
                        let _ = sender.send(value);
                    }
                    continue;
                }
                if let Some(payload) = line.strip_prefix(COMPLETION_MARKER) {
                    let completion: ManagedPlaywrightCompletionInput =
                        serde_json::from_str(payload).expect("parse fixture completion");
                    assert!(reader_bridge
                        .complete(completion)
                        .expect("complete bridge request"));
                    continue;
                }
                if let Some(payload) = line.strip_prefix(DISPATCH_PHASE_MARKER) {
                    let phase: ManagedPlaywrightDispatchPhaseInput =
                        serde_json::from_str(payload).expect("parse fixture dispatch phase");
                    let request_id = phase.request_id.clone();
                    let phase_name = phase.phase;
                    let accepted = reader_bridge
                        .acknowledge_dispatch_phase(phase)
                        .expect("acknowledge fixture dispatch phase");
                    let response = json!({
                        "jsonrpc": "2.0",
                        "method": "fixture.managedPlaywright.dispatchPhaseAck",
                        "params": {
                            "requestId": request_id,
                            "phase": phase_name,
                            "accepted": accepted,
                        },
                    });
                    reader_fixture_input
                        .lock()
                        .expect("fixture input lock")
                        .as_ref()
                        .expect("fixture input remains attached while acknowledging dispatch")
                        .send(response)
                        .expect("write fixture dispatch phase acknowledgement");
                    continue;
                }
                if let Some(payload) = line.strip_prefix(RISK_AUTHORIZE_MARKER) {
                    observed_authorize_count.fetch_add(1, Ordering::AcqRel);
                    let input: BrowserRiskAuthorizeInput =
                        serde_json::from_str(payload).expect("parse fixture Browser-risk request");
                    let request_id = input.request_id.clone();
                    let output = reader_risk_coordinator
                        .authorize(
                            input,
                            "managed-playwright-e2e-conversation".to_string(),
                            "managed-playwright-e2e-assistant".to_string(),
                            risk_notifications.clone(),
                        )
                        .await;
                    let response = json!({
                        "jsonrpc": "2.0",
                        "method": "fixture.browserRisk.decision",
                        "params": {"requestId": request_id, "output": output},
                    });
                    reader_fixture_input
                        .lock()
                        .expect("fixture input lock")
                        .as_ref()
                        .expect("fixture input remains attached while authorizing")
                        .send(response)
                        .expect("write fixture Browser-risk decision");
                    continue;
                }
                if let Some(payload) = line.strip_prefix(RISK_CANCEL_MARKER) {
                    let input: BrowserRiskCancelInput =
                        serde_json::from_str(payload).expect("parse fixture Browser-risk cancel");
                    assert_eq!(input.schema_version, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION);
                    reader_risk_coordinator.cancel_request(input);
                    continue;
                }
                if let Some(payload) = line.strip_prefix(RESULT_MARKER) {
                    let value: Value = serde_json::from_str(payload).expect("parse fixture result");
                    if let Some(sender) = result_sender.take() {
                        let _ = sender.send(value);
                    }
                }
            }
            reader_bridge.close_now();
        });
        let stderr_reader = tokio::spawn(async move {
            let mut stderr = String::new();
            let mut lines = BufReader::new(child_stderr.take(1_000_000)).lines();
            while let Some(line) = lines.next_line().await.expect("read fixture stderr") {
                if stderr.len() < 64 * 1024 {
                    stderr.push_str(&line);
                    stderr.push('\n');
                }
                eprintln!("managed Playwright fixture: {line}");
            }
            stderr
        });

        let ready = tokio::time::timeout(Duration::from_secs(15), ready_receiver)
            .await
            .expect("fixture ready timeout")
            .expect("fixture ready channel");
        let fixture_url = ready["fixtureUrl"]
            .as_str()
            .expect("fixture URL")
            .to_string();
        let secondary_url = ready["secondaryUrl"]
            .as_str()
            .expect("secondary fixture URL")
            .to_string();
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        let status = runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap();
        assert_eq!(status.state, mycopilot_mcp_client::McpServerState::Ready);
        let catalog = runtime.manager.catalog(runtime.server_id).unwrap().unwrap();
        let reviewed_manifest = load_playwright_browser_manifest().unwrap();
        assert_eq!(catalog.tools.len(), reviewed_manifest.tools.len());
        let catalog_names = catalog
            .tools
            .iter()
            .map(|tool| tool.raw_name.as_str())
            .collect::<BTreeSet<_>>();
        for expected in [
            "browser_drag",
            "browser_handle_dialog",
            "browser_hover",
            "browser_navigate_back",
            "browser_select_option",
        ] {
            assert!(catalog_names.contains(expected), "{expected}");
        }
        let forbidden = "browser_run_code_unsafe";
        assert!(!catalog_names.contains(forbidden), "{forbidden}");

        let fixture_origin = fixture_url
            .strip_suffix("/interactive")
            .expect("fixture origin");
        let zero_tab_cookies = invoke_approved_browser_sensitive_tool_after_waiting_event(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_list",
            json!({
                "path": "/",
                "call_reason": "Verify the managed browser profile before any visible tab exists."
            }),
            vec![BuiltinMcpToolRiskKind::CookieRead],
            vec![BuiltinMcpToolRiskKindDto::CookieRead],
            &fixture_events,
        )
        .await;
        assert!(
            !zero_tab_cookies.is_error,
            "zero-tab managed cookie list failed: {}",
            tool_result_text(&zero_tab_cookies)
        );
        let first_tab = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_navigate",
            json!({
                "url": fixture_url.clone(),
                "call_reason": "Navigate the empty managed context directly to the first local fixture."
            }),
        )
        .await;
        assert!(
            !first_tab.is_error,
            "managed first browser_navigate failed: {}",
            tool_result_text(&first_tab)
        );
        let first_tab_list = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({
                "action": "list",
                "call_reason": "Verify the empty group became exactly one managed tab."
            }),
        )
        .await;
        let first_tab_list_text = tool_result_text(&first_tab_list);
        assert!(
            !first_tab_list.is_error && first_tab_list_text.contains("0:"),
            "first managed tab was not listed: {first_tab_list_text}"
        );
        assert!(
            !first_tab_list_text.lines().any(|line| {
                let line = line.trim_start();
                line.starts_with("1:") || line.starts_with("- 1:")
            }),
            "zero-group browser_tabs new created more than one tab: {first_tab_list_text}"
        );
        let approval_sequence_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Locate the local approval-lifecycle smoke button."}),
        )
        .await;
        let approval_sequence_snapshot_text = tool_result_text(&approval_sequence_snapshot);
        let approval_sequence_button_ref = snapshot_ref(&approval_sequence_snapshot_text, "Apply");
        let approval_sequence_click = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": approval_sequence_button_ref,
                "call_reason": "Click before the approved local script lifecycle check."
            }),
        )
        .await;
        assert!(
            !approval_sequence_click.is_error,
            "managed pre-approval browser_click failed: {}",
            tool_result_text(&approval_sequence_click)
        );
        let workspace_fixture_directory = tempfile::Builder::new()
            .prefix(".mycopilot-managed-playwright-files-")
            .tempdir_in(workspace)
            .expect("create repository-local managed file fixture directory");
        let admissions_fixture = workspace_fixture_directory
            .path()
            .join("浙江大学2026年招生资料汇编.pptx");
        let admissions_fixture_content =
            b"repository-owned synthetic admissions presentation fixture";
        std::fs::write(&admissions_fixture, admissions_fixture_content)
            .expect("write synthetic admissions presentation fixture");
        let admissions_fixture = admissions_fixture
            .canonicalize()
            .expect("canonicalize admissions presentation fixture")
            .to_string_lossy()
            .into_owned();
        let large_drop_fixture = workspace_fixture_directory
            .path()
            .join("fixture-large-drop.bin");
        std::fs::write(&large_drop_fixture, vec![b'L'; 1_100_000])
            .expect("write large managed drop fixture");
        let large_drop_fixture = large_drop_fixture
            .canonicalize()
            .expect("canonicalize large managed drop fixture")
            .to_string_lossy()
            .into_owned();
        let storage_state_fixture = workspace_fixture_directory
            .path()
            .join("fixture-storage-state.json");
        std::fs::write(
            &storage_state_fixture,
            serde_json::to_vec(&json!({
                "cookies": [{
                    "name": "restored-cookie",
                    "value": "repository-owned-storage-cookie",
                    "domain": "127.0.0.1",
                    "path": "/",
                    "expires": -1,
                    "httpOnly": false,
                    "secure": false,
                    "sameSite": "Lax"
                }],
                "origins": [{
                    "origin": fixture_origin,
                    "localStorage": [{
                        "name": "restored-local",
                        "value": "repository-owned-storage-local-value"
                    }]
                }]
            }))
            .expect("serialize storage-state fixture"),
        )
        .expect("write storage-state fixture");
        let storage_state_fixture = storage_state_fixture
            .canonicalize()
            .expect("canonicalize storage-state fixture")
            .to_string_lossy()
            .into_owned();
        let evaluate_started = tokio::time::Instant::now();
        let evaluated = invoke_approved_browser_evaluate(
            &runtime,
            &capability_grant,
            fixture_origin,
            &fixture_events,
        )
        .await;
        assert!(
            evaluate_started.elapsed() < Duration::from_secs(5),
            "synchronous fixed-official browser_evaluate did not complete promptly"
        );
        assert!(
            !evaluated.is_error,
            "managed browser_evaluate failed: {}",
            tool_result_text(&evaluated)
        );
        assert!(tool_result_text(&evaluated).contains("builtin-evaluate-ok"));

        let snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the local fixture."}),
        )
        .await;
        let snapshot_text = tool_result_text(&snapshot);
        assert!(snapshot_text.contains("Managed Playwright Bridge Fixture"));
        assert!(snapshot_text.contains("evaluated:你好"));
        let evaluated_wait = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({
                "text": "evaluated:你好",
                "call_reason": "Verify ordinary page tools remain usable after approved evaluation."
            }),
        )
        .await;
        assert!(
            !evaluated_wait.is_error
                && tool_result_text(&evaluated_wait).contains("evaluated:你好"),
            "managed post-approval browser_wait_for failed: {}",
            tool_result_text(&evaluated_wait)
        );
        let cross_frame_subject_ref = snapshot_ref(&snapshot_text, "Cross Frame Subject");
        let cross_frame_evaluate = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_evaluate",
            json!({
                "element": "Cross-origin local fixture subject",
                "target": cross_frame_subject_ref,
                "function": r#"(element) => {
                    element.value = 'cross-frame-evaluated';
                    element.dispatchEvent(new Event('input', { bubbles: true }));
                    element.ownerDocument.querySelector('#evaluate-value').textContent =
                        'evaluate:' + element.value;
                    return element.value;
                }"#,
                "call_reason": "Evaluate the exact OOPIF fixture element in the managed surface."
            }),
            vec![BuiltinMcpToolRiskKind::PageScriptExecution],
            vec![BuiltinMcpToolRiskKindDto::PageScriptExecution],
        )
        .await;
        assert!(
            !cross_frame_evaluate.is_error,
            "managed OOPIF browser_evaluate failed: {}",
            tool_result_text(&cross_frame_evaluate)
        );
        assert!(tool_result_text(&cross_frame_evaluate).contains("cross-frame-evaluated"));

        let upload_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Locate the repository-owned file chooser fixture."}),
        )
        .await;
        let upload_snapshot_text = tool_result_text(&upload_snapshot);
        assert!(upload_snapshot_text.contains("evaluate:cross-frame-evaluated"));
        let upload_ref = snapshot_ref(&upload_snapshot_text, "Fixture upload");
        let chooser = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": upload_ref.clone(),
                "call_reason": "Open the repository-owned fixture file chooser."
            }),
        )
        .await;
        assert!(
            !chooser.is_error,
            "managed fixture chooser click failed: {}",
            tool_result_text(&chooser)
        );
        let cancelled_chooser = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_file_upload",
            json!({
                "call_reason": "Cancel the pending fixture chooser using fixed official semantics."
            }),
        )
        .await;
        assert!(
            !cancelled_chooser.is_error,
            "fixed official chooser cancel failed: {}",
            tool_result_text(&cancelled_chooser)
        );
        assert!(!tool_result_text(&cancelled_chooser).contains("browser-file:"));
        let reopened_chooser = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": upload_ref,
                "call_reason": "Reopen the chooser for the one-call workspace attachment."
            }),
        )
        .await;
        assert!(
            !reopened_chooser.is_error,
            "managed fixture chooser reopen failed: {}",
            tool_result_text(&reopened_chooser)
        );
        let uploaded = invoke_approved_browser_sensitive_tool_with_resolved_files(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_file_upload",
            json!({
                "paths": [admissions_fixture.clone()],
                "call_reason": "Upload the exact workspace admissions presentation in one original call."
            }),
            vec![
                BuiltinMcpToolRiskKind::FileRead,
                BuiltinMcpToolRiskKind::FileUpload,
            ],
            vec![
                BuiltinMcpToolRiskKindDto::FileRead,
                BuiltinMcpToolRiskKindDto::FileUpload,
            ],
            vec![admissions_fixture.clone()],
        )
        .await;
        assert!(
            !uploaded.is_error,
            "managed browser_file_upload failed: {}",
            tool_result_text(&uploaded)
        );
        assert!(!tool_result_text(&uploaded).contains(&admissions_fixture));
        let admissions_basename = "浙江大学2026年招生资料汇编.pptx";
        let upload_selection = format!(
            "upload-selection:{admissions_basename}:{}",
            admissions_fixture_content.len()
        );
        let upload_effect = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({
                "text": upload_selection,
                "call_reason": "Wait for the exact fixture file chooser selection effect."
            }),
        )
        .await;
        assert!(
            !upload_effect.is_error,
            "managed fixture upload effect missing: {}",
            tool_result_text(&upload_effect)
        );
        let upload_text = format!(
            "upload:{admissions_basename}:{}",
            std::str::from_utf8(admissions_fixture_content).expect("fixture UTF-8")
        );
        let upload_content = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({
                "text": upload_text,
                "call_reason": "Wait for delayed File.text() after the upload tool returned."
            }),
        )
        .await;
        assert!(
            !upload_content.is_error,
            "managed fixture delayed file read failed: {}",
            tool_result_text(&upload_content)
        );
        let submit_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Locate the delayed fixture upload submit action."}),
        )
        .await;
        let submit_ref = snapshot_ref(&tool_result_text(&submit_snapshot), "Submit fixture upload");
        let submitted = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": submit_ref,
                "call_reason": "Submit the selected fixture after browser_file_upload returned."
            }),
        )
        .await;
        assert!(
            !submitted.is_error,
            "managed fixture delayed upload submit failed: {}",
            tool_result_text(&submitted)
        );
        let submit_effect = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({
                "text": format!("upload-submit:{admissions_basename}:content-ok"),
                "call_reason": "Verify the local fixture received the delayed file submission."
            }),
        )
        .await;
        assert!(
            !submit_effect.is_error,
            "managed fixture delayed upload receipt missing: {}",
            tool_result_text(&submit_effect)
        );

        let drop_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Locate the repository-owned cross-frame drop fixture."}),
        )
        .await;
        let drop_snapshot_text = tool_result_text(&drop_snapshot);
        assert!(drop_snapshot_text.contains(&upload_selection));
        let cross_frame_drop_ref = snapshot_ref(&drop_snapshot_text, "Cross file drop target");
        let dropped = invoke_approved_browser_sensitive_tool_with_resolved_files(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_drop",
            json!({
                "element": "Cross-origin local fixture file drop target",
                "target": cross_frame_drop_ref,
                "paths": [admissions_fixture.clone()],
                "call_reason": "Drop the exact workspace admissions presentation in the managed OOPIF."
            }),
            vec![
                BuiltinMcpToolRiskKind::FileRead,
                BuiltinMcpToolRiskKind::FileUpload,
            ],
            vec![
                BuiltinMcpToolRiskKindDto::FileRead,
                BuiltinMcpToolRiskKindDto::FileUpload,
            ],
            vec![admissions_fixture.clone()],
        )
        .await;
        assert!(
            !dropped.is_error,
            "managed browser_drop(paths) failed: {}",
            tool_result_text(&dropped)
        );
        let drop_diagnostic = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the exact OOPIF drop event effect."}),
        )
        .await;
        let dropped_text = tool_result_text(&drop_diagnostic);
        assert!(
            dropped_text.contains(&format!(
                "drop-selection:{admissions_basename}:{}",
                admissions_fixture_content.len()
            )),
            "managed browser_drop returned success without its exact drop effect: {dropped_text}"
        );
        assert!(
            dropped_text.contains("drop-events:dragenter,dragover,drop"),
            "managed browser_drop event order drifted: {dropped_text}"
        );
        assert!(
            dropped_text.contains(&format!(
                "drop:{admissions_basename}:{}",
                std::str::from_utf8(admissions_fixture_content).expect("fixture UTF-8")
            )),
            "managed browser_drop file bytes were not readable: {dropped_text}"
        );

        let large_drop_ref = snapshot_ref(&dropped_text, "Cross file drop target");
        let large_dropped = invoke_approved_browser_sensitive_tool_with_resolved_files(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_drop",
            json!({
                "element": "Cross-origin local large-file drop target",
                "target": large_drop_ref,
                "paths": [large_drop_fixture.clone()],
                "call_reason": "Drop a brokered file larger than the managed CDP string limit."
            }),
            vec![
                BuiltinMcpToolRiskKind::FileRead,
                BuiltinMcpToolRiskKind::FileUpload,
            ],
            vec![
                BuiltinMcpToolRiskKindDto::FileRead,
                BuiltinMcpToolRiskKindDto::FileUpload,
            ],
            vec![large_drop_fixture.clone()],
        )
        .await;
        assert!(
            !large_dropped.is_error,
            "managed large browser_drop(paths) failed: {}",
            tool_result_text(&large_dropped)
        );
        let large_drop_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the exact large OOPIF drop effect."}),
        )
        .await;
        let large_drop_text = tool_result_text(&large_drop_snapshot);
        assert!(large_drop_text.contains("drop-selection:fixture-large-drop.bin:1100000"));
        assert!(
            large_drop_text.contains("drop-events:dragenter,dragover,drop,dragenter,dragover,drop")
        );
        assert!(large_drop_text.contains("drop:fixture-large-drop.bin:1100000:L:L"));

        let network_ready = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({
                "text": "network-ready",
                "call_reason": "Wait for the local fixture request ledger."
            }),
        )
        .await;
        assert!(!network_ready.is_error);
        let request_list = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_network_requests",
            json!({
                "static": false,
                "call_reason": "Locate the repository-owned ping request."
            }),
        )
        .await;
        let ping_index = network_request_index(&request_list, "/api/ping");
        let request_detail = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_network_request",
            json!({
                "index": ping_index,
                "part": "response-body",
                "call_reason": "Read the exact repository-owned ping response body."
            }),
            vec![BuiltinMcpToolRiskKind::NetworkSensitiveRead],
            vec![BuiltinMcpToolRiskKindDto::NetworkSensitiveRead],
        )
        .await;
        assert!(
            !request_detail.is_error,
            "managed browser_network_request failed: {}",
            tool_result_text(&request_detail)
        );
        assert!(tool_result_text(&request_detail).contains(r#"{"ok":true}"#));

        let cookie_set = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_set",
            json!({
                "name": "managed-cookie",
                "value": "repository-owned-cookie-value",
                "call_reason": "Set a repository-owned cookie in the managed BrowserContext."
            }),
            vec![BuiltinMcpToolRiskKind::CookieWrite],
            vec![BuiltinMcpToolRiskKindDto::CookieWrite],
        )
        .await;
        assert!(
            !cookie_set.is_error,
            "managed browser_cookie_set failed: {}",
            tool_result_text(&cookie_set)
        );
        let cookie_get = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_get",
            json!({
                "name": "managed-cookie",
                "call_reason": "Read the repository-owned managed cookie."
            }),
            vec![BuiltinMcpToolRiskKind::CookieRead],
            vec![BuiltinMcpToolRiskKindDto::CookieRead],
        )
        .await;
        assert!(
            !cookie_get.is_error,
            "managed browser_cookie_get failed: {}",
            tool_result_text(&cookie_get)
        );
        assert!(tool_result_text(&cookie_get).contains("repository-owned-cookie-value"));
        let cookie_list = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_list",
            json!({
                "domain": "127.0.0.1",
                "path": "/",
                "call_reason": "List repository-owned cookies in the managed BrowserContext."
            }),
            vec![BuiltinMcpToolRiskKind::CookieRead],
            vec![BuiltinMcpToolRiskKindDto::CookieRead],
        )
        .await;
        assert!(
            !cookie_list.is_error,
            "managed browser_cookie_list failed: {}",
            tool_result_text(&cookie_list)
        );
        assert!(tool_result_text(&cookie_list).contains("repository-owned-cookie-value"));

        let storage_export = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_storage_state",
            json!({
                "filename": "fixture-storage-export.json",
                "call_reason": "Export repository-owned managed storage state."
            }),
            vec![
                BuiltinMcpToolRiskKind::FileWrite,
                BuiltinMcpToolRiskKind::CookieRead,
                BuiltinMcpToolRiskKind::LocalStorageRead,
                BuiltinMcpToolRiskKind::StorageStateExport,
            ],
            vec![
                BuiltinMcpToolRiskKindDto::FileWrite,
                BuiltinMcpToolRiskKindDto::CookieRead,
                BuiltinMcpToolRiskKindDto::LocalStorageRead,
                BuiltinMcpToolRiskKindDto::StorageStateExport,
            ],
        )
        .await;
        assert!(
            !storage_export.is_error,
            "managed browser_storage_state failed: {}",
            tool_result_text(&storage_export)
        );
        assert_safe_browser_artifact(&storage_export, "json");

        let storage_import = invoke_approved_browser_sensitive_tool_with_resolved_files(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_set_storage_state",
            json!({
                "filename": storage_state_fixture.clone(),
                "call_reason": "Restore the exact workspace storage state in one original call."
            }),
            vec![
                BuiltinMcpToolRiskKind::FileRead,
                BuiltinMcpToolRiskKind::CookieWrite,
                BuiltinMcpToolRiskKind::LocalStorageWrite,
                BuiltinMcpToolRiskKind::StorageStateImport,
            ],
            vec![
                BuiltinMcpToolRiskKindDto::FileRead,
                BuiltinMcpToolRiskKindDto::CookieWrite,
                BuiltinMcpToolRiskKindDto::LocalStorageWrite,
                BuiltinMcpToolRiskKindDto::StorageStateImport,
            ],
            vec![storage_state_fixture.clone()],
        )
        .await;
        assert!(
            !storage_import.is_error,
            "managed browser_set_storage_state failed: {}",
            tool_result_text(&storage_import)
        );
        let restored_cookie = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_get",
            json!({
                "name": "restored-cookie",
                "call_reason": "Verify the repository-owned restored cookie."
            }),
            vec![BuiltinMcpToolRiskKind::CookieRead],
            vec![BuiltinMcpToolRiskKindDto::CookieRead],
        )
        .await;
        assert!(tool_result_text(&restored_cookie).contains("repository-owned-storage-cookie"));
        let restored_local_storage = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_localstorage_get",
            json!({
                "key": "restored-local",
                "call_reason": "Verify repository-owned restored local storage."
            }),
            vec![BuiltinMcpToolRiskKind::LocalStorageRead],
            vec![BuiltinMcpToolRiskKindDto::LocalStorageRead],
        )
        .await;
        assert!(tool_result_text(&restored_local_storage)
            .contains("repository-owned-storage-local-value"));
        let cookie_delete = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_delete",
            json!({
                "name": "restored-cookie",
                "call_reason": "Delete the repository-owned restored cookie."
            }),
            vec![BuiltinMcpToolRiskKind::CookieWrite],
            vec![BuiltinMcpToolRiskKindDto::CookieWrite],
        )
        .await;
        assert!(
            !cookie_delete.is_error,
            "managed browser_cookie_delete failed: {}",
            tool_result_text(&cookie_delete)
        );
        let cookie_clear = invoke_approved_browser_sensitive_tool(
            &runtime,
            &capability_grant,
            fixture_origin,
            "browser_cookie_clear",
            json!({
                "call_reason": "Clear repository-owned cookies from the managed BrowserContext."
            }),
            vec![BuiltinMcpToolRiskKind::CookieWrite],
            vec![BuiltinMcpToolRiskKindDto::CookieWrite],
        )
        .await;
        assert!(
            !cookie_clear.is_error,
            "managed browser_cookie_clear failed: {}",
            tool_result_text(&cookie_clear)
        );

        let parity_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Verify sensitive managed-surface parity effects."}),
        )
        .await;
        let parity_snapshot_text = tool_result_text(&parity_snapshot);
        assert!(parity_snapshot_text.contains("evaluate:cross-frame-evaluated"));
        assert!(parity_snapshot_text.contains(&upload_selection));
        assert!(parity_snapshot_text.contains(&upload_text));
        assert!(parity_snapshot_text
            .contains(&format!("upload-submit:{admissions_basename}:content-ok")));
        assert!(parity_snapshot_text.contains("drop-selection:fixture-large-drop.bin:1100000"));
        assert!(parity_snapshot_text.contains("drop:fixture-large-drop.bin:1100000:L:L"));
        let input_ref = snapshot_ref(&parity_snapshot_text, "Message");
        let button_ref = snapshot_ref(&parity_snapshot_text, "Apply");
        let dialog_button_ref = snapshot_ref(&parity_snapshot_text, "Show dialog");
        let select_ref = snapshot_ref(&parity_snapshot_text, "Plan");
        let drag_source_ref = snapshot_ref(&parity_snapshot_text, "Drag source");
        let drop_target_ref = snapshot_ref(&parity_snapshot_text, "Drop target");
        let list_ref = snapshot_ref(&parity_snapshot_text, "Visible items");
        let download_ref = snapshot_ref(&parity_snapshot_text, "Download fixture");
        let frame_subject_ref = snapshot_ref(&parity_snapshot_text, "Frame Subject");
        let mail_body_target = frame_editor_target(&parity_snapshot_text, 2);

        let frame_form = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_fill_form",
            json!({
                "fields": [{
                    "target": frame_subject_ref,
                    "name": "Frame Subject",
                    "type": "textbox",
                    "value": "iframe-subject"
                }],
                "call_reason": "Fill a repository-owned iframe input."
            }),
        )
        .await;
        assert!(
            !frame_form.is_error,
            "managed iframe browser_fill_form failed: {}",
            tool_result_text(&frame_form)
        );
        let frame_type = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_type",
            json!({
                "target": mail_body_target,
                "text": "你好",
                "call_reason": "Fill the repository-owned iframe editor in Chinese."
            }),
        )
        .await;
        assert!(
            !frame_type.is_error,
            "managed iframe Chinese browser_type failed: {}",
            tool_result_text(&frame_type)
        );
        let frame_result_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Verify repository-owned iframe input parity."}),
        )
        .await;
        let frame_result_text = tool_result_text(&frame_result_snapshot);
        assert!(frame_result_text.contains("subject:iframe-subject"));
        assert!(frame_result_text.contains("body:你好"));
        assert!(
            frame_result_text.contains("focused:mail-body"),
            "iframe focus projection mismatch: {frame_result_text}"
        );
        assert!(
            frame_result_text.contains("body-events:beforeinput,input"),
            "iframe event projection mismatch: {frame_result_text}"
        );

        for (tool, arguments) in [
            (
                "browser_resize",
                json!({"width": 360, "height": 240, "call_reason": "Resize the visible fixture guest."}),
            ),
            (
                "browser_route",
                json!({
                    "pattern": "**/api/host-adapted",
                    "status": 204,
                    "call_reason": "Install a run-scoped local fixture route."
                }),
            ),
            (
                "browser_route_list",
                json!({"call_reason": "Inspect the run-scoped local fixture route."}),
            ),
            (
                "browser_unroute",
                json!({
                    "pattern": "**/api/host-adapted",
                    "call_reason": "Remove the run-scoped local fixture route."
                }),
            ),
            (
                "browser_network_state_set",
                json!({"state": "offline", "call_reason": "Exercise local offline state."}),
            ),
            (
                "browser_network_state_set",
                json!({"state": "online", "call_reason": "Restore local online state."}),
            ),
        ] {
            let result =
                invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
            assert!(
                !result.is_error,
                "managed fixture {tool} Host adapter failed: {}",
                tool_result_text(&result)
            );
        }

        for (tool, arguments, kind) in [
            (
                "browser_take_screenshot",
                json!({
                    "scale": "css",
                    "filename": "fixture-page.png",
                    "call_reason": "Capture the repository-owned fixture."
                }),
                "image",
            ),
            (
                "browser_snapshot",
                json!({
                    "filename": "fixture-snapshot.yml",
                    "call_reason": "Export the repository-owned fixture snapshot."
                }),
                "snapshot",
            ),
        ] {
            let result =
                invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
            assert!(
                !result.is_error,
                "managed fixture {tool} Artifact failed: {}",
                tool_result_text(&result)
            );
            assert_safe_browser_artifact(&result, kind);
        }
        let pdf = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_pdf_save",
            json!({
                "filename": "fixture-page.pdf",
                "call_reason": "Verify the current managed Electron PDF capability gate."
            }),
        )
        .await;
        assert!(pdf.is_error, "managed Electron PDF must fail closed");
        assert_eq!(
            pdf.structured_content
                .as_ref()
                .and_then(|value| value["status"].as_str()),
            Some("unavailable")
        );
        assert_eq!(
            pdf.structured_content
                .as_ref()
                .and_then(|value| value["code"].as_str()),
            Some("browser.pdf_unavailable")
        );
        assert_eq!(
            tool_result_text(&pdf),
            "PDF export is unavailable in the current managed Electron browser."
        );
        let trace_start = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_start_tracing",
            json!({"call_reason": "Start a trace for the repository-owned fixture."}),
        )
        .await;
        assert!(!trace_start.is_error);
        let trace_stop = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_stop_tracing",
            json!({"call_reason": "Publish the repository-owned fixture trace."}),
        )
        .await;
        assert!(!trace_stop.is_error);
        assert_safe_browser_artifact(&trace_stop, "trace");
        let download = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": download_ref,
                "call_reason": "Download the repository-owned text fixture."
            }),
        )
        .await;
        assert!(
            !download.is_error,
            "managed fixture download failed: {}",
            tool_result_text(&download)
        );
        assert_safe_browser_artifact(&download, "download");

        for (tool, arguments) in [
            (
                "browser_find",
                json!({
                    "text": "Managed Playwright Bridge Fixture",
                    "call_reason": "Find the local fixture heading."
                }),
            ),
            (
                "browser_generate_locator",
                json!({
                    "target": button_ref.clone(),
                    "call_reason": "Generate a locator for the local apply button."
                }),
            ),
            (
                "browser_highlight",
                json!({
                    "target": button_ref.clone(),
                    "call_reason": "Highlight the local apply button."
                }),
            ),
            (
                "browser_hide_highlight",
                json!({
                    "target": button_ref.clone(),
                    "call_reason": "Remove the local fixture highlight."
                }),
            ),
            (
                "browser_verify_element_visible",
                json!({
                    "role": "heading",
                    "accessibleName": "Managed Playwright Bridge Fixture",
                    "call_reason": "Verify the local fixture heading."
                }),
            ),
            (
                "browser_verify_text_visible",
                json!({
                    "text": "Managed Playwright Bridge Fixture",
                    "call_reason": "Verify the local fixture text."
                }),
            ),
            (
                "browser_verify_list_visible",
                json!({
                    "element": "fixture item list",
                    "target": list_ref.clone(),
                    "items": ["Alpha", "Beta"],
                    "call_reason": "Verify the local fixture list."
                }),
            ),
            (
                "browser_route_list",
                json!({"call_reason": "List active routes for the local fixture."}),
            ),
        ] {
            let result =
                invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
            assert!(
                !result.is_error,
                "managed fixture {tool} failed: {}",
                tool_result_text(&result)
            );
        }

        let hovered = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_hover",
            json!({
                "target": button_ref.clone(),
                "call_reason": "Exercise the newly exposed hover path."
            }),
        )
        .await;
        assert!(
            !hovered.is_error,
            "managed fixture hover failed: {}",
            tool_result_text(&hovered)
        );

        let opened_dialog = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({
                "target": dialog_button_ref,
                "call_reason": "Open the repository-owned fixture dialog."
            }),
        )
        .await;
        assert!(
            !opened_dialog.is_error,
            "managed fixture dialog trigger failed: {}",
            tool_result_text(&opened_dialog)
        );
        let handled_dialog = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_handle_dialog",
            json!({
                "accept": true,
                "call_reason": "Accept the repository-owned fixture dialog."
            }),
        )
        .await;
        assert!(
            !handled_dialog.is_error,
            "managed fixture dialog handling failed: {}",
            tool_result_text(&handled_dialog)
        );

        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_fill_form",
                json!({
                    "fields": [{
                        "target": input_ref,
                        "name": "Message",
                        "type": "textbox",
                        "value": "from-form"
                    }],
                    "call_reason": "Fill the local test form."
                }),
            )
            .await
            .is_error
        );
        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_select_option",
                json!({
                    "target": select_ref.clone(),
                    "values": ["pro"],
                    "call_reason": "Select the local fixture plan."
                }),
            )
            .await
            .is_error
        );
        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_verify_value",
                json!({
                    "type": "combobox",
                    "element": "Plan",
                    "target": select_ref,
                    "value": "pro",
                    "call_reason": "Verify the selected local fixture plan."
                }),
            )
            .await
            .is_error
        );
        let dragged = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_drag",
            json!({
                "startTarget": drag_source_ref,
                "endTarget": drop_target_ref,
                "call_reason": "Drag between local fixture elements."
            }),
        )
        .await;
        assert!(
            !dragged.is_error,
            "managed fixture drag failed: {}",
            tool_result_text(&dragged)
        );
        let drag_wait = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({"text": "dragged", "call_reason": "Wait for the local drag result."}),
        )
        .await;
        assert!(tool_result_text(&drag_wait).contains("dragged"));

        for (tool, arguments) in [
            (
                "browser_mouse_move_xy",
                json!({"x": 4, "y": 4, "call_reason": "Move within the local fixture."}),
            ),
            (
                "browser_mouse_down",
                json!({"button": "left", "call_reason": "Press the mouse in the local fixture."}),
            ),
            (
                "browser_mouse_up",
                json!({"button": "left", "call_reason": "Release the mouse in the local fixture."}),
            ),
            (
                "browser_mouse_click_xy",
                json!({"x": 4, "y": 4, "call_reason": "Click a safe local fixture coordinate."}),
            ),
            (
                "browser_mouse_drag_xy",
                json!({
                    "startX": 4,
                    "startY": 4,
                    "endX": 8,
                    "endY": 8,
                    "call_reason": "Drag across safe local fixture coordinates."
                }),
            ),
            (
                "browser_mouse_wheel",
                json!({
                    "deltaX": 0,
                    "deltaY": 8,
                    "call_reason": "Scroll the local fixture."
                }),
            ),
        ] {
            let result =
                invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
            assert!(
                !result.is_error,
                "managed fixture {tool} failed: {}",
                tool_result_text(&result)
            );
        }
        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_type",
                json!({
                    "target": input_ref,
                    "text": "hello",
                    "call_reason": "Replace the local test input."
                }),
            )
            .await
            .is_error
        );
        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_press_key",
                json!({"key": "End", "call_reason": "Exercise the local input keyboard path."}),
            )
            .await
            .is_error
        );
        assert!(
            !invoke_browser_tool_with_grant(
                &runtime,
                &capability_grant,
                "browser_click",
                json!({"target": button_ref, "call_reason": "Apply the local test value."}),
            )
            .await
            .is_error
        );
        let waited = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_wait_for",
            json!({"text": "applied:hello", "call_reason": "Wait for the local result."}),
        )
        .await;
        assert!(tool_result_text(&waited).contains("applied:hello"));
        let tabs = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({"action": "list", "call_reason": "List the managed browser page."}),
        )
        .await;
        assert!(!tabs.is_error);
        assert_eq!(managed_tab_count(&tabs), 1);

        let new_tab = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({
                "action": "new",
                "url": secondary_url.clone(),
                "call_reason": "Open a second repository-owned fixture tab."
            }),
        )
        .await;
        assert!(
            !new_tab.is_error,
            "managed fixture tabs new failed: {}",
            tool_result_text(&new_tab)
        );
        let second_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the second managed fixture tab."}),
        )
        .await;
        assert!(tool_result_text(&second_snapshot).contains("Secondary Fixture Page"));
        let two_tabs = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({"action": "list", "call_reason": "Verify both managed fixture tabs."}),
        )
        .await;
        assert_eq!(managed_tab_count(&two_tabs), 2);

        let select_first = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({"action": "select", "index": 0, "call_reason": "Return to the first tab."}),
        )
        .await;
        assert!(!select_first.is_error);
        let restored_first = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Verify the first tab retained its page."}),
        )
        .await;
        assert!(tool_result_text(&restored_first).contains("Managed Playwright Bridge Fixture"));
        let close_second = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({"action": "close", "index": 1, "call_reason": "Close the background fixture tab."}),
        )
        .await;
        assert!(!close_second.is_error);
        let one_tab = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_tabs",
            json!({"action": "list", "call_reason": "Verify one managed fixture tab remains."}),
        )
        .await;
        assert_eq!(managed_tab_count(&one_tab), 1);

        let console = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_console_messages",
            json!({
                "level": "warning",
                "call_reason": "Read bounded local fixture warnings."
            }),
        )
        .await;
        assert!(!console.is_error);
        let console_text = tool_result_text(&console);
        assert!(console_text.contains("### Result"));
        assert!(console_text.contains("managed fixture warning"));
        let requests = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_network_requests",
            json!({
                "static": false,
                "call_reason": "Read the bounded local fixture request list."
            }),
        )
        .await;
        assert!(!requests.is_error);
        let requests_text = tool_result_text(&requests);
        assert!(requests_text.contains("### Result"));
        assert!(requests_text.contains("fixture-query-canary"));
        for (tool, arguments, kind) in [
            (
                "browser_console_messages",
                json!({
                    "level": "warning",
                    "filename": "fixture-console.log",
                    "call_reason": "Export bounded local fixture warnings."
                }),
                "console",
            ),
            (
                "browser_network_requests",
                json!({
                    "static": false,
                    "filename": "fixture-network.log",
                    "call_reason": "Export the bounded local fixture request list."
                }),
                "network",
            ),
        ] {
            let result =
                invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
            assert!(!result.is_error, "{tool}: {}", tool_result_text(&result));
            assert_safe_browser_artifact(&result, kind);
            assert_browser_artifact_preview(&result, "none");
        }

        let secondary = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_navigate",
            json!({
                "url": secondary_url.clone(),
                "call_reason": "Open the secondary local fixture page."
            }),
        )
        .await;
        assert!(
            !secondary.is_error,
            "managed fixture secondary navigate failed: {}",
            tool_result_text(&secondary)
        );
        let secondary_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the secondary local fixture page."}),
        )
        .await;
        assert!(tool_result_text(&secondary_snapshot).contains("Secondary Fixture Page"));
        let back = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_navigate_back",
            json!({"call_reason": "Return to the primary local fixture page."}),
        )
        .await;
        assert!(
            !back.is_error,
            "managed fixture navigate back failed: {}",
            tool_result_text(&back)
        );
        let back_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the restored primary local fixture page."}),
        )
        .await;
        assert!(tool_result_text(&back_snapshot).contains("Managed Playwright Bridge Fixture"));

        let retained_run_one = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_navigate",
            json!({
                "url": secondary_url.clone(),
                "call_reason": "Leave one local page alive for the fresh Host reconnect regression."
            }),
        )
        .await;
        assert!(
            !retained_run_one.is_error,
            "managed retained-page setup navigate failed: {}",
            tool_result_text(&retained_run_one)
        );

        let closed = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_close",
            json!({"call_reason": "Close the managed fixture page."}),
        )
        .await;
        assert!(!closed.is_error);

        runtime
            .stop()
            .await
            .expect("stop the first managed Host generation");
        let reconnect_capability_grant = approve_managed_playwright_e2e_activation(
            &capability_runtime,
            "run_managed_playwright_e2e_reconnect",
            "managed-playwright-e2e-reconnect-activation",
        );
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        assert_eq!(
            runtime
                .manager
                .get_status(runtime.server_id)
                .unwrap()
                .unwrap()
                .state,
            mycopilot_mcp_client::McpServerState::Ready
        );

        // This is the fresh Host's first Tool call. Electron still owns the run-one Page, while
        // fixed official Playwright starts with an unhydrated Context._tabs array. The Host must
        // import retained pages before exact selection instead of failing with `Tab 0 not found`
        // or creating a replacement Surface.
        let fresh_host_navigate = invoke_browser_tool_with_grant(
            &runtime,
            &reconnect_capability_grant,
            "browser_navigate",
            json!({
                "url": fixture_url.clone(),
                "call_reason": "Navigate the retained page from a fresh managed Host generation."
            }),
        )
        .await;
        assert!(
            !fresh_host_navigate.is_error,
            "fresh Host retained-page navigate failed: {}",
            tool_result_text(&fresh_host_navigate)
        );
        let fresh_host_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &reconnect_capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the retained page after fresh Host navigation."}),
        )
        .await;
        assert!(
            !fresh_host_snapshot.is_error
                && tool_result_text(&fresh_host_snapshot)
                    .contains("Managed Playwright Bridge Fixture"),
            "fresh Host retained-page snapshot failed: {}",
            tool_result_text(&fresh_host_snapshot)
        );
        let fresh_host_evaluate = invoke_approved_browser_evaluate(
            &runtime,
            &reconnect_capability_grant,
            fixture_origin,
            &fixture_events,
        )
        .await;
        assert!(
            !fresh_host_evaluate.is_error
                && tool_result_text(&fresh_host_evaluate).contains("builtin-evaluate-ok"),
            "fresh Host retained-page evaluate failed: {}",
            tool_result_text(&fresh_host_evaluate)
        );
        let fresh_host_evaluated_snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &reconnect_capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Confirm the fresh Host script result on the retained page."}),
        )
        .await;
        assert!(
            tool_result_text(&fresh_host_evaluated_snapshot).contains("evaluated:你好"),
            "fresh Host evaluated snapshot missed the script result: {}",
            tool_result_text(&fresh_host_evaluated_snapshot)
        );

        prepare_unconsumed_profile_binding_for_terminal_event(
            &runtime,
            &reconnect_capability_grant,
        )
        .await;
        send_fixture_agent_done(
            &fixture_events,
            &reconnect_capability_grant.run_id,
            AgentRunStatus::Completed,
        );
        drop(fixture_events);

        runtime.shutdown().await;
        fixture_input.lock().expect("fixture input lock").take();
        tokio::time::timeout(Duration::from_secs(10), writer)
            .await
            .expect("fixture writer shutdown timeout")
            .expect("fixture writer task");
        let result_outcome = tokio::time::timeout(Duration::from_secs(15), result_receiver).await;
        let (exit, child_timed_out) =
            match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
                Ok(exit) => (exit.expect("wait for fixture"), false),
                Err(_) => {
                    let _ = child.start_kill();
                    let exit = tokio::time::timeout(Duration::from_secs(5), child.wait())
                        .await
                        .expect("fixture kill timeout")
                        .expect("wait for killed fixture");
                    (exit, true)
                }
            };
        let stderr = stderr_reader.await.expect("fixture stderr task");
        reader.await.expect("fixture reader task");
        risk_approver.abort();
        let _ = risk_approver.await;
        assert!(risk_coordinator.list_pending().is_empty());
        assert_eq!(approval_count.load(Ordering::Acquire), 0);
        assert_eq!(risk_authorize_count.load(Ordering::Acquire), 0);
        let result = match result_outcome {
            Ok(Ok(result)) if !child_timed_out => result,
            Ok(Ok(_)) => panic!("fixture timed out after RESULT: exit={exit}; stderr={stderr}"),
            Ok(Err(_)) => {
                panic!("fixture closed without RESULT: exit={exit}; stderr={stderr}")
            }
            Err(_) => panic!("fixture RESULT timed out: exit={exit}; stderr={stderr}"),
        };
        assert!(exit.success(), "fixture failed: {stderr}");
        let lifecycle_snapshots = result["agentLifecycleSnapshots"]
            .as_array()
            .expect("fixture Agent lifecycle snapshots");
        assert_eq!(lifecycle_snapshots.len(), 4);
        for snapshot in &lifecycle_snapshots[..3] {
            assert_eq!(snapshot["status"], "waiting_for_approval");
            assert_eq!(snapshot["sensitiveBindings"], 1);
            assert_eq!(snapshot["sensitiveRequests"], 1);
        }
        assert_eq!(lifecycle_snapshots[3]["status"], "completed");
        assert_eq!(lifecycle_snapshots[3]["sensitiveBindings"], 0);
        assert_eq!(lifecycle_snapshots[3]["sensitiveRequests"], 0);
        let ensure_commands = result["ensureCommands"]
            .as_u64()
            .expect("fixture ensure command count");
        let close_commands = result["closeCommands"]
            .as_u64()
            .expect("fixture close command count");
        assert!(ensure_commands >= 2);
        assert!(close_commands >= 1);
        assert!(close_commands <= ensure_commands);
        let detach_snapshots = result["automationDetachSnapshots"]
            .as_array()
            .expect("fixture automation detach snapshots");
        assert!(
            detach_snapshots.len() >= 2,
            "browser_close and final fresh-Host shutdown must both detach automation"
        );
        let first_detach = &detach_snapshots[0];
        assert_eq!(
            first_detach["surfaces"]
                .as_array()
                .expect("first detach retained surfaces")
                .len(),
            1
        );
        assert_eq!(
            first_detach["ensureCommands"], result["preShutdownEnsureCommands"],
            "fresh Host reconnect must not issue a new ensure/create Surface command"
        );
        assert_eq!(
            first_detach["surfaces"], result["preShutdownSurfaces"],
            "fresh Host reconnect must retain the exact Surface generation"
        );
        assert_eq!(result["preShutdownEnsureCommands"], ensure_commands);
        assert_eq!(
            result["preShutdownSurfaces"]
                .as_array()
                .expect("pre-shutdown retained surfaces")
                .len(),
            1
        );
        assert_eq!(result["surfaceCount"], 0);
        assert_eq!(result["guestCount"], 0);
        assert_eq!(result["targetClosed"], true);
        assert_eq!(result["mainWindowAlive"], true);
        assert_eq!(result["broker"]["activeConnections"], 0);
        assert_eq!(result["broker"]["claimedSurfaces"], 0);
        assert_eq!(result["broker"]["registeredGuests"], 0);
        assert_eq!(result["fileSelectionCount"], 0);
        assert_eq!(result["fileBroker"]["handles"], 0);
        assert_eq!(result["fileBroker"]["bytes"], 0);
        assert_eq!(result["riskGuard"]["activeOperations"], 0);
        assert_eq!(result["riskGuard"]["downloads"], 0);
        assert_eq!(result["riskGuard"]["guests"], 0);
        assert_eq!(result["riskGuard"]["redirectMarkers"], 0);
        assert_eq!(result["riskGuard"]["stickyContexts"], 0);
    }

    #[cfg(target_os = "macos")]
    fn managed_playwright_e2e_risk_authority() -> (
        tempfile::TempDir,
        mycopilot_core::storage::service::StorageService,
        BuiltinCapabilityRuntime,
        Arc<BrowserRiskCoordinator>,
        CapabilityGrant,
    ) {
        let directory = tempfile::tempdir().expect("create managed Browser risk database");
        let database_path = directory.path().join("storage.sqlite");
        let storage = mycopilot_core::storage::service::StorageService::open(&database_path)
            .expect("migrate managed Browser risk database");
        let policies = Arc::new(
            SqliteBuiltinCapabilityPolicyStore::open(&database_path)
                .expect("open managed Browser capability policy"),
        );
        policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .expect("allow managed Browser capability in fixture");
        let (capability_runtime, _provider) =
            HostBuiltinCapabilityProvider::runtime_and_provider(policies, None)
                .expect("create managed Browser capability runtime");
        let grant = approve_managed_playwright_e2e_activation(
            &capability_runtime,
            "run_managed_playwright_e2e",
            "managed-playwright-e2e-activation",
        );
        let coordinator = BrowserRiskCoordinator::new(capability_runtime.clone());
        (directory, storage, capability_runtime, coordinator, grant)
    }

    #[cfg(target_os = "macos")]
    fn approve_managed_playwright_e2e_activation(
        capability_runtime: &BuiltinCapabilityRuntime,
        run_id: &str,
        call_id: &str,
    ) -> CapabilityGrant {
        let manifest = capability_runtime
            .manifests()
            .iter()
            .next()
            .expect("managed Browser manifest");
        let now = unix_millis() / 1_000;
        let mut approval = AgentBuiltinCapabilityActivationApproval {
            action_id: Uuid::new_v4().to_string(),
            activation_id: Uuid::new_v4().to_string(),
            run_id: run_id.to_string(),
            call_id: call_id.to_string(),
            capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
            display_name: manifest.descriptor.display_name.clone(),
            reason: "Exercise the repository-owned local Browser fixture.".to_string(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: now,
            expires_at: now.saturating_add(BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS),
            approval_status: AgentApprovalStatus::Required,
        };
        approval.approval_status = AgentApprovalStatus::Approved;
        capability_runtime
            .approve_activation(&approval)
            .expect("approve managed Browser capability in fixture")
    }

    async fn invoke_browser_tool(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        raw_name: &str,
        arguments: Value,
    ) -> McpToolResult {
        invoke_browser_tool_result(runtime, raw_name, arguments)
            .await
            .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
    }

    async fn invoke_browser_tool_result(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        raw_name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, McpError> {
        let invocation_id = Uuid::new_v4().to_string();
        runtime
            .invoke(
                raw_name,
                &Uuid::new_v4().to_string(),
                arguments,
                ManagedPlaywrightAuthorizationContext {
                    run_id: "run_managed_playwright_unit".to_string(),
                    capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
                    activation_id: Uuid::new_v4().to_string(),
                    manifest_digest: format!("sha256:{}", "a".repeat(64)),
                    policy_revision: 1,
                    grant_expires_at_ms: u64::MAX,
                    invocation_id,
                    call_id: Uuid::new_v4().to_string(),
                    trigger_tool_name: raw_name.to_string(),
                    call_reason: "Exercise the managed browser unit fixture.".to_string(),
                    builtin_tool_grant: None,
                },
                McpCancellationToken::new(),
            )
            .await
    }

    async fn invoke_browser_tool_with_grant(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        raw_name: &str,
        arguments: Value,
    ) -> McpToolResult {
        invoke_browser_tool_with_grant_result(runtime, capability_grant, raw_name, arguments)
            .await
            .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
    }

    async fn invoke_browser_tool_with_grant_result(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        raw_name: &str,
        arguments: Value,
    ) -> Result<McpToolResult, McpError> {
        let invocation_id = Uuid::new_v4().to_string();
        runtime
            .invoke(
                raw_name,
                &Uuid::new_v4().to_string(),
                arguments,
                ManagedPlaywrightAuthorizationContext {
                    run_id: capability_grant.run_id.clone(),
                    capability_id: capability_grant.capability_id.as_str().to_string(),
                    activation_id: capability_grant.activation_id.as_str().to_string(),
                    manifest_digest: capability_grant.manifest_digest.clone(),
                    policy_revision: capability_grant.policy_revision,
                    grant_expires_at_ms: capability_grant.expires_at.saturating_mul(1_000),
                    invocation_id,
                    call_id: Uuid::new_v4().to_string(),
                    trigger_tool_name: raw_name.to_string(),
                    call_reason: "Exercise the local managed browser fixture.".to_string(),
                    builtin_tool_grant: None,
                },
                McpCancellationToken::new(),
            )
            .await
    }

    #[cfg(target_os = "macos")]
    async fn invoke_approved_browser_evaluate(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        origin: &str,
        fixture_events: &mpsc::UnboundedSender<Value>,
    ) -> McpToolResult {
        let raw_name = "browser_evaluate";
        let call_reason = "Run one synchronous script in the repository-owned fixture.";
        let arguments = json!({
            "function": r#"() => {
                document.querySelector('#output').textContent = 'evaluated:你好';
                return 'builtin-evaluate-ok';
            }"#,
            "call_reason": call_reason,
        });
        invoke_approved_browser_sensitive_tool_after_waiting_event(
            runtime,
            capability_grant,
            origin,
            raw_name,
            arguments,
            vec![BuiltinMcpToolRiskKind::PageScriptExecution],
            vec![BuiltinMcpToolRiskKindDto::PageScriptExecution],
            fixture_events,
        )
        .await
    }

    #[cfg(target_os = "macos")]
    async fn prepare_unconsumed_profile_binding_for_terminal_event(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
    ) {
        let arguments = json!({
            "path": "/",
            "call_reason": "Freeze one profile-scoped binding for terminal lifecycle cleanup."
        });
        let now_ms = unix_millis();
        let expires_at_ms = now_ms
            .saturating_add(60_000)
            .min(capability_grant.expires_at.saturating_mul(1_000));
        let prepared = runtime
            .bridge()
            .prepare_sensitive_tool(ManagedPlaywrightPrepareSensitiveToolInput {
                binding_request_id: Uuid::new_v4().to_string(),
                binding_scope: ManagedPlaywrightSensitiveBindingScopeDto::ManagedBrowserProfile,
                run_id: capability_grant.run_id.clone(),
                capability_id: capability_grant.capability_id.as_str().to_string(),
                activation_id: capability_grant.activation_id.as_str().to_string(),
                manifest_digest: capability_grant.manifest_digest.clone(),
                policy_revision: capability_grant.policy_revision,
                grant_expires_at_ms: capability_grant.expires_at.saturating_mul(1_000),
                call_id: Uuid::new_v4().to_string(),
                tool_name: "browser_cookie_list".to_string(),
                arguments_digest: mycopilot_core::builtin_mcp_tool_arguments_digest(&arguments)
                    .expect("digest terminal profile binding arguments"),
                created_at_ms: now_ms,
                expires_at_ms,
                file_preparation: None,
            })
            .await
            .expect("prepare terminal profile binding");
        assert!(matches!(
            prepared,
            ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared { origin: None, .. }
        ));
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    async fn invoke_approved_browser_sensitive_tool_after_waiting_event(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        origin: &str,
        raw_name: &str,
        arguments: Value,
        risks: Vec<BuiltinMcpToolRiskKind>,
        risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
        fixture_events: &mpsc::UnboundedSender<Value>,
    ) -> McpToolResult {
        invoke_approved_browser_sensitive_tool_with_file_preparation(
            runtime,
            capability_grant,
            origin,
            raw_name,
            arguments,
            risks,
            risk_dtos,
            None,
            Some(fixture_events),
        )
        .await
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    async fn invoke_approved_browser_sensitive_tool(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        origin: &str,
        raw_name: &str,
        arguments: Value,
        risks: Vec<BuiltinMcpToolRiskKind>,
        risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
    ) -> McpToolResult {
        invoke_approved_browser_sensitive_tool_with_file_preparation(
            runtime,
            capability_grant,
            origin,
            raw_name,
            arguments,
            risks,
            risk_dtos,
            None,
            None,
        )
        .await
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    async fn invoke_approved_browser_sensitive_tool_with_resolved_files(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        origin: &str,
        raw_name: &str,
        arguments: Value,
        risks: Vec<BuiltinMcpToolRiskKind>,
        risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
        paths: Vec<String>,
    ) -> McpToolResult {
        invoke_approved_browser_sensitive_tool_with_file_preparation(
            runtime,
            capability_grant,
            origin,
            raw_name,
            arguments,
            risks,
            risk_dtos,
            Some(ManagedPlaywrightSensitiveFilePreparation::ResolvedPaths { paths }),
            None,
        )
        .await
    }

    #[cfg(target_os = "macos")]
    #[allow(clippy::too_many_arguments)]
    async fn invoke_approved_browser_sensitive_tool_with_file_preparation(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        capability_grant: &CapabilityGrant,
        origin: &str,
        raw_name: &str,
        arguments: Value,
        risks: Vec<BuiltinMcpToolRiskKind>,
        risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
        file_preparation: Option<ManagedPlaywrightSensitiveFilePreparation>,
        waiting_event: Option<&mpsc::UnboundedSender<Value>>,
    ) -> McpToolResult {
        let call_reason = arguments["call_reason"]
            .as_str()
            .expect("sensitive fixture call reason")
            .to_string();
        let arguments_digest =
            mycopilot_core::builtin_mcp_tool_arguments_digest(&arguments).unwrap();
        let now_ms = unix_millis();
        let grant_expires_at_ms = capability_grant.expires_at.saturating_mul(1_000);
        let expires_at_ms = now_ms.saturating_add(60_000).min(grant_expires_at_ms);
        let call_id = Uuid::new_v4().to_string();
        let profile_scoped = matches!(
            raw_name,
            "browser_cookie_clear"
                | "browser_cookie_delete"
                | "browser_cookie_get"
                | "browser_cookie_list"
                | "browser_set_storage_state"
                | "browser_storage_state"
        ) || (raw_name == "browser_cookie_set"
            && arguments
                .get("domain")
                .and_then(Value::as_str)
                .is_some_and(|domain| !domain.trim().is_empty()));
        let (binding_scope, resource_scope, expected_origin) = if profile_scoped {
            (
                ManagedPlaywrightSensitiveBindingScopeDto::ManagedBrowserProfile,
                "managed_browser_profile",
                None,
            )
        } else {
            (
                ManagedPlaywrightSensitiveBindingScopeDto::ManagedSurface,
                "managed_surface",
                Some(origin),
            )
        };
        let expected_file_basenames = file_preparation
            .as_ref()
            .map(|preparation| match preparation {
                ManagedPlaywrightSensitiveFilePreparation::ResolvedPaths { paths } => paths
                    .iter()
                    .map(|path| {
                        std::path::Path::new(path)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .expect("resolved fixture basename")
                            .to_string()
                    })
                    .collect::<Vec<_>>(),
            })
            .unwrap_or_default();
        let prepared = runtime
            .bridge()
            .prepare_sensitive_tool(ManagedPlaywrightPrepareSensitiveToolInput {
                binding_request_id: Uuid::new_v4().to_string(),
                binding_scope,
                run_id: capability_grant.run_id.clone(),
                capability_id: capability_grant.capability_id.as_str().to_string(),
                activation_id: capability_grant.activation_id.as_str().to_string(),
                manifest_digest: capability_grant.manifest_digest.clone(),
                policy_revision: capability_grant.policy_revision,
                grant_expires_at_ms,
                call_id: call_id.clone(),
                tool_name: raw_name.to_string(),
                arguments_digest: arguments_digest.clone(),
                created_at_ms: now_ms,
                expires_at_ms,
                file_preparation,
            })
            .await
            .unwrap_or_else(|error| panic!("prepare exact {raw_name} target binding: {error:?}"));
        let ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
            binding_id,
            target_binding_digest,
            origin: prepared_origin,
            created_at_ms,
            expires_at_ms: prepared_expires_at_ms,
            file_basenames,
            file_revision_digest,
            ..
        } = prepared
        else {
            panic!("Main returned an invalid {raw_name} target binding");
        };
        assert_eq!(prepared_origin.as_deref(), expected_origin);
        assert_eq!(file_basenames, expected_file_basenames);
        assert_eq!(file_revision_digest.is_some(), !file_basenames.is_empty());
        assert!(file_basenames
            .iter()
            .all(|basename| !basename.contains('/') && !basename.contains('\\')));
        assert_eq!(created_at_ms, now_ms);
        assert_eq!(prepared_expires_at_ms, expires_at_ms);
        if let Some(fixture_events) = waiting_event {
            send_fixture_agent_done(
                fixture_events,
                &capability_grant.run_id,
                AgentRunStatus::WaitingForApproval,
            );
        }
        let resource_scope_digest = mycopilot_core::builtin_mcp_tool_resource_scope_digest_v2(
            raw_name,
            &arguments_digest,
            expected_origin,
            &risks,
            resource_scope,
            &target_binding_digest,
        )
        .unwrap();
        runtime
            .invoke(
                raw_name,
                &Uuid::new_v4().to_string(),
                arguments,
                ManagedPlaywrightAuthorizationContext {
                    run_id: capability_grant.run_id.clone(),
                    capability_id: capability_grant.capability_id.as_str().to_string(),
                    activation_id: capability_grant.activation_id.as_str().to_string(),
                    manifest_digest: capability_grant.manifest_digest.clone(),
                    policy_revision: capability_grant.policy_revision,
                    grant_expires_at_ms,
                    invocation_id: Uuid::new_v4().to_string(),
                    call_id,
                    trigger_tool_name: raw_name.to_string(),
                    call_reason,
                    builtin_tool_grant: Some(Box::new(ManagedPlaywrightBuiltinToolGrantContext {
                        grant_id: Uuid::new_v4().to_string(),
                        approval_id: Uuid::new_v4().to_string(),
                        arguments_digest,
                        resource_scope_digest,
                        target_binding_id: binding_id,
                        target_binding_digest,
                        origin: prepared_origin,
                        risk_kinds: risk_dtos,
                        expires_at_ms: prepared_expires_at_ms,
                    })),
                },
                McpCancellationToken::new(),
            )
            .await
            .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
    }

    #[cfg(target_os = "macos")]
    fn send_fixture_agent_done(
        fixture_events: &mpsc::UnboundedSender<Value>,
        run_id: &str,
        status: AgentRunStatus,
    ) {
        let success = matches!(
            status,
            AgentRunStatus::WaitingForApproval | AgentRunStatus::Completed
        );
        fixture_events
            .send(json!({
                "jsonrpc": "2.0",
                "method": FIXTURE_AGENT_EVENT_METHOD,
                "params": AgentEvent::Done {
                    run_id: run_id.to_string(),
                    success,
                    status: Some(status),
                    content: None,
                    usage: None,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                },
            }))
            .expect("send fixture Agent done event");
    }

    #[cfg(target_os = "macos")]
    fn network_request_index(result: &McpToolResult, expected_path: &str) -> u64 {
        assert!(
            !result.is_error,
            "managed network list failed: {}",
            tool_result_text(result)
        );
        tool_result_text(result)
            .lines()
            .find(|line| line.contains(expected_path) && line.contains(" => ["))
            .and_then(|line| line.split_once(". ["))
            .and_then(|(index, _)| index.parse::<u64>().ok())
            .unwrap_or_else(|| {
                panic!(
                    "managed network list omitted {expected_path}: {}",
                    tool_result_text(result)
                )
            })
    }

    #[cfg(target_os = "macos")]
    fn tool_result_text(result: &McpToolResult) -> String {
        result
            .content
            .iter()
            .filter_map(|block| match block {
                McpContentBlock::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[cfg(target_os = "macos")]
    fn assert_safe_browser_artifact(result: &McpToolResult, expected_kind: &str) {
        let structured = result
            .structured_content
            .as_ref()
            .expect("managed Artifact structured content");
        let artifacts = structured["artifacts"]
            .as_array()
            .expect("managed Artifact reference array");
        assert_eq!(artifacts.len(), 1);
        let artifact = artifacts[0].as_object().expect("managed Artifact object");
        assert_eq!(artifact["schemaVersion"], 1);
        assert_eq!(artifact["kind"], expected_kind);
        assert_eq!(artifact["owner"], "browser_automation");
        assert_eq!(artifact["lifecycle"], "run");
        assert_eq!(artifact.len(), 11);
        for forbidden in ["path", "managedPath", "outputDir", "data", "body"] {
            assert!(!artifact.contains_key(forbidden));
        }
        let serialized = serde_json::to_string(artifact).unwrap();
        assert!(!serialized.contains("base64"));
    }

    #[cfg(target_os = "macos")]
    fn assert_browser_artifact_preview(result: &McpToolResult, expected_preview: &str) {
        let artifact = result
            .structured_content
            .as_ref()
            .and_then(|structured| structured["artifacts"].as_array())
            .and_then(|artifacts| artifacts.first())
            .unwrap_or_else(|| {
                panic!(
                    "managed Artifact reference missing: {}",
                    tool_result_text(result)
                )
            });
        assert_eq!(artifact["preview"], expected_preview);
    }

    #[cfg(target_os = "macos")]
    fn managed_tab_count(result: &McpToolResult) -> usize {
        if let Some(count) = result
            .structured_content
            .as_ref()
            .and_then(|structured| structured["tabs"].as_array())
            .map(Vec::len)
        {
            return count;
        }
        let count = tool_result_text(result)
            .lines()
            .filter(|line| {
                line.trim()
                    .strip_prefix("- ")
                    .and_then(|line| line.split_once(": "))
                    .is_some_and(|(index, description)| {
                        !index.is_empty()
                            && index.bytes().all(|byte| byte.is_ascii_digit())
                            && !description.is_empty()
                    })
            })
            .count();
        assert!(count > 0, "managed tabs result omitted official tab rows");
        count
    }

    #[cfg(target_os = "macos")]
    fn snapshot_ref(snapshot: &str, accessible_name: &str) -> String {
        let line = snapshot
            .lines()
            .find(|line| line.contains(accessible_name) && line.contains("[ref="))
            .unwrap_or_else(|| panic!("snapshot omitted {accessible_name:?}: {snapshot}"));
        let start = line.find("[ref=").expect("snapshot ref start") + "[ref=".len();
        let end = line[start..].find(']').expect("snapshot ref end") + start;
        line[start..end].to_string()
    }

    #[cfg(target_os = "macos")]
    fn frame_editor_target(snapshot: &str, editor_index: usize) -> String {
        let marker = format!("contenteditable {editor_index}: ");
        snapshot
            .lines()
            .find_map(|line| line.split_once(&marker).map(|(_, target)| target.trim()))
            .filter(|target| target.starts_with("managed-frame-editor:"))
            .unwrap_or_else(|| {
                panic!("snapshot omitted safe iframe editor {editor_index}: {snapshot}")
            })
            .to_string()
    }
}
