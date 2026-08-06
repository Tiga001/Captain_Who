//! Host ownership for long-lived Agent command processes.
//!
//! The Core command manager owns OS processes. This registry binds those process Sessions to
//! durable Agent/conversation identity, persists their non-draining transcript projection, and
//! publishes lifecycle events. None of the callbacks in this module invoke the model runtime.

mod admission;
mod projection;
mod settlement_scheduler;

use admission::{
    HostCommandSessionAdmission, HostCommandSessionAdmissionLease,
    HostCommandSessionAdmissionLimits,
};
#[cfg(test)]
use projection::command_digest;
use projection::{
    archive_sequence, exit_event_status, host_snapshot, model_execution_output_from_read,
    protocol_exit_code, protocol_status, validate_conversation_id, validate_owner,
};
use settlement_scheduler::CommandSessionSettlementScheduler;

use super::run_lifecycle::FileEffectGuard;
use super::*;
use mycopilot_core::command::{
    CommandSessionId, CommandSessionLifecycleEvent, CommandSessionLifecycleObserver,
    CommandSessionManager, CommandSessionScopeId, CommandSessionSnapshot as CoreSessionSnapshot,
    CommandSessionState, CommandStartOptions, CommandStartOutcome, CommandTerminalResult,
};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionModelReadRequest,
    AgentCommandSessionOutputAppend, AgentCommandSessionTerminalUpdate,
    AgentCommandSessionTransitionOutcome,
};
use mycopilot_core::storage::conversation_history_archive_repository::{
    ConversationHistoryArchiveDescriptor, ConversationHistoryArchiveFileInput,
    ConversationHistoryArchiveInput,
};
use mycopilot_core::{
    AgentCommandSessionAction, AgentCommandSessionExecutionOutput,
    AgentCommandSessionExecutionRequest, AgentCommandSessionExecutor, AgentCommandSessionGetInput,
    AgentCommandSessionGetOutput, AgentCommandSessionListInput, AgentCommandSessionListOutput,
    AgentCommandSessionOutputChunk, AgentCommandSessionSnapshot, AgentCommandSessionStatus,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{self, TrySendError};
use std::sync::{Condvar, Weak};
use std::thread;

const HOST_TRANSCRIPT_DEFAULT_BYTES: usize = 256 * 1024;
const HOST_TRANSCRIPT_MAX_BYTES: usize = 1024 * 1024;
const SESSION_TERMINAL_LIST_LIMIT: usize = 128;
const SESSION_CONTROL_WAIT_MAX_MS: u64 = 30_000;
const SESSION_TERMINATE_WAIT: Duration = Duration::from_secs(5);
const SESSION_LIFECYCLE_QUEUE_CAPACITY: usize = 256;

#[derive(Clone)]
pub(super) struct AgentCommandSessionRegistry {
    inner: Arc<AgentCommandSessionRegistryInner>,
}

struct AgentCommandSessionRegistryInner {
    manager: CommandSessionManager,
    storage: Arc<StorageService>,
    sessions: Mutex<HashMap<String, Arc<HostCommandSession>>>,
    interaction_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    admission: Arc<HostCommandSessionAdmission>,
    settlement_scheduler: Arc<CommandSessionSettlementScheduler>,
    initial_yield: Duration,
}

struct HostCommandSession {
    owner: CommandSessionOwner,
    authorization_source: CommandAuthorizationSource,
    approval_provenance: Value,
    permission_provenance: Value,
    notifications: Option<CoreServerNotificationSender>,
    state: Mutex<HostCommandSessionState>,
    changed: Condvar,
}

struct HostCommandSessionState {
    session_id: Option<CommandSessionId>,
    handoff: HandoffState,
    terminal: Option<CommandTerminalResult>,
    file_effect_guard: Option<FileEffectGuard>,
    persistence_error: Option<String>,
    terminal_settled: bool,
    terminal_archive_ref: Option<String>,
    output_persistence_truncated: bool,
    persisted_sequence: u64,
    admission_lease: Option<HostCommandSessionAdmissionLease>,
}

enum QueuedCommandSessionLifecycle {
    Output {
        session_id: CommandSessionId,
        chunk: mycopilot_core::command::CommandOutputChunk,
        latest_sequence: u64,
        output_truncated: bool,
    },
    Terminal(Box<CommandTerminalResult>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HandoffState {
    Pending,
    Adopted,
    Synchronous,
    Aborted,
}

#[derive(Debug, Clone)]
pub(super) struct CommandSessionOwner {
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub origin_run_id: String,
    pub call_id: String,
    pub project_id: Option<String>,
}

pub(super) struct StartAgentCommandSession<'a> {
    pub owner: CommandSessionOwner,
    pub workspace_root: Option<&'a Path>,
    pub command: &'a mycopilot_core::AgentCommandRequest,
    pub permissions: mycopilot_core::AgentPermissions,
    pub authorization_source: CommandAuthorizationSource,
    pub approval_provenance: Value,
    pub artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
    pub file_inputs: Option<&'a AgentFileInputExecutionContext>,
    pub notifications: Option<CoreServerNotificationSender>,
    pub cancellation_token: AgentCancellationToken,
    pub cancel_probe: Option<Arc<dyn Fn() -> bool + Send + Sync>>,
}

#[derive(Debug)]
pub(super) enum AgentCommandSessionLaunch {
    Exited(Box<CommandTerminalResult>),
    Running {
        snapshot: Box<AgentCommandSessionSnapshot>,
        tool_result: AgentToolResult,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentCommandHandoffOutcome {
    Adopted,
    CancelledBeforeCommit,
    PersistenceFailed(String),
}

impl AgentCommandSessionRegistry {
    pub(super) fn new(storage: Arc<StorageService>) -> Self {
        Self::with_manager(
            storage,
            CommandSessionManager::default(),
            CommandStartOptions::default().initial_yield,
        )
    }

    pub(super) fn with_manager(
        storage: Arc<StorageService>,
        manager: CommandSessionManager,
        initial_yield: Duration,
    ) -> Self {
        Self::with_manager_and_admission_config(
            storage,
            manager,
            initial_yield,
            HostCommandSessionAdmissionLimits::default(),
        )
    }

    #[cfg(test)]
    pub(super) fn with_manager_and_admission_limits(
        storage: Arc<StorageService>,
        manager: CommandSessionManager,
        initial_yield: Duration,
        max_sessions: usize,
        max_sessions_per_conversation: usize,
    ) -> Self {
        Self::with_manager_and_admission_config(
            storage,
            manager,
            initial_yield,
            HostCommandSessionAdmissionLimits {
                max_sessions,
                max_sessions_per_conversation,
            },
        )
    }

    fn with_manager_and_admission_config(
        storage: Arc<StorageService>,
        manager: CommandSessionManager,
        initial_yield: Duration,
        admission_limits: HostCommandSessionAdmissionLimits,
    ) -> Self {
        let admission = HostCommandSessionAdmission::new(admission_limits)
            .expect("command Session Host admission limits must be valid");
        // The worker receives only a Weak registry reference. It cannot receive work until this
        // constructor returns, so `Arc::new_cyclic` never requires upgrading the still-building
        // registry. This also lets the last Host owner stop the worker instead of forming a cycle.
        let inner = Arc::new_cyclic(|weak| AgentCommandSessionRegistryInner {
            manager,
            storage,
            sessions: Mutex::new(HashMap::new()),
            interaction_locks: Mutex::new(HashMap::new()),
            admission,
            settlement_scheduler: CommandSessionSettlementScheduler::start(weak.clone()),
            initial_yield,
        });
        Self { inner }
    }

    pub(super) fn start(
        &self,
        request: StartAgentCommandSession<'_>,
    ) -> Result<AgentCommandSessionLaunch, String> {
        validate_owner(&request.owner)?;
        let admission_lease = self
            .inner
            .admission
            .try_acquire(&request.owner.conversation_id)?;
        let scope = CommandSessionScopeId::new(request.owner.conversation_id.clone())
            .map_err(|error| error.to_string())?;
        let session = Arc::new(HostCommandSession {
            owner: request.owner,
            authorization_source: request.authorization_source,
            approval_provenance: request.approval_provenance,
            permission_provenance: serde_json::to_value(request.permissions)
                .map_err(|error| format!("无法冻结命令 Session 权限来源：{error}"))?,
            notifications: request.notifications,
            state: Mutex::new(HostCommandSessionState {
                session_id: None,
                handoff: HandoffState::Pending,
                terminal: None,
                file_effect_guard: None,
                persistence_error: None,
                terminal_settled: false,
                terminal_archive_ref: None,
                output_persistence_truncated: false,
                persisted_sequence: 0,
                admission_lease: Some(admission_lease),
            }),
            changed: Condvar::new(),
        });
        let (lifecycle_tx, lifecycle_rx) =
            mpsc::sync_channel::<QueuedCommandSessionLifecycle>(SESSION_LIFECYCLE_QUEUE_CAPACITY);
        let writer_registry = Arc::clone(&self.inner);
        let writer_session = Arc::clone(&session);
        thread::Builder::new()
            .name("agent-command-session-writer".to_string())
            .spawn(move || {
                writer_registry.run_lifecycle_writer(&writer_session, lifecycle_rx);
            })
            .map_err(|error| format!("无法启动命令 Session 持久化任务：{error}"))?;
        let weak_registry = Arc::downgrade(&self.inner);
        let observed_session = Arc::clone(&session);
        let lifecycle_observer: CommandSessionLifecycleObserver = Arc::new(move |event| {
            match event {
                CommandSessionLifecycleEvent::Started(snapshot) => {
                    let Some(registry) = weak_registry.upgrade() else {
                        observed_session.record_persistence_error(
                            "命令 Session Host 已关闭，无法保存启动状态。".to_string(),
                        );
                        return;
                    };
                    if let Err(error) = registry.persist_started(&observed_session, &snapshot) {
                        observed_session.record_persistence_error(error);
                    }
                }
                CommandSessionLifecycleEvent::Output {
                    session_id,
                    chunk,
                    latest_sequence,
                    output_truncated,
                } => {
                    let event = QueuedCommandSessionLifecycle::Output {
                        session_id,
                        chunk,
                        latest_sequence,
                        output_truncated,
                    };
                    match lifecycle_tx.try_send(event) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => {
                            observed_session.mark_output_persistence_truncated();
                        }
                        Err(TrySendError::Disconnected(_)) => {
                            observed_session.mark_output_persistence_truncated();
                            observed_session.record_persistence_error(
                                "命令 Session 输出持久化任务已停止。".to_string(),
                            );
                        }
                    }
                }
                CommandSessionLifecycleEvent::Terminal(terminal) => {
                    // The process has exited and both pipes have already received a bounded drain.
                    // A blocking enqueue here preserves Output-before-Terminal ordering without
                    // ever applying backpressure to the stdout/stderr reader threads.
                    if lifecycle_tx
                        .send(QueuedCommandSessionLifecycle::Terminal(terminal))
                        .is_err()
                    {
                        observed_session.record_persistence_error(
                            "命令 Session 终态持久化任务已停止。".to_string(),
                        );
                    }
                }
            }
        });

        let outcome = match self
            .inner
            .manager
            .start_authorized_command_with_runtime_and_lifecycle(
                scope,
                request.workspace_root,
                request.command,
                request.permissions,
                request.authorization_source,
                CommandStartOptions {
                    initial_yield: self.inner.initial_yield,
                },
                request.artifact_runtime,
                request.file_inputs,
                lifecycle_observer,
                request.cancellation_token,
                request.cancel_probe,
            ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let original_error = error.to_string();
                if let Some(session_id) = session.session_id_string() {
                    if let Err(cleanup_error) = self.abort_before_handoff(&session_id) {
                        session.record_persistence_error(format!(
                            "命令启动失败后的交接前清理未完成：{cleanup_error}"
                        ));
                    }
                } else {
                    session.release_admission();
                }
                return Err(original_error);
            }
        };

        match outcome {
            CommandStartOutcome::Exited(terminal) => {
                // Managed-runtime preflight can produce a terminal result without spawning an OS
                // process; such a result deliberately has no durable process Session row.
                if session.session_id().is_some() {
                    if let Err(error) =
                        self.finish_synchronous(terminal.snapshot.session_id.as_str())
                    {
                        session.record_persistence_error(error);
                    }
                }
                Ok(AgentCommandSessionLaunch::Exited(terminal))
            }
            CommandStartOutcome::Running(snapshot) => {
                if let Some(error) = session.persistence_error() {
                    let terminal = self.abort_before_handoff(snapshot.session_id.as_str())?;
                    session.record_persistence_error(format!(
                        "命令已在持久交接前终止，因为 Session 状态无法安全保存：{error}"
                    ));
                    return Ok(AgentCommandSessionLaunch::Exited(Box::new(terminal)));
                }
                session.wait_for_persisted_sequence(
                    snapshot.latest_output_sequence,
                    Duration::from_secs(1),
                );
                match self.running_receipt(&snapshot, &session) {
                    Ok((host_snapshot, tool_result)) => Ok(AgentCommandSessionLaunch::Running {
                        snapshot: Box::new(host_snapshot),
                        tool_result,
                    }),
                    Err(receipt_error) => {
                        let terminal = self.abort_before_handoff(snapshot.session_id.as_str())?;
                        session.record_persistence_error(format!(
                            "命令已在持久交接前终止，因为 running 回执无法安全生成：{receipt_error}"
                        ));
                        Ok(AgentCommandSessionLaunch::Exited(Box::new(terminal)))
                    }
                }
            }
        }
    }

    /// Serializes cancellation, the durable running receipt, and process ownership transfer.
    ///
    /// Holding the Session state lock across `persist` gives cancellation one unambiguous order:
    /// cancellation which acquires the fence first changes `Pending -> Aborted`; a handoff which
    /// acquires it first commits the durable receipt and changes `Pending -> Adopted` before the
    /// cancellation path can inspect this Session.
    pub(super) fn commit_handoff(
        &self,
        session_id: &str,
        file_effect_guard: &mut Option<FileEffectGuard>,
        cancelled: impl FnOnce() -> bool,
        persist: impl FnOnce() -> Result<(), String>,
    ) -> Result<AgentCommandHandoffOutcome, String> {
        let session = self.live_session(session_id)?;
        {
            let mut state = lock(&session.state);
            if state.handoff != HandoffState::Pending {
                return Ok(if state.handoff == HandoffState::Aborted {
                    AgentCommandHandoffOutcome::CancelledBeforeCommit
                } else {
                    return Err("命令 Session 的持久交接状态已被结算。".to_string());
                });
            }
            if cancelled() {
                state.handoff = HandoffState::Aborted;
                session.changed.notify_all();
                return Ok(AgentCommandHandoffOutcome::CancelledBeforeCommit);
            }
            if let Err(error) = persist() {
                state.handoff = HandoffState::Aborted;
                session.changed.notify_all();
                return Ok(AgentCommandHandoffOutcome::PersistenceFailed(error));
            }
            state.handoff = HandoffState::Adopted;
            state.file_effect_guard = Some(
                file_effect_guard
                    .take()
                    .ok_or_else(|| "命令 Session 缺少待交接的 File Effect lease。".to_string())?,
            );
            session.changed.notify_all();
        }
        if session.terminal().is_some() {
            self.inner.schedule_terminal_settlement(session)?;
        }
        Ok(AgentCommandHandoffOutcome::Adopted)
    }

    /// Cancels only Sessions which have not crossed the durable handoff fence. Adopted process
    /// Sessions intentionally outlive their originating Agent Run.
    pub(super) fn cancel_pre_handoff_for_run(&self, run_id: &str) -> usize {
        self.cancel_pre_handoff_matching(|session| session.owner.origin_run_id == run_id)
    }

    /// Action-scoped counterpart used by `agent.cancelAction`. A single Agent Run may own more
    /// than one command call, so cancelling one approval must not terminate sibling Sessions.
    pub(super) fn cancel_pre_handoff_for_action(&self, run_id: &str, call_id: &str) -> usize {
        self.cancel_pre_handoff_matching(|session| {
            session.owner.origin_run_id == run_id && session.owner.call_id == call_id
        })
    }

    fn cancel_pre_handoff_matching(
        &self,
        predicate: impl Fn(&HostCommandSession) -> bool,
    ) -> usize {
        let sessions = lock(&self.inner.sessions)
            .values()
            .filter(|session| predicate(session))
            .cloned()
            .collect::<Vec<_>>();
        let mut cancelled = 0;
        for session in sessions {
            let session_id = {
                let mut state = lock(&session.state);
                if state.handoff != HandoffState::Pending {
                    continue;
                }
                state.handoff = HandoffState::Aborted;
                session.changed.notify_all();
                cancelled += 1;
                state.session_id.clone()
            };
            if let Some(session_id) = session_id {
                let _ = self
                    .inner
                    .manager
                    .force_terminate(&session_id, Duration::ZERO);
            }
        }
        cancelled
    }

    /// Cancels a process which has not crossed the persistent handoff boundary. The originating
    /// Agent action remains responsible for its ordinary terminal audit and file-effect lease.
    pub(super) fn abort_before_handoff(
        &self,
        session_id: &str,
    ) -> Result<CommandTerminalResult, String> {
        let session = self.live_session(session_id)?;
        {
            let mut state = lock(&session.state);
            if state.handoff == HandoffState::Adopted {
                return Err("命令 Session 已完成持久交接，不能按交接前语义终止。".to_string());
            }
            state.handoff = HandoffState::Aborted;
        }
        let id = CommandSessionId::parse(session_id).map_err(|error| error.to_string())?;
        let mut last_error = None;
        let mut terminal = None;
        for _ in 0..3 {
            if let Err(error) = self
                .inner
                .manager
                .force_terminate(&id, SESSION_TERMINATE_WAIT)
            {
                last_error = Some(error.to_string());
            }
            match self
                .inner
                .manager
                .wait_terminal_result(&id, SESSION_TERMINATE_WAIT)
            {
                Ok(Some(result)) => {
                    terminal = Some(result);
                    break;
                }
                Ok(None) => {}
                Err(error) => last_error = Some(error.to_string()),
            }
        }
        let terminal = terminal.ok_or_else(|| {
            format!(
                "命令 Session 在交接前反复终止后仍未确认退出；Host 将继续保留进程所有权和未结算副作用保护。{}",
                last_error
                    .as_deref()
                    .map(|error| format!(" 最后错误：{error}"))
                    .unwrap_or_default()
            )
        })?;
        if let Err(error) = self.finish_synchronous(session_id) {
            // The process is authoritatively terminal. Preserve that exact execution result for
            // the ordinary action audit even when auxiliary Session persistence is unavailable;
            // startup reconciliation will conservatively repair any surviving active row.
            session.record_persistence_error(error);
        }
        Ok(terminal)
    }

    fn finish_synchronous(&self, session_id: &str) -> Result<(), String> {
        let session = self.live_session(session_id)?;
        let terminal = session
            .wait_for_terminal(SESSION_TERMINATE_WAIT)
            .ok_or_else(|| {
                "命令进程已经结束，但 Host 输出队列未在有界窗口内完成排空；保留未结算 Session，禁止抢先提交终态。"
                    .to_string()
            })?;
        {
            let mut state = lock(&session.state);
            if state.handoff == HandoffState::Pending {
                state.handoff = HandoffState::Synchronous;
            }
        }
        match self.inner.settle_synchronous(&session, &terminal) {
            Ok(()) => Ok(()),
            Err(error) => {
                session.record_persistence_error(error.clone());
                self.inner
                    .schedule_terminal_settlement(Arc::clone(&session))
                    .map_err(|schedule_error| {
                        format!("{error}；且无法安排有界的后台结算重试：{schedule_error}")
                    })?;
                Err(error)
            }
        }
    }

    pub(super) fn list(
        &self,
        input: AgentCommandSessionListInput,
    ) -> Result<AgentCommandSessionListOutput, String> {
        validate_conversation_id(&input.conversation_id)?;
        let sessions = self
            .inner
            .storage
            .list_agent_command_sessions(&input.conversation_id, SESSION_TERMINAL_LIST_LIMIT)?
            .into_iter()
            // The Session row is the Host API's authoritative cut. In-memory process state can be
            // ahead of the async durable transcript and must not create an internally inconsistent
            // snapshot/transcript pair for reload consumers.
            .map(|record| record.snapshot)
            .collect();
        Ok(AgentCommandSessionListOutput { sessions })
    }

    pub(super) fn get(
        &self,
        input: AgentCommandSessionGetInput,
    ) -> Result<AgentCommandSessionGetOutput, String> {
        validate_conversation_id(&input.conversation_id)?;
        CommandSessionId::parse(&input.session_id).map_err(|error| error.to_string())?;
        let max_bytes = input
            .max_bytes
            .unwrap_or(HOST_TRANSCRIPT_DEFAULT_BYTES)
            .clamp(1, HOST_TRANSCRIPT_MAX_BYTES);
        let (session, transcript) = self
            .inner
            .storage
            .load_agent_command_session_with_transcript(
                &input.conversation_id,
                &input.session_id,
                input.after_sequence.unwrap_or(0),
                max_bytes,
            )?
            .ok_or_else(|| "命令 Session 不存在或不属于当前会话。".to_string())?;
        Ok(AgentCommandSessionGetOutput {
            session: session.snapshot,
            transcript,
        })
    }

    pub(super) fn terminate_conversation(&self, conversation_id: &str) {
        self.terminate_matching(|session| session.owner.conversation_id == conversation_id);
    }

    pub(super) fn terminate_project(&self, project_id: &str) {
        self.terminate_matching(|session| session.owner.project_id.as_deref() == Some(project_id));
    }

    pub(super) fn terminate_messages(&self, conversation_id: &str, message_ids: &HashSet<String>) {
        self.terminate_matching(|session| {
            session.owner.conversation_id == conversation_id
                && message_ids.contains(&session.owner.assistant_message_id)
        });
    }

    pub(super) fn shutdown(&self, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        let report = self.inner.manager.shutdown(wait);
        let sessions = lock(&self.inner.sessions)
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut settled = report.still_running.is_empty();
        for session in sessions {
            let state = lock(&session.state);
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (state, _) = session
                .changed
                .wait_timeout_while(state, remaining, |state| {
                    matches!(
                        state.handoff,
                        HandoffState::Adopted | HandoffState::Synchronous | HandoffState::Aborted
                    ) && state.terminal.is_some()
                        && !state.terminal_settled
                })
                .unwrap_or_else(|error| error.into_inner());
            settled &= !matches!(
                state.handoff,
                HandoffState::Adopted | HandoffState::Synchronous | HandoffState::Aborted
            ) || state.terminal.is_none()
                || state.terminal_settled;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        settled & self.inner.settlement_scheduler.shutdown(remaining)
    }

    fn terminate_matching(&self, predicate: impl Fn(&HostCommandSession) -> bool) {
        let ids = lock(&self.inner.sessions)
            .iter()
            .filter(|(_, session)| predicate(session) && !session.is_terminal_settled())
            .filter_map(|(id, _)| CommandSessionId::parse(id).ok())
            .collect::<Vec<_>>();
        for id in ids {
            let _ = self
                .inner
                .manager
                .force_terminate(&id, SESSION_TERMINATE_WAIT);
        }
    }

    fn running_receipt(
        &self,
        snapshot: &CoreSessionSnapshot,
        session: &HostCommandSession,
    ) -> Result<(AgentCommandSessionSnapshot, AgentToolResult), String> {
        let record = self
            .inner
            .storage
            .load_agent_command_session(snapshot.scope_id.as_str(), snapshot.session_id.as_str())?
            .ok_or_else(|| "命令 Session 的持久状态不存在。".to_string())?;
        let read = self
            .inner
            .storage
            .read_or_create_agent_command_session_model_read(
                &AgentCommandSessionModelReadRequest {
                    conversation_id: &record.snapshot.conversation_id,
                    session_id: &record.snapshot.session_id,
                    // The initial running result belongs to the original run_command ToolCall.
                    // Persisting that exact identity makes this output cut replayable if the Host
                    // stops after advancing the cursor but before the handoff audit is committed.
                    run_id: &record.snapshot.origin_run_id,
                    call_id: &record.snapshot.call_id,
                    action: AgentCommandSessionAction::Poll,
                    max_output_bytes: mycopilot_core::AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                    host_output_truncated: session.output_persistence_truncated(),
                    created_at: now_ms(),
                },
            )?
            .ok_or_else(|| "命令 Session 的持久状态在生成 running 回执期间消失。".to_string())?;
        let output = read
            .chunks
            .iter()
            .map(|chunk| chunk.output.as_str())
            .collect::<String>();
        let tool_result = AgentToolResult {
            exact_archive_file: None,
            call_id: record.snapshot.call_id.clone(),
            tool: "run_command".to_string(),
            ok: true,
            result: Some(json!({
                "status": "running",
                "sessionId": record.snapshot.session_id,
                "output": output,
                "startedAt": record.snapshot.started_at,
                "latestSequence": read.receipt.latest_sequence,
                "outputTruncated": read.receipt.output_truncated,
            })),
            error: None,
        };
        Ok((record.snapshot, tool_result))
    }

    fn live_session(&self, session_id: &str) -> Result<Arc<HostCommandSession>, String> {
        lock(&self.inner.sessions)
            .get(session_id)
            .cloned()
            .ok_or_else(|| "命令 Session 不存在或已不再由当前 Host 控制。".to_string())
    }

    #[cfg(test)]
    pub(super) fn settlement_scheduler_stats(&self) -> (usize, usize, usize) {
        self.inner.settlement_scheduler.stats()
    }

    #[cfg(test)]
    pub(super) fn retained_admission_count(&self) -> usize {
        self.inner.admission.retained()
    }
}

impl AgentService {
    pub fn list_command_sessions(
        &self,
        input: AgentCommandSessionListInput,
    ) -> Result<AgentCommandSessionListOutput, String> {
        self.command_sessions.list(input)
    }

    pub fn get_command_session(
        &self,
        input: AgentCommandSessionGetInput,
    ) -> Result<AgentCommandSessionGetOutput, String> {
        self.command_sessions.get(input)
    }
}

impl AgentCommandSessionExecutor for AgentCommandSessionRegistry {
    fn execute_command_session(
        &self,
        request: AgentCommandSessionExecutionRequest,
    ) -> AgentResult<AgentCommandSessionExecutionOutput> {
        let id = CommandSessionId::parse(&request.session_id)
            .map_err(|_| AgentError::new("command_session.sessionId 格式无效。"))?;
        // Establish conversation ownership before consulting the process registry. Session IDs are
        // globally unguessable, but possession of an ID must never bypass conversation isolation.
        self.inner
            .storage
            .load_agent_command_session(&request.conversation_id, &request.session_id)
            .map_err(AgentError::new)?
            .ok_or_else(|| {
                AgentError::structured(
                    "agent.command_session_not_found",
                    "命令 Session 不存在或不属于当前会话。",
                    json!({"type":"command_session","code":"notFound"}),
                )
            })?;
        let live = lock(&self.inner.sessions).get(&request.session_id).cloned();
        if live
            .as_ref()
            .is_some_and(|session| session.owner.conversation_id != request.conversation_id)
        {
            return Err(AgentError::structured(
                "agent.command_session_not_found",
                "命令 Session 不存在或不属于当前会话。",
                json!({"type":"command_session","code":"notFound"}),
            ));
        }
        // The interaction fence is keyed independently from the live process object. Terminal
        // Sessions are removed from `sessions`, but their durable model cursor must remain just as
        // serialized as a live Session's cursor. Weak entries make the lock table self-pruning
        // without coupling its lifetime to terminal Session retention.
        let interaction = self.inner.interaction_lock(&request.session_id);
        let _interaction = lock(&interaction);
        let receipt_lookup = AgentCommandSessionModelReadRequest {
            conversation_id: &request.conversation_id,
            session_id: &request.session_id,
            run_id: &request.run_id,
            call_id: &request.call_id,
            action: request.action,
            max_output_bytes: request.max_output_bytes,
            host_output_truncated: false,
            created_at: now_ms(),
        };
        if let Some(read) = self
            .inner
            .storage
            .load_agent_command_session_model_read(&receipt_lookup)
            .map_err(AgentError::new)?
        {
            // A retried runtime ToolCall must replay its already-committed transcript cut before
            // any process control. In particular, an `interrupt` retry must not signal twice.
            return Ok(model_execution_output_from_read(&read));
        }
        let record = self
            .inner
            .storage
            .load_agent_command_session(&request.conversation_id, &request.session_id)
            .map_err(AgentError::new)?
            .ok_or_else(|| AgentError::new("命令 Session 在交互期间消失。"))?;

        let mut observed_sequence = record.snapshot.latest_sequence;
        let wait = Duration::from_millis(request.wait_ms.min(SESSION_CONTROL_WAIT_MAX_MS));
        if !record.snapshot.status.is_terminal() {
            let Some(live) = live.as_ref() else {
                return Err(AgentError::structured(
                    "agent.command_session_not_controllable",
                    "命令 Session 已不再由当前 Host 控制。",
                    json!({"type":"command_session","code":"notControllable"}),
                ));
            };
            let control = if let Some(terminal) = live.terminal() {
                Ok(terminal.snapshot.latest_output_sequence)
            } else {
                match request.action {
                    AgentCommandSessionAction::Interrupt => self
                        .inner
                        .manager
                        .interrupt(&id, wait)
                        .map(|snapshot| snapshot.latest_output_sequence),
                    AgentCommandSessionAction::Poll => {
                        // Waiting observes the in-process transcript but drains neither the Host
                        // nor model cursor. The durable model cursor advances atomically below.
                        self.inner
                            .manager
                            .read_output(&id, record.model_read_sequence, wait)
                            .map(|poll| poll.snapshot.latest_output_sequence)
                    }
                }
            };
            match control {
                Ok(sequence) => observed_sequence = sequence,
                Err(error) => {
                    // Terminal settlement may remove the Core Session between the durable reload
                    // and the manager call. Accept that race only when either the Host terminal is
                    // already visible or the durable row has reached a terminal state.
                    if let Some(terminal) = live.wait_for_terminal(Duration::from_millis(50)) {
                        observed_sequence = terminal.snapshot.latest_output_sequence;
                    } else {
                        let refreshed = self
                            .inner
                            .storage
                            .load_agent_command_session(
                                &request.conversation_id,
                                &request.session_id,
                            )
                            .map_err(AgentError::new)?
                            .ok_or_else(|| AgentError::new("命令 Session 在交互期间消失。"))?;
                        if !refreshed.snapshot.status.is_terminal() {
                            return Err(AgentError::new(error.to_string()));
                        }
                    }
                }
            }
            live.wait_for_persisted_sequence(observed_sequence, wait);
            if live.terminal().is_some() {
                // Do not project an in-memory terminal state ahead of Exact Archive + terminal
                // lifecycle + Session CAS. A zero-wait poll may still observe `running` briefly and
                // can poll again; a normal bounded poll waits for the authoritative durable cut.
                live.wait_for_terminal_settled(wait);
            }
        }

        let receipt_request = AgentCommandSessionModelReadRequest {
            conversation_id: &request.conversation_id,
            session_id: &request.session_id,
            run_id: &request.run_id,
            call_id: &request.call_id,
            action: request.action,
            max_output_bytes: request.max_output_bytes,
            host_output_truncated: live
                .as_ref()
                .is_some_and(|session| session.output_persistence_truncated()),
            created_at: now_ms(),
        };
        let read = self
            .inner
            .storage
            .read_or_create_agent_command_session_model_read(&receipt_request)
            .map_err(AgentError::new)?
            .ok_or_else(|| AgentError::new("命令 Session 不存在或不属于当前会话。"))?;
        Ok(model_execution_output_from_read(&read))
    }
}

impl AgentCommandSessionRegistryInner {
    fn interaction_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        let mut locks = lock(&self.interaction_locks);
        locks.retain(|_, candidate| candidate.strong_count() > 0);
        if let Some(existing) = locks.get(session_id).and_then(Weak::upgrade) {
            return existing;
        }
        let interaction = Arc::new(Mutex::new(()));
        locks.insert(session_id.to_string(), Arc::downgrade(&interaction));
        interaction
    }

    fn run_lifecycle_writer(
        self: &Arc<Self>,
        session: &Arc<HostCommandSession>,
        receiver: mpsc::Receiver<QueuedCommandSessionLifecycle>,
    ) {
        while let Ok(event) = receiver.recv() {
            let (result, was_output) = match event {
                QueuedCommandSessionLifecycle::Output {
                    session_id,
                    chunk,
                    latest_sequence,
                    output_truncated,
                } => (
                    self.persist_output(
                        session,
                        session_id.as_str(),
                        AgentCommandSessionOutputChunk {
                            sequence: chunk.sequence,
                            stream: chunk.stream,
                            output: chunk.text,
                        },
                        latest_sequence,
                        output_truncated || session.output_persistence_truncated(),
                    ),
                    true,
                ),
                QueuedCommandSessionLifecycle::Terminal(mut terminal) => {
                    if session.output_persistence_truncated() {
                        terminal.snapshot.output_truncated = true;
                    }
                    self.observe_terminal(session, *terminal);
                    break;
                }
            };
            if let Err(error) = result {
                if was_output {
                    // Any failed append creates a permanent gap in the Host transcript. Preserve
                    // that fact in every subsequent model/Host projection and terminal receipt.
                    session.mark_output_persistence_truncated();
                }
                session.record_persistence_error(error);
            }
        }
    }

    fn persist_started(
        self: &Arc<Self>,
        session: &Arc<HostCommandSession>,
        snapshot: &CoreSessionSnapshot,
    ) -> Result<(), String> {
        // Register the live process identity before the first durable write. If persistence fails,
        // `start` must still be able to address and terminate this exact OS process before it can
        // escape the pre-handoff boundary.
        {
            let mut state = lock(&session.state);
            state.session_id = Some(snapshot.session_id.clone());
            session.changed.notify_all();
        }
        lock(&self.sessions).insert(snapshot.session_id.to_string(), Arc::clone(session));
        let mut host_snapshot = host_snapshot(&session.owner, snapshot);
        host_snapshot.status = AgentCommandSessionStatus::Starting;
        let create = AgentCommandSessionCreate {
            snapshot: host_snapshot,
            authorization_source: session.authorization_source,
            approval_provenance: session.approval_provenance.clone(),
            permission_provenance: session.permission_provenance.clone(),
            created_at: i64::try_from(snapshot.started_at)
                .map_err(|_| "命令 Session 启动时间超出持久化范围。".to_string())?,
        };
        self.storage.create_agent_command_session(&create)?;
        match self
            .storage
            .mark_agent_command_session_running_with_lifecycle(
                &session.owner.conversation_id,
                snapshot.session_id.as_str(),
                now_ms(),
            )? {
            AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent => {}
            outcome => return Err(format!("命令 Session 无法进入 running 状态：{outcome:?}")),
        }
        if let Some(notifications) = session.notifications.as_ref() {
            let _ = notifications.send(agent_event_notification(AgentEvent::CommandStarted {
                run_id: session.owner.origin_run_id.clone(),
                conversation_id: session.owner.conversation_id.clone(),
                assistant_message_id: session.owner.assistant_message_id.clone(),
                project_id: session.owner.project_id.clone(),
                call_id: session.owner.call_id.clone(),
                session_id: snapshot.session_id.to_string(),
                started_at: snapshot.started_at,
            }));
        }
        Ok(())
    }

    fn persist_output(
        &self,
        session: &HostCommandSession,
        session_id: &str,
        chunk: AgentCommandSessionOutputChunk,
        latest_sequence: u64,
        output_truncated: bool,
    ) -> Result<(), String> {
        match self.storage.append_agent_command_session_output(
            &AgentCommandSessionOutputAppend {
                conversation_id: &session.owner.conversation_id,
                session_id,
                chunks: std::slice::from_ref(&chunk),
                latest_sequence,
                transcript_truncated: output_truncated,
                output_capture_truncated: false,
                updated_at: now_ms(),
            },
        )? {
            AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent => {}
            outcome => return Err(format!("命令 Session 输出无法持久化：{outcome:?}")),
        }
        if let Some(notifications) = session.notifications.as_ref() {
            let _ = notifications.send(agent_event_notification(AgentEvent::CommandOutput {
                run_id: session.owner.origin_run_id.clone(),
                conversation_id: session.owner.conversation_id.clone(),
                assistant_message_id: session.owner.assistant_message_id.clone(),
                project_id: session.owner.project_id.clone(),
                call_id: session.owner.call_id.clone(),
                session_id: session_id.to_string(),
                sequence: chunk.sequence,
                stream: chunk.stream,
                output: chunk.output,
            }));
        }
        session.mark_output_persisted(latest_sequence);
        Ok(())
    }

    fn observe_terminal(
        self: &Arc<Self>,
        session: &Arc<HostCommandSession>,
        terminal: CommandTerminalResult,
    ) {
        let handed_off = {
            let mut state = lock(&session.state);
            state.terminal = Some(terminal.clone());
            session.changed.notify_all();
            state.handoff == HandoffState::Adopted
        };
        if handed_off {
            if let Err(error) = self.schedule_terminal_settlement(Arc::clone(session)) {
                session.record_persistence_error(error);
            }
        }
    }

    fn schedule_terminal_settlement(&self, session: Arc<HostCommandSession>) -> Result<(), String> {
        self.settlement_scheduler.schedule(session)
    }

    fn settle_synchronous(
        &self,
        session: &HostCommandSession,
        terminal: &CommandTerminalResult,
    ) -> Result<(), String> {
        let status = protocol_status(&terminal.snapshot.state);
        let session_id = terminal.snapshot.session_id.as_str();
        let update = AgentCommandSessionTerminalUpdate {
            conversation_id: &session.owner.conversation_id,
            session_id,
            status,
            ended_at: terminal
                .snapshot
                .ended_at
                .unwrap_or(terminal.snapshot.started_at),
            exit_code: protocol_exit_code(&terminal.snapshot.state, terminal.snapshot.exit_code),
            latest_sequence: terminal.snapshot.latest_output_sequence,
            transcript_truncated: terminal.snapshot.output_truncated,
            output_capture_truncated: terminal.execution.output_capture.truncated_at_source,
            archive_ref: None,
            terminal_reason: terminal.snapshot.error.as_deref(),
            committed_at: now_ms(),
        };
        match self
            .storage
            .settle_agent_command_session_with_lifecycle(&update)?
        {
            AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent => {
                self.publish_terminal(session, terminal);
                session.mark_terminal_settled();
                self.forget_terminal_session(session_id);
                session.release_admission();
                Ok(())
            }
            outcome => Err(format!("同步命令 Session 终态无法持久化：{outcome:?}")),
        }
    }

    fn settle_handed_off(
        &self,
        session: &HostCommandSession,
        terminal: &CommandTerminalResult,
    ) -> Result<(), String> {
        // Artifact-after observation is already attached by the Core completion hook before this
        // lifecycle callback runs. Archive is deliberately completed before lifecycle/Session CAS.
        let archive_ref = if let Some(archive_ref) = session.terminal_archive_ref() {
            archive_ref
        } else {
            let archive = self.archive_terminal_result(session, terminal)?;
            session.cache_terminal_archive_ref(archive.archive_ref)
        };
        let status = protocol_status(&terminal.snapshot.state);
        let update = AgentCommandSessionTerminalUpdate {
            conversation_id: &session.owner.conversation_id,
            session_id: terminal.snapshot.session_id.as_str(),
            status,
            ended_at: terminal
                .snapshot
                .ended_at
                .unwrap_or(terminal.snapshot.started_at),
            exit_code: protocol_exit_code(&terminal.snapshot.state, terminal.snapshot.exit_code),
            latest_sequence: terminal.snapshot.latest_output_sequence,
            transcript_truncated: terminal.snapshot.output_truncated,
            output_capture_truncated: terminal.execution.output_capture.truncated_at_source,
            archive_ref: Some(&archive_ref),
            terminal_reason: terminal.snapshot.error.as_deref(),
            committed_at: now_ms(),
        };
        match self
            .storage
            .settle_agent_command_session_with_lifecycle(&update)?
        {
            AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent => {}
            outcome => return Err(format!("后台命令 Session 终态无法持久化：{outcome:?}")),
        }
        self.publish_terminal(session, terminal);
        let mut state = lock(&session.state);
        if let Some(mut guard) = state.file_effect_guard.take() {
            guard.mark_durably_settled();
        }
        drop(state);
        self.forget_terminal_session(terminal.snapshot.session_id.as_str());
        session.release_admission();
        Ok(())
    }

    fn forget_terminal_session(&self, session_id: &str) {
        lock(&self.sessions).remove(session_id);
        if let Ok(session_id) = CommandSessionId::parse(session_id) {
            let _ = self.manager.remove_terminal(&session_id);
        }
    }

    fn archive_terminal_result(
        &self,
        session: &HostCommandSession,
        terminal: &CommandTerminalResult,
    ) -> Result<ConversationHistoryArchiveDescriptor, String> {
        let archive_call_id = format!("command-session:{}", terminal.snapshot.session_id);
        let tool_result =
            mycopilot_core::command::command_tool_result(&archive_call_id, &terminal.execution);
        let projected = project_persisted_continuation_for_archive(&tool_result);
        let archive_projection_truncated = serde_json::to_value(&projected)
            .map_err(|error| format!("无法比较命令 Session 归档投影：{error}"))?
            != serde_json::to_value(&tool_result)
                .map_err(|error| format!("无法比较命令 Session 原始结果：{error}"))?;
        let sequence = archive_sequence(terminal.snapshot.session_id.as_str());
        let common = (
            session.owner.conversation_id.clone(),
            session.owner.assistant_message_id.clone(),
            sequence,
            archive_call_id,
            "run_command".to_string(),
            "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            mycopilot_core::tool_result_truncated_at_source(&tool_result),
            true,
            archive_projection_truncated,
            now_ms(),
        );
        if let Some(file) = tool_result
            .exact_archive_file
            .as_ref()
            .or(projected.exact_archive_file.as_ref())
        {
            self.storage.archive_conversation_tool_result_file(
                ConversationHistoryArchiveFileInput {
                    conversation_id: common.0,
                    assistant_message_id: common.1,
                    sequence: common.2,
                    call_id: common.3,
                    tool: common.4,
                    content_type: common.5,
                    content_path: file.path().to_path_buf(),
                    truncated_at_source: common.6,
                    model_projection_truncated: common.7,
                    archive_projection_truncated: common.8,
                    created_at: common.9,
                },
            )
        } else {
            let content = serde_json::to_string(&projected)
                .map_err(|error| format!("无法序列化命令 Session 终态归档：{error}"))?;
            self.storage
                .archive_conversation_tool_result(ConversationHistoryArchiveInput {
                    conversation_id: common.0,
                    assistant_message_id: common.1,
                    sequence: common.2,
                    call_id: common.3,
                    tool: common.4,
                    content_type: common.5,
                    content,
                    truncated_at_source: common.6,
                    model_projection_truncated: common.7,
                    archive_projection_truncated: common.8,
                    created_at: common.9,
                })
        }
    }

    fn publish_terminal(&self, session: &HostCommandSession, terminal: &CommandTerminalResult) {
        let Some(notifications) = session.notifications.as_ref() else {
            return;
        };
        let common = (
            session.owner.origin_run_id.clone(),
            session.owner.conversation_id.clone(),
            session.owner.assistant_message_id.clone(),
            session.owner.project_id.clone(),
            session.owner.call_id.clone(),
            terminal.snapshot.session_id.to_string(),
            terminal
                .snapshot
                .ended_at
                .unwrap_or(terminal.snapshot.started_at),
            terminal.snapshot.latest_output_sequence,
            terminal.snapshot.output_truncated,
        );
        let event = if matches!(terminal.snapshot.state, CommandSessionState::Interrupted) {
            AgentEvent::CommandInterrupted {
                run_id: common.0,
                conversation_id: common.1,
                assistant_message_id: common.2,
                project_id: common.3,
                call_id: common.4,
                session_id: common.5,
                ended_at: common.6,
                latest_sequence: common.7,
                output_truncated: common.8,
            }
        } else {
            AgentEvent::CommandExited {
                run_id: common.0,
                conversation_id: common.1,
                assistant_message_id: common.2,
                project_id: common.3,
                call_id: common.4,
                session_id: common.5,
                status: exit_event_status(&terminal.snapshot.state),
                exit_code: protocol_exit_code(
                    &terminal.snapshot.state,
                    terminal.snapshot.exit_code,
                ),
                ended_at: common.6,
                latest_sequence: common.7,
                output_truncated: common.8,
            }
        };
        let _ = notifications.send(agent_event_notification(event));
    }
}

impl HostCommandSession {
    fn session_id(&self) -> Option<CommandSessionId> {
        lock(&self.state).session_id.clone()
    }

    fn session_id_string(&self) -> Option<String> {
        self.session_id().map(|value| value.to_string())
    }

    fn terminal(&self) -> Option<CommandTerminalResult> {
        lock(&self.state).terminal.clone()
    }

    fn handoff_state(&self) -> HandoffState {
        lock(&self.state).handoff
    }

    fn wait_for_terminal(&self, wait: Duration) -> Option<CommandTerminalResult> {
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| state.terminal.is_none())
            .unwrap_or_else(|error| error.into_inner());
        state.terminal.clone()
    }

    fn persistence_error(&self) -> Option<String> {
        lock(&self.state).persistence_error.clone()
    }

    fn record_persistence_error(&self, error: String) {
        let mut state = lock(&self.state);
        if state.persistence_error.is_none() {
            state.persistence_error = Some(error);
        }
        self.changed.notify_all();
    }

    fn mark_output_persistence_truncated(&self) {
        let mut state = lock(&self.state);
        state.output_persistence_truncated = true;
        self.changed.notify_all();
    }

    fn output_persistence_truncated(&self) -> bool {
        lock(&self.state).output_persistence_truncated
    }

    fn mark_output_persisted(&self, sequence: u64) {
        let mut state = lock(&self.state);
        state.persisted_sequence = state.persisted_sequence.max(sequence);
        self.changed.notify_all();
    }

    fn wait_for_persisted_sequence(&self, sequence: u64, wait: Duration) {
        if sequence == 0 || wait.is_zero() {
            return;
        }
        let state = lock(&self.state);
        let _ = self
            .changed
            .wait_timeout_while(state, wait, |state| {
                state.persisted_sequence < sequence
                    && !state.output_persistence_truncated
                    && state.persistence_error.is_none()
            })
            .unwrap_or_else(|error| error.into_inner());
    }

    fn mark_terminal_settled(&self) {
        let mut state = lock(&self.state);
        state.terminal_settled = true;
        self.changed.notify_all();
    }

    fn is_terminal_settled(&self) -> bool {
        lock(&self.state).terminal_settled
    }

    fn wait_for_terminal_settled(&self, wait: Duration) -> bool {
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| !state.terminal_settled)
            .unwrap_or_else(|error| error.into_inner());
        state.terminal_settled
    }

    fn terminal_archive_ref(&self) -> Option<String> {
        lock(&self.state).terminal_archive_ref.clone()
    }

    fn cache_terminal_archive_ref(&self, archive_ref: String) -> String {
        let mut state = lock(&self.state);
        state
            .terminal_archive_ref
            .get_or_insert(archive_ref)
            .clone()
    }

    fn release_admission(&self) {
        let lease = lock(&self.state).admission_lease.take();
        if let Some(lease) = lease {
            lease.release();
        }
    }
}

impl Drop for AgentCommandSessionRegistryInner {
    fn drop(&mut self) {
        self.settlement_scheduler.request_stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_sequence_is_stable_and_sqlite_safe() {
        let id = "cmd_0123456789abcdef0123456789abcdef";
        assert_eq!(archive_sequence(id), archive_sequence(id));
        assert!(archive_sequence(id) <= i64::MAX as u64);
        assert_ne!(archive_sequence(id), 0);
    }

    #[test]
    fn command_digest_uses_tagged_lowercase_sha256() {
        let digest = command_digest("printf ok");
        assert_eq!(digest.len(), 71);
        assert!(digest.starts_with("sha256:"));
        assert!(digest[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    }
}
