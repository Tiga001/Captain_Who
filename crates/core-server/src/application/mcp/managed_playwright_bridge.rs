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
    host_artifact_publish_paths: StdMutex<HashMap<String, String>>,
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
            host_artifact_publish_paths: StdMutex::new(HashMap::new()),
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
        // Managed Browser queueing and execution are bounded separately in Electron Main, where
        // BrowserRisk human approval waits can pause (but not reset) the execution budget.
        // The reverse bridge waits indefinitely only for CallTool completion or an explicit
        // cancellation/shutdown token; every other operation retains this transport deadline.
        let deadline = async {
            if operation == PendingOperation::CallTool {
                std::future::pending::<()>().await;
            } else {
                tokio::time::sleep(timeout).await;
            }
        };
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

    // These values are independent pieces of frozen interruption evidence; grouping them into a
    // loosely typed bag would make certainty regressions easier to introduce.
    #[allow(clippy::too_many_arguments)]
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

    fn stash_host_artifact_publish_path(&self, call_id: String, path: String) {
        if call_id.trim().is_empty()
            || path.trim().is_empty()
            || !std::path::Path::new(&path).is_absolute()
        {
            return;
        }
        if let Ok(mut paths) = self.host_artifact_publish_paths.lock() {
            if paths.len() >= MAX_PENDING_REQUESTS {
                paths.clear();
            }
            paths.insert(call_id, path);
        }
    }

    fn take_host_artifact_publish_path(&self, call_id: &str) -> Option<String> {
        self.host_artifact_publish_paths
            .lock()
            .ok()?
            .remove(call_id)
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
                host_artifact_publish_path,
            } = outcome
            else {
                return Err(error_from_outcome(outcome, PendingOperation::CallTool));
            };
            if let Some(path) = host_artifact_publish_path {
                self.bridge.stash_host_artifact_publish_path(call_id, path);
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

    fn owns_tool_timeout(&self) -> bool {
        // Electron Main owns separate queue and execution budgets; the execution budget excludes
        // queue waits and explicit BrowserRisk human-wait intervals. Cancellation and shutdown
        // still flow through the Manager token passed to `call_tool_with_dispatch`.
        true
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

    pub(crate) fn take_host_artifact_publish_path(&self, call_id: &str) -> Option<String> {
        self.bridge.take_host_artifact_publish_path(call_id)
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
        | ManagedPlaywrightBridgeErrorCode::QueueTimeout
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
        ManagedPlaywrightBridgeErrorCode::QueueTimeout => {
            "browser queue wait timed out before this action started; wait for the current browser operation or its approval to finish, then retry"
        }
        ManagedPlaywrightBridgeErrorCode::Closed => "managed Playwright Host is closed",
        ManagedPlaywrightBridgeErrorCode::Busy => {
            "browser capacity or shared browser state is occupied; wait for the current browser operation to finish or release its shared state, then retry"
        }
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
mod tests;
