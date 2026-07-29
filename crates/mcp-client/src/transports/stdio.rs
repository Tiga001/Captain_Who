use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::process::Stdio;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use rmcp::model::{
    ClientCapabilities, ClientInfo, Implementation, ProtocolVersion, ServerCapabilities,
    ServerNotification, SubscriptionFilter,
};
use rmcp::{ClientLifecycleMode, ClientServiceExt};
use tokio::io::{AsyncRead, AsyncReadExt, BufReader, ReadBuf};
use tokio::process::{Child, ChildStderr, Command};
use tokio::sync::{oneshot, Mutex};
use tokio::task::JoinHandle;

use crate::connection::{McpClientEventHandler, STATE_FAILED, STATE_READY};
use crate::{
    BoxMcpFuture, McpCapabilitySnapshot, McpClientHandle, McpConnector, McpEnvBinding, McpError,
    McpImplementationInfo, McpLifecycleKind, McpPeer, McpPeerNotificationState,
    McpPeerSignalPublisher, McpProtocolSnapshot, McpServerConfig, McpStderrSnapshot,
    McpStdioConfig, McpTransportConfig, McpTrustLevel,
};

const DEFAULT_STDOUT_MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
const DEFAULT_STDERR_MAX_RETAINED_BYTES: usize = 64 * 1024;
const DEFAULT_STDERR_RATE_LIMIT_BYTES_PER_SECOND: usize = 16 * 1024;

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
    pub stderr_max_retained_bytes: usize,
    pub stderr_rate_limit_bytes_per_second: usize,
}

impl Default for McpStdioPolicy {
    fn default() -> Self {
        Self {
            allowed_programs: BTreeSet::new(),
            allowed_host_variables: BTreeSet::new(),
            stdout_max_line_bytes: DEFAULT_STDOUT_MAX_LINE_BYTES,
            stderr_max_retained_bytes: DEFAULT_STDERR_MAX_RETAINED_BYTES,
            stderr_rate_limit_bytes_per_second: DEFAULT_STDERR_RATE_LIMIT_BYTES_PER_SECOND,
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
        let McpTransportConfig::Stdio(stdio) = &config.transport;
        let mut command = build_command(stdio, &self.policy)?;
        let mut child = command
            .spawn()
            .map_err(|_| McpError::spawn("failed to start MCP stdio server"))?;
        let stdin = match child.stdin.take() {
            Some(stdin) => stdin,
            None => {
                terminate_and_reap(&mut child, config.shutdown_timeout()).await;
                return Err(McpError::spawn("MCP server stdin pipe was unavailable"));
            }
        };
        let stdout = match child.stdout.take() {
            Some(stdout) => stdout,
            None => {
                terminate_and_reap(&mut child, config.shutdown_timeout()).await;
                return Err(McpError::spawn("MCP server stdout pipe was unavailable"));
            }
        };
        let stderr = match child.stderr.take() {
            Some(stderr) => stderr,
            None => {
                terminate_and_reap(&mut child, config.shutdown_timeout()).await;
                return Err(McpError::spawn("MCP server stderr pipe was unavailable"));
            }
        };
        let stderr_capture = Arc::new(Mutex::new(StderrAccumulator::new(
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
        let negotiation = tokio::time::timeout(
            config.connect_timeout(),
            client_handler.serve_with_lifecycle(
                (
                    LineLimitedReader::new(stdout, self.policy.stdout_max_line_bytes),
                    stdin,
                ),
                lifecycle,
            ),
        )
        .await;
        let service = match negotiation {
            Ok(Ok(service)) => service,
            Ok(Err(_)) => {
                let error = negotiation_failure(&mut child, config.shutdown_timeout()).await;
                stderr_task.abort();
                let _ = stderr_task.await;
                return Err(error);
            }
            Err(_) => {
                terminate_and_reap(&mut child, config.shutdown_timeout()).await;
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
            terminate_and_reap(&mut child, config.shutdown_timeout()).await;
            stderr_task.abort();
            let _ = stderr_task.await;
            return Err(McpError::negotiation(
                "MCP server omitted negotiated peer info",
            ));
        };
        let protocol = map_protocol_snapshot(&peer_info);
        let peer = service.peer().clone();
        let notification_task =
            establish_tool_notifications(&peer, &protocol, &signals, config.connect_timeout())
                .await;
        Ok(Arc::new(McpClientHandle::new(
            config.id,
            protocol,
            peer,
            service,
            child,
            stderr_capture,
            stderr_task,
            notification_task,
            signals,
            config.request_timeout(),
            config.shutdown_timeout(),
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
    task: JoinHandle<Result<(), McpError>>,
    shutdown_timeout: Duration,
}

pub(crate) fn spawn_process_supervisor(
    child: Child,
    state: Arc<AtomicU8>,
    shutdown_timeout: Duration,
    signals: McpPeerSignalPublisher,
) -> (StdioProcessSupervisor, ProcessExitCode) {
    let exit_code = Arc::new(StdMutex::new(None));
    let task_exit_code = Arc::clone(&exit_code);
    let (shutdown, shutdown_requested) = oneshot::channel();
    let task = tokio::spawn(supervise_process(
        child,
        shutdown_requested,
        state,
        task_exit_code,
        shutdown_timeout,
        signals,
    ));
    (
        StdioProcessSupervisor {
            shutdown: Some(shutdown),
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
        let join_timeout = self.shutdown_timeout.saturating_mul(3);
        match tokio::time::timeout(join_timeout, &mut self.task).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(McpError::shutdown(
                "MCP process supervisor task failed while closing",
            )),
            Err(_) => {
                self.task.abort();
                let _ = (&mut self.task).await;
                Err(McpError::shutdown(
                    "MCP process supervisor did not finish while closing",
                ))
            }
        }
    }
}

async fn supervise_process(
    mut child: Child,
    mut shutdown_requested: oneshot::Receiver<()>,
    state: Arc<AtomicU8>,
    exit_code: ProcessExitCode,
    shutdown_timeout: Duration,
    signals: McpPeerSignalPublisher,
) -> Result<(), McpError> {
    let outcome = tokio::select! {
        status = child.wait() => status
            .map(|status| (status, false))
            .map_err(|_| McpError::shutdown("failed to reap MCP server process")),
        _ = &mut shutdown_requested => wait_then_force(&mut child, shutdown_timeout)
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
    let mut subscription = match tokio::time::timeout(timeout, peer.listen(filter)).await {
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
    shutdown_timeout: Duration,
) -> Result<std::process::ExitStatus, McpError> {
    match tokio::time::timeout(shutdown_timeout, child.wait()).await {
        Ok(Ok(status)) => Ok(status),
        Ok(Err(_)) => Err(McpError::shutdown("failed to reap MCP server process")),
        Err(_) => {
            terminate_child(child);
            match tokio::time::timeout(shutdown_timeout, child.wait()).await {
                Ok(Ok(status)) => Ok(status),
                _ => Err(McpError::shutdown(
                    "MCP server process did not exit after forced termination",
                )),
            }
        }
    }
}

pub(crate) struct StderrAccumulator {
    retained: Vec<u8>,
    max_retained_bytes: usize,
    rate_limit_bytes_per_second: usize,
    window_started: Instant,
    retained_in_window: usize,
    dropped_bytes: u64,
}

impl StderrAccumulator {
    fn new(max_retained_bytes: usize, rate_limit_bytes_per_second: usize) -> Self {
        Self {
            retained: Vec::with_capacity(max_retained_bytes.min(8 * 1024)),
            max_retained_bytes,
            rate_limit_bytes_per_second,
            window_started: Instant::now(),
            retained_in_window: 0,
            dropped_bytes: 0,
        }
    }

    fn ingest(&mut self, bytes: &[u8]) {
        if self.window_started.elapsed() >= Duration::from_secs(1) {
            self.window_started = Instant::now();
            self.retained_in_window = 0;
        }
        let rate_remaining = self
            .rate_limit_bytes_per_second
            .saturating_sub(self.retained_in_window);
        let capacity_remaining = self.max_retained_bytes.saturating_sub(self.retained.len());
        let accepted = bytes.len().min(rate_remaining).min(capacity_remaining);
        self.retained.extend_from_slice(&bytes[..accepted]);
        self.retained_in_window += accepted;
        self.dropped_bytes += (bytes.len() - accepted) as u64;
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
            Ok(0) | Err(_) => break,
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

fn map_protocol_snapshot(peer: &rmcp::model::ServerPeerInfo) -> McpProtocolSnapshot {
    let negotiated_version = peer.protocol_version.to_string();
    let lifecycle = if peer.protocol_version == ProtocolVersion::V_2026_07_28 {
        McpLifecycleKind::Discover
    } else {
        McpLifecycleKind::InitializeFallback
    };
    McpProtocolSnapshot {
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
    }
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

async fn negotiation_failure(child: &mut Child, shutdown_timeout: Duration) -> McpError {
    match child.try_wait() {
        Ok(Some(status)) => McpError::server_exited(status.code()),
        Ok(None) => match tokio::time::timeout(Duration::from_millis(100), child.wait()).await {
            Ok(Ok(status)) => McpError::server_exited(status.code()),
            Ok(Err(_)) => McpError::negotiation("MCP protocol negotiation failed"),
            Err(_) => {
                terminate_and_reap(child, shutdown_timeout).await;
                McpError::negotiation("MCP protocol negotiation failed")
            }
        },
        Err(_) => {
            terminate_and_reap(child, shutdown_timeout).await;
            McpError::negotiation("MCP protocol negotiation failed")
        }
    }
}

async fn terminate_and_reap(child: &mut Child, timeout: Duration) {
    terminate_child(child);
    let _ = tokio::time::timeout(timeout, child.wait()).await;
}

#[cfg(unix)]
fn configure_process_isolation(command: &mut Command) {
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_process_isolation(_command: &mut Command) {
    // Windows Job Object containment is intentionally deferred to the process
    // hardening round. The retained Child and kill_on_drop still provide a
    // bounded single-process fallback.
}

#[cfg(unix)]
pub(crate) fn terminate_child(child: &mut Child) {
    let Some(id) = child.id() else {
        return;
    };
    let Ok(process_group) = i32::try_from(id) else {
        let _ = child.start_kill();
        return;
    };
    // SAFETY: the child was created as leader of a fresh process group. A
    // negative PID targets only that group and does not dereference memory.
    if unsafe { libc::kill(-process_group, libc::SIGKILL) } != 0 {
        let _ = child.start_kill();
    }
}

#[cfg(not(unix))]
pub(crate) fn terminate_child(child: &mut Child) {
    let _ = child.start_kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

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
}
