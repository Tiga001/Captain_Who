use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::num::NonZeroUsize;
use std::ops::{Deref, DerefMut};
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use rmcp::model::{
    ClientCapabilities, ClientInfo, ClientRequest, ErrorCode, Implementation, JsonRpcMessage,
    ProtocolVersion, RequestId, ServerCapabilities, ServerNotification, SubscriptionFilter,
};
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::{async_rw::AsyncRwTransport, Transport};
use rmcp::{ClientLifecycleMode, ClientServiceExt, RoleClient};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader, ReadBuf};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::connection::{McpClientEventHandler, STATE_FAILED, STATE_READY};
use crate::{
    BoxMcpFuture, McpCancellationToken, McpCapabilitySnapshot, McpClientHandle, McpConnector,
    McpEnvBinding, McpError, McpImplementationInfo, McpLifecycleKind, McpPeer,
    McpPeerNotificationState, McpPeerSignalPublisher, McpProtocolSnapshot, McpSecurityLimits,
    McpServerConfig, McpStderrSnapshot, McpStdioConfig, McpTransportConfig, McpTrustLevel,
};

const MIN_FORCE_REAP_WINDOW: Duration = Duration::from_millis(250);
const EXITED_LEADER_TERM_GRACE: Duration = Duration::from_millis(25);
const LEGACY_PYTHON_INVALID_PARAMS_MESSAGE: &str = "Invalid request parameters";

/// Narrow wire-compatibility shim for Python SDK releases that predate
/// `server/discover`.
///
/// MCP requires an older peer to reject an unknown discovery method with
/// `METHOD_NOT_FOUND`. Python SDK 1.x instead validates the request against a
/// closed Pydantic union and returns `INVALID_PARAMS` before dispatch. rmcp's
/// Auto lifecycle correctly falls back only on `METHOD_NOT_FOUND`, so this
/// adapter translates that one proven legacy fingerprint for the one exact
/// discovery request id. It never changes initialize, tool, or post-connect
/// responses.
struct LegacyDiscoverCompatibilityTransport<T> {
    inner: T,
    state: LegacyDiscoverCompatibilityState,
}

enum LegacyDiscoverCompatibilityState {
    AwaitingDiscover,
    AwaitingResponse(RequestId),
    Finished,
}

impl<T> LegacyDiscoverCompatibilityTransport<T> {
    fn new(inner: T) -> Self {
        Self {
            inner,
            state: LegacyDiscoverCompatibilityState::AwaitingDiscover,
        }
    }

    fn observe_outbound(&mut self, message: &TxJsonRpcMessage<RoleClient>) {
        if !matches!(
            self.state,
            LegacyDiscoverCompatibilityState::AwaitingDiscover
        ) {
            return;
        }
        let JsonRpcMessage::Request(request) = message else {
            return;
        };
        if matches!(request.request, ClientRequest::DiscoverRequest(_)) {
            self.state = LegacyDiscoverCompatibilityState::AwaitingResponse(request.id.clone());
        }
    }

    fn observe_inbound(&mut self, message: &mut RxJsonRpcMessage<RoleClient>) {
        let LegacyDiscoverCompatibilityState::AwaitingResponse(expected_id) = &self.state else {
            return;
        };
        let matches_response = match message {
            JsonRpcMessage::Response(response) => &response.id == expected_id,
            JsonRpcMessage::Error(response) => response.id.as_ref() == Some(expected_id),
            JsonRpcMessage::Request(_) | JsonRpcMessage::Notification(_) => false,
        };
        if !matches_response {
            return;
        }

        if let JsonRpcMessage::Error(response) = message {
            if is_legacy_python_discover_error(&response.error) {
                response.error.code = ErrorCode::METHOD_NOT_FOUND;
                response.error.message = "Method not found".into();
                response.error.data = None;
            }
        }
        self.state = LegacyDiscoverCompatibilityState::Finished;
    }
}

impl<T> Transport<RoleClient> for LegacyDiscoverCompatibilityTransport<T>
where
    T: Transport<RoleClient> + 'static,
{
    type Error = T::Error;

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        self.observe_outbound(&item);
        self.inner.send(item)
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        let mut message = self.inner.receive().await?;
        self.observe_inbound(&mut message);
        Some(message)
    }

    fn close(&mut self) -> impl Future<Output = Result<(), Self::Error>> + Send {
        self.inner.close()
    }
}

fn is_legacy_python_discover_error(error: &rmcp::ErrorData) -> bool {
    error.code == ErrorCode::INVALID_PARAMS
        && error.message.as_ref() == LEGACY_PYTHON_INVALID_PARAMS_MESSAGE
        && error
            .data
            .as_ref()
            .is_none_or(|data| data.as_str() == Some(""))
}

struct SpawnedChildGuard {
    child: Option<Child>,
    containment: Option<ProcessContainment>,
}

impl SpawnedChildGuard {
    fn new(child: Child, containment: ProcessContainment) -> Self {
        Self {
            child: Some(child),
            containment: Some(containment),
        }
    }

    fn into_parts(mut self) -> (Child, ProcessContainment) {
        (
            self.child
                .take()
                .expect("owned MCP child must be present until supervision starts"),
            self.containment
                .take()
                .expect("MCP process containment must accompany its child"),
        )
    }

    fn containment(&self) -> ProcessContainment {
        *self
            .containment
            .as_ref()
            .expect("MCP process containment must accompany its child")
    }
}

impl Deref for SpawnedChildGuard {
    type Target = Child;

    fn deref(&self) -> &Self::Target {
        self.child
            .as_ref()
            .expect("owned MCP child must be present")
    }
}

impl DerefMut for SpawnedChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.child
            .as_mut()
            .expect("owned MCP child must be present")
    }
}

impl Drop for SpawnedChildGuard {
    fn drop(&mut self) {
        if let (Some(mut child), Some(containment)) = (self.child.take(), self.containment.take()) {
            // Async connector cancellation can drop this future at any await. Kill the isolated
            // process group synchronously, then transfer the Child to a detached reaper instead
            // of relying only on kill_on_drop (which does not itself wait for exit).
            request_terminate_child(&containment, &mut child);
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    tokio::time::sleep(EXITED_LEADER_TERM_GRACE).await;
                    force_kill_child(&containment, &mut child);
                    let _ = child.wait().await;
                });
            } else {
                // Runtime teardown must not turn an owned child into an
                // unreaped zombie. A small OS thread owns a private
                // current-thread runtime solely long enough to finish the
                // bounded kill-and-wait sequence. No protocol data crosses
                // this fallback boundary.
                let _ = std::thread::Builder::new()
                    .name("mcp-child-reaper".to_string())
                    .spawn(move || {
                        let runtime = tokio::runtime::Builder::new_current_thread()
                            .enable_time()
                            .build();
                        if let Ok(runtime) = runtime {
                            runtime.block_on(async move {
                                tokio::time::sleep(EXITED_LEADER_TERM_GRACE).await;
                                force_kill_child(&containment, &mut child);
                                let _ = child.wait().await;
                            });
                        } else {
                            force_kill_child(&containment, &mut child);
                            for _ in 0..25 {
                                match child.try_wait() {
                                    Ok(Some(_)) | Err(_) => break,
                                    Ok(None) => {
                                        std::thread::sleep(Duration::from_millis(10));
                                    }
                                }
                            }
                        }
                    });
            }
        }
    }
}

/// Spawn-time identity of the OS containment boundary.
///
/// On Unix the group leader is deliberately kept unreaped until the group has
/// received its final signal. The zombie leader pins both its PID and PGID, so
/// a delayed cleanup cannot target an unrelated process group that reused the
/// numeric identifier.
#[derive(Clone, Copy)]
pub(crate) struct ProcessContainment {
    #[cfg(unix)]
    leader_pid: libc::pid_t,
    #[cfg(unix)]
    process_group_id: libc::pid_t,
}

impl ProcessContainment {
    fn capture(child: &Child) -> Result<Self, McpError> {
        #[cfg(unix)]
        {
            let leader_pid = child
                .id()
                .and_then(|id| libc::pid_t::try_from(id).ok())
                .filter(|id| *id > 0)
                .ok_or_else(|| {
                    McpError::spawn("failed to retain MCP process containment identity")
                })?;
            // SAFETY: getpgid only inspects kernel process metadata for the
            // freshly spawned repository-authorized child.
            let process_group_id = unsafe { libc::getpgid(leader_pid) };
            if process_group_id != leader_pid {
                return Err(McpError::spawn(
                    "MCP child did not enter its dedicated process group",
                ));
            }
            Ok(Self {
                leader_pid,
                process_group_id,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = child;
            Ok(Self {})
        }
    }
}

#[derive(Clone, Debug)]
pub struct McpStdioPolicy {
    /// Executables this connector is authorized to launch.
    ///
    /// The default is deliberately empty. This is a coarse, fail-closed first
    /// gate; a future registry must additionally bind authorization to the
    /// complete launch-spec digest (arguments, cwd, provenance, and server ID).
    pub allowed_programs: BTreeSet<PathBuf>,
    pub allowed_host_variables: BTreeSet<String>,
    pub stdout_max_line_bytes: usize,
    pub stderr_max_line_bytes: usize,
    pub stderr_max_retained_bytes: usize,
    pub stderr_rate_limit_bytes_per_second: usize,
    pub security_limits: McpSecurityLimits,
}

impl Default for McpStdioPolicy {
    fn default() -> Self {
        let security_limits = McpSecurityLimits::default();
        Self {
            allowed_programs: BTreeSet::new(),
            allowed_host_variables: BTreeSet::new(),
            stdout_max_line_bytes: security_limits.max_protocol_message_bytes,
            stderr_max_line_bytes: security_limits.max_stderr_line_bytes,
            stderr_max_retained_bytes: security_limits.max_stderr_retained_bytes,
            stderr_rate_limit_bytes_per_second: security_limits.stderr_rate_limit_bytes_per_second,
            security_limits,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct McpStdioConnector {
    policy: McpStdioPolicy,
}

impl McpStdioConnector {
    pub fn new(policy: McpStdioPolicy) -> Self {
        Self { policy }
    }

    pub async fn connect(
        &self,
        config: &McpServerConfig,
    ) -> Result<Arc<McpClientHandle>, McpError> {
        config.validate_timeouts()?;
        if !config.enabled {
            return Err(McpError::config("disabled MCP server cannot be connected"));
        }
        if matches!(config.trust, McpTrustLevel::Untrusted) {
            return Err(McpError::config(
                "untrusted MCP server is not authorized for process launch",
            ));
        }
        let McpTransportConfig::Stdio(stdio) = &config.transport else {
            return Err(McpError::config(
                "stdio connector cannot start a non-stdio MCP transport",
            ));
        };
        let mut command = build_command(stdio, &self.policy)?;
        let mut spawned = command
            .spawn()
            .map_err(|_| McpError::spawn("failed to start MCP stdio server"))?;
        let containment = match ProcessContainment::capture(&spawned) {
            Ok(containment) => containment,
            Err(error) => {
                let _ = spawned.start_kill();
                let _ = spawned.wait().await;
                return Err(error);
            }
        };
        let mut child = SpawnedChildGuard::new(spawned, containment);
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout())
                    .await;
                return Err(McpError::spawn("MCP server stdin pipe was unavailable"));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout())
                    .await;
                return Err(McpError::spawn("MCP server stdout pipe was unavailable"));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout())
                    .await;
                return Err(McpError::spawn("MCP server stderr pipe was unavailable"));
            }
        };
        let stderr_capture = Arc::new(Mutex::new(StderrAccumulator::new(
            self.policy.stderr_max_line_bytes,
            self.policy.stderr_max_retained_bytes,
            self.policy.stderr_rate_limit_bytes_per_second,
        )));
        let stderr_task = tokio::spawn(drain_stderr(stderr, Arc::clone(&stderr_capture)));

        let client_info = ClientInfo::new(
            ClientCapabilities::default(),
            Implementation::new("mycopilot-mcp-client", env!("CARGO_PKG_VERSION")),
        );
        let signals = McpPeerSignalPublisher::new();
        let client_handler = McpClientEventHandler::new(client_info, signals.clone());
        let lifecycle = ClientLifecycleMode::Auto {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
            legacy_version: Some(ProtocolVersion::V_2025_11_25),
        };
        let transport =
            LegacyDiscoverCompatibilityTransport::new(
                AsyncRwTransport::<RoleClient, _, _>::new_client(
                    LineLimitedReader::new(stdout, self.policy.stdout_max_line_bytes),
                    stdin,
                ),
            );
        let negotiation = tokio::time::timeout(
            config.connect_timeout(),
            client_handler.serve_with_lifecycle(transport, lifecycle),
        )
        .await;
        let service = match negotiation {
            Ok(Ok(service)) => service,
            Ok(Err(_)) => {
                let error =
                    negotiation_failure(child.containment(), &mut child, config.shutdown_timeout())
                        .await;
                stderr_task.abort();
                let _ = stderr_task.await;
                return Err(error);
            }
            Err(_) => {
                terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout())
                    .await;
                stderr_task.abort();
                let _ = stderr_task.await;
                return Err(McpError::timeout(
                    "MCP protocol negotiation",
                    config.connect_timeout_ms.max(1),
                ));
            }
        };
        let Some(peer_info) = service.peer_info() else {
            let mut failed_service = service;
            let _ = failed_service
                .close_with_timeout(config.shutdown_timeout())
                .await;
            terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout()).await;
            stderr_task.abort();
            let _ = stderr_task.await;
            return Err(McpError::negotiation(
                "MCP server omitted negotiated peer info",
            ));
        };
        let protocol = match map_protocol_snapshot(&peer_info, &self.policy.security_limits) {
            Ok(protocol) => protocol,
            Err(error) => {
                let mut failed_service = service;
                let _ = failed_service
                    .close_with_timeout(config.shutdown_timeout())
                    .await;
                terminate_and_reap(child.containment(), &mut child, config.shutdown_timeout())
                    .await;
                stderr_task.abort();
                let _ = stderr_task.await;
                return Err(error);
            }
        };
        let peer = service.peer().clone();
        let notification_task =
            establish_tool_notifications(&peer, &protocol, &signals, config.connect_timeout())
                .await;
        let (child, containment) = child.into_parts();
        Ok(Arc::new(McpClientHandle::new(
            config.id,
            protocol,
            peer,
            service,
            child,
            containment,
            stderr_capture,
            stderr_task,
            notification_task,
            signals,
            config.request_timeout(),
            config.shutdown_timeout(),
            self.policy.security_limits.clone(),
        )))
    }
}

impl McpConnector for McpStdioConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async move {
            let peer: Arc<dyn McpPeer> = self.connect(config).await?;
            Ok(peer)
        })
    }
}

struct LineLimitedReader<R> {
    inner: R,
    max_line_bytes: usize,
    current_line_bytes: usize,
    rejected: bool,
}

impl<R> LineLimitedReader<R> {
    fn new(inner: R, max_line_bytes: usize) -> Self {
        Self {
            inner,
            max_line_bytes,
            current_line_bytes: 0,
            rejected: false,
        }
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for LineLimitedReader<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.rejected {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "MCP protocol message exceeded the configured size limit",
            )));
        }
        if buffer.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }

        // Limit each inner read to the remaining line budget plus one byte.
        // The extra byte lets us detect an over-limit line without allowing
        // rmcp's line accumulator to grow without bound.
        let remaining_budget = self.max_line_bytes.saturating_sub(self.current_line_bytes);
        let read_capacity = buffer.remaining().min(remaining_budget.saturating_add(1));
        let unfilled = buffer.initialize_unfilled_to(read_capacity);
        let mut inner_buffer = ReadBuf::new(unfilled);
        match Pin::new(&mut self.inner).poll_read(context, &mut inner_buffer) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => {
                let filled_len = {
                    let filled = inner_buffer.filled();
                    for byte in filled {
                        if *byte == b'\n' {
                            self.current_line_bytes = 0;
                        } else {
                            self.current_line_bytes = self.current_line_bytes.saturating_add(1);
                            if self.current_line_bytes > self.max_line_bytes {
                                self.rejected = true;
                                return Poll::Ready(Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "MCP protocol message exceeded the configured size limit",
                                )));
                            }
                        }
                    }
                    filled.len()
                };
                buffer.advance(filled_len);
                Poll::Ready(Ok(()))
            }
        }
    }
}

pub(crate) type ProcessExitCode = Arc<StdMutex<Option<i32>>>;

pub(crate) struct StdioProcessSupervisor {
    shutdown: Option<oneshot::Sender<()>>,
    force: McpCancellationToken,
    task: JoinHandle<Result<(), McpError>>,
    shutdown_timeout: Duration,
}

struct ContainedChild {
    child: Child,
    containment: ProcessContainment,
}

pub(crate) fn spawn_process_supervisor(
    child: Child,
    containment: ProcessContainment,
    state: Arc<AtomicU8>,
    shutdown_timeout: Duration,
    signals: McpPeerSignalPublisher,
    force: McpCancellationToken,
) -> (StdioProcessSupervisor, ProcessExitCode) {
    let exit_code = Arc::new(StdMutex::new(None));
    let task_exit_code = Arc::clone(&exit_code);
    let (shutdown, shutdown_requested) = oneshot::channel();
    let task_force = force.clone();
    let task = tokio::spawn(supervise_process(
        ContainedChild { child, containment },
        shutdown_requested,
        task_force,
        state,
        task_exit_code,
        shutdown_timeout,
        signals,
    ));
    (
        StdioProcessSupervisor {
            shutdown: Some(shutdown),
            force,
            task,
            shutdown_timeout,
        },
        exit_code,
    )
}

impl StdioProcessSupervisor {
    pub(crate) async fn shutdown(&mut self) -> Result<(), McpError> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // Two cooperative phases are allowed here: an stdin/transport-close grace period and,
        // on Unix, a process-group TERM grace period. KILL + reap receives its own independent
        // window below instead of borrowing time from either cooperative phase.
        let join_timeout = self.shutdown_timeout.saturating_mul(2);
        let initial_join = tokio::time::timeout(join_timeout, &mut self.task).await;
        match initial_join {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(McpError::shutdown(
                "MCP process supervisor task failed while closing",
            )),
            Err(_) => {
                // The supervisor owns the Child and is the only task allowed to reap it. Do not
                // abort that task when the cooperative TERM path exceeds its outer deadline:
                // switch it to the process-group KILL path, then grant that path a separate
                // bounded window. If even that window expires, dropping the JoinHandle detaches
                // (rather than aborts) the supervisor so it can still reap a delayed exit.
                self.force.cancel();
                match tokio::time::timeout(force_reap_window(self.shutdown_timeout), &mut self.task)
                    .await
                {
                    Ok(Ok(result)) => result,
                    Ok(Err(_)) => Err(McpError::shutdown(
                        "MCP process supervisor task failed during forced cleanup",
                    )),
                    Err(_) => Err(McpError::shutdown(
                        "MCP process supervisor is still reaping after forced termination",
                    )),
                }
            }
        }
    }
}

impl Drop for StdioProcessSupervisor {
    fn drop(&mut self) {
        // `supervise_process` owns the Child. Waking its biased force branch performs the
        // best-effort process-group KILL while letting the detached task retain responsibility
        // for wait/reap. Never abort that task: doing so could strand a zombie.
        self.force.cancel();
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

async fn supervise_process(
    process: ContainedChild,
    mut shutdown_requested: oneshot::Receiver<()>,
    force: McpCancellationToken,
    state: Arc<AtomicU8>,
    exit_code: ProcessExitCode,
    shutdown_timeout: Duration,
    signals: McpPeerSignalPublisher,
) -> Result<(), McpError> {
    let ContainedChild {
        mut child,
        containment,
    } = process;
    let outcome = tokio::select! {
        biased;
        _ = force.cancelled() => force_then_reap(&mut child, &containment, shutdown_timeout)
            .await
            .map(|status| (status, true)),
        status = wait_for_natural_exit(&mut child, &containment, shutdown_timeout) => status
            .map(|status| (status, false)),
        _ = &mut shutdown_requested => wait_then_force(
            &mut child,
            &containment,
            shutdown_timeout,
            &force,
        )
            .await
            .map(|status| (status, true)),
    };
    let (status, intentional_shutdown) = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            state.store(STATE_FAILED, Ordering::Release);
            return Err(error);
        }
    };
    if let Ok(mut stored_exit_code) = exit_code.lock() {
        *stored_exit_code = status.code();
    }
    if !intentional_shutdown {
        let _ = state.compare_exchange(
            STATE_READY,
            STATE_FAILED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
        signals.transport_closed(status.code());
    }
    Ok(())
}

async fn force_then_reap(
    child: &mut Child,
    containment: &ProcessContainment,
    shutdown_timeout: Duration,
) -> Result<std::process::ExitStatus, McpError> {
    request_terminate_child(containment, child);
    tokio::time::sleep(exited_leader_term_grace(shutdown_timeout)).await;
    force_kill_child(containment, child);
    // Keep the sole Child owner alive until the OS reports the process as reaped. Callers apply
    // their own bounded wait and may detach this task, but they must not cancel this final wait.
    child.wait().await.map_err(|_| {
        McpError::shutdown("failed to reap MCP server process after forced termination")
    })
}

fn force_reap_window(shutdown_timeout: Duration) -> Duration {
    shutdown_timeout.max(MIN_FORCE_REAP_WINDOW)
}

fn exited_leader_term_grace(shutdown_timeout: Duration) -> Duration {
    EXITED_LEADER_TERM_GRACE.min(shutdown_timeout)
}

async fn wait_for_natural_exit(
    child: &mut Child,
    containment: &ProcessContainment,
    shutdown_timeout: Duration,
) -> Result<std::process::ExitStatus, McpError> {
    wait_for_leader_exit_without_reaping(child, containment)
        .await
        .map_err(|_| McpError::shutdown("failed to observe MCP server process exit"))?;
    cleanup_after_observed_exit(child, containment, shutdown_timeout).await
}

async fn cleanup_after_observed_exit(
    child: &mut Child,
    containment: &ProcessContainment,
    shutdown_timeout: Duration,
) -> Result<std::process::ExitStatus, McpError> {
    // The exited Unix leader remains a waitable zombie here. That pins the PGID while descendants
    // receive TERM and then KILL, eliminating the PID-reuse window created by Child::try_wait.
    request_terminate_child(containment, child);
    tokio::time::sleep(exited_leader_term_grace(shutdown_timeout)).await;
    force_kill_child(containment, child);
    reap_child(child).await
}

async fn reap_child(child: &mut Child) -> Result<std::process::ExitStatus, McpError> {
    child
        .wait()
        .await
        .map_err(|_| McpError::shutdown("failed to reap MCP server process"))
}

#[cfg(unix)]
async fn wait_for_leader_exit_without_reaping(
    _child: &mut Child,
    containment: &ProcessContainment,
) -> std::io::Result<()> {
    loop {
        match leader_exit_observed(containment) {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[cfg(unix)]
fn leader_exit_observed(containment: &ProcessContainment) -> std::io::Result<bool> {
    let mut information = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
    // SAFETY: the siginfo storage is valid, and leader_pid is the immutable spawn-time PID of our
    // own child. WNOWAIT intentionally leaves the leader waitable so it anchors its PGID.
    let result = unsafe {
        libc::waitid(
            libc::P_PID,
            containment.leader_pid as libc::id_t,
            information.as_mut_ptr(),
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: a successful waitid initializes siginfo_t; POSIX specifies si_pid == 0 when WNOHANG
    // found no waitable status, and si_pid is defined for SIGCHLD information from WEXITED.
    Ok(unsafe { information.assume_init().si_pid() } != 0)
}

#[cfg(not(unix))]
async fn wait_for_leader_exit_without_reaping(
    child: &mut Child,
    _containment: &ProcessContainment,
) -> std::io::Result<()> {
    // Degraded direct-child platforms cannot observe without reaping. A second Child::wait call
    // returns Tokio's cached exit status during the shared cleanup path.
    child.wait().await.map(|_| ())
}

async fn establish_tool_notifications(
    peer: &rmcp::Peer<rmcp::RoleClient>,
    protocol: &McpProtocolSnapshot,
    signals: &McpPeerSignalPublisher,
    timeout: Duration,
) -> Option<JoinHandle<()>> {
    if !protocol.capabilities.tools_list_changed {
        signals.set_notification_state(McpPeerNotificationState::Unsupported);
        return None;
    }
    if protocol.lifecycle == McpLifecycleKind::InitializeFallback {
        signals.set_notification_state(McpPeerNotificationState::Active);
        return None;
    }

    let filter = SubscriptionFilter::builder().tools_list_changed().build();
    let mut subscription = match tokio::time::timeout(
        timeout,
        peer.listen_with_capacity(filter, NonZeroUsize::MIN),
    )
    .await
    {
        Ok(Ok(subscription)) if subscription.acknowledged().tools_list_changed == Some(true) => {
            subscription
        }
        Ok(Ok(mut subscription)) => {
            let _ = tokio::time::timeout(Duration::from_millis(100), subscription.cancel()).await;
            signals.set_notification_state(McpPeerNotificationState::Unavailable);
            return None;
        }
        Ok(Err(_)) | Err(_) => {
            signals.set_notification_state(McpPeerNotificationState::Unavailable);
            return None;
        }
    };
    signals.set_notification_state(McpPeerNotificationState::Active);
    let task_signals = signals.clone();
    Some(tokio::spawn(async move {
        loop {
            match subscription.next().await {
                Ok(Some(ServerNotification::ToolListChangedNotification(_))) => {
                    task_signals.tools_changed();
                }
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => {
                    task_signals.notifications_unavailable();
                    break;
                }
            }
        }
    }))
}

async fn wait_then_force(
    child: &mut Child,
    containment: &ProcessContainment,
    shutdown_timeout: Duration,
    force: &McpCancellationToken,
) -> Result<std::process::ExitStatus, McpError> {
    enum WaitOutcome {
        Forced,
        Child(Result<Result<(), std::io::Error>, tokio::time::error::Elapsed>),
    }

    let outcome = tokio::select! {
        biased;
        _ = force.cancelled() => WaitOutcome::Forced,
        result = tokio::time::timeout(
            shutdown_timeout,
            wait_for_leader_exit_without_reaping(child, containment),
        ) => {
            WaitOutcome::Child(result)
        }
    };
    match outcome {
        WaitOutcome::Forced => force_then_reap(child, containment, shutdown_timeout).await,
        WaitOutcome::Child(Ok(Ok(()))) => {
            cleanup_after_observed_exit(child, containment, shutdown_timeout).await
        }
        WaitOutcome::Child(Ok(Err(_))) => {
            Err(McpError::shutdown("failed to reap MCP server process"))
        }
        WaitOutcome::Child(Err(_)) => {
            request_terminate_child(containment, child);
            let term_outcome = tokio::select! {
                biased;
                _ = force.cancelled() => None,
                result = tokio::time::timeout(
                    shutdown_timeout,
                    wait_for_leader_exit_without_reaping(child, containment),
                ) => Some(result),
            };
            match term_outcome {
                Some(Ok(Ok(()))) => {
                    // TERM got its full bounded grace period. KILL the still-pinned group before
                    // reaping the leader so TERM-ignoring descendants cannot survive.
                    force_kill_child(containment, child);
                    reap_child(child).await
                }
                Some(Ok(Err(_))) => Err(McpError::shutdown("failed to reap MCP server process")),
                Some(Err(_)) | None => force_then_reap(child, containment, shutdown_timeout).await,
            }
        }
    }
}

pub(crate) struct StderrAccumulator {
    retained: Vec<u8>,
    pending_line: Vec<u8>,
    pending_line_truncated: bool,
    max_line_bytes: usize,
    max_retained_bytes: usize,
    rate_limit_bytes_per_second: usize,
    window_started: Instant,
    retained_in_window: usize,
    dropped_bytes: u64,
}

impl StderrAccumulator {
    fn new(
        max_line_bytes: usize,
        max_retained_bytes: usize,
        rate_limit_bytes_per_second: usize,
    ) -> Self {
        Self {
            retained: Vec::with_capacity(max_retained_bytes.min(8 * 1024)),
            pending_line: Vec::with_capacity(max_line_bytes.min(1024)),
            pending_line_truncated: false,
            max_line_bytes,
            max_retained_bytes,
            rate_limit_bytes_per_second,
            window_started: Instant::now(),
            retained_in_window: 0,
            dropped_bytes: 0,
        }
    }

    fn ingest(&mut self, bytes: &[u8]) {
        for byte in bytes {
            if *byte == b'\n' {
                self.flush_pending_line(true);
            } else if self.pending_line.len() < self.max_line_bytes {
                self.pending_line.push(*byte);
            } else {
                self.pending_line_truncated = true;
                self.dropped_bytes = self.dropped_bytes.saturating_add(1);
            }
        }
    }

    fn finish(&mut self) {
        if !self.pending_line.is_empty() || self.pending_line_truncated {
            self.flush_pending_line(false);
        }
    }

    fn flush_pending_line(&mut self, newline: bool) {
        let original_len = self.pending_line.len() + usize::from(newline);
        // Server stderr is never trusted display text. Retain only a
        // host-generated marker so neutral-looking credentials and terminal
        // control/log-injection sequences cannot escape heuristic redaction.
        self.dropped_bytes = self.dropped_bytes.saturating_add(original_len as u64);
        let mut safe_line = b"[mcp stderr omitted]".to_vec();
        if newline {
            safe_line.push(b'\n');
        }
        self.pending_line.clear();
        self.pending_line_truncated = false;

        if self.window_started.elapsed() >= Duration::from_secs(1) {
            self.window_started = Instant::now();
            self.retained_in_window = 0;
        }
        let rate_remaining = self
            .rate_limit_bytes_per_second
            .saturating_sub(self.retained_in_window);
        let capacity_remaining = self.max_retained_bytes.saturating_sub(self.retained.len());
        let accepted = safe_line.len().min(rate_remaining).min(capacity_remaining);
        self.retained.extend_from_slice(&safe_line[..accepted]);
        self.retained_in_window += accepted;
        self.dropped_bytes = self
            .dropped_bytes
            .saturating_add((safe_line.len() - accepted) as u64);
    }

    pub(crate) fn snapshot(&self) -> McpStderrSnapshot {
        McpStderrSnapshot {
            retained: String::from_utf8_lossy(&self.retained).into_owned(),
            retained_bytes: self.retained.len(),
            dropped_bytes: self.dropped_bytes,
            truncated: self.dropped_bytes > 0,
        }
    }
}

async fn drain_stderr(stderr: ChildStderr, capture: Arc<Mutex<StderrAccumulator>>) {
    let mut reader = BufReader::new(stderr);
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        match reader.read(&mut buffer).await {
            Ok(0) | Err(_) => {
                capture.lock().await.finish();
                break;
            }
            Ok(read) => capture.lock().await.ingest(&buffer[..read]),
        }
    }
}

fn build_command(config: &McpStdioConfig, policy: &McpStdioPolicy) -> Result<Command, McpError> {
    validate_stdio_config(config, policy)?;
    let environment = resolve_environment(config, policy)?;
    let mut command = Command::new(&config.program);
    command
        .args(&config.arguments)
        .current_dir(&config.cwd)
        .env_clear()
        .envs(environment)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    configure_process_isolation(&mut command);
    Ok(command)
}

fn validate_stdio_config(config: &McpStdioConfig, policy: &McpStdioPolicy) -> Result<(), McpError> {
    policy.security_limits.validate()?;
    if !config.program.is_absolute() {
        return Err(McpError::config(
            "MCP stdio executable path must be absolute",
        ));
    }
    if !policy.allowed_programs.contains(&config.program) {
        return Err(McpError::config(
            "MCP stdio executable is not authorized by connector policy",
        ));
    }
    if !config.cwd.is_absolute() {
        return Err(McpError::config("MCP stdio cwd must be absolute"));
    }
    if !config.cwd.is_dir() {
        return Err(McpError::config(
            "MCP stdio cwd must be an existing directory",
        ));
    }
    if policy.stderr_max_retained_bytes == 0 {
        return Err(McpError::config(
            "MCP stderr retained-byte limit must be greater than zero",
        ));
    }
    if policy.stderr_max_line_bytes == 0
        || policy.stderr_max_line_bytes > policy.security_limits.max_stderr_line_bytes
        || policy.stderr_max_retained_bytes > policy.security_limits.max_stderr_retained_bytes
        || policy.stderr_rate_limit_bytes_per_second
            > policy.security_limits.stderr_rate_limit_bytes_per_second
        || policy.stdout_max_line_bytes > policy.security_limits.max_protocol_message_bytes
    {
        return Err(McpError::config(
            "MCP stdio limits exceed the centralized security policy",
        ));
    }
    if policy.stdout_max_line_bytes == 0 {
        return Err(McpError::config(
            "MCP stdout message-size limit must be greater than zero",
        ));
    }
    if policy.stderr_rate_limit_bytes_per_second == 0 {
        return Err(McpError::config(
            "MCP stderr rate limit must be greater than zero",
        ));
    }
    let mut names = BTreeSet::new();
    for binding in &config.environment {
        let name = binding.name();
        validate_environment_name(name)?;
        if !names.insert(environment_name_key(name)) {
            return Err(McpError::config(
                "MCP stdio environment contains a duplicate variable name",
            ));
        }
        match binding {
            McpEnvBinding::SecretRef { .. } => {
                return Err(McpError::config(
                    "SecretRef resolution is not available in MCP client round 1",
                ));
            }
            McpEnvBinding::AllowlistedHostVariable { name }
                if !policy.allowed_host_variables.contains(name) =>
            {
                return Err(McpError::config(
                    "MCP host environment variable is not allowed by connector policy",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(windows)]
fn environment_name_key(name: &str) -> String {
    name.to_ascii_uppercase()
}

#[cfg(not(windows))]
fn environment_name_key(name: &str) -> String {
    name.to_string()
}

fn validate_environment_name(name: &str) -> Result<(), McpError> {
    if name.is_empty()
        || name.contains('=')
        || name.contains('\0')
        || !name
            .bytes()
            .all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
    {
        return Err(McpError::config("MCP environment variable name is invalid"));
    }
    Ok(())
}

fn resolve_environment(
    config: &McpStdioConfig,
    policy: &McpStdioPolicy,
) -> Result<BTreeMap<OsString, OsString>, McpError> {
    let mut environment = BTreeMap::new();
    for binding in &config.environment {
        match binding {
            McpEnvBinding::Plain { name, value } => {
                environment.insert(OsString::from(name), OsString::from(value));
            }
            McpEnvBinding::AllowlistedHostVariable { name } => {
                if !policy.allowed_host_variables.contains(name) {
                    return Err(McpError::config(
                        "MCP host environment variable is not allowed by connector policy",
                    ));
                }
                if let Some(value) = std::env::var_os(name) {
                    environment.insert(OsString::from(name), value);
                }
            }
            McpEnvBinding::SecretRef { .. } => {
                return Err(McpError::config(
                    "SecretRef resolution is not available in MCP client round 1",
                ));
            }
        }
    }
    Ok(environment)
}

fn map_protocol_snapshot(
    peer: &rmcp::model::ServerPeerInfo,
    limits: &McpSecurityLimits,
) -> Result<McpProtocolSnapshot, McpError> {
    if peer
        .instructions
        .as_ref()
        .is_some_and(|instructions| instructions.len() > limits.max_server_instructions_bytes)
    {
        return Err(McpError::negotiation(
            "MCP server instructions exceeded the configured size limit",
        ));
    }
    let encoded_capabilities = serde_json::to_vec(&peer.capabilities)
        .map_err(|_| McpError::negotiation("MCP server capabilities could not be measured"))?;
    if encoded_capabilities.len() > limits.max_capability_metadata_bytes {
        return Err(McpError::negotiation(
            "MCP server capabilities exceeded the configured metadata limit",
        ));
    }
    let negotiated_version = peer.protocol_version.to_string();
    let lifecycle = if peer.protocol_version == ProtocolVersion::V_2026_07_28 {
        McpLifecycleKind::Discover
    } else if peer.protocol_version == ProtocolVersion::V_2025_11_25 {
        McpLifecycleKind::InitializeFallback
    } else {
        return Err(McpError::negotiation(
            "MCP server selected an unsupported protocol version",
        ));
    };
    let snapshot = McpProtocolSnapshot {
        negotiated_version,
        lifecycle,
        server: peer
            .server_info
            .as_ref()
            .map(|server| McpImplementationInfo {
                name: server.name.clone(),
                version: server.version.clone(),
            }),
        capabilities: map_capabilities(&peer.capabilities),
    };
    limits.validate_protocol_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn map_capabilities(capabilities: &ServerCapabilities) -> McpCapabilitySnapshot {
    let mut extensions = capabilities
        .extensions
        .as_ref()
        .map(|extensions| extensions.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    extensions.sort();
    let tools = capabilities.tools.as_ref();
    let resources = capabilities.resources.as_ref();
    let prompts = capabilities.prompts.as_ref();
    McpCapabilitySnapshot {
        tools: tools.is_some(),
        tools_list_changed: tools.and_then(|value| value.list_changed).unwrap_or(false),
        resources: resources.is_some(),
        resources_list_changed: resources
            .and_then(|value| value.list_changed)
            .unwrap_or(false),
        resources_subscribe: resources.and_then(|value| value.subscribe).unwrap_or(false),
        prompts: prompts.is_some(),
        prompts_list_changed: prompts
            .and_then(|value| value.list_changed)
            .unwrap_or(false),
        logging: capabilities.logging.is_some(),
        completions: capabilities.completions.is_some(),
        tasks: capabilities.supports_tasks(),
        extensions,
    }
}

async fn negotiation_failure(
    containment: ProcessContainment,
    child: &mut Child,
    shutdown_timeout: Duration,
) -> McpError {
    match tokio::time::timeout(
        Duration::from_millis(100),
        wait_for_leader_exit_without_reaping(child, &containment),
    )
    .await
    {
        Ok(Ok(())) => {
            match cleanup_after_observed_exit(child, &containment, shutdown_timeout).await {
                Ok(status) => McpError::server_exited(status.code()),
                Err(_) => McpError::negotiation("MCP protocol negotiation failed"),
            }
        }
        Ok(Err(_)) | Err(_) => {
            terminate_and_reap(containment, child, shutdown_timeout).await;
            McpError::negotiation("MCP protocol negotiation failed")
        }
    }
}

async fn terminate_and_reap(containment: ProcessContainment, child: &mut Child, timeout: Duration) {
    request_terminate_child(&containment, child);
    match tokio::time::timeout(
        timeout,
        wait_for_leader_exit_without_reaping(child, &containment),
    )
    .await
    {
        Ok(Ok(())) => {
            force_kill_child(&containment, child);
            let _ = child.wait().await;
        }
        Ok(Err(_)) | Err(_) => {
            force_kill_child(&containment, child);
            let _ = tokio::time::timeout(force_reap_window(timeout), child.wait()).await;
        }
    }
}

#[cfg(unix)]
fn configure_process_isolation(command: &mut Command) {
    command.process_group(0);
}

#[cfg(windows)]
fn configure_process_isolation(_command: &mut Command) {
    // Degraded Windows backend: this release supervises only the direct child. It deliberately
    // does not claim Job Object containment or descendant-tree termination.
}

#[cfg(all(not(unix), not(windows)))]
fn configure_process_isolation(_command: &mut Command) {
    // Unknown non-Unix targets use the same direct-child degraded backend.
}

#[cfg(unix)]
fn signal_process_group(containment: &ProcessContainment, child: &mut Child, signal: libc::c_int) {
    // Every call occurs before the sole Child owner reaps the leader. A live or zombie leader pins
    // this spawn-time PGID, so the negative kill cannot target a newly reused group.
    // A completed cleanup may leave the connector guard to run after Child::wait cleared id().
    // Disarm in that case: the group was already signalled while pinned, and using only the saved
    // numeric PGID after reap would create exactly the reuse hazard this containment prevents.
    if child.id().and_then(|id| libc::pid_t::try_from(id).ok()) != Some(containment.leader_pid) {
        return;
    }
    // Do not issue a separate kill(..., 0) preflight: the real signal syscall is the atomic
    // existence check and action. A probe-then-signal sequence would add a needless TOCTOU gap.
    // SAFETY: kill does not dereference memory. The negative immutable PGID addresses only the
    // dedicated process group captured immediately after spawn.
    if unsafe { libc::kill(-containment.process_group_id, signal) } != 0 && signal == libc::SIGKILL
    {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            let _ = child.start_kill();
        }
    }
}

#[cfg(unix)]
pub(crate) fn request_terminate_child(containment: &ProcessContainment, child: &mut Child) {
    signal_process_group(containment, child, libc::SIGTERM);
}

#[cfg(unix)]
pub(crate) fn force_kill_child(containment: &ProcessContainment, child: &mut Child) {
    signal_process_group(containment, child, libc::SIGKILL);
}

#[cfg(windows)]
pub(crate) fn request_terminate_child(_containment: &ProcessContainment, child: &mut Child) {
    // There is no portable graceful console signal for an arbitrary child. This is explicitly a
    // direct-child degraded backend; Job Object containment remains future work.
    let _ = child.start_kill();
}

#[cfg(windows)]
pub(crate) fn force_kill_child(_containment: &ProcessContainment, child: &mut Child) {
    let _ = child.start_kill();
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn request_terminate_child(_containment: &ProcessContainment, child: &mut Child) {
    let _ = child.start_kill();
}

#[cfg(all(not(unix), not(windows)))]
pub(crate) fn force_kill_child(_containment: &ProcessContainment, child: &mut Child) {
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn peer_info(value: serde_json::Value) -> rmcp::model::ServerPeerInfo {
        serde_json::from_value(value).expect("test peer info should deserialize")
    }

    fn stdio_config() -> McpStdioConfig {
        McpStdioConfig {
            program: std::env::current_exe().expect("test executable"),
            arguments: Vec::new(),
            cwd: std::env::current_dir().expect("test cwd"),
            environment: vec![McpEnvBinding::Plain {
                name: "MYCOPILOT_MCP_ALLOWED_TEST_VALUE".to_string(),
                value: "fixed-test-value".to_string(),
            }],
        }
    }

    fn policy_for(config: &McpStdioConfig) -> McpStdioPolicy {
        McpStdioPolicy {
            allowed_programs: BTreeSet::from([config.program.clone()]),
            ..McpStdioPolicy::default()
        }
    }

    #[test]
    fn command_environment_contains_only_explicit_bindings() {
        let config = stdio_config();
        let command = build_command(&config, &policy_for(&config)).expect("build command");
        let environment = command
            .as_std()
            .get_envs()
            .map(|(name, value)| {
                (
                    name.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>();
        assert_eq!(
            environment,
            BTreeMap::from([(
                "MYCOPILOT_MCP_ALLOWED_TEST_VALUE".to_string(),
                Some("fixed-test-value".to_string())
            )])
        );
        assert!(!environment.contains_key("MYCOPILOT_MCP_FORBIDDEN_TEST_VALUE"));
    }

    #[test]
    fn secret_refs_fail_closed_until_a_secret_store_is_integrated() {
        let mut config = stdio_config();
        config.environment = vec![McpEnvBinding::SecretRef {
            name: "TOKEN".to_string(),
            secret_id: "test-only-reference".to_string(),
        }];
        let error = build_command(&config, &policy_for(&config)).unwrap_err();
        assert_eq!(error.kind, crate::McpErrorKind::Config);
    }

    #[test]
    fn command_debug_does_not_include_arguments_or_values() {
        let mut config = stdio_config();
        config.arguments = vec!["sensitive-argument".to_string()];
        let rendered = format!("{config:?}");
        assert!(!rendered.contains("sensitive-argument"));
        assert!(!rendered.contains("fixed-test-value"));
        assert!(rendered.contains("argument_count"));
    }

    #[test]
    fn relative_program_is_rejected() {
        let mut config = stdio_config();
        config.program = PathBuf::from("fixture");
        let error = build_command(&config, &McpStdioPolicy::default()).unwrap_err();
        assert_eq!(error.kind, crate::McpErrorKind::Config);
    }

    #[test]
    fn executable_launch_is_denied_by_default() {
        let config = stdio_config();
        let error = build_command(&config, &McpStdioPolicy::default()).unwrap_err();
        assert_eq!(error.kind, crate::McpErrorKind::Config);
    }

    #[test]
    fn protocol_snapshot_rejects_unknown_versions_and_oversized_capability_metadata() {
        let limits = McpSecurityLimits::default();
        let valid = peer_info(serde_json::json!({
            "protocolVersion": "2026-07-28",
            "capabilities": {
                "tools": {},
                "extensions": {"io.example.safe": {}}
            },
            "serverInfo": {"name": "owned-fixture", "version": "1.0.0"}
        }));
        map_protocol_snapshot(&valid, &limits).unwrap();

        let unknown = peer_info(serde_json::json!({
            "protocolVersion": "2099-01-01",
            "capabilities": {}
        }));
        assert!(map_protocol_snapshot(&unknown, &limits).is_err());

        let oversized = peer_info(serde_json::json!({
            "protocolVersion": "2026-07-28",
            "capabilities": {
                "experimental": {
                    "io.example.large": {
                        "data": "x".repeat(limits.max_capability_metadata_bytes)
                    }
                }
            }
        }));
        assert!(map_protocol_snapshot(&oversized, &limits).is_err());
    }

    #[test]
    fn protocol_snapshot_rejects_peer_identity_and_extension_budgets() {
        let limits = McpSecurityLimits::default();
        let oversized_name = peer_info(serde_json::json!({
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "serverInfo": {
                "name": "n".repeat(limits.max_server_implementation_name_bytes + 1),
                "version": "1.0.0"
            }
        }));
        assert!(map_protocol_snapshot(&oversized_name, &limits).is_err());

        let extensions = (0..=limits.max_capability_extensions)
            .map(|index| {
                (
                    format!("io.example.extension.{index}"),
                    serde_json::json!({}),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let too_many_extensions = peer_info(serde_json::json!({
            "protocolVersion": "2026-07-28",
            "capabilities": {"extensions": extensions}
        }));
        assert!(map_protocol_snapshot(&too_many_extensions, &limits).is_err());
    }

    #[test]
    fn stderr_capture_never_retains_server_controlled_text() {
        let mut capture = StderrAccumulator::new(128, 1024, 1024);
        capture.ingest(
            b"neutral-canary-without-secret-keywords\nAuthorization: Bearer fixed-test-secret\n",
        );
        let snapshot = capture.snapshot();
        assert!(!snapshot
            .retained
            .contains("neutral-canary-without-secret-keywords"));
        assert!(snapshot.retained.contains("[mcp stderr omitted]"));
        assert!(!snapshot.retained.contains("fixed-test-secret"));
        assert!(snapshot.truncated);
    }

    #[test]
    fn stderr_capture_bounds_each_line_and_total_retention() {
        let mut capture = StderrAccumulator::new(8, 32, 32);
        capture.ingest(b"0123456789abcdef\nsecond-safe-line\n");
        let snapshot = capture.snapshot();
        assert!(snapshot.retained_bytes <= 32);
        assert!(snapshot.dropped_bytes > 0);
        assert!(snapshot.truncated);
    }
}
