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
    ManagedCommandWorkspaceLease,
};
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionModelReadRequest,
    AgentCommandSessionOutputAppend, AgentCommandSessionRecord, AgentCommandSessionTerminalUpdate,
    AgentCommandSessionTransitionOutcome,
};
use mycopilot_core::storage::conversation_history_archive_repository::{
    ConversationHistoryArchiveDescriptor, ConversationHistoryArchiveFileInput,
    ConversationHistoryArchiveInput,
};
use mycopilot_core::{
    AgentCommandSessionAction, AgentCommandSessionExecutionControl,
    AgentCommandSessionExecutionOutput, AgentCommandSessionExecutionRequest,
    AgentCommandSessionExecutor, AgentCommandSessionGetInput, AgentCommandSessionGetOutput,
    AgentCommandSessionListInput, AgentCommandSessionListOutput, AgentCommandSessionOutputChunk,
    AgentCommandSessionSnapshot, AgentCommandSessionStatus, AGENT_COMMAND_SESSION_MAX_WAIT_MS,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::sync::{Condvar, Weak};
use std::thread;

const HOST_TRANSCRIPT_DEFAULT_BYTES: usize = 256 * 1024;
const HOST_TRANSCRIPT_MAX_BYTES: usize = 1024 * 1024;
const SESSION_TERMINAL_LIST_LIMIT: usize = 128;
const SESSION_CONTROL_WAIT_MAX_MS: u64 = AGENT_COMMAND_SESSION_MAX_WAIT_MS;
const SESSION_OBSERVATION_SLICE: Duration = Duration::from_millis(50);
const SESSION_TERMINATE_WAIT: Duration = Duration::from_secs(5);
const SESSION_LIFECYCLE_QUEUE_CAPACITY: usize = 256;
const SESSION_PENDING_HANDOFF_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub(super) struct AgentCommandSessionRegistry {
    inner: Arc<AgentCommandSessionRegistryInner>,
}

struct AgentCommandSessionRegistryInner {
    manager: CommandSessionManager,
    storage: Arc<StorageService>,
    sessions: Mutex<HashMap<String, Arc<HostCommandSession>>>,
    interaction_locks: Mutex<HashMap<String, Weak<Mutex<()>>>>,
    managed_workspaces: Mutex<HashMap<(String, String), ManagedCommandWorkspaceLease>>,
    admission: Arc<HostCommandSessionAdmission>,
    settlement_scheduler: Arc<CommandSessionSettlementScheduler>,
    initial_yield: Duration,
    pending_handoff_timeout: Duration,
    #[cfg(test)]
    before_file_effect_install_hook: Mutex<Option<BeforeFileEffectInstallHook>>,
    #[cfg(test)]
    after_durable_create_hook: Mutex<Option<AfterDurableCreateHook>>,
    #[cfg(test)]
    durable_start_inspection_hook: Mutex<Option<DurableStartInspectionHook>>,
}

#[cfg(test)]
type BeforeFileEffectInstallHook = Arc<dyn Fn(&str) + Send + Sync>;

#[cfg(test)]
type AfterDurableCreateHook = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

#[cfg(test)]
type DurableStartInspectionHook = Arc<dyn Fn(&str) -> Result<(), String> + Send + Sync>;

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
    durable_start: DurableStartState,
    terminal: Option<CommandTerminalResult>,
    file_effect_guard: Option<FileEffectGuard>,
    persistence_error: Option<String>,
    terminal_settled: bool,
    terminal_archive_ref: Option<String>,
    /// A terminal callback may race ahead of `start`, but Archive/Trace/Session settlement must
    /// remain closed until the caller's File Effect ownership has been installed and the launch
    /// path has selected its synchronous, aborted, or externally guarded state.
    terminal_settlement_ready: bool,
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
enum DurableStartState {
    Pending,
    Ready,
    Failed {
        error: String,
        durable_row: DurableRowState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DurableRowState {
    Absent,
    Present,
    Indeterminate,
}

#[derive(Debug, Clone)]
struct DurableStartFailure {
    error: String,
    durable_row: DurableRowState,
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
    /// The file-effect lease stays caller-owned until a process Session identity exists. Once a
    /// Session starts, the Host atomically takes this lease before returning either launch outcome.
    pub file_effect_guard: &'a mut Option<FileEffectGuard>,
}

#[derive(Debug)]
pub(super) enum AgentCommandSessionLaunch {
    Exited(Box<CommandTerminalResult>),
    Running {
        snapshot: Box<AgentCommandSessionSnapshot>,
        tool_result: AgentToolResult,
        handoff_guard: AgentCommandSessionHandoffGuard,
    },
}

/// Linear ownership token for the narrow `Running receipt -> durable handoff` interval.
///
/// Dropping this token (including unwinding a caller panic) aborts the still-pending Session
/// without blocking the dropping thread. A separate Host deadline closes the same boundary if a
/// caller leaks the token instead of dropping it.
pub(super) struct AgentCommandSessionHandoffGuard {
    registry: AgentCommandSessionRegistry,
    session_id: String,
    armed: bool,
}

impl std::fmt::Debug for AgentCommandSessionHandoffGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentCommandSessionHandoffGuard")
            .field("session_id", &self.session_id)
            .field("armed", &self.armed)
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum AgentCommandHandoffOutcome {
    Adopted,
    CancelledBeforeCommit,
    PersistenceFailed(String),
}

impl AgentCommandSessionHandoffGuard {
    pub(super) fn commit(
        &mut self,
        cancelled: impl FnOnce() -> bool,
        persist: impl FnOnce() -> Result<(), String>,
    ) -> Result<AgentCommandHandoffOutcome, String> {
        let registry = self.registry.clone();
        registry.commit_handoff(self, cancelled, persist)
    }

    pub(super) fn abort_before_handoff(&mut self) -> Result<CommandTerminalResult, String> {
        let registry = self.registry.clone();
        registry.abort_before_handoff(self)
    }

    fn finish_synchronous(&mut self) -> Result<(), String> {
        let registry = self.registry.clone();
        registry.finish_synchronous(self)
    }

    fn validate_registry(&self, registry: &AgentCommandSessionRegistry) -> Result<(), String> {
        if !self.armed {
            return Err("命令 Session 的交接所有权已经结算。".to_string());
        }
        if !Arc::ptr_eq(&self.registry.inner, &registry.inner) {
            return Err("命令 Session 交接所有权不属于当前 Host。".to_string());
        }
        Ok(())
    }
}

impl Drop for AgentCommandSessionHandoffGuard {
    fn drop(&mut self) {
        if self.armed {
            self.registry
                .abandon_pending_handoff(self.session_id.as_str());
            self.armed = false;
        }
    }
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
            SESSION_PENDING_HANDOFF_TIMEOUT,
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
            SESSION_PENDING_HANDOFF_TIMEOUT,
        )
    }

    #[cfg(test)]
    pub(super) fn with_manager_and_handoff_timeout(
        storage: Arc<StorageService>,
        manager: CommandSessionManager,
        initial_yield: Duration,
        pending_handoff_timeout: Duration,
    ) -> Self {
        Self::with_manager_and_admission_config(
            storage,
            manager,
            initial_yield,
            HostCommandSessionAdmissionLimits::default(),
            pending_handoff_timeout,
        )
    }

    fn with_manager_and_admission_config(
        storage: Arc<StorageService>,
        manager: CommandSessionManager,
        initial_yield: Duration,
        admission_limits: HostCommandSessionAdmissionLimits,
        pending_handoff_timeout: Duration,
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
            managed_workspaces: Mutex::new(HashMap::new()),
            admission,
            settlement_scheduler: CommandSessionSettlementScheduler::start(weak.clone()),
            initial_yield,
            pending_handoff_timeout,
            #[cfg(test)]
            before_file_effect_install_hook: Mutex::new(None),
            #[cfg(test)]
            after_durable_create_hook: Mutex::new(None),
            #[cfg(test)]
            durable_start_inspection_hook: Mutex::new(None),
        });
        Self { inner }
    }

    pub(super) fn start(
        &self,
        request: StartAgentCommandSession<'_>,
    ) -> Result<AgentCommandSessionLaunch, String> {
        validate_owner(&request.owner)?;
        let managed_workspace =
            self.managed_workspace_for_command(&request.owner, request.command)?;
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
                durable_start: DurableStartState::Pending,
                terminal: None,
                file_effect_guard: None,
                persistence_error: None,
                terminal_settled: false,
                terminal_archive_ref: None,
                terminal_settlement_ready: false,
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
        let pending_handoff_deadline =
            Instant::now() + self.inner.initial_yield + self.inner.pending_handoff_timeout;
        thread::Builder::new()
            .name("agent-command-session-writer".to_string())
            .spawn(move || {
                writer_registry.run_lifecycle_writer(
                    &writer_session,
                    lifecycle_rx,
                    pending_handoff_deadline,
                );
            })
            .map_err(|error| format!("无法启动命令 Session 持久化任务：{error}"))?;
        let weak_registry = Arc::downgrade(&self.inner);
        let observed_session = Arc::clone(&session);
        let lifecycle_observer: CommandSessionLifecycleObserver = Arc::new(move |event| {
            match event {
                CommandSessionLifecycleEvent::Started(snapshot) => {
                    let Some(registry) = weak_registry.upgrade() else {
                        let error = "命令 Session Host 已关闭，无法保存启动状态。".to_string();
                        observed_session.mark_durable_start_failed(
                            error.clone(),
                            DurableRowState::Indeterminate,
                        );
                        observed_session.record_persistence_error(error);
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
                    // Output is useful only after the authoritative Session row exists and its
                    // starting -> running lifecycle transaction has committed. In particular,
                    // never enqueue output after a failed create: the writer would otherwise
                    // retry against a row which can never appear.
                    if !observed_session.durable_start_is_ready() {
                        observed_session.mark_output_persistence_truncated();
                        return;
                    }
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
                managed_workspace,
                lifecycle_observer,
                request.cancellation_token,
                request.cancel_probe,
            ) {
            Ok(outcome) => outcome,
            Err(error) => {
                let original_error = error.to_string();
                if let Some(session_id) = session.session_id_string() {
                    if let Err(start_failure) =
                        session.wait_for_durable_start(SESSION_TERMINATE_WAIT)
                    {
                        return self
                            .fail_durable_start(
                                &session,
                                start_failure,
                                None,
                                request.file_effect_guard,
                            )
                            .map(AgentCommandSessionLaunch::Exited);
                    }
                    session.install_file_effect_guard(request.file_effect_guard.take())?;
                    let mut handoff_guard = self.handoff_guard(session_id);
                    if let Err(cleanup_error) = handoff_guard.abort_before_handoff() {
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

        if session.session_id_string().is_some() {
            if let Err(start_failure) = session.wait_for_durable_start(SESSION_TERMINATE_WAIT) {
                let terminal_hint = match outcome {
                    CommandStartOutcome::Exited(terminal) => Some(*terminal),
                    CommandStartOutcome::Running(_) => None,
                };
                return self
                    .fail_durable_start(
                        &session,
                        start_failure,
                        terminal_hint,
                        request.file_effect_guard,
                    )
                    .map(AgentCommandSessionLaunch::Exited);
            }
        }

        #[cfg(test)]
        if let Some(session_id) = session.session_id_string() {
            self.inner
                .run_before_file_effect_install_hook(session_id.as_str());
        }

        match outcome {
            CommandStartOutcome::Exited(mut terminal) => {
                // Managed-runtime preflight can produce a terminal result without spawning an OS
                // process; such a result deliberately has no durable process Session row.
                if let Some(session_id) = session.session_id_string() {
                    session.install_file_effect_guard(request.file_effect_guard.take())?;
                    let mut handoff_guard = self.handoff_guard(session_id);
                    if let Err(error) = handoff_guard.finish_synchronous() {
                        session.record_persistence_error(error.clone());
                        return Err(format!(
                            "命令已经结束，但 Host 未能确认唯一的 Archive/Trace/Session 终态；完整输出由后台 Session 继续持有：{error}"
                        ));
                    }
                    if let Some(authoritative) = session.terminal() {
                        // The lifecycle writer owns managed-output publication. Return that same
                        // authoritative terminal receipt to the synchronous caller so run_command
                        // and a later command_session read cannot disagree about outputs.
                        terminal = Box::new(authoritative);
                    }
                    session.clear_archived_execution_spools(&mut terminal.execution)?;
                }
                Ok(AgentCommandSessionLaunch::Exited(terminal))
            }
            CommandStartOutcome::Running(snapshot) => {
                session.install_file_effect_guard(request.file_effect_guard.take())?;
                let mut handoff_guard = self.handoff_guard(snapshot.session_id.to_string());
                if session.mark_terminal_settlement_ready() == HandoffState::Aborted {
                    let mut terminal = handoff_guard.abort_before_handoff()?;
                    session.clear_archived_execution_spools(&mut terminal.execution)?;
                    return Ok(AgentCommandSessionLaunch::Exited(Box::new(terminal)));
                }
                if let Some(error) = session.persistence_error() {
                    let mut terminal = handoff_guard.abort_before_handoff()?;
                    session.record_persistence_error(format!(
                        "命令已在持久交接前终止，因为 Session 状态无法安全保存：{error}"
                    ));
                    session.clear_archived_execution_spools(&mut terminal.execution)?;
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
                        handoff_guard,
                    }),
                    Err(receipt_error) => {
                        let mut terminal = handoff_guard.abort_before_handoff()?;
                        session.record_persistence_error(format!(
                            "命令已在持久交接前终止，因为 running 回执无法安全生成：{receipt_error}"
                        ));
                        session.clear_archived_execution_spools(&mut terminal.execution)?;
                        Ok(AgentCommandSessionLaunch::Exited(Box::new(terminal)))
                    }
                }
            }
        }
    }

    fn managed_workspace_for_command(
        &self,
        owner: &CommandSessionOwner,
        command: &mycopilot_core::AgentCommandRequest,
    ) -> Result<Option<ManagedCommandWorkspaceLease>, String> {
        let is_managed_pdf = command.runtime_binding.as_deref().is_some_and(|binding| {
            binding.profile == mycopilot_core::AgentCommandRuntimeProfile::Pdf
        });
        if !is_managed_pdf {
            return Ok(None);
        }

        let key = (owner.conversation_id.clone(), owner.origin_run_id.clone());
        let mut workspaces = lock(&self.inner.managed_workspaces);
        if let Some(existing) = workspaces.get(&key) {
            return Ok(Some(existing.clone()));
        }
        let workspace = self
            .inner
            .storage
            .acquire_managed_command_workspace(&owner.conversation_id, &owner.origin_run_id)?;
        workspaces.insert(key, workspace.clone());
        Ok(Some(workspace))
    }

    /// Releases the Host's run-level retention. A still-running command or unpublished terminal
    /// result keeps its own clone, so physical cleanup cannot race process exit or publication.
    pub(super) fn release_managed_workspace(
        &self,
        conversation_id: &str,
        run_id: &str,
    ) -> Result<(), String> {
        self.inner
            .storage
            .cleanup_managed_command_workspace(conversation_id, run_id)?;
        lock(&self.inner.managed_workspaces)
            .remove(&(conversation_id.to_string(), run_id.to_string()));
        Ok(())
    }

    /// Closes a process whose durable `starting -> running` fence did not commit.
    ///
    /// A missing Session row is deliberately *not* sent to the terminal settlement scheduler:
    /// there is no future retry which could make such an UPDATE succeed. The ordinary command
    /// ToolResult/audit remains the durable failure record, so the caller keeps its File Effect
    /// guard. If the row was created before a later lifecycle transaction failed, the Host takes
    /// the guard and uses the normal archive/terminal cut to close that recoverable row.
    fn fail_durable_start(
        &self,
        session: &Arc<HostCommandSession>,
        failure: DurableStartFailure,
        terminal_hint: Option<CommandTerminalResult>,
        caller_file_effect_guard: &mut Option<FileEffectGuard>,
    ) -> Result<Box<CommandTerminalResult>, String> {
        {
            let mut state = lock(&session.state);
            state.handoff = HandoffState::Aborted;
            // Keep terminal settlement closed until the File Effect guard and the authoritative
            // Failed terminal have been installed under this same state fence below.
            state.terminal_settlement_ready = false;
            session.changed.notify_all();
        }

        let session_id = session
            .session_id()
            .ok_or_else(|| "命令 Session 启动持久化失败后丢失了进程身份。".to_string())?;
        let mut last_termination_error = None;
        let mut terminal = terminal_hint;
        if terminal.is_none() {
            for _ in 0..3 {
                if let Err(error) = self
                    .inner
                    .manager
                    .force_terminate(&session_id, SESSION_TERMINATE_WAIT)
                {
                    last_termination_error = Some(error.to_string());
                }
                match self
                    .inner
                    .manager
                    .wait_terminal_result(&session_id, SESSION_TERMINATE_WAIT)
                {
                    Ok(Some(result)) => {
                        terminal = Some(result);
                        break;
                    }
                    Ok(None) => {}
                    Err(error) => last_termination_error = Some(error.to_string()),
                }
            }
        }
        if terminal.is_none() {
            // The writer may have observed Terminal between the manager wait and this fence.
            // Inspect it while holding the same state lock used by `observe_terminal`; otherwise
            // returning the guard to the caller could race a still-live process.
            let mut state = lock(&session.state);
            terminal = state.terminal.clone();
            if terminal.is_none() {
                if state.file_effect_guard.is_some() {
                    return Err(
                        "命令 Session 启动失败清理重复接管了 File Effect lease。".to_string()
                    );
                }
                state.file_effect_guard = caller_file_effect_guard.take();
                // This flag is the ownership-installation fence, not a statement that the durable
                // row is already known. A later Terminal callback may schedule only after this
                // fence is open. Definite Absent is then promoted to Indeterminate and repaired
                // through the synthetic-row reconciliation lane.
                state.terminal_settlement_ready = true;
                session.changed.notify_all();
                drop(state);
                return Err(format!(
                    "命令 Session 的启动状态无法持久化，且 Host 未能在有界窗口内确认进程终止；Host 继续持有进程、File Effect 与 admission 所有权：{}{}",
                    failure.error,
                    last_termination_error
                        .as_deref()
                        .map(|error| format!("；终止错误：{error}"))
                        .unwrap_or_default()
                ));
            }
        }
        let terminal = terminal.expect("terminal was checked under the Host state fence");
        let mut terminal = project_durable_start_failure(
            terminal,
            &failure.error,
            session.output_persistence_truncated(),
        );

        if failure.durable_row == DurableRowState::Indeterminate {
            let guard = caller_file_effect_guard.take();
            {
                let mut state = lock(&session.state);
                if state.file_effect_guard.is_some() {
                    return Err("命令 Session 已经持有 File Effect lease。".to_string());
                }
                // Indeterminate may reconcile to Present and settle immediately. Install effect
                // ownership and the immutable Failed terminal in one fence *before* scheduling,
                // so no worker can publish/forget this Session ahead of its guard.
                state.file_effect_guard = guard;
                state.terminal = Some(terminal.clone());
                state.terminal_settlement_ready = true;
                session.changed.notify_all();
            }
            let settlement = self.schedule_and_wait_terminal_settlement(session);
            let settled = session.is_terminal_settled();
            let durable_row = session.durable_row_state();
            if let Err(error) = settlement {
                // The guard was installed before scheduling. If the worker completed exactly at
                // the wait boundary, observe its state; otherwise the Host safely retains all
                // ownership while the same bounded worker keeps reconciling.
                if settled {
                    if durable_row == DurableRowState::Present {
                        session.clear_archived_execution_spools(&mut terminal.execution)?;
                    }
                    return Ok(Box::new(terminal));
                }
                return Err(format!(
                    "命令 Session 启动写入结果无法权威判定；Host 继续持有 File Effect 与 admission 所有权并在后台核对：{}；调度状态：{error}",
                    failure.error
                ));
            }
            if durable_row == DurableRowState::Present {
                session.clear_archived_execution_spools(&mut terminal.execution)?;
            }
            return Ok(Box::new(terminal));
        }

        if failure.durable_row == DurableRowState::Present {
            let guard = caller_file_effect_guard.take();
            {
                let mut state = lock(&session.state);
                if state.file_effect_guard.is_some() {
                    return Err("命令 Session 已经持有 File Effect lease。".to_string());
                }
                state.file_effect_guard = guard;
                // The projected Failed terminal is the only authoritative Host terminal. Install
                // it before opening settlement so an earlier/later Core Interrupted callback can
                // neither be archived nor published in its place.
                state.terminal = Some(terminal.clone());
                state.terminal_settlement_ready = true;
                session.changed.notify_all();
            }
            if let Err(error) = self.schedule_and_wait_terminal_settlement(session) {
                // The row exists, so the bounded shared scheduler may safely keep retrying this
                // ordinary recoverable terminal cut while retaining admission and File Effect.
                session.record_persistence_error(error.clone());
                return Err(format!(
                    "命令因启动状态持久化失败而终止，但 Host 未能确认唯一的 Archive/Trace/Session 终态；完整输出由后台 Session 继续持有：{error}"
                ));
            }
            // The Session archive owns the exact body once its ref is authoritative. Do not let
            // the ordinary failed run_command ToolResult materialize the same spool a second time.
            session.clear_archived_execution_spools(&mut terminal.execution)?;
        } else {
            // No database row can ever satisfy a terminal UPDATE. The caller-owned File Effect
            // guard will be paired with the returned failed ToolResult by the normal action audit.
            {
                let mut state = lock(&session.state);
                state.terminal = Some(terminal.clone());
                state.terminal_settled = true;
                session.changed.notify_all();
            }
            self.inner.forget_terminal_session(session_id.as_str());
            session.release_admission();
        }

        Ok(Box::new(terminal))
    }

    /// Serializes cancellation, the durable running receipt, and process ownership transfer.
    ///
    /// Holding the Session state lock across `persist` gives cancellation one unambiguous order:
    /// cancellation which acquires the fence first changes `Pending -> Aborted`; a handoff which
    /// acquires it first commits the durable receipt and changes `Pending -> Adopted` before the
    /// cancellation path can inspect this Session.
    fn handoff_guard(&self, session_id: String) -> AgentCommandSessionHandoffGuard {
        AgentCommandSessionHandoffGuard {
            registry: self.clone(),
            session_id,
            armed: true,
        }
    }

    fn commit_handoff(
        &self,
        handoff_guard: &mut AgentCommandSessionHandoffGuard,
        cancelled: impl FnOnce() -> bool,
        persist: impl FnOnce() -> Result<(), String>,
    ) -> Result<AgentCommandHandoffOutcome, String> {
        handoff_guard.validate_registry(self)?;
        let session_id = handoff_guard.session_id.as_str();
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
            handoff_guard.armed = false;
            session.changed.notify_all();
        }
        if session.terminal().is_some() {
            if let Err(error) = self
                .inner
                .schedule_terminal_settlement(Arc::clone(&session))
            {
                // Ownership has already crossed the durable handoff fence. A scheduler outage may
                // delay terminal materialization, but must never make the caller attempt a
                // pre-handoff abort against an adopted process.
                session.record_persistence_error(error);
            }
        }
        Ok(AgentCommandHandoffOutcome::Adopted)
    }

    /// Cancels only Sessions which have not crossed the durable handoff fence. Adopted process
    /// Sessions outlive ordinary Run teardown; the explicit user-stop path separately interrupts
    /// them through [`Self::interrupt_origin_run`].
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
                if session.terminal_settlement_ready() && session.terminal().is_some() {
                    let _ = self
                        .inner
                        .schedule_terminal_settlement(Arc::clone(&session));
                }
            }
        }
        cancelled
    }

    /// Non-blocking fallback used by the handoff token's `Drop` and by the Host deadline. The
    /// process watcher performs bounded output drain and the shared scheduler owns durable
    /// Archive/Trace/Session settlement once the terminal callback arrives.
    fn abandon_pending_handoff(&self, session_id: &str) {
        let Ok(session) = self.live_session(session_id) else {
            return;
        };
        let session_id = {
            let mut state = lock(&session.state);
            match state.handoff {
                HandoffState::Pending => state.handoff = HandoffState::Aborted,
                HandoffState::Aborted | HandoffState::Synchronous => {}
                HandoffState::Adopted => return,
            }
            session.changed.notify_all();
            state.session_id.clone()
        };
        if let Some(session_id) = session_id {
            let _ = self
                .inner
                .manager
                .force_terminate(&session_id, Duration::ZERO);
        }
        if session.terminal_settlement_ready() && session.terminal().is_some() {
            let _ = self.inner.schedule_terminal_settlement(session);
        }
    }

    /// Cancels a process which has not crossed the persistent handoff boundary. The originating
    /// Agent action remains responsible for its ordinary terminal audit and file-effect lease.
    fn abort_before_handoff(
        &self,
        handoff_guard: &mut AgentCommandSessionHandoffGuard,
    ) -> Result<CommandTerminalResult, String> {
        handoff_guard.validate_registry(self)?;
        let session_id = handoff_guard.session_id.clone();
        let session = self.live_session(&session_id)?;
        {
            let mut state = lock(&session.state);
            if state.handoff == HandoffState::Adopted {
                return Err("命令 Session 已完成持久交接，不能按交接前语义终止。".to_string());
            }
            state.handoff = HandoffState::Aborted;
            state.terminal_settlement_ready = true;
            handoff_guard.armed = false;
            session.changed.notify_all();
        }
        let id = CommandSessionId::parse(&session_id).map_err(|error| error.to_string())?;
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
        if let Err(error) = self.schedule_and_wait_terminal_settlement(&session) {
            session.record_persistence_error(error.clone());
            return Err(format!(
                "命令进程已经终止，但 Host 未能确认唯一的 Archive/Trace/Session 终态；完整输出由后台 Session 继续持有：{error}"
            ));
        }
        Ok(terminal)
    }

    fn finish_synchronous(
        &self,
        handoff_guard: &mut AgentCommandSessionHandoffGuard,
    ) -> Result<(), String> {
        handoff_guard.validate_registry(self)?;
        let session_id = handoff_guard.session_id.clone();
        let session = self.live_session(&session_id)?;
        session
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
            state.terminal_settlement_ready = true;
            handoff_guard.armed = false;
            session.changed.notify_all();
        }
        self.schedule_and_wait_terminal_settlement(&session)
    }

    fn schedule_and_wait_terminal_settlement(
        &self,
        session: &Arc<HostCommandSession>,
    ) -> Result<(), String> {
        self.inner
            .schedule_terminal_settlement(Arc::clone(session))?;
        if session.wait_for_terminal_settled(SESSION_TERMINATE_WAIT) {
            Ok(())
        } else {
            Err("命令 Session 终态未在有界窗口内完成持久结算；后台调度器将继续重试。".to_string())
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

    /// Requests controlled interruption of process Sessions owned by one explicit Agent Run
    /// cancellation. The command manager escalates to force termination after its bounded grace.
    ///
    /// Both conversation and Run identity are required even though Run IDs are generated by the
    /// Host. This keeps a stale or malformed cancellation from crossing a conversation boundary,
    /// while still stopping every command call launched by the cancelled turn. Ordinary runtime
    /// cancellation tokens do not call this method: model-observation cancellation and provider
    /// failure must remain unable to signal an already handed-off process.
    pub(super) fn interrupt_origin_run(&self, conversation_id: &str, run_id: &str) -> usize {
        let sessions = lock(&self.inner.sessions)
            .values()
            .filter(|session| {
                session.owner.conversation_id == conversation_id
                    && session.owner.origin_run_id == run_id
            })
            .cloned()
            .collect::<Vec<_>>();
        let mut ids = Vec::new();
        for session in sessions {
            let session_id = {
                // This is the same fence used by `commit_handoff`. If user stop wins, Pending is
                // made Aborted and a running receipt can no longer commit. If commit wins, the
                // state is Adopted and the process is still selected for termination below.
                let mut state = lock(&session.state);
                if state.terminal.is_some() {
                    continue;
                }
                if state.handoff == HandoffState::Pending {
                    state.handoff = HandoffState::Aborted;
                    session.changed.notify_all();
                }
                state.session_id.clone()
            };
            if let Some(session_id) = session_id {
                ids.push(session_id);
            }
        }
        let interrupted = ids.len();
        for id in ids {
            let _ = self.inner.manager.interrupt(&id, Duration::ZERO);
        }
        interrupted
    }

    /// Resolves the conversation side of a live, Host-owned origin Run binding.
    ///
    /// This fallback covers the narrow interval after ActiveRunControl is retired but before the
    /// Renderer observes the durable terminal event. Ambiguous identity fails closed instead of
    /// permitting a Run-only cancellation to cross conversation scope.
    pub(super) fn conversation_for_origin_run(&self, run_id: &str) -> Option<String> {
        let sessions = lock(&self.inner.sessions);
        let mut conversation_id = None::<String>;
        for session in sessions
            .values()
            .filter(|session| session.owner.origin_run_id == run_id)
        {
            match conversation_id.as_deref() {
                None => conversation_id = Some(session.owner.conversation_id.clone()),
                Some(current) if current == session.owner.conversation_id => {}
                Some(_) => return None,
            }
        }
        conversation_id
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
                    ) && !state.terminal_settled
                })
                .unwrap_or_else(|error| error.into_inner());
            settled &= !matches!(
                state.handoff,
                HandoffState::Adopted | HandoffState::Synchronous | HandoffState::Aborted
            ) || state.terminal_settled;
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

    #[cfg(test)]
    pub(super) fn set_before_file_effect_install_hook(&self, hook: BeforeFileEffectInstallHook) {
        *lock(&self.inner.before_file_effect_install_hook) = Some(hook);
    }

    #[cfg(test)]
    pub(super) fn set_after_durable_create_hook(&self, hook: AfterDurableCreateHook) {
        *lock(&self.inner.after_durable_create_hook) = Some(hook);
    }

    #[cfg(test)]
    pub(super) fn set_durable_start_inspection_hook(&self, hook: DurableStartInspectionHook) {
        *lock(&self.inner.durable_start_inspection_hook) = Some(hook);
    }

    #[cfg(test)]
    pub(super) fn wait_for_live_terminal(&self, session_id: &str, wait: Duration) -> bool {
        self.live_session(session_id)
            .ok()
            .and_then(|session| session.wait_for_terminal(wait))
            .is_some()
    }

    #[cfg(test)]
    pub(super) fn retained_live_session_count(&self) -> usize {
        lock(&self.inner.sessions).len()
    }

    #[cfg(test)]
    pub(super) fn retained_core_session_count(&self) -> usize {
        self.inner.manager.list(None).len()
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
        control: AgentCommandSessionExecutionControl,
    ) -> AgentResult<AgentCommandSessionExecutionOutput> {
        if control.is_cancelled() {
            return Err(AgentError::cancelled());
        }
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
        if control.is_cancelled() {
            return Err(AgentError::cancelled());
        }
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
            let history_open = terminal_model_history_open(
                &self.inner.storage,
                &request.conversation_id,
                &request.session_id,
                read.receipt.status,
            )?;
            return Ok(model_execution_output_from_read(&read, history_open));
        }
        let record = self
            .inner
            .storage
            .load_agent_command_session(&request.conversation_id, &request.session_id)
            .map_err(AgentError::new)?
            .ok_or_else(|| AgentError::new("命令 Session 在交互期间消失。"))?;

        let mut observed_sequence = record.snapshot.latest_sequence;
        let wait = Duration::from_millis(request.wait_ms.min(SESSION_CONTROL_WAIT_MAX_MS));
        let deadline = Instant::now() + wait;
        if !record.snapshot.status.is_terminal() {
            let Some(live) = live.as_ref() else {
                return Err(AgentError::structured(
                    "agent.command_session_not_controllable",
                    "命令 Session 已不再由当前 Host 控制。",
                    json!({"type":"command_session","code":"notControllable"}),
                ));
            };
            let process_observation = if let Some(terminal) = live.terminal() {
                Ok((terminal.snapshot.latest_output_sequence, true))
            } else {
                match request.action {
                    AgentCommandSessionAction::Interrupt => self
                        .inner
                        .manager
                        .interrupt(
                            &id,
                            deadline
                                .saturating_duration_since(Instant::now())
                                .min(SESSION_OBSERVATION_SLICE),
                        )
                        .map(|snapshot| {
                            (
                                snapshot.latest_output_sequence,
                                snapshot.state.is_terminal(),
                            )
                        }),
                    AgentCommandSessionAction::Poll => Ok((observed_sequence, false)),
                }
            };
            match process_observation {
                Ok((sequence, terminal_observed)) => {
                    observed_sequence = sequence;
                    // The Core owner publishes its terminal snapshot before the asynchronous
                    // Host lifecycle writer commits Archive/Trace/Session state. Preserve the
                    // caller's remaining wait budget across that handoff instead of racing ahead
                    // and manufacturing a stale `running` receipt after interrupt or terminal
                    // poll already observed process exit.
                    while terminal_observed
                        && live.terminal().is_none()
                        && Instant::now() < deadline
                        && !control.has_pending_guidance()
                    {
                        if control.is_cancelled() {
                            return Err(AgentError::cancelled());
                        }
                        live.wait_for_terminal(
                            deadline
                                .saturating_duration_since(Instant::now())
                                .min(SESSION_OBSERVATION_SLICE),
                        );
                    }
                }
                Err(error) => {
                    // Terminal settlement may remove the Core Session between the durable reload
                    // and the manager call. Accept that race only when either the Host terminal is
                    // already visible or the durable row has reached a terminal state.
                    if let Some(terminal) = live.wait_for_terminal(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .min(Duration::from_millis(50)),
                    ) {
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

            // Quiet long-polling is a Host concern. Ordinary output continues through lifecycle
            // persistence and Timeline notifications, but does not complete this model Tool.
            // Small terminal-or-deadline slices make Run cancellation and queued guidance visible
            // promptly without giving either event authority to signal the handed-off process.
            while live.terminal().is_none()
                && Instant::now() < deadline
                && !control.has_pending_guidance()
            {
                if control.is_cancelled() {
                    return Err(AgentError::cancelled());
                }
                let slice = deadline
                    .saturating_duration_since(Instant::now())
                    .min(SESSION_OBSERVATION_SLICE);
                if slice.is_zero() {
                    break;
                }
                match self.inner.manager.read_output_until_terminal_or_deadline(
                    &id,
                    record.model_read_sequence,
                    slice,
                ) {
                    Ok(poll) => {
                        observed_sequence =
                            observed_sequence.max(poll.snapshot.latest_output_sequence);
                        if poll.snapshot.state.is_terminal() {
                            while live.terminal().is_none()
                                && Instant::now() < deadline
                                && !control.has_pending_guidance()
                            {
                                if control.is_cancelled() {
                                    return Err(AgentError::cancelled());
                                }
                                live.wait_for_terminal(
                                    deadline
                                        .saturating_duration_since(Instant::now())
                                        .min(SESSION_OBSERVATION_SLICE),
                                );
                            }
                            break;
                        }
                    }
                    Err(error) => {
                        if let Some(terminal) = live.terminal() {
                            observed_sequence =
                                observed_sequence.max(terminal.snapshot.latest_output_sequence);
                            break;
                        }
                        let refreshed = self
                            .inner
                            .storage
                            .load_agent_command_session(
                                &request.conversation_id,
                                &request.session_id,
                            )
                            .map_err(AgentError::new)?
                            .ok_or_else(|| AgentError::new("命令 Session 在观察期间消失。"))?;
                        if refreshed.snapshot.status.is_terminal() {
                            observed_sequence =
                                observed_sequence.max(refreshed.snapshot.latest_sequence);
                            break;
                        }
                        return Err(AgentError::new(error.to_string()));
                    }
                }
            }
            if control.is_cancelled() {
                return Err(AgentError::cancelled());
            }

            while !live.wait_for_persisted_sequence(
                observed_sequence,
                deadline
                    .saturating_duration_since(Instant::now())
                    .min(SESSION_OBSERVATION_SLICE),
            ) && Instant::now() < deadline
                && !control.has_pending_guidance()
            {
                if control.is_cancelled() {
                    return Err(AgentError::cancelled());
                }
            }
            if live.terminal().is_some() {
                // Do not project an in-memory terminal state ahead of Exact Archive + terminal
                // lifecycle + Session CAS. A zero-wait poll may still observe `running` briefly and
                // can poll again; a normal bounded poll waits for the authoritative durable cut.
                while !live.wait_for_terminal_settled(
                    deadline
                        .saturating_duration_since(Instant::now())
                        .min(SESSION_OBSERVATION_SLICE),
                ) && Instant::now() < deadline
                    && !control.has_pending_guidance()
                {
                    if control.is_cancelled() {
                        return Err(AgentError::cancelled());
                    }
                }
            }
        }

        if control.is_cancelled() {
            return Err(AgentError::cancelled());
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
        let history_open = terminal_model_history_open(
            &self.inner.storage,
            &request.conversation_id,
            &request.session_id,
            read.receipt.status,
        )?;
        Ok(model_execution_output_from_read(&read, history_open))
    }
}

fn terminal_model_history_open(
    storage: &StorageService,
    conversation_id: &str,
    session_id: &str,
    status: AgentCommandSessionStatus,
) -> AgentResult<Option<String>> {
    if !status.is_terminal() {
        return Ok(None);
    }
    let record = storage
        .load_agent_command_session(conversation_id, session_id)
        .map_err(AgentError::new)?
        .ok_or_else(|| AgentError::new("命令 Session 在生成历史恢复位置期间消失。"))?;
    let Some(archive_ref) = record.snapshot.archive_ref.as_deref() else {
        // Legacy or outcome-unknown rows may not own an Exact Archive. Preserve compatibility
        // without inventing a route or exposing an internal identifier.
        return Ok(None);
    };
    storage
        .conversation_history_archive_open(conversation_id, archive_ref)
        .map_err(AgentError::new)?
        .ok_or_else(|| {
            AgentError::structured(
                "agent.command_session_history_unavailable",
                "命令 Session 的完整历史归档不可用。",
                json!({
                    "type": "command_session",
                    "code": "historyUnavailable"
                }),
            )
        })
        .map(Some)
}

impl AgentCommandSessionRegistryInner {
    #[cfg(test)]
    fn run_before_file_effect_install_hook(&self, session_id: &str) {
        let hook = lock(&self.before_file_effect_install_hook).take();
        if let Some(hook) = hook {
            hook(session_id);
        }
    }

    #[cfg(test)]
    fn run_after_durable_create_hook(&self, session_id: &str) -> Result<(), String> {
        let hook = lock(&self.after_durable_create_hook).clone();
        if let Some(hook) = hook {
            hook(session_id)?;
        }
        Ok(())
    }

    fn create_agent_command_session_for_start(
        &self,
        create: &AgentCommandSessionCreate,
    ) -> Result<(), String> {
        self.storage.create_agent_command_session(create)?;
        #[cfg(test)]
        self.run_after_durable_create_hook(&create.snapshot.session_id)?;
        Ok(())
    }

    fn inspect_agent_command_session_start(
        &self,
        conversation_id: &str,
        session_id: &str,
    ) -> Result<Option<AgentCommandSessionRecord>, String> {
        #[cfg(test)]
        if let Some(hook) = lock(&self.durable_start_inspection_hook).clone() {
            hook(session_id)?;
        }
        self.storage
            .load_agent_command_session(conversation_id, session_id)
    }

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
        pending_handoff_deadline: Instant,
    ) {
        loop {
            let event = if session.handoff_state() == HandoffState::Pending {
                // An absolute ownership deadline must win even when a noisy child keeps the
                // bounded lifecycle queue continuously readable. `recv_timeout(Duration::ZERO)`
                // is still allowed to dequeue an available item, so relying on it alone can
                // starve handoff reclamation indefinitely under sustained output.
                if Instant::now() >= pending_handoff_deadline {
                    if let Some(session_id) = session.session_id_string() {
                        AgentCommandSessionRegistry {
                            inner: Arc::clone(self),
                        }
                        .abandon_pending_handoff(&session_id);
                    }
                    continue;
                }
                match receiver.recv_timeout(
                    pending_handoff_deadline.saturating_duration_since(Instant::now()),
                ) {
                    Ok(event) => event,
                    Err(RecvTimeoutError::Timeout) => {
                        if let Some(session_id) = session.session_id_string() {
                            AgentCommandSessionRegistry {
                                inner: Arc::clone(self),
                            }
                            .abandon_pending_handoff(&session_id);
                        }
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match receiver.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                }
            };
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
                    // A process may exit in the narrow interval after the initial yield produced a
                    // Running receipt but before the caller commits ownership. The terminal event
                    // is the final lifecycle item, so the writer can now wait directly on the
                    // handoff fence. Without this wait, consuming Terminal would remove the only
                    // watchdog before a leaked caller token reached its absolute deadline.
                    let state = lock(&session.state);
                    let (state, _) = session
                        .changed
                        .wait_timeout_while(
                            state,
                            pending_handoff_deadline.saturating_duration_since(Instant::now()),
                            |state| state.handoff == HandoffState::Pending,
                        )
                        .unwrap_or_else(|error| error.into_inner());
                    let handoff_is_still_pending = state.handoff == HandoffState::Pending;
                    let session_id = state.session_id.clone();
                    drop(state);
                    if handoff_is_still_pending {
                        if let Some(session_id) = session_id {
                            AgentCommandSessionRegistry {
                                inner: Arc::clone(self),
                            }
                            .abandon_pending_handoff(session_id.as_str());
                        }
                    }
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
        let mut durable_row = DurableRowState::Absent;
        let result = (|| {
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
            if let Err(first_error) = self.create_agent_command_session_for_start(&create) {
                // A SQLite COMMIT error is outcome-unknown: the row may already be durable. Retry
                // the immutable create (idempotent by identity), then inspect the authoritative
                // row before deciding whether terminal settlement is reachable.
                if let Err(retry_error) = self.create_agent_command_session_for_start(&create) {
                    match self.inspect_agent_command_session_start(
                        &session.owner.conversation_id,
                        snapshot.session_id.as_str(),
                    ) {
                        Ok(Some(record)) if recovered_create_matches(&record, &create) => {
                            durable_row = DurableRowState::Present;
                        }
                        Ok(Some(_)) => {
                            durable_row = DurableRowState::Indeterminate;
                            return Err(format!(
                                "命令 Session create 返回错误，权威回读发现同 ID 的不一致记录；首次错误：{first_error}；重试错误：{retry_error}"
                            ));
                        }
                        Ok(None) => {
                            durable_row = DurableRowState::Absent;
                            return Err(format!(
                                "命令 Session create 未落盘；首次错误：{first_error}；重试错误：{retry_error}"
                            ));
                        }
                        Err(inspect_error) => {
                            durable_row = DurableRowState::Indeterminate;
                            return Err(format!(
                                "命令 Session create 结果无法权威核对；首次错误：{first_error}；重试错误：{retry_error}；回读错误：{inspect_error}"
                            ));
                        }
                    }
                } else {
                    durable_row = DurableRowState::Present;
                }
            } else {
                durable_row = DurableRowState::Present;
            }
            match self
                .storage
                .mark_agent_command_session_running_with_lifecycle(
                    &session.owner.conversation_id,
                    snapshot.session_id.as_str(),
                    now_ms(),
                )? {
                AgentCommandSessionTransitionOutcome::Updated
                | AgentCommandSessionTransitionOutcome::Idempotent => {}
                outcome => {
                    return Err(format!("命令 Session 无法进入 running 状态：{outcome:?}"));
                }
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                session.mark_durable_start_ready();
                if let Some(notifications) = session.notifications.as_ref() {
                    let _ =
                        notifications.send(agent_event_notification(AgentEvent::CommandStarted {
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
            Err(error) => {
                session.mark_durable_start_failed(error.clone(), durable_row);
                Err(error)
            }
        }
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
        mut terminal: CommandTerminalResult,
    ) {
        if let Err(error) = mycopilot_core::command::publish_managed_command_outputs(
            &self.storage,
            &mut terminal.execution,
            &session.owner.conversation_id,
            &session.owner.origin_run_id,
            &session.owner.call_id,
        ) {
            let message = format!("受管命令输出发布失败：{error}");
            terminal.execution.error = Some(message.clone());
            terminal.snapshot.state = CommandSessionState::Failed;
            terminal.snapshot.exit_code = None;
            terminal.snapshot.error = Some(message);
        }
        let (handoff, settlement_ready, durable_row) = {
            let mut state = lock(&session.state);
            let durable_failure = match &state.durable_start {
                DurableStartState::Failed { error, durable_row } => {
                    Some((error.clone(), *durable_row))
                }
                DurableStartState::Pending | DurableStartState::Ready => None,
            };
            if let Some((error, _)) = durable_failure.as_ref() {
                // Once a durable-start failure has been projected, it is immutable. A late Core
                // Interrupted callback is process evidence only and must not roll the Host,
                // Archive, Session row, Trace, event, or ToolResult back to another terminal.
                let output_persistence_truncated = state.output_persistence_truncated;
                let authoritative = state
                    .terminal
                    .as_ref()
                    .filter(|existing| {
                        matches!(existing.snapshot.state, CommandSessionState::Failed)
                    })
                    .cloned()
                    .unwrap_or_else(|| {
                        project_durable_start_failure(terminal, error, output_persistence_truncated)
                    });
                state.terminal = Some(authoritative);
            } else {
                state.terminal = Some(terminal);
            }
            let mut durable_row = durable_failure
                .as_ref()
                .map(|(_, durable_row)| *durable_row)
                .unwrap_or(DurableRowState::Present);
            if durable_failure.is_some()
                && durable_row == DurableRowState::Absent
                && state.terminal_settlement_ready
            {
                // A previously unconfirmed process forced the Host to retain the guard. Once its
                // terminal arrives, re-enter the authoritative reconciliation lane; an Absent row
                // is synthesized and terminally settled instead of becoming a permanent fence.
                let error = match &state.durable_start {
                    DurableStartState::Failed { error, .. } => error.clone(),
                    DurableStartState::Pending | DurableStartState::Ready => unreachable!(),
                };
                durable_row = DurableRowState::Indeterminate;
                state.durable_start = DurableStartState::Failed { error, durable_row };
            }
            session.changed.notify_all();
            (state.handoff, state.terminal_settlement_ready, durable_row)
        };
        if settlement_ready
            && matches!(
                durable_row,
                DurableRowState::Indeterminate | DurableRowState::Present
            )
            && matches!(
                handoff,
                HandoffState::Adopted | HandoffState::Synchronous | HandoffState::Aborted
            )
        {
            if let Err(error) = self.schedule_terminal_settlement(Arc::clone(session)) {
                session.record_persistence_error(error);
            }
        }
    }

    fn schedule_terminal_settlement(&self, session: Arc<HostCommandSession>) -> Result<(), String> {
        if session.durable_row_state() == DurableRowState::Absent {
            return Err("命令 Session 没有可结算的持久行，禁止进入终态重试队列。".to_string());
        }
        self.settlement_scheduler.schedule(session)
    }

    fn reconcile_indeterminate_durable_start(
        &self,
        session: &HostCommandSession,
    ) -> Result<(), String> {
        let session_id = session
            .session_id_string()
            .ok_or_else(|| "命令 Session 启动核对缺少进程身份。".to_string())?;
        let terminal = session
            .terminal()
            .ok_or_else(|| "命令 Session 启动核对缺少进程终态。".to_string())?;
        let expected_create = durable_start_create_for_session(session, &terminal)?;
        match self
            .inspect_agent_command_session_start(&session.owner.conversation_id, &session_id)?
        {
            Some(record) if recovered_create_matches(&record, &expected_create) => {
                let terminal = {
                    let mut state = lock(&session.state);
                    let error = match &state.durable_start {
                        DurableStartState::Failed { error, .. } => error.clone(),
                        DurableStartState::Pending | DurableStartState::Ready => {
                            return Err(
                                "命令 Session 启动核对发现了不一致的 Host 状态。".to_string()
                            );
                        }
                    };
                    state.durable_start = DurableStartState::Failed {
                        error,
                        durable_row: DurableRowState::Present,
                    };
                    state.terminal_settlement_ready = true;
                    session.changed.notify_all();
                    state
                        .terminal
                        .clone()
                        .ok_or_else(|| "命令 Session 启动核对缺少进程终态。".to_string())?
                };
                self.settle_terminal(session, &terminal)
            }
            Some(_) => {
                Err("命令 Session 启动核对发现同 ID 的不一致持久记录；继续保留所有权。".to_string())
            }
            None => {
                // Storage is readable again and authoritatively says the outcome-unknown create
                // did not land. Reconstruct the immutable Starting row from Host-owned identity,
                // then use the same Failed Archive/Trace/Session settlement as every Present row.
                // If this create fails, the single scheduler retries while retaining all guards.
                self.storage
                    .create_agent_command_session(&expected_create)?;
                {
                    let mut state = lock(&session.state);
                    let error = match &state.durable_start {
                        DurableStartState::Failed { error, .. } => error.clone(),
                        DurableStartState::Pending | DurableStartState::Ready => {
                            return Err(
                                "命令 Session 启动核对发现了不一致的 Host 状态。".to_string()
                            );
                        }
                    };
                    state.durable_start = DurableStartState::Failed {
                        error,
                        durable_row: DurableRowState::Present,
                    };
                    state.terminal_settlement_ready = true;
                    session.changed.notify_all();
                }
                self.settle_terminal(session, &terminal)
            }
        }
    }

    /// Commits the single authoritative terminal cut for short, aborted, and handed-off Sessions.
    /// Exact History is materialized first; Trace and Session state share one SQLite transaction;
    /// only then may UI notification and File Effect release become visible.
    fn settle_terminal(
        &self,
        session: &HostCommandSession,
        terminal: &CommandTerminalResult,
    ) -> Result<(), String> {
        if !session.terminal_settlement_ready() {
            return Err("命令 Session 的 File Effect 所有权尚未安装，禁止提交终态。".to_string());
        }
        if session.durable_row_state() != DurableRowState::Present {
            return Err("命令 Session 启动行不存在，禁止提交不可达的终态更新。".to_string());
        }
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
            published_outputs: &terminal.execution.outputs,
            committed_at: now_ms(),
        };
        match self
            .storage
            .settle_agent_command_session_with_lifecycle(&update)?
        {
            AgentCommandSessionTransitionOutcome::Updated
            | AgentCommandSessionTransitionOutcome::Idempotent => {}
            outcome => return Err(format!("命令 Session 终态无法持久化：{outcome:?}")),
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
                outputs: terminal.execution.outputs.clone(),
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
                outputs: terminal.execution.outputs.clone(),
            }
        };
        let _ = notifications.send(agent_event_notification(event));
    }
}

impl HostCommandSession {
    fn mark_durable_start_ready(&self) {
        let mut state = lock(&self.state);
        state.durable_start = DurableStartState::Ready;
        self.changed.notify_all();
    }

    fn mark_durable_start_failed(&self, error: String, durable_row: DurableRowState) {
        let mut state = lock(&self.state);
        state.durable_start = DurableStartState::Failed { error, durable_row };
        self.changed.notify_all();
    }

    fn durable_start_is_ready(&self) -> bool {
        matches!(&lock(&self.state).durable_start, DurableStartState::Ready)
    }

    fn durable_row_state(&self) -> DurableRowState {
        match &lock(&self.state).durable_start {
            DurableStartState::Ready => DurableRowState::Present,
            DurableStartState::Failed { durable_row, .. } => *durable_row,
            DurableStartState::Pending => DurableRowState::Indeterminate,
        }
    }

    fn wait_for_durable_start(&self, wait: Duration) -> Result<(), DurableStartFailure> {
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| {
                matches!(state.durable_start, DurableStartState::Pending)
            })
            .unwrap_or_else(|error| error.into_inner());
        match &state.durable_start {
            DurableStartState::Ready => Ok(()),
            DurableStartState::Failed { error, durable_row } => Err(DurableStartFailure {
                error: error.clone(),
                durable_row: *durable_row,
            }),
            DurableStartState::Pending => Err(DurableStartFailure {
                error: "命令 Session 启动状态未在有界窗口内完成持久化。".to_string(),
                durable_row: DurableRowState::Indeterminate,
            }),
        }
    }

    fn install_file_effect_guard(&self, guard: Option<FileEffectGuard>) -> Result<(), String> {
        let Some(guard) = guard else {
            return Ok(());
        };
        let mut state = lock(&self.state);
        if state.file_effect_guard.is_some() {
            return Err("命令 Session 已经持有 File Effect lease。".to_string());
        }
        state.file_effect_guard = Some(guard);
        self.changed.notify_all();
        Ok(())
    }

    fn mark_terminal_settlement_ready(&self) -> HandoffState {
        let mut state = lock(&self.state);
        state.terminal_settlement_ready = true;
        self.changed.notify_all();
        state.handoff
    }

    fn terminal_settlement_ready(&self) -> bool {
        lock(&self.state).terminal_settlement_ready
    }

    fn clear_archived_execution_spools(
        &self,
        execution: &mut AgentCommandExecutionResult,
    ) -> Result<(), String> {
        if let Some(archive_ref) = self.terminal_archive_ref() {
            // The immutable full-body archive is now authoritative. Keeping only bounded previews
            // in the ordinary ToolResult prevents the same multi-MB stdout/stderr body from being
            // archived a second time by the normal trace pipeline.
            mycopilot_core::command::bind_authoritative_command_archive(execution, archive_ref)?;
            execution.stdout_spool = Default::default();
            execution.stderr_spool = Default::default();
        }
        Ok(())
    }

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

    fn wait_for_persisted_sequence(&self, sequence: u64, wait: Duration) -> bool {
        if sequence == 0 || wait.is_zero() {
            let state = lock(&self.state);
            return sequence == 0
                || state.persisted_sequence >= sequence
                || state.output_persistence_truncated
                || state.persistence_error.is_some();
        }
        let state = lock(&self.state);
        let (state, _) = self
            .changed
            .wait_timeout_while(state, wait, |state| {
                state.persisted_sequence < sequence
                    && !state.output_persistence_truncated
                    && state.persistence_error.is_none()
            })
            .unwrap_or_else(|error| error.into_inner());
        state.persisted_sequence >= sequence
            || state.output_persistence_truncated
            || state.persistence_error.is_some()
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

fn project_durable_start_failure(
    mut terminal: CommandTerminalResult,
    persistence_error: &str,
    output_persistence_truncated: bool,
) -> CommandTerminalResult {
    let projected_error = format!(
        "Command Session was terminated because its durable start state could not be persisted: {persistence_error}"
    );
    terminal.snapshot.state = CommandSessionState::Failed;
    terminal.snapshot.error = Some(projected_error.clone());
    terminal.snapshot.output_truncated |= output_persistence_truncated;
    terminal.execution.error = Some(projected_error);
    // This is a Host durability failure, not a user cancellation or process timeout. Keeping
    // those flags clear makes the ordinary action audit and ToolResult project `failed`.
    terminal.execution.cancelled = false;
    terminal.execution.timed_out = false;
    terminal
}

fn recovered_create_matches(
    record: &AgentCommandSessionRecord,
    create: &AgentCommandSessionCreate,
) -> bool {
    record.snapshot == create.snapshot
        && record.authorization_source == create.authorization_source
        && record.approval_provenance == create.approval_provenance
        && record.permission_provenance == create.permission_provenance
        && record.created_at == create.created_at
}

fn durable_start_create_for_session(
    session: &HostCommandSession,
    terminal: &CommandTerminalResult,
) -> Result<AgentCommandSessionCreate, String> {
    let mut snapshot = host_snapshot(&session.owner, &terminal.snapshot);
    snapshot.status = AgentCommandSessionStatus::Starting;
    snapshot.ended_at = None;
    snapshot.exit_code = None;
    snapshot.latest_sequence = 0;
    snapshot.output_truncated = false;
    snapshot.archive_ref = None;
    Ok(AgentCommandSessionCreate {
        snapshot,
        authorization_source: session.authorization_source,
        approval_provenance: session.approval_provenance.clone(),
        permission_provenance: session.permission_provenance.clone(),
        created_at: i64::try_from(terminal.snapshot.started_at)
            .map_err(|_| "命令 Session 启动时间超出持久化范围。".to_string())?,
    })
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
