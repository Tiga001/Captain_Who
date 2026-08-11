//! Registry, quotas, and lifecycle operations for managed command sessions.

use super::output_capture::{
    spawn_process_output_capture_with_observers, ProcessOutputTranscriptObserver,
};
use super::session::{
    run_session_watcher, CaptureReceiver, CommandSessionCompletionHook,
    CommandSessionLifecycleObserver, ManagedCommandSession, ProcessControl,
};
use super::{
    canonicalize_workspace_root, enforce_command_policy, force_terminate_command_process_group,
    normalize_command_text, resolve_command_cwd, AgentCommandArtifactObservationPhase,
    AgentCommandRequest, AgentPermissions, CommandArtifactObserver, CommandAuthorizationSource,
    CommandExecutionError, CommandSessionError, CommandSessionId, CommandSessionPoll,
    CommandSessionProjection, CommandSessionScopeId, CommandSessionSnapshot, CommandSpawnPlan,
    CommandStartOutcome, CommandTerminalResult, ManagedCommandChild, ManagedCommandWorkspaceLease,
    ProcessOutputCaptureBudget, ProcessOutputCaptureHandle, ProcessOutputCapturePolicy,
    ProcessOutputObserver, MAX_TIMEOUT_MS,
};
use crate::artifact_runtime::ArtifactRuntimeProvider;
use crate::file_input::AgentFileInputExecutionContext;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_INITIAL_YIELD_MS: u64 = 10_000;
pub const MIN_INITIAL_YIELD_MS: u64 = 10;
pub const MAX_INITIAL_YIELD_MS: u64 = 30_000;
pub const DEFAULT_COMMAND_TRANSCRIPT_BYTES: usize = 256 * 1024;
pub const DEFAULT_COMMAND_POLL_BYTES: usize = 256 * 1024;
pub const MIN_COMMAND_POLL_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub struct CommandSessionManagerConfig {
    pub max_active_sessions: usize,
    pub max_active_sessions_per_scope: usize,
    pub max_retained_terminal_sessions: usize,
    pub transcript_bytes: usize,
    pub poll_bytes: usize,
    pub drain_grace: Duration,
    pub interrupt_grace: Duration,
    pub shutdown_grace: Duration,
}

impl Default for CommandSessionManagerConfig {
    fn default() -> Self {
        Self {
            max_active_sessions: 32,
            max_active_sessions_per_scope: 8,
            max_retained_terminal_sessions: 256,
            transcript_bytes: DEFAULT_COMMAND_TRANSCRIPT_BYTES,
            poll_bytes: DEFAULT_COMMAND_POLL_BYTES,
            drain_grace: Duration::from_secs(1),
            interrupt_grace: Duration::from_millis(500),
            shutdown_grace: Duration::from_secs(3),
        }
    }
}

impl CommandSessionManagerConfig {
    fn validate(&self) -> Result<(), CommandSessionError> {
        if self.max_active_sessions == 0
            || self.max_active_sessions_per_scope == 0
            || self.max_active_sessions_per_scope > self.max_active_sessions
            || self.max_retained_terminal_sessions == 0
            || self.transcript_bytes < 2
            || self.poll_bytes < MIN_COMMAND_POLL_BYTES
            || self.drain_grace.is_zero()
            || self.interrupt_grace.is_zero()
            || self.shutdown_grace.is_zero()
        {
            return Err(CommandSessionError::InvalidConfiguration);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CommandStartOptions {
    pub initial_yield: Duration,
}

impl Default for CommandStartOptions {
    fn default() -> Self {
        Self {
            initial_yield: Duration::from_millis(DEFAULT_INITIAL_YIELD_MS),
        }
    }
}

impl CommandStartOptions {
    fn normalized_yield(self) -> Duration {
        self.initial_yield.clamp(
            Duration::from_millis(MIN_INITIAL_YIELD_MS),
            Duration::from_millis(MAX_INITIAL_YIELD_MS),
        )
    }
}

#[derive(Debug)]
pub enum CommandSessionStartError {
    Authorization(CommandExecutionError),
    Session(CommandSessionError),
}

impl std::fmt::Display for CommandSessionStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Authorization(error) => error.fmt(formatter),
            Self::Session(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CommandSessionStartError {}

impl From<CommandExecutionError> for CommandSessionStartError {
    fn from(error: CommandExecutionError) -> Self {
        Self::Authorization(error)
    }
}

impl From<CommandSessionError> for CommandSessionStartError {
    fn from(error: CommandSessionError) -> Self {
        Self::Session(error)
    }
}

impl From<String> for CommandSessionStartError {
    fn from(error: String) -> Self {
        Self::Authorization(CommandExecutionError::from(error))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandTerminationReport {
    pub requested: usize,
    pub terminated: usize,
    pub still_running: Vec<CommandSessionId>,
}

#[derive(Debug)]
struct RegistryEntry {
    session: Arc<ManagedCommandSession>,
    active: bool,
}

#[derive(Debug, Default)]
struct RegistryState {
    sessions: HashMap<CommandSessionId, RegistryEntry>,
    active_sessions: usize,
    active_by_scope: HashMap<CommandSessionScopeId, usize>,
}

#[derive(Debug)]
struct ManagerInner {
    config: CommandSessionManagerConfig,
    accepting_starts: AtomicBool,
    registry: Mutex<RegistryState>,
}

#[derive(Debug, Clone)]
pub struct CommandSessionManager {
    inner: Arc<ManagerInner>,
}

impl Default for CommandSessionManager {
    fn default() -> Self {
        Self::new(CommandSessionManagerConfig::default())
            .expect("default command session manager config must be valid")
    }
}

impl CommandSessionManager {
    pub fn new(config: CommandSessionManagerConfig) -> Result<Self, CommandSessionError> {
        config.validate()?;
        Ok(Self {
            inner: Arc::new(ManagerInner {
                config,
                accepting_starts: AtomicBool::new(true),
                registry: Mutex::new(RegistryState::default()),
            }),
        })
    }

    /// Authorizes, starts, and briefly yields an ordinary shell command.
    /// `request.timeout_ms == None` deliberately means no process hard deadline.
    #[allow(clippy::too_many_arguments)]
    pub fn start_authorized_command(
        &self,
        scope_id: CommandSessionScopeId,
        workspace_root: Option<&Path>,
        request: &AgentCommandRequest,
        permissions: AgentPermissions,
        authorization_source: CommandAuthorizationSource,
        options: CommandStartOptions,
        output_observer: Option<ProcessOutputObserver>,
    ) -> Result<CommandStartOutcome, CommandSessionStartError> {
        self.start_authorized_command_with_cancel_probe(
            scope_id,
            workspace_root,
            request,
            permissions,
            authorization_source,
            options,
            output_observer,
            None,
            None,
        )
    }

    /// Starts an ordinary authorized command while exposing lifecycle events
    /// from the authoritative session state machine to the process host.
    #[allow(clippy::too_many_arguments)]
    pub fn start_authorized_command_with_lifecycle_observer(
        &self,
        scope_id: CommandSessionScopeId,
        workspace_root: Option<&Path>,
        request: &AgentCommandRequest,
        permissions: AgentPermissions,
        authorization_source: CommandAuthorizationSource,
        options: CommandStartOptions,
        lifecycle_observer: CommandSessionLifecycleObserver,
    ) -> Result<CommandStartOutcome, CommandSessionStartError> {
        self.start_authorized_command_with_cancel_probe(
            scope_id,
            workspace_root,
            request,
            permissions,
            authorization_source,
            options,
            None,
            Some(lifecycle_observer),
            None,
        )
    }

    /// Unified host entry for ordinary shell commands and frozen managed-runtime commands.
    /// Cancellation is consulted only until the caller accepts a Running handoff; afterwards the
    /// process session has an independent lifecycle.
    #[allow(clippy::too_many_arguments)]
    pub fn start_authorized_command_with_runtime_and_lifecycle(
        &self,
        scope_id: CommandSessionScopeId,
        workspace_root: Option<&Path>,
        request: &AgentCommandRequest,
        permissions: AgentPermissions,
        authorization_source: CommandAuthorizationSource,
        options: CommandStartOptions,
        artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
        file_inputs: Option<&AgentFileInputExecutionContext>,
        managed_workspace: Option<ManagedCommandWorkspaceLease>,
        lifecycle_observer: CommandSessionLifecycleObserver,
        cancellation_token: crate::AgentCancellationToken,
        cancel_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<CommandStartOutcome, CommandSessionStartError> {
        if request.runtime.is_none() && request.runtime_binding.is_none() {
            if !request.inputs.is_empty() {
                return Err(CommandExecutionError::from(
                    "run_command.inputs requires a frozen managed runtime binding".to_string(),
                )
                .into());
            }
            return self.start_authorized_command_with_cancel_probe(
                scope_id,
                workspace_root,
                request,
                permissions,
                authorization_source,
                options,
                None,
                Some(lifecycle_observer),
                cancel_probe,
            );
        }

        match super::managed_runtime::prepare_managed_command_session(
            workspace_root,
            request,
            permissions,
            authorization_source,
            cancellation_token,
            super::managed_runtime::ManagedCommandSessionServices {
                artifact_runtime,
                file_inputs,
                managed_workspace,
            },
        )? {
            super::managed_runtime::ManagedCommandSessionPreparation::Immediate(execution) => {
                Ok(immediate_terminal_outcome(scope_id, *execution))
            }
            super::managed_runtime::ManagedCommandSessionPreparation::Ready {
                plan,
                completion_hook,
            } => self
                .start_plan_with_observers(
                    scope_id,
                    plan,
                    options,
                    None,
                    Some(lifecycle_observer),
                    cancel_probe,
                    Some(completion_hook),
                )
                .map_err(Into::into),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_authorized_command_with_cancel_probe(
        &self,
        scope_id: CommandSessionScopeId,
        workspace_root: Option<&Path>,
        request: &AgentCommandRequest,
        permissions: AgentPermissions,
        authorization_source: CommandAuthorizationSource,
        options: CommandStartOptions,
        output_observer: Option<ProcessOutputObserver>,
        lifecycle_observer: Option<CommandSessionLifecycleObserver>,
        cancel_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<CommandStartOutcome, CommandSessionStartError> {
        if request.runtime.is_some()
            || request.runtime_binding.is_some()
            || !request.inputs.is_empty()
        {
            return Err(CommandExecutionError::from(
                "host-bound runtimes and file inputs require a managed-runtime spawn plan"
                    .to_string(),
            )
            .into());
        }
        let root = workspace_root
            .map(canonicalize_workspace_root)
            .transpose()?;
        let cwd = resolve_command_cwd(root.as_deref(), request.cwd.as_deref(), permissions.write)?;
        enforce_command_policy(
            &request.command,
            permissions,
            authorization_source,
            root.as_deref(),
            Some(&cwd),
        )?;
        // Policy and process launch must consume the exact same canonical script. The model/tool
        // ingestion path normally already freezes this representation; repeat the deterministic
        // normalization here so lower-level Host callers cannot execute CRLF/lone-CR bytes that
        // were classified as LF.
        let canonical_command = normalize_command_text(&request.command).map_err(|error| {
            CommandExecutionError::from(format!("命令无效（{}）：{}", error.code, error.reason))
        })?;
        // Artifact observation is a terminal-session concern, not an Agent Run concern. Capture
        // the before image before spawn and move the lease into the process watcher so a handed-
        // off command can still produce its bounded after image after the originating run ends.
        let artifact_observer = CommandArtifactObserver::prepare(
            root.as_deref(),
            &cwd,
            request.observe.as_ref(),
            permissions,
        );
        let artifact_before = artifact_observer
            .as_ref()
            .map(|observer| observer.capture(AgentCommandArtifactObservationPhase::Before, None));
        let completion_hook: Option<CommandSessionCompletionHook> = artifact_observer
            .zip(artifact_before)
            .map(|(observer, before)| {
                Box::new(move |result: &mut super::AgentCommandExecutionResult| {
                    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
                    result.artifact_observation = Some(observer.finish(before, after));
                }) as CommandSessionCompletionHook
            });
        let hard_timeout = request
            .timeout_ms
            .map(|timeout| Duration::from_millis(timeout.clamp(1, MAX_TIMEOUT_MS)));
        let plan = CommandSpawnPlan::shell(canonical_command, cwd, root.as_deref(), hard_timeout);
        self.start_plan_with_observers(
            scope_id,
            plan,
            options,
            output_observer,
            lifecycle_observer,
            cancel_probe,
            completion_hook,
        )
        .map_err(Into::into)
    }

    pub(super) fn start_plan(
        &self,
        scope_id: CommandSessionScopeId,
        plan: CommandSpawnPlan,
        options: CommandStartOptions,
        output_observer: Option<ProcessOutputObserver>,
        cancel_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
    ) -> Result<CommandStartOutcome, CommandSessionError> {
        self.start_plan_with_observers(
            scope_id,
            plan,
            options,
            output_observer,
            None,
            cancel_probe,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn start_plan_with_observers(
        &self,
        scope_id: CommandSessionScopeId,
        plan: CommandSpawnPlan,
        options: CommandStartOptions,
        output_observer: Option<ProcessOutputObserver>,
        lifecycle_observer: Option<CommandSessionLifecycleObserver>,
        cancel_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
        completion_hook: Option<CommandSessionCompletionHook>,
    ) -> Result<CommandStartOutcome, CommandSessionError> {
        if !self.inner.accepting_starts.load(Ordering::Acquire) {
            return Err(CommandSessionError::ManagerShuttingDown);
        }

        let session_id = CommandSessionId::new();
        let projection = CommandSessionProjection {
            command: plan.command().to_string(),
            cwd: plan.cwd_projection().to_string(),
        };
        let started_at = super::session::unix_time_millis();
        let (session, control_rx) = ManagedCommandSession::new(
            session_id.clone(),
            scope_id.clone(),
            projection,
            started_at,
            self.inner.config.transcript_bytes,
            output_observer,
            lifecycle_observer,
        );
        self.reserve(session.clone())?;

        if cancel_probe.as_ref().is_some_and(|probe| probe()) {
            session.cancel_before_running();
            mark_terminal(&self.inner, &session_id);
            return Ok(CommandStartOutcome::Exited(Box::new(
                session
                    .terminal_result()
                    .expect("pre-start cancellation commits a terminal result"),
            )));
        }

        let started = Instant::now();
        let mut command = plan.build();
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                let message = format!("启动命令失败：{error}");
                session.fail_before_running(message.clone());
                mark_terminal(&self.inner, &session_id);
                return Err(CommandSessionError::SpawnFailed(message));
            }
        };
        let Some(stdout) = child.stdout.take() else {
            cleanup_failed_spawn(&mut child);
            let message = "无法读取命令 stdout。".to_string();
            session.fail_before_running(message.clone());
            mark_terminal(&self.inner, &session_id);
            return Err(CommandSessionError::SpawnFailed(message));
        };
        let Some(stderr) = child.stderr.take() else {
            cleanup_failed_spawn(&mut child);
            let message = "无法读取命令 stderr。".to_string();
            session.fail_before_running(message.clone());
            mark_terminal(&self.inner, &session_id);
            return Err(CommandSessionError::SpawnFailed(message));
        };

        // The process is now live and both output pipes are owned by the host. Publish the
        // authoritative started boundary before reader threads can emit sequence 1.
        session.mark_running();

        let capture_policy = ProcessOutputCapturePolicy::process_default();
        let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
        let output_redactions = plan.output_redactions().clone();
        let weak_session = Arc::downgrade(&session);
        let transcript_observer: ProcessOutputTranscriptObserver = Arc::new(move |stream, text| {
            if let Some(session) = weak_session.upgrade() {
                session.commit_output(stream, text);
            }
        });
        let stdout_reader = spawn_process_output_capture_with_observers(
            stdout,
            capture_budget.clone(),
            capture_policy,
            Some(crate::AgentCommandOutputStream::Stdout),
            None,
            Some(transcript_observer.clone()),
            output_redactions.clone(),
        );
        let stderr_reader = spawn_process_output_capture_with_observers(
            stderr,
            capture_budget,
            capture_policy,
            Some(crate::AgentCommandOutputStream::Stderr),
            None,
            Some(transcript_observer),
            output_redactions,
        );
        let stdout_rx = relay_capture(stdout_reader, "stdout");
        let stderr_rx = relay_capture(stderr_reader, "stderr");

        let weak_manager = Arc::downgrade(&self.inner);
        let terminal_session_id = session_id.clone();
        let on_terminal: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
            if let Some(manager) = weak_manager.upgrade() {
                mark_terminal(&manager, &terminal_session_id);
            }
        });
        let watcher_session = session.clone();
        let hard_timeout = plan.hard_timeout();
        let interrupt_grace = self.inner.config.interrupt_grace;
        let drain_grace = self.inner.config.drain_grace;
        let child = ManagedCommandChild::new(child);
        let watcher = thread::Builder::new()
            .name(format!("command-session-{session_id}"))
            .spawn(move || {
                run_session_watcher(
                    watcher_session,
                    child,
                    control_rx,
                    stdout_rx,
                    stderr_rx,
                    started,
                    hard_timeout,
                    interrupt_grace,
                    drain_grace,
                    completion_hook,
                    on_terminal,
                );
            });
        let watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                // Dropping the never-started closure drops ManagedCommandChild,
                // which kills the complete group and performs a best-effort reap.
                let message = format!("启动命令 watcher 失败：{error}");
                session.fail_before_running(message);
                mark_terminal(&self.inner, &session_id);
                // `mark_running` already published the authoritative Started boundary. Return the
                // paired terminal outcome so the Host can settle that durable Session instead of
                // leaving a phantom Running row until restart reconciliation.
                return Ok(CommandStartOutcome::Exited(Box::new(
                    session
                        .terminal_result()
                        .expect("watcher startup failure commits a terminal result"),
                )));
            }
        };
        session.attach_watcher(watcher);

        let yield_duration = options.normalized_yield();
        let yield_deadline = Instant::now() + yield_duration;
        loop {
            let remaining = yield_deadline.saturating_duration_since(Instant::now());
            let slice = remaining.min(Duration::from_millis(25));
            let outcome = session.wait_start_outcome(slice);
            if matches!(outcome, CommandStartOutcome::Exited(_)) || remaining.is_zero() {
                return Ok(outcome);
            }
            if cancel_probe.as_ref().is_some_and(|probe| probe()) {
                let _ = session.control(
                    ProcessControl::ForceTerminate,
                    self.inner.config.shutdown_grace,
                );
                return Ok(session.wait_start_outcome(self.inner.config.shutdown_grace));
            }
        }
    }

    fn reserve(&self, session: Arc<ManagedCommandSession>) -> Result<(), CommandSessionError> {
        let mut registry = lock(&self.inner.registry);
        if !self.inner.accepting_starts.load(Ordering::Acquire) {
            return Err(CommandSessionError::ManagerShuttingDown);
        }
        prune_terminal(
            &mut registry,
            self.inner.config.max_retained_terminal_sessions,
        );
        if registry.active_sessions >= self.inner.config.max_active_sessions {
            return Err(CommandSessionError::GlobalLimitReached);
        }
        let scope_active = registry
            .active_by_scope
            .get(session.scope_id())
            .copied()
            .unwrap_or(0);
        if scope_active >= self.inner.config.max_active_sessions_per_scope {
            return Err(CommandSessionError::ScopeLimitReached);
        }
        registry.active_sessions += 1;
        *registry
            .active_by_scope
            .entry(session.scope_id().clone())
            .or_default() += 1;
        registry.sessions.insert(
            session.id().clone(),
            RegistryEntry {
                session,
                active: true,
            },
        );
        Ok(())
    }

    pub fn poll(
        &self,
        session_id: &CommandSessionId,
        wait: Duration,
    ) -> Result<CommandSessionPoll, CommandSessionError> {
        Ok(self
            .lookup(session_id)?
            .poll(wait, self.inner.config.poll_bytes))
    }

    pub fn read_output(
        &self,
        session_id: &CommandSessionId,
        after_sequence: u64,
        wait: Duration,
    ) -> Result<CommandSessionPoll, CommandSessionError> {
        Ok(self
            .lookup(session_id)?
            .read_output(after_sequence, wait, self.inner.config.poll_bytes))
    }

    /// Reads output accumulated until the session publishes a terminal state or
    /// `wait` elapses. Ordinary output remains observable during the wait but does
    /// not make this operation return early. Use [`Self::read_output`] for the
    /// immediate, sequence-driven view used by live consumers.
    pub fn read_output_until_terminal_or_deadline(
        &self,
        session_id: &CommandSessionId,
        after_sequence: u64,
        wait: Duration,
    ) -> Result<CommandSessionPoll, CommandSessionError> {
        Ok(self
            .lookup(session_id)?
            .read_output_until_terminal_or_deadline(
                after_sequence,
                wait,
                self.inner.config.poll_bytes,
            ))
    }

    pub fn snapshot(
        &self,
        session_id: &CommandSessionId,
    ) -> Result<CommandSessionSnapshot, CommandSessionError> {
        Ok(self.lookup(session_id)?.snapshot())
    }

    pub fn terminal_result(
        &self,
        session_id: &CommandSessionId,
    ) -> Result<Option<CommandTerminalResult>, CommandSessionError> {
        Ok(self.lookup(session_id)?.terminal_result())
    }

    pub fn wait_terminal_result(
        &self,
        session_id: &CommandSessionId,
        wait: Duration,
    ) -> Result<Option<CommandTerminalResult>, CommandSessionError> {
        Ok(self.lookup(session_id)?.wait_terminal_result(wait))
    }

    pub fn interrupt(
        &self,
        session_id: &CommandSessionId,
        wait: Duration,
    ) -> Result<CommandSessionSnapshot, CommandSessionError> {
        self.lookup(session_id)?
            .control(ProcessControl::Interrupt, wait)
    }

    pub fn force_terminate(
        &self,
        session_id: &CommandSessionId,
        wait: Duration,
    ) -> Result<CommandSessionSnapshot, CommandSessionError> {
        self.lookup(session_id)?
            .control(ProcessControl::ForceTerminate, wait)
    }

    pub fn list(&self, scope: Option<&CommandSessionScopeId>) -> Vec<CommandSessionSnapshot> {
        let sessions = {
            let registry = lock(&self.inner.registry);
            registry
                .sessions
                .values()
                .filter(|entry| scope.is_none_or(|scope| entry.session.scope_id() == scope))
                .map(|entry| entry.session.clone())
                .collect::<Vec<_>>()
        };
        let mut snapshots = sessions
            .iter()
            .map(|session| session.snapshot())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.started_at);
        snapshots
    }

    pub fn remove_terminal(
        &self,
        session_id: &CommandSessionId,
    ) -> Result<bool, CommandSessionError> {
        let mut registry = lock(&self.inner.registry);
        let Some(entry) = registry.sessions.get(session_id) else {
            return Err(CommandSessionError::NotFound);
        };
        if entry.active {
            return Ok(false);
        }
        registry.sessions.remove(session_id);
        Ok(true)
    }

    pub fn terminate_all(&self, wait: Duration) -> CommandTerminationReport {
        let sessions = {
            let registry = lock(&self.inner.registry);
            registry
                .sessions
                .values()
                .filter(|entry| entry.active)
                .map(|entry| entry.session.clone())
                .collect::<Vec<_>>()
        };
        for session in &sessions {
            session.force_for_shutdown();
        }
        let deadline = Instant::now() + wait;
        let mut terminated = 0;
        let mut still_running = Vec::new();
        for session in &sessions {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if session.wait_terminal_result(remaining).is_some() {
                terminated += 1;
            } else {
                still_running.push(session.id().clone());
            }
        }
        CommandTerminationReport {
            requested: sessions.len(),
            terminated,
            still_running,
        }
    }

    pub fn shutdown(&self, wait: Duration) -> CommandTerminationReport {
        self.inner.accepting_starts.store(false, Ordering::Release);
        self.terminate_all(wait)
    }

    fn lookup(
        &self,
        session_id: &CommandSessionId,
    ) -> Result<Arc<ManagedCommandSession>, CommandSessionError> {
        lock(&self.inner.registry)
            .sessions
            .get(session_id)
            .map(|entry| entry.session.clone())
            .ok_or(CommandSessionError::NotFound)
    }
}

fn immediate_terminal_outcome(
    scope_id: CommandSessionScopeId,
    execution: super::AgentCommandExecutionResult,
) -> CommandStartOutcome {
    let timestamp = super::session::unix_time_millis();
    let state = if execution.cancelled {
        super::CommandSessionState::Interrupted
    } else if execution.timed_out {
        super::CommandSessionState::TimedOut
    } else if execution.error.is_some() {
        super::CommandSessionState::Failed
    } else {
        super::CommandSessionState::Exited {
            exit_code: execution.exit_code,
        }
    };
    let snapshot = CommandSessionSnapshot {
        session_id: CommandSessionId::new(),
        scope_id,
        state,
        started_at: timestamp,
        ended_at: Some(timestamp),
        exit_code: execution.exit_code,
        latest_output_sequence: 0,
        output_truncated: execution.output_capture.truncated_at_source,
        projection: CommandSessionProjection {
            command: execution.command.clone(),
            cwd: execution.cwd.clone(),
        },
        error: execution.error.clone(),
    };
    CommandStartOutcome::Exited(Box::new(CommandTerminalResult {
        snapshot,
        execution,
    }))
}

impl Drop for CommandSessionManager {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            let _ = self.shutdown(self.inner.config.shutdown_grace);
        }
    }
}

fn cleanup_failed_spawn(child: &mut std::process::Child) {
    force_terminate_command_process_group(child);
    let _ = child.wait();
}

fn relay_capture(handle: ProcessOutputCaptureHandle, stream: &'static str) -> CaptureReceiver {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let result = handle
            .join()
            .map_err(|_| format!("读取命令 {stream} 的线程异常退出。"))
            .and_then(|result| result.map_err(|error| format!("读取命令 {stream} 失败：{error}")));
        let _ = sender.send(result);
    });
    receiver
}

fn mark_terminal(inner: &Arc<ManagerInner>, session_id: &CommandSessionId) {
    let mut registry = lock(&inner.registry);
    let Some(entry) = registry.sessions.get_mut(session_id) else {
        return;
    };
    if !entry.active {
        return;
    }
    entry.active = false;
    let scope_id = entry.session.scope_id().clone();
    registry.active_sessions = registry.active_sessions.saturating_sub(1);
    if let Some(active) = registry.active_by_scope.get_mut(&scope_id) {
        *active = active.saturating_sub(1);
        if *active == 0 {
            registry.active_by_scope.remove(&scope_id);
        }
    }
}

fn prune_terminal(registry: &mut RegistryState, max_retained: usize) {
    let terminal_count = registry
        .sessions
        .values()
        .filter(|entry| !entry.active)
        .count();
    let remove = terminal_count.saturating_sub(max_retained.saturating_sub(1));
    if remove == 0 {
        return;
    }
    let mut terminal = registry
        .sessions
        .iter()
        .filter(|(_, entry)| !entry.active)
        .map(|(id, entry)| (entry.session.snapshot().ended_at.unwrap_or(0), id.clone()))
        .collect::<Vec<_>>();
    terminal.sort_by_key(|(ended_at, _)| *ended_at);
    for (_, id) in terminal.into_iter().take(remove) {
        registry.sessions.remove(&id);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
