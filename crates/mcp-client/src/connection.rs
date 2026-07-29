use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use rmcp::model::{
    CacheScope, CallToolRequest, CallToolRequestParams, ClientInfo, ClientRequest, ContentBlock,
    ListToolsRequest, PaginatedRequestParams, ResourceContents, ServerResult, Tool,
};
use rmcp::service::{
    NotificationContext, PeerRequestOptions, RequestHandle, RunningService, ServiceError,
};
use rmcp::{ClientHandler, RoleClient};
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::config::MAX_TIMEOUT_MS;
use crate::connector::BoxMcpFuture;
use crate::transports::stdio::{
    spawn_process_supervisor, ProcessExitCode, StderrAccumulator, StdioProcessSupervisor,
};
use crate::{
    McpCacheScope, McpCancellationToken, McpConnectionState, McpContentBlock, McpEmbeddedResource,
    McpError, McpPeerSignalPublisher, McpPeerSignalReceiver, McpProtocolSnapshot, McpResourceLink,
    McpServerId, McpStderrSnapshot, McpToolAnnotations, McpToolCall, McpToolDescriptor,
    McpToolPage, McpToolResult,
};

pub(crate) const STATE_CONNECTING: u8 = 0;
pub(crate) const STATE_READY: u8 = 1;
pub(crate) const STATE_CLOSING: u8 = 2;
pub(crate) const STATE_CLOSED: u8 = 3;
pub(crate) const STATE_FAILED: u8 = 4;

const CANCELLATION_NOTIFICATION_GRACE: Duration = Duration::from_millis(100);

#[derive(Clone)]
pub(crate) struct McpClientEventHandler {
    info: ClientInfo,
    signals: McpPeerSignalPublisher,
}

impl McpClientEventHandler {
    pub(crate) fn new(info: ClientInfo, signals: McpPeerSignalPublisher) -> Self {
        Self { info, signals }
    }
}

impl ClientHandler for McpClientEventHandler {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    fn on_tool_list_changed(
        &self,
        _context: NotificationContext<RoleClient>,
    ) -> impl Future<Output = ()> + Send + '_ {
        self.signals.tools_changed();
        std::future::ready(())
    }
}

type RmcpClientService = RunningService<RoleClient, McpClientEventHandler>;

struct ConnectionResources {
    service: RmcpClientService,
    process: StdioProcessSupervisor,
    stderr_task: JoinHandle<()>,
    notification_task: Option<JoinHandle<()>>,
    transport_monitor_cancel: McpCancellationToken,
    transport_monitor_task: JoinHandle<()>,
}

/// Stable protocol operations exposed to the future registry/runtime adapter.
pub trait McpPeer: Send + Sync {
    fn server_id(&self) -> McpServerId;
    fn connection_state(&self) -> McpConnectionState;
    fn protocol_snapshot(&self) -> &McpProtocolSnapshot;
    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage>;
    fn call_tool<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult>;
    fn subscribe_signals(&self) -> Option<McpPeerSignalReceiver> {
        None
    }
    /// Synchronously signal transport-specific forced cleanup.
    ///
    /// Returns true when the peer has a force path independent of polling [`Self::close`].
    fn force_close(&self) -> bool {
        false
    }
    fn close(&self) -> BoxMcpFuture<'_, ()>;
}

/// A connected MCP client and the owned stdio child supervisor.
///
/// All `rmcp` types remain private implementation details.
pub struct McpClientHandle {
    server_id: McpServerId,
    protocol: McpProtocolSnapshot,
    peer: rmcp::Peer<RoleClient>,
    resources: Mutex<Option<ConnectionResources>>,
    shutdown_task: Mutex<Option<JoinHandle<Result<(), McpError>>>>,
    close_result: Arc<StdMutex<Option<Result<(), McpError>>>>,
    process_exit_code: ProcessExitCode,
    stderr: Arc<Mutex<StderrAccumulator>>,
    request_timeout: Duration,
    shutdown_timeout: Duration,
    state: Arc<AtomicU8>,
    force_close: McpCancellationToken,
    close_guard: Mutex<()>,
    signals: McpPeerSignalPublisher,
}

impl fmt::Debug for McpClientHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpClientHandle")
            .field("server_id", &self.server_id)
            .field("protocol", &self.protocol)
            .field("state", &self.connection_state())
            .finish_non_exhaustive()
    }
}

impl McpClientHandle {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        server_id: McpServerId,
        protocol: McpProtocolSnapshot,
        peer: rmcp::Peer<RoleClient>,
        service: RmcpClientService,
        child: tokio::process::Child,
        stderr: Arc<Mutex<StderrAccumulator>>,
        stderr_task: JoinHandle<()>,
        notification_task: Option<JoinHandle<()>>,
        signals: McpPeerSignalPublisher,
        request_timeout: Duration,
        shutdown_timeout: Duration,
    ) -> Self {
        let state = Arc::new(AtomicU8::new(STATE_READY));
        let force_close = McpCancellationToken::new();
        let (process, process_exit_code) = spawn_process_supervisor(
            child,
            Arc::clone(&state),
            shutdown_timeout,
            signals.clone(),
            force_close.clone(),
        );
        let transport_monitor_cancel = McpCancellationToken::new();
        let transport_monitor_task = spawn_transport_monitor(
            peer.clone(),
            signals.clone(),
            Arc::clone(&process_exit_code),
            transport_monitor_cancel.clone(),
        );
        Self {
            server_id,
            protocol,
            peer,
            resources: Mutex::new(Some(ConnectionResources {
                service,
                process,
                stderr_task,
                notification_task,
                transport_monitor_cancel,
                transport_monitor_task,
            })),
            shutdown_task: Mutex::new(None),
            close_result: Arc::new(StdMutex::new(None)),
            process_exit_code,
            stderr,
            request_timeout,
            shutdown_timeout,
            state,
            force_close,
            close_guard: Mutex::new(()),
            signals,
        }
    }

    pub fn server_id(&self) -> McpServerId {
        self.server_id
    }

    pub fn connection_state(&self) -> McpConnectionState {
        if self.state.load(Ordering::Acquire) == STATE_READY && self.peer.is_transport_closed() {
            let _ = self.state.compare_exchange(
                STATE_READY,
                STATE_FAILED,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
        }
        decode_state(self.state.load(Ordering::Acquire))
    }

    pub fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        &self.protocol
    }

    pub fn subscribe_signals(&self) -> McpPeerSignalReceiver {
        self.signals.subscribe()
    }

    pub async fn stderr_snapshot(&self) -> McpStderrSnapshot {
        self.stderr.lock().await.snapshot()
    }

    pub async fn list_tools(&self, cursor: Option<String>) -> Result<McpToolPage, McpError> {
        self.require_ready("tools/list")?;
        let request = ClientRequest::ListToolsRequest(ListToolsRequest {
            method: Default::default(),
            params: Some(PaginatedRequestParams::default().with_cursor(cursor)),
            extensions: Default::default(),
        });
        let deadline = tokio::time::Instant::now() + self.request_timeout;
        let mut handle = match tokio::time::timeout_at(
            deadline,
            self.peer
                .send_cancellable_request(request, PeerRequestOptions::no_options()),
        )
        .await
        {
            Ok(result) => {
                result.map_err(|error| self.map_service_error_sync("tools/list", error))?
            }
            Err(_) => {
                return Err(McpError::timeout(
                    "tools/list",
                    self.request_timeout.as_millis() as u64,
                ));
            }
        };
        let response = match tokio::time::timeout_at(deadline, &mut handle.rx).await {
            Ok(Ok(response)) => {
                response.map_err(|error| self.map_service_error_sync("tools/list", error))?
            }
            Ok(Err(_)) => {
                return Err(
                    self.map_service_error_sync("tools/list", ServiceError::TransportClosed)
                );
            }
            Err(_) => {
                schedule_request_cancellation(handle, "request timeout");
                return Err(McpError::timeout(
                    "tools/list",
                    self.request_timeout.as_millis() as u64,
                ));
            }
        };
        let ServerResult::ListToolsResult(result) = response else {
            return Err(McpError::protocol(
                "tools/list returned an unexpected response type",
            ));
        };
        let tools = result.tools.into_iter().map(map_tool).collect::<Vec<_>>();
        Ok(McpToolPage {
            tools,
            next_cursor: result.next_cursor,
            ttl_ms: result.ttl_ms,
            cache_scope: result.cache_scope.map(map_cache_scope),
        })
    }

    pub async fn call_tool(
        &self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> Result<McpToolResult, McpError> {
        self.require_ready("tools/call")?;
        if cancellation.is_cancelled() {
            return Err(McpError::cancelled("tools/call"));
        }
        let arguments = match call.arguments {
            Value::Null => None,
            Value::Object(arguments) => Some(arguments),
            _ => {
                return Err(McpError::config(
                    "MCP tool arguments must be a JSON object or null",
                ));
            }
        };
        let timeout_ms = call
            .timeout_ms
            .unwrap_or(self.request_timeout.as_millis() as u64);
        if !(1..=MAX_TIMEOUT_MS).contains(&timeout_ms) {
            return Err(McpError::config(
                "MCP tool timeout must be between 1 ms and 24 hours",
            ));
        }
        let timeout = Duration::from_millis(timeout_ms);
        let mut params = CallToolRequestParams::new(call.name);
        if let Some(arguments) = arguments {
            params = params.with_arguments(arguments);
        }
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
        let deadline = tokio::time::Instant::now() + timeout;
        let send_request = self
            .peer
            .send_cancellable_request(request, PeerRequestOptions::no_options());
        tokio::pin!(send_request);
        let mut handle = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                return Err(McpError::cancelled("tools/call"));
            }
            _ = tokio::time::sleep_until(deadline) => {
                return Err(McpError::timeout("tools/call", timeout.as_millis() as u64));
            }
            result = &mut send_request => {
                result.map_err(|error| self.map_service_error_sync("tools/call", error))?
            }
        };

        enum CallWait {
            Response(Box<Result<ServerResult, ServiceError>>),
            Cancelled,
            TimedOut,
            TransportClosed,
        }
        let wait = tokio::select! {
            biased;
            _ = cancellation.cancelled() => CallWait::Cancelled,
            _ = tokio::time::sleep_until(deadline) => CallWait::TimedOut,
            response = &mut handle.rx => match response {
                Ok(response) => CallWait::Response(Box::new(response)),
                Err(_) => CallWait::TransportClosed,
            },
        };
        let result = match wait {
            CallWait::Response(result) => {
                (*result).map_err(|error| self.map_service_error_sync("tools/call", error))?
            }
            CallWait::Cancelled => {
                schedule_request_cancellation(handle, "cancelled by host");
                return Err(McpError::cancelled("tools/call"));
            }
            CallWait::TimedOut => {
                schedule_request_cancellation(handle, "request timeout");
                return Err(McpError::timeout("tools/call", timeout.as_millis() as u64));
            }
            CallWait::TransportClosed => {
                return Err(
                    self.map_service_error_sync("tools/call", ServiceError::TransportClosed)
                );
            }
        };

        let ServerResult::CallToolResult(result) = result else {
            return Err(McpError::protocol(
                "tools/call returned an unsupported response type",
            ));
        };
        if result
            .result_type
            .as_ref()
            .is_some_and(|result_type| !result_type.is_complete())
        {
            return Err(McpError::protocol(
                "tools/call returned a non-complete result unsupported in MCP client round 1",
            ));
        }
        let content = result
            .content
            .into_iter()
            .map(map_content)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(McpToolResult {
            content,
            structured_content: result.structured_content,
            is_error: result.is_error.unwrap_or(false),
        })
    }

    pub async fn close(&self) -> Result<(), McpError> {
        let _close_guard = self.close_guard.lock().await;
        if let Some(result) = self
            .close_result
            .lock()
            .ok()
            .and_then(|result| result.clone())
        {
            return result;
        }

        let mut shutdown_task = self.shutdown_task.lock().await;
        if shutdown_task.is_none() {
            let Some(resources) = self.resources.lock().await.take() else {
                let error = McpError::shutdown("MCP shutdown resources were unavailable");
                self.state.store(STATE_FAILED, Ordering::Release);
                if let Ok(mut close_result) = self.close_result.lock() {
                    *close_result = Some(Err(error.clone()));
                }
                return Err(error);
            };
            self.state.store(STATE_CLOSING, Ordering::Release);
            let state = Arc::clone(&self.state);
            let close_result = Arc::clone(&self.close_result);
            let shutdown_timeout = self.shutdown_timeout;
            *shutdown_task = Some(tokio::spawn(async move {
                let result =
                    shutdown_resources(resources, shutdown_timeout, Arc::clone(&state)).await;
                if let Ok(mut stored_result) = close_result.lock() {
                    *stored_result = Some(result.clone());
                }
                result
            }));
        }

        let joined = shutdown_task
            .as_mut()
            .expect("shutdown task must exist")
            .await;
        shutdown_task.take();
        match joined {
            Ok(result) => result,
            Err(_) => {
                let error = McpError::shutdown("MCP shutdown supervisor task failed");
                self.state.store(STATE_FAILED, Ordering::Release);
                if let Ok(mut close_result) = self.close_result.lock() {
                    *close_result = Some(Err(error.clone()));
                }
                Err(error)
            }
        }
    }

    pub fn force_close(&self) {
        self.force_close.cancel();
    }

    fn require_ready(&self, operation: &str) -> Result<(), McpError> {
        match self.connection_state() {
            McpConnectionState::Ready => Ok(()),
            McpConnectionState::Failed => Err(McpError::server_exited(self.process_exit_code())),
            state => Err(McpError::protocol(format!(
                "{operation} is unavailable while connection is {state:?}"
            ))),
        }
    }

    fn map_service_error_sync(&self, operation: &str, error: ServiceError) -> McpError {
        match error {
            ServiceError::Timeout { timeout } => {
                McpError::timeout(operation, timeout.as_millis() as u64)
            }
            ServiceError::Cancelled { .. } => McpError::cancelled(operation),
            ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
                self.state.store(STATE_FAILED, Ordering::Release);
                McpError::server_exited(self.process_exit_code())
            }
            ServiceError::McpError(error) => McpError::protocol(format!(
                "MCP peer rejected {operation} with JSON-RPC error code {}",
                error.code.0
            )),
            ServiceError::UnexpectedResponse => {
                McpError::protocol(format!("{operation} returned an unexpected response"))
            }
            ServiceError::SubscriptionLagged { .. } => {
                McpError::protocol(format!("{operation} notification buffer lagged"))
            }
            ServiceError::InputRequiredRoundsExceeded { .. } => McpError::protocol(format!(
                "{operation} requested unsupported additional input"
            )),
            _ => McpError::protocol(format!("{operation} failed with an unsupported SDK error")),
        }
    }

    fn process_exit_code(&self) -> Option<i32> {
        self.process_exit_code
            .lock()
            .ok()
            .and_then(|exit_code| *exit_code)
    }
}

impl Drop for McpClientHandle {
    fn drop(&mut self) {
        self.force_close.cancel();
        if let Ok(mut shutdown_task) = self.shutdown_task.try_lock() {
            if let Some(task) = shutdown_task.take() {
                task.abort();
            }
        }
    }
}

fn schedule_request_cancellation(handle: RequestHandle<RoleClient>, reason: &'static str) {
    tokio::spawn(async move {
        let _ = tokio::time::timeout(
            CANCELLATION_NOTIFICATION_GRACE,
            handle.cancel(Some(reason.to_string())),
        )
        .await;
    });
}

async fn shutdown_resources(
    mut resources: ConnectionResources,
    shutdown_timeout: Duration,
    state: Arc<AtomicU8>,
) -> Result<(), McpError> {
    if let Some(notification_task) = resources.notification_task.take() {
        notification_task.abort();
        let _ = notification_task.await;
    }
    resources.transport_monitor_cancel.cancel();
    let _ = resources.transport_monitor_task.await;
    let mut shutdown_error = match resources.service.close_with_timeout(shutdown_timeout).await {
        Ok(Some(_)) => None,
        Ok(None) => Some(McpError::shutdown(
            "MCP protocol transport did not close within the shutdown grace period",
        )),
        Err(_) => Some(McpError::shutdown("MCP protocol task failed while closing")),
    };

    if let Err(error) = resources.process.shutdown().await {
        // Process containment/reaping takes precedence over protocol cleanup
        // diagnostics because it represents the stronger safety failure.
        shutdown_error = Some(error);
    }

    if tokio::time::timeout(shutdown_timeout, &mut resources.stderr_task)
        .await
        .is_err()
    {
        resources.stderr_task.abort();
        let _ = resources.stderr_task.await;
        if shutdown_error.is_none() {
            shutdown_error = Some(McpError::shutdown(
                "MCP stderr drain did not finish during shutdown",
            ));
        }
    }

    match shutdown_error {
        Some(error) => {
            state.store(STATE_FAILED, Ordering::Release);
            Err(error)
        }
        None => {
            state.store(STATE_CLOSED, Ordering::Release);
            Ok(())
        }
    }
}

fn spawn_transport_monitor(
    peer: rmcp::Peer<RoleClient>,
    signals: McpPeerSignalPublisher,
    process_exit_code: ProcessExitCode,
    cancel: McpCancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
            if !peer.is_transport_closed() {
                continue;
            }
            // Give the process supervisor a short opportunity to retain a real
            // exit code. If the child remains alive after protocol EOF, None is
            // the correct safe representation.
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(25)) => {}
            }
            let exit_code = process_exit_code
                .lock()
                .ok()
                .and_then(|exit_code| *exit_code);
            signals.transport_closed(exit_code);
            return;
        }
    })
}

impl McpPeer for McpClientHandle {
    fn server_id(&self) -> McpServerId {
        self.server_id()
    }

    fn connection_state(&self) -> McpConnectionState {
        self.connection_state()
    }

    fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        self.protocol_snapshot()
    }

    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
        Box::pin(async move { self.list_tools(cursor).await })
    }

    fn call_tool<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        Box::pin(async move { self.call_tool(call, cancellation).await })
    }

    fn subscribe_signals(&self) -> Option<McpPeerSignalReceiver> {
        Some(self.subscribe_signals())
    }

    fn force_close(&self) -> bool {
        self.force_close();
        true
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async move { self.close().await })
    }
}

fn decode_state(value: u8) -> McpConnectionState {
    match value {
        STATE_CONNECTING => McpConnectionState::Connecting,
        STATE_READY => McpConnectionState::Ready,
        STATE_CLOSING => McpConnectionState::Closing,
        STATE_CLOSED => McpConnectionState::Closed,
        STATE_FAILED => McpConnectionState::Failed,
        _ => McpConnectionState::Failed,
    }
}

fn map_tool(tool: Tool) -> McpToolDescriptor {
    let annotations = tool.annotations.map(|annotations| McpToolAnnotations {
        title: annotations.title,
        read_only_hint: annotations.read_only_hint,
        destructive_hint: annotations.destructive_hint,
        idempotent_hint: annotations.idempotent_hint,
        open_world_hint: annotations.open_world_hint,
    });
    McpToolDescriptor {
        name: tool.name.into_owned(),
        title: tool.title,
        description: tool.description.map(|description| description.into_owned()),
        input_schema: Value::Object((*tool.input_schema).clone()),
        output_schema: tool
            .output_schema
            .map(|schema| Value::Object((*schema).clone())),
        annotations,
    }
}

fn map_cache_scope(scope: CacheScope) -> McpCacheScope {
    match scope {
        CacheScope::Private => McpCacheScope::Private,
        CacheScope::Public => McpCacheScope::Public,
        _ => McpCacheScope::Unknown,
    }
}

fn map_content(content: ContentBlock) -> Result<McpContentBlock, McpError> {
    match content {
        ContentBlock::Text(text) => Ok(McpContentBlock::Text { text: text.text }),
        ContentBlock::Image(image) => Ok(McpContentBlock::Image {
            data: image.data,
            mime_type: image.mime_type,
        }),
        ContentBlock::Audio(audio) => Ok(McpContentBlock::Audio {
            data: audio.data,
            mime_type: audio.mime_type,
        }),
        ContentBlock::Resource(embedded) => {
            let resource = match embedded.resource {
                ResourceContents::TextResourceContents {
                    uri,
                    mime_type,
                    text,
                    ..
                } => McpEmbeddedResource::Text {
                    uri,
                    mime_type,
                    text,
                },
                ResourceContents::BlobResourceContents {
                    uri,
                    mime_type,
                    blob,
                    ..
                } => McpEmbeddedResource::Blob {
                    uri,
                    mime_type,
                    data: blob,
                },
                _ => {
                    return Err(McpError::protocol(
                        "tools/call returned an unsupported embedded resource type",
                    ));
                }
            };
            Ok(McpContentBlock::EmbeddedResource { resource })
        }
        ContentBlock::ResourceLink(resource) => Ok(McpContentBlock::ResourceLink {
            resource: McpResourceLink {
                uri: resource.uri,
                name: resource.name,
                title: resource.title,
                description: resource.description,
                mime_type: resource.mime_type,
                size: resource.size,
            },
        }),
        _ => Err(McpError::protocol(
            "tools/call returned an unsupported content block type",
        )),
    }
}
