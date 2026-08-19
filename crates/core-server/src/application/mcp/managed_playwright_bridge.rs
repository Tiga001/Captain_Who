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
    McpConnectionManager, McpConnectionState, McpConnector, McpDispatchCertainty, McpError,
    McpHostBridgeConfig, McpInvocationId, McpLifecycleKind, McpManagerPolicy, McpModelCallId,
    McpOutcomeUnknownReason, McpPeer, McpProtocolSnapshot, McpRegistry, McpServerConfig,
    McpServerId, McpServerScope, McpToolCall, McpToolId, McpToolPage, McpToolResult,
    McpTransportConfig, McpTrustLevel,
};
use mycopilot_protocol_rs::{
    ManagedPlaywrightAuthorizationContext, ManagedPlaywrightBridgeErrorCode,
    ManagedPlaywrightCancelNotification, ManagedPlaywrightCancelReason, ManagedPlaywrightCommand,
    ManagedPlaywrightCommandNotification, ManagedPlaywrightCompletionInput,
    ManagedPlaywrightCompletionOutcome, ManagedPlaywrightDispatchCertainty,
    MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION, MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD,
    MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD,
};
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::task::{JoinHandle, JoinSet};
use uuid::{Uuid, Version};

use super::playwright_manifest::{
    load_playwright_browser_manifest, BROWSER_AUTOMATION_MANAGED_SERVER_ID,
};

pub(crate) const MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL: &str = "builtin.playwright.v1";
const MAX_PENDING_REQUESTS: usize = 8;
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
    CallTool,
    Close,
}

struct PendingRequest {
    operation: PendingOperation,
    completion: oneshot::Sender<ManagedPlaywrightCompletionOutcome>,
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
    closed: AtomicBool,
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
        let pending = self
            .pending
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright pending lock is unavailable"))?
            .remove(&request_id);
        let Some(pending) = pending else {
            return Ok(false);
        };
        if !outcome_matches(pending.operation, &input.outcome) {
            let _ = pending
                .completion
                .send(ManagedPlaywrightCompletionOutcome::Error {
                    code: ManagedPlaywrightBridgeErrorCode::ProtocolError,
                    dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
                });
            return Ok(false);
        }
        Ok(pending.completion.send(input.outcome).is_ok())
    }

    pub(crate) fn close_now(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut pending) = self.pending.lock() {
            for (request_id, request) in pending.drain() {
                self.send_cancel(request_id, ManagedPlaywrightCancelReason::Shutdown);
                let _ = request
                    .completion
                    .send(ManagedPlaywrightCompletionOutcome::Error {
                        code: ManagedPlaywrightBridgeErrorCode::Closed,
                        dispatch_certainty: certainty_for_operation(request.operation),
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
                let _ = request
                    .completion
                    .send(ManagedPlaywrightCompletionOutcome::Error {
                        code: ManagedPlaywrightBridgeErrorCode::Closed,
                        dispatch_certainty: certainty_for_operation(request.operation),
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
    ) -> Result<ManagedPlaywrightCompletionOutcome, McpError> {
        if self.closed.load(Ordering::Acquire) {
            return Err(McpError::shutdown("managed Playwright bridge is closed"));
        }
        let request_id = Uuid::new_v4();
        let (completion, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| {
                McpError::protocol("managed Playwright pending lock is unavailable")
            })?;
            if pending.len() >= MAX_PENDING_REQUESTS {
                return Err(McpError::capacity(
                    "managed Playwright bridge request limit was reached",
                ));
            }
            pending.insert(
                request_id,
                PendingRequest {
                    operation,
                    completion,
                },
            );
        }
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
        let sent = self
            .outbound
            .lock()
            .map_err(|_| McpError::protocol("managed Playwright outbound lock is unavailable"))?
            .as_ref()
            .is_some_and(|outbound| outbound.send(notification).is_ok());
        if !sent {
            self.remove_pending(request_id);
            return Err(McpError::shutdown(
                "managed Playwright Host bridge is unavailable",
            ));
        }

        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let outcome = if let Some(cancellation) = cancellation {
            tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    self.remove_pending(request_id);
                    self.send_cancel(request_id, ManagedPlaywrightCancelReason::Cancelled);
                    return Err(interrupted_error(operation, McpOutcomeUnknownReason::Cancelled));
                }
                result = tokio::time::timeout(timeout, receiver) => result,
            }
        } else {
            tokio::time::timeout(timeout, receiver).await
        };
        match outcome {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => Err(McpError::shutdown(
                "managed Playwright completion channel closed",
            )),
            Err(_) => {
                self.remove_pending(request_id);
                self.send_cancel(request_id, ManagedPlaywrightCancelReason::Timeout);
                if operation == PendingOperation::CallTool {
                    Err(McpError::outcome_unknown(
                        "managed Playwright tools/call",
                        McpOutcomeUnknownReason::TimedOut,
                        McpDispatchCertainty::PossiblyDispatched,
                    ))
                } else {
                    Err(McpError::timeout(
                        "managed Playwright Host bridge",
                        timeout_ms,
                    ))
                }
            }
        }
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
            McpError::protocol("managed Playwright authorization context is unavailable")
        })?;
        if contexts.len() >= MAX_PENDING_REQUESTS || contexts.contains_key(&invocation_id) {
            return Err(McpError::capacity(
                "managed Playwright authorization context limit was reached",
            ));
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
                McpError::protocol("managed Playwright authorization context is unavailable")
            })?
            .remove(&invocation_id)
            .ok_or_else(|| {
                McpError::protocol("managed Playwright authorization context is missing")
            })
    }

    fn remove_authorization_context(&self, invocation_id: McpInvocationId) {
        if let Ok(mut contexts) = self.authorization_contexts.lock() {
            contexts.remove(&invocation_id);
        }
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
        Box::pin(async move {
            if self.connection_state() != McpConnectionState::Ready {
                return Err(McpError::shutdown("managed Playwright peer is not ready"));
            }
            let timeout = call
                .timeout_ms
                .map(Duration::from_millis)
                .unwrap_or(self.request_timeout)
                .min(Duration::from_secs(300));
            let invocation_id = call.invocation_id.ok_or_else(|| {
                McpError::protocol("managed Playwright active-call identity is missing")
            })?;
            let authorization_context = self.bridge.take_authorization_context(invocation_id)?;
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
                )
                .await?;
            let ManagedPlaywrightCompletionOutcome::ToolCalled { result } = outcome else {
                return Err(error_from_outcome(outcome, PendingOperation::CallTool));
            };
            serde_json::from_value(result)
                .map_err(|_| McpError::protocol("managed Playwright tools/call result is invalid"))
        })
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
        let reviewed = load_playwright_browser_manifest()
            .map_err(|_| McpError::config("managed Playwright manifest is invalid"))?;
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
            let Some(tool) = catalog
                .tools
                .iter()
                .find(|tool| tool.raw_name == reviewed_tool.tool_id && tool.routable)
            else {
                return Err(McpError::config(
                    "managed Playwright reviewed tool is unavailable",
                ));
            };
            if tool.descriptor.input_schema != reviewed_tool.input_schema
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
        ManagedPlaywrightBridgeErrorCode::Busy => McpError::capacity(message),
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
    error.with_dispatch_certainty(certainty)
}

fn interrupted_error(operation: PendingOperation, reason: McpOutcomeUnknownReason) -> McpError {
    if operation == PendingOperation::CallTool {
        McpError::outcome_unknown(
            "managed Playwright tools/call",
            reason,
            McpDispatchCertainty::PossiblyDispatched,
        )
    } else {
        McpError::cancelled("managed Playwright Host bridge")
    }
}

fn certainty_for_operation(operation: PendingOperation) -> ManagedPlaywrightDispatchCertainty {
    if operation == PendingOperation::CallTool {
        ManagedPlaywrightDispatchCertainty::PossiblyDispatched
    } else {
        ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched
    }
}

fn safe_error_message(code: ManagedPlaywrightBridgeErrorCode) -> &'static str {
    match code {
        ManagedPlaywrightBridgeErrorCode::Cancelled => "managed Playwright operation cancelled",
        ManagedPlaywrightBridgeErrorCode::Timeout => "managed Playwright operation timed out",
        ManagedPlaywrightBridgeErrorCode::Closed => "managed Playwright Host is closed",
        ManagedPlaywrightBridgeErrorCode::Busy => "managed Playwright Host is busy",
        ManagedPlaywrightBridgeErrorCode::SurfaceUnavailable => "browser surface is unavailable",
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
    use std::collections::VecDeque;

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
        AgentApprovalStatus, AgentBuiltinCapabilityActivationApproval, AgentProposedAction,
        CapabilityGrant, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    };
    #[cfg(target_os = "macos")]
    use mycopilot_protocol_rs::{
        BrowserRiskAuthorizeInput, BrowserRiskCancelInput, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
    };
    #[cfg(target_os = "macos")]
    use std::process::Stdio;
    #[cfg(target_os = "macos")]
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    #[cfg(target_os = "macos")]
    use tokio::process::Command;

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
                        ManagedPlaywrightCompletionOutcome::ToolCalled {
                            result: json!({"content":[{"type":"text","text":"ok"}],
                              "structuredContent":null,"isError":false}),
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
        // The reader must be able to answer Main's reverse Browser-risk request on the same
        // newline-framed stdin without retaining a Sender forever (which would deadlock fixture
        // shutdown). The slot is explicitly cleared after managed runtime shutdown.
        let fixture_input = Arc::new(StdMutex::new(Some(outbound.clone())));
        bridge.attach_outbound(outbound).unwrap();

        let (_risk_directory, _risk_storage, risk_coordinator, capability_grant) =
            managed_playwright_e2e_risk_authority();
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
        runtime.request_start().unwrap();
        join_owned_lifecycle_tasks(&runtime).await;
        let status = runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap();
        assert_eq!(status.state, mycopilot_mcp_client::McpServerState::Ready);
        let catalog = runtime.manager.catalog(runtime.server_id).unwrap().unwrap();
        assert_eq!(catalog.tools.len(), 10);

        let navigate = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_navigate",
            json!({"url": fixture_url, "call_reason": "Open the local fixture."}),
        )
        .await;
        assert!(
            !navigate.is_error,
            "managed fixture navigate failed: {}",
            tool_result_text(&navigate)
        );

        let snapshot = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_snapshot",
            json!({"call_reason": "Inspect the local fixture."}),
        )
        .await;
        let snapshot_text = tool_result_text(&snapshot);
        assert!(snapshot_text.contains("Managed Playwright Bridge Fixture"));
        let input_ref = snapshot_ref(&snapshot_text, "Message");
        let button_ref = snapshot_ref(&snapshot_text, "Apply");

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

        let closed = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_close",
            json!({"call_reason": "Close the managed fixture page."}),
        )
        .await;
        assert!(!closed.is_error);

        runtime.shutdown().await;
        fixture_input.lock().expect("fixture input lock").take();
        tokio::time::timeout(Duration::from_secs(10), writer)
            .await
            .expect("fixture writer shutdown timeout")
            .expect("fixture writer task");
        let result = tokio::time::timeout(Duration::from_secs(15), result_receiver)
            .await
            .expect("fixture result timeout")
            .expect("fixture result channel");
        let exit = tokio::time::timeout(Duration::from_secs(15), child.wait())
            .await
            .expect("fixture exit timeout")
            .expect("wait for fixture");
        let stderr = stderr_reader.await.expect("fixture stderr task");
        reader.await.expect("fixture reader task");
        risk_approver.abort();
        let _ = risk_approver.await;
        assert!(risk_coordinator.list_pending().is_empty());
        assert_eq!(approval_count.load(Ordering::Acquire), 0);
        assert_eq!(risk_authorize_count.load(Ordering::Acquire), 0);
        assert!(exit.success(), "fixture failed: {stderr}");
        assert_eq!(result["ensureCommands"], 1);
        assert_eq!(result["closeCommands"], 1);
        assert_eq!(result["targetClosed"], true);
        assert_eq!(result["mainWindowAlive"], true);
        assert_eq!(result["broker"]["activeConnections"], 0);
        assert_eq!(result["broker"]["claimedSurfaces"], 0);
        assert_eq!(result["broker"]["registeredGuests"], 0);
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
            HostBuiltinCapabilityProvider::runtime_and_provider(policies)
                .expect("create managed Browser capability runtime");
        let manifest = capability_runtime
            .manifests()
            .iter()
            .next()
            .expect("managed Browser manifest");
        let now = unix_millis() / 1_000;
        let mut approval = AgentBuiltinCapabilityActivationApproval {
            action_id: Uuid::new_v4().to_string(),
            activation_id: Uuid::new_v4().to_string(),
            run_id: "run_managed_playwright_e2e".to_string(),
            call_id: "managed-playwright-e2e-activation".to_string(),
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
        let grant = capability_runtime
            .approve_activation(&approval)
            .expect("approve managed Browser capability in fixture");
        (
            directory,
            storage,
            BrowserRiskCoordinator::new(capability_runtime),
            grant,
        )
    }

    async fn invoke_browser_tool(
        runtime: &Arc<ManagedPlaywrightMcpRuntime>,
        raw_name: &str,
        arguments: Value,
    ) -> McpToolResult {
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
                },
                McpCancellationToken::new(),
            )
            .await
            .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
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
                },
                McpCancellationToken::new(),
            )
            .await
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
    fn snapshot_ref(snapshot: &str, accessible_name: &str) -> String {
        let line = snapshot
            .lines()
            .find(|line| line.contains(accessible_name) && line.contains("[ref="))
            .unwrap_or_else(|| panic!("snapshot omitted {accessible_name:?}: {snapshot}"));
        let start = line.find("[ref=").expect("snapshot ref start") + "[ref=".len();
        let end = line[start..].find(']').expect("snapshot ref end") + start;
        line[start..end].to_string()
    }
}
