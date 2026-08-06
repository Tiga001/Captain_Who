//! One managed command session and its observable state machine.

use super::transcript::{CommandOutputBatch, CommandOutputChunk, CommandTranscript};
use super::{
    force_terminate_command_process_group, interrupt_command_process_group,
    try_wait_command_process_group, AgentCommandExecutionResult, CapturedProcessOutput,
    ManagedCommandChild, ProcessOutputCaptureMetadata, ProcessOutputObserver, ProcessOutputSpool,
    MAX_OUTPUT_BYTES,
};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandSessionId(String);

impl CommandSessionId {
    pub(crate) fn new() -> Self {
        Self(format!("cmd_{}", uuid::Uuid::new_v4().simple()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(value: &str) -> Result<Self, CommandSessionError> {
        let Some(value_hex) = value.strip_prefix("cmd_") else {
            return Err(CommandSessionError::InvalidSessionId);
        };
        if value_hex.len() != 32 || !value_hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(CommandSessionError::InvalidSessionId);
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

impl fmt::Display for CommandSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::str::FromStr for CommandSessionId {
    type Err = CommandSessionError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl TryFrom<&str> for CommandSessionId {
    type Error = CommandSessionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommandSessionScopeId(String);

impl CommandSessionScopeId {
    pub fn new(value: impl Into<String>) -> Result<Self, CommandSessionError> {
        let value = value.into();
        if value.trim().is_empty() || value.len() > 256 {
            return Err(CommandSessionError::InvalidScope);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSessionState {
    Starting,
    Running,
    Exited { exit_code: Option<i32> },
    Interrupted,
    TimedOut,
    Failed,
}

impl CommandSessionState {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Exited { .. } | Self::Interrupted | Self::TimedOut | Self::Failed
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSessionProjection {
    pub command: String,
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSessionSnapshot {
    pub session_id: CommandSessionId,
    pub scope_id: CommandSessionScopeId,
    pub state: CommandSessionState,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub latest_output_sequence: u64,
    pub output_truncated: bool,
    pub projection: CommandSessionProjection,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CommandTerminalResult {
    pub snapshot: CommandSessionSnapshot,
    pub execution: AgentCommandExecutionResult,
}

#[derive(Debug, Clone)]
pub enum CommandStartOutcome {
    Exited(Box<CommandTerminalResult>),
    Running(CommandSessionSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandSessionPoll {
    pub snapshot: CommandSessionSnapshot,
    pub output: CommandOutputBatch,
}

/// Host-facing lifecycle notifications emitted from the one authoritative
/// session state machine. Output notifications carry the exact transcript
/// sequence used by poll/read operations; hosts must not invent a second
/// sequence domain.
#[derive(Debug, Clone)]
pub enum CommandSessionLifecycleEvent {
    Started(CommandSessionSnapshot),
    Output {
        session_id: CommandSessionId,
        chunk: CommandOutputChunk,
        latest_sequence: u64,
        output_truncated: bool,
    },
    Terminal(Box<CommandTerminalResult>),
}

pub type CommandSessionLifecycleObserver =
    Arc<dyn Fn(CommandSessionLifecycleEvent) + Send + Sync + 'static>;

pub(crate) type CommandSessionCompletionHook =
    Box<dyn FnOnce(&mut AgentCommandExecutionResult) + Send + 'static>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSessionError {
    InvalidSessionId,
    InvalidScope,
    NotFound,
    GlobalLimitReached,
    ScopeLimitReached,
    InvalidConfiguration,
    ManagerShuttingDown,
    ControlChannelClosed,
    SpawnFailed(String),
}

impl fmt::Display for CommandSessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidSessionId => formatter.write_str("command session id is invalid"),
            Self::InvalidScope => formatter.write_str("command session scope is invalid"),
            Self::NotFound => formatter.write_str("command session was not found"),
            Self::GlobalLimitReached => formatter.write_str("global command session limit reached"),
            Self::ScopeLimitReached => formatter.write_str("command session scope limit reached"),
            Self::InvalidConfiguration => {
                formatter.write_str("command session manager configuration is invalid")
            }
            Self::ManagerShuttingDown => {
                formatter.write_str("command session manager is shutting down")
            }
            Self::ControlChannelClosed => {
                formatter.write_str("command session control channel is closed")
            }
            Self::SpawnFailed(error) => {
                write!(formatter, "failed to spawn command session: {error}")
            }
        }
    }
}

impl std::error::Error for CommandSessionError {}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ProcessControl {
    Interrupt,
    ForceTerminate,
}

#[derive(Debug)]
struct ObservedSessionState {
    state: CommandSessionState,
    ended_at: Option<u64>,
    error: Option<String>,
    transcript: CommandTranscript,
    live_output: Option<LiveOutputState>,
    terminal_result: Option<AgentCommandExecutionResult>,
}

#[derive(Debug)]
struct LiveOutputState {
    sender: mpsc::SyncSender<(crate::AgentCommandOutputStream, String)>,
    done: Receiver<()>,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

#[derive(Debug, Default)]
struct ModelInteractionState {
    output_cursor: u64,
}

pub(crate) struct ManagedCommandSession {
    id: CommandSessionId,
    scope_id: CommandSessionScopeId,
    projection: CommandSessionProjection,
    started_at: u64,
    observed: Mutex<ObservedSessionState>,
    changed: Condvar,
    /// Serializes state transitions with lifecycle delivery. Output sequence numbers are assigned
    /// under `observed`; taking this fence first guarantees callbacks observe Started, every
    /// Output, and Terminal in exactly that same order even when stdout/stderr readers race.
    lifecycle_order: Mutex<()>,
    interaction: Mutex<ModelInteractionState>,
    control_tx: SyncSender<ProcessControl>,
    watcher: Mutex<Option<std::thread::JoinHandle<()>>>,
    lifecycle_observer: Option<CommandSessionLifecycleObserver>,
}

impl fmt::Debug for ManagedCommandSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ManagedCommandSession")
            .field("id", &self.id)
            .field("scope_id", &self.scope_id)
            .field("projection", &self.projection)
            .field("started_at", &self.started_at)
            .field("snapshot", &self.snapshot())
            .finish_non_exhaustive()
    }
}

impl ManagedCommandSession {
    pub(crate) fn new(
        id: CommandSessionId,
        scope_id: CommandSessionScopeId,
        projection: CommandSessionProjection,
        started_at: u64,
        transcript_bytes: usize,
        output_observer: Option<ProcessOutputObserver>,
        lifecycle_observer: Option<CommandSessionLifecycleObserver>,
    ) -> (Arc<Self>, Receiver<ProcessControl>) {
        let (control_tx, control_rx) = mpsc::sync_channel(4);
        let live_output = output_observer.and_then(start_live_output_dispatcher);
        let session = Arc::new(Self {
            id,
            scope_id,
            projection,
            started_at,
            observed: Mutex::new(ObservedSessionState {
                state: CommandSessionState::Starting,
                ended_at: None,
                error: None,
                transcript: CommandTranscript::new(transcript_bytes),
                live_output,
                terminal_result: None,
            }),
            changed: Condvar::new(),
            lifecycle_order: Mutex::new(()),
            interaction: Mutex::new(ModelInteractionState::default()),
            control_tx,
            watcher: Mutex::new(None),
            lifecycle_observer,
        });
        (session, control_rx)
    }

    pub(crate) fn id(&self) -> &CommandSessionId {
        &self.id
    }

    pub(crate) fn scope_id(&self) -> &CommandSessionScopeId {
        &self.scope_id
    }

    pub(crate) fn attach_watcher(&self, watcher: std::thread::JoinHandle<()>) {
        *lock(&self.watcher) = Some(watcher);
    }

    pub(crate) fn mark_running(&self) {
        let _lifecycle_order = lock(&self.lifecycle_order);
        let snapshot = {
            let mut observed = lock(&self.observed);
            if observed.state != CommandSessionState::Starting {
                return;
            }
            observed.state = CommandSessionState::Running;
            self.changed.notify_all();
            self.snapshot_from(&observed)
        };
        self.notify_lifecycle(CommandSessionLifecycleEvent::Started(snapshot));
    }

    pub(crate) fn commit_output(
        &self,
        stream: crate::AgentCommandOutputStream,
        text: String,
    ) -> Vec<CommandOutputChunk> {
        let _lifecycle_order = lock(&self.lifecycle_order);
        let (committed, output_truncated) = {
            let mut observed = lock(&self.observed);
            if let Some(live) = observed.live_output.as_mut() {
                live.emit(stream, &text);
            }
            let committed = observed.transcript.commit(stream, text);
            if !committed.is_empty() {
                self.changed.notify_all();
            }
            (committed, observed.transcript.output_truncated())
        };
        for chunk in committed.iter().cloned() {
            // One lifecycle event represents one durably appendable prefix. Using the batch's
            // final sequence on every chunk would let consumers advance their persistence watermark
            // before the remaining chunks have actually been delivered.
            let latest_sequence = chunk.sequence;
            self.notify_lifecycle(CommandSessionLifecycleEvent::Output {
                session_id: self.id.clone(),
                chunk,
                latest_sequence,
                output_truncated,
            });
        }
        committed
    }

    pub(crate) fn complete(
        &self,
        state: CommandSessionState,
        result: AgentCommandExecutionResult,
        error: Option<String>,
        output_capture_incomplete: bool,
    ) -> bool {
        let _lifecycle_order = lock(&self.lifecycle_order);
        let live_output = {
            let mut observed = lock(&self.observed);
            if observed.state.is_terminal() {
                return false;
            }
            if output_capture_incomplete || result.output_capture.truncated_at_source {
                observed.transcript.mark_capture_truncated();
            }
            observed.transcript.close();
            observed.live_output.take()
        };
        if let Some(live_output) = live_output {
            live_output.finish(Duration::from_millis(250));
        }
        let terminal = {
            let mut observed = lock(&self.observed);
            observed.state = state;
            observed.ended_at = Some(unix_time_millis());
            observed.error = error;
            observed.terminal_result = Some(result.clone());
            self.changed.notify_all();
            CommandTerminalResult {
                snapshot: self.snapshot_from(&observed),
                execution: result,
            }
        };
        self.notify_lifecycle(CommandSessionLifecycleEvent::Terminal(Box::new(terminal)));
        true
    }

    pub(crate) fn fail_before_running(&self, error: String) {
        self.complete_before_running(CommandSessionState::Failed, false, Some(error));
    }

    pub(crate) fn cancel_before_running(&self) {
        self.complete_before_running(CommandSessionState::Interrupted, true, None);
    }

    fn complete_before_running(
        &self,
        state: CommandSessionState,
        cancelled: bool,
        error: Option<String>,
    ) {
        let result = AgentCommandExecutionResult {
            command: self.projection.command.clone(),
            cwd: self.projection.cwd.clone(),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled,
            duration_ms: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            output_capture: Default::default(),
            stdout_spool: Default::default(),
            stderr_spool: Default::default(),
            error: error.clone(),
            policy_evaluation: None,
            artifact_observation: None,
            input_files: Vec::new(),
            runtime: None,
        };
        self.complete(state, result, error, false);
    }

    pub(crate) fn snapshot(&self) -> CommandSessionSnapshot {
        let observed = lock(&self.observed);
        self.snapshot_from(&observed)
    }

    fn snapshot_from(&self, observed: &ObservedSessionState) -> CommandSessionSnapshot {
        let exit_code = match observed.state {
            CommandSessionState::Exited { exit_code } => exit_code,
            _ => observed
                .terminal_result
                .as_ref()
                .and_then(|result| result.exit_code),
        };
        CommandSessionSnapshot {
            session_id: self.id.clone(),
            scope_id: self.scope_id.clone(),
            state: observed.state.clone(),
            started_at: self.started_at,
            ended_at: observed.ended_at,
            exit_code,
            latest_output_sequence: observed.transcript.latest_sequence(),
            output_truncated: observed.transcript.output_truncated(),
            projection: self.projection.clone(),
            error: observed.error.clone(),
        }
    }

    pub(crate) fn wait_start_outcome(&self, wait: Duration) -> CommandStartOutcome {
        let observed = lock(&self.observed);
        let (observed, _) = self
            .changed
            .wait_timeout_while(observed, wait, |state| !state.state.is_terminal())
            .unwrap_or_else(|error| error.into_inner());
        let snapshot = self.snapshot_from(&observed);
        if let Some(execution) = observed.terminal_result.clone() {
            CommandStartOutcome::Exited(Box::new(CommandTerminalResult {
                snapshot,
                execution,
            }))
        } else {
            CommandStartOutcome::Running(snapshot)
        }
    }

    pub(crate) fn terminal_result(&self) -> Option<CommandTerminalResult> {
        let observed = lock(&self.observed);
        observed
            .terminal_result
            .clone()
            .map(|execution| CommandTerminalResult {
                snapshot: self.snapshot_from(&observed),
                execution,
            })
    }

    pub(crate) fn wait_terminal_result(&self, wait: Duration) -> Option<CommandTerminalResult> {
        let observed = lock(&self.observed);
        let (observed, _) = self
            .changed
            .wait_timeout_while(observed, wait, |state| !state.state.is_terminal())
            .unwrap_or_else(|error| error.into_inner());
        observed
            .terminal_result
            .clone()
            .map(|execution| CommandTerminalResult {
                snapshot: self.snapshot_from(&observed),
                execution,
            })
    }

    pub(crate) fn poll(&self, wait: Duration, max_bytes: usize) -> CommandSessionPoll {
        let mut interaction = lock(&self.interaction);
        let observed = lock(&self.observed);
        let cursor = interaction.output_cursor;
        let (observed, _) = self
            .changed
            .wait_timeout_while(observed, wait, |state| {
                !state.state.is_terminal() && state.transcript.latest_sequence() <= cursor
            })
            .unwrap_or_else(|error| error.into_inner());
        let output = observed.transcript.read_after(cursor, max_bytes);
        if let Some(last) = output.chunks.last() {
            interaction.output_cursor = last.sequence;
        } else if output.latest_sequence > cursor && output.truncated_before {
            interaction.output_cursor = output.latest_sequence;
        }
        CommandSessionPoll {
            snapshot: self.snapshot_from(&observed),
            output,
        }
    }

    pub(crate) fn read_output(
        &self,
        after: u64,
        wait: Duration,
        max_bytes: usize,
    ) -> CommandSessionPoll {
        let observed = lock(&self.observed);
        let (observed, _) = self
            .changed
            .wait_timeout_while(observed, wait, |state| {
                !state.state.is_terminal() && state.transcript.latest_sequence() <= after
            })
            .unwrap_or_else(|error| error.into_inner());
        CommandSessionPoll {
            snapshot: self.snapshot_from(&observed),
            output: observed.transcript.read_after(after, max_bytes),
        }
    }

    pub(crate) fn control(
        &self,
        control: ProcessControl,
        wait: Duration,
    ) -> Result<CommandSessionSnapshot, CommandSessionError> {
        let _interaction = lock(&self.interaction);
        if self.snapshot().state.is_terminal() {
            return Ok(self.snapshot());
        }
        self.control_tx
            .send(control)
            .map_err(|_| CommandSessionError::ControlChannelClosed)?;
        let observed = lock(&self.observed);
        let (observed, _) = self
            .changed
            .wait_timeout_while(observed, wait, |state| !state.state.is_terminal())
            .unwrap_or_else(|error| error.into_inner());
        Ok(self.snapshot_from(&observed))
    }

    /// Lifecycle-only escape hatch.  Application shutdown must not wait behind
    /// a model poll that is intentionally holding the interaction lock.
    pub(crate) fn force_for_shutdown(&self) {
        if !self.snapshot().state.is_terminal() {
            let _ = self.control_tx.try_send(ProcessControl::ForceTerminate);
        }
    }

    fn notify_lifecycle(&self, event: CommandSessionLifecycleEvent) {
        let Some(observer) = self.lifecycle_observer.as_ref() else {
            return;
        };
        let observer = Arc::clone(observer);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| observer(event)));
    }
}

fn start_live_output_dispatcher(observer: ProcessOutputObserver) -> Option<LiveOutputState> {
    let (sender, receiver) = mpsc::sync_channel::<(crate::AgentCommandOutputStream, String)>(64);
    let (done_sender, done) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("command-output-observer".to_string())
        .spawn(move || {
            while let Ok((stream, text)) = receiver.recv() {
                let callback = observer.clone();
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback(stream, text);
                }));
            }
            let _ = done_sender.send(());
        })
        .ok()?;
    Some(LiveOutputState {
        sender,
        done,
        stdout_bytes: 0,
        stderr_bytes: 0,
    })
}

impl LiveOutputState {
    fn emit(&mut self, stream: crate::AgentCommandOutputStream, text: &str) {
        let emitted = match stream {
            crate::AgentCommandOutputStream::Stdout => &mut self.stdout_bytes,
            crate::AgentCommandOutputStream::Stderr => &mut self.stderr_bytes,
        };
        let remaining = MAX_OUTPUT_BYTES.saturating_sub(*emitted);
        let prefix = utf8_prefix(text, remaining);
        if prefix.is_empty() {
            return;
        }
        *emitted = emitted.saturating_add(prefix.len());
        let _ = self.sender.try_send((stream, prefix.to_string()));
    }

    fn finish(self, grace: Duration) {
        let Self { sender, done, .. } = self;
        drop(sender);
        let _ = done.recv_timeout(grace);
    }
}

fn utf8_prefix(text: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

pub(crate) type CaptureReceiver = Receiver<Result<CapturedProcessOutput, String>>;

#[derive(Debug, Clone, Copy)]
enum TerminationIntent {
    Interrupted,
    TimedOut,
    Failed,
}

/// Sole owner loop for the OS child.  No manager operation directly touches
/// the child or process-group ID.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_session_watcher(
    session: Arc<ManagedCommandSession>,
    mut child: ManagedCommandChild,
    control_rx: Receiver<ProcessControl>,
    stdout_rx: CaptureReceiver,
    stderr_rx: CaptureReceiver,
    started: Instant,
    hard_timeout: Option<Duration>,
    interrupt_grace: Duration,
    drain_grace: Duration,
    completion_hook: Option<CommandSessionCompletionHook>,
    on_terminal: Arc<dyn Fn() + Send + Sync>,
) {
    let mut intent = None;
    let mut interrupt_deadline = None;
    let mut watcher_error = None;

    let exit_status = loop {
        match try_wait_command_process_group(child.child_mut()) {
            Ok(Some(status)) => {
                child.mark_reaped();
                break Some(status);
            }
            Ok(None) => {}
            Err(error) => {
                intent.get_or_insert(TerminationIntent::Failed);
                watcher_error = Some(format!("等待命令失败：{error}"));
                force_terminate_command_process_group(child.child_mut());
                let status = child.child_mut().wait().ok();
                child.mark_reaped();
                break status;
            }
        }

        if intent.is_none() && hard_timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            intent = Some(TerminationIntent::TimedOut);
            force_terminate_command_process_group(child.child_mut());
            let status = match child.child_mut().wait() {
                Ok(status) => Some(status),
                Err(error) => {
                    watcher_error = Some(format!("等待超时命令失败：{error}"));
                    None
                }
            };
            child.mark_reaped();
            break status;
        }

        if interrupt_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            force_terminate_command_process_group(child.child_mut());
            let status = match child.child_mut().wait() {
                Ok(status) => Some(status),
                Err(error) => {
                    watcher_error = Some(format!("等待已中断命令失败：{error}"));
                    None
                }
            };
            child.mark_reaped();
            break status;
        }

        match control_rx.recv_timeout(Duration::from_millis(20)) {
            Ok(ProcessControl::Interrupt) => {
                if intent.is_none() {
                    intent = Some(TerminationIntent::Interrupted);
                    interrupt_command_process_group(child.child_mut());
                    interrupt_deadline = Some(Instant::now() + interrupt_grace);
                }
            }
            Ok(ProcessControl::ForceTerminate) => {
                intent.get_or_insert(TerminationIntent::Interrupted);
                force_terminate_command_process_group(child.child_mut());
                let status = match child.child_mut().wait() {
                    Ok(status) => Some(status),
                    Err(error) => {
                        watcher_error = Some(format!("等待已终止命令失败：{error}"));
                        None
                    }
                };
                child.mark_reaped();
                break status;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                intent.get_or_insert(TerminationIntent::Interrupted);
                force_terminate_command_process_group(child.child_mut());
                let status = child.child_mut().wait().ok();
                child.mark_reaped();
                break status;
            }
        }
    };

    let drain_deadline = Instant::now() + drain_grace;
    let (stdout_capture, stdout_error) = receive_capture(&stdout_rx, drain_deadline, "stdout");
    let (stderr_capture, stderr_error) = receive_capture(&stderr_rx, drain_deadline, "stderr");
    let output_capture_incomplete = stdout_capture.is_none() || stderr_capture.is_none();
    let mut errors = Vec::new();
    errors.extend(watcher_error);
    errors.extend(stdout_error);
    errors.extend(stderr_error);
    let error = (!errors.is_empty()).then(|| errors.join(" "));

    let stdout = stdout_capture
        .as_ref()
        .map_or_else(String::new, |capture| capture.preview().to_string());
    let stderr = stderr_capture
        .as_ref()
        .map_or_else(String::new, |capture| capture.preview().to_string());
    let stdout_truncated = stdout_capture
        .as_ref()
        .is_none_or(CapturedProcessOutput::preview_truncated);
    let stderr_truncated = stderr_capture
        .as_ref()
        .is_none_or(CapturedProcessOutput::preview_truncated);
    let output_capture = ProcessOutputCaptureMetadata::from_optional_streams(
        stdout_capture.as_ref(),
        stderr_capture.as_ref(),
        output_capture_incomplete,
    );
    let stdout_spool = stdout_capture
        .as_ref()
        .map_or_else(ProcessOutputSpool::default, CapturedProcessOutput::spool);
    let stderr_spool = stderr_capture
        .as_ref()
        .map_or_else(ProcessOutputSpool::default, CapturedProcessOutput::spool);
    let state = match intent {
        Some(TerminationIntent::Interrupted) => CommandSessionState::Interrupted,
        Some(TerminationIntent::TimedOut) => CommandSessionState::TimedOut,
        Some(TerminationIntent::Failed) => CommandSessionState::Failed,
        None if error.is_some() => CommandSessionState::Failed,
        None => CommandSessionState::Exited {
            exit_code: exit_status
                .as_ref()
                .and_then(std::process::ExitStatus::code),
        },
    };
    let mut result = AgentCommandExecutionResult {
        command: session.projection.command.clone(),
        cwd: session.projection.cwd.clone(),
        exit_code: exit_status
            .as_ref()
            .and_then(std::process::ExitStatus::code),
        stdout,
        stderr,
        timed_out: state == CommandSessionState::TimedOut,
        cancelled: state == CommandSessionState::Interrupted,
        duration_ms: elapsed_millis(started),
        stdout_truncated,
        stderr_truncated,
        output_capture,
        stdout_spool,
        stderr_spool,
        error: error.clone(),
        policy_evaluation: None,
        artifact_observation: None,
        input_files: Vec::new(),
        runtime: None,
    };
    if let Some(completion_hook) = completion_hook {
        let completion = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            completion_hook(&mut result);
        }));
        if completion.is_err() {
            result.error = Some("命令终态结算回调异常退出。".to_string());
        }
    }
    session.complete(state, result, error, output_capture_incomplete);
    on_terminal();
}

fn receive_capture(
    receiver: &CaptureReceiver,
    deadline: Instant,
    stream: &str,
) -> (Option<CapturedProcessOutput>, Option<String>) {
    let remaining = deadline.saturating_duration_since(Instant::now());
    match receiver.recv_timeout(remaining) {
        Ok(Ok(capture)) => (Some(capture), None),
        Ok(Err(error)) => (None, Some(error)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            (None, Some(format!("读取命令 {stream} 超过有限排空期限。")))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            (None, Some(format!("读取命令 {stream} 的线程异常退出。")))
        }
    }
}

pub(crate) fn unix_time_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn elapsed_millis(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}
