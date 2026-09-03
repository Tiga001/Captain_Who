use super::*;

#[cfg(test)]
const FILE_EFFECT_DRAIN_TIMEOUT: Duration = Duration::from_millis(300);
#[cfg(not(test))]
const FILE_EFFECT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

#[cfg(test)]
type BeforeWaitingPersistenceHook = Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
static BEFORE_WAITING_PERSISTENCE_HOOKS: Mutex<Vec<(String, BeforeWaitingPersistenceHook)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn install_before_waiting_persistence_hook(
    run_id: &str,
    hook: BeforeWaitingPersistenceHook,
) {
    BEFORE_WAITING_PERSISTENCE_HOOKS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((run_id.to_string(), hook));
}

#[cfg(test)]
pub(super) fn run_before_waiting_persistence_hook(run_id: &str) {
    let hook = {
        let mut hooks = BEFORE_WAITING_PERSISTENCE_HOOKS
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        hooks
            .iter()
            .position(|(candidate, _)| candidate == run_id)
            .map(|index| hooks.swap_remove(index).1)
    };
    if let Some(hook) = hook {
        hook();
    }
}

#[derive(Debug, Default)]
pub(super) struct FileEffectTracker {
    state: Mutex<FileEffectState>,
    idle: Condvar,
}

#[derive(Debug, Default)]
pub(super) struct DeletionLifecycleState {
    projects: HashSet<String>,
    conversations: HashSet<String>,
}

/// Temporarily fences every file-producing continuation for one conversation and, when the
/// conversation belongs to a project, for that project as well. Conversation-scoped destructive
/// mutations use the same marker as deletion so a project deletion cannot race a message or
/// conversation mutation in the opposite lock order.
#[derive(Debug)]
struct ConversationMutationGuard {
    lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    conversation_id: String,
    project_id: Option<String>,
    retain_conversation_tombstone: bool,
}

#[derive(Debug, Default)]
struct AgentTreeWakeCancellationProgress {
    affected: bool,
    lost_ownership: bool,
    errors: Vec<String>,
}

/// Process-local result of cancelling one exact Agent Run.
///
/// A child interrupt is confirmed only when either the live Runtime owner received its token or
/// the Turn was atomically terminalized without a live owner. Session/process cleanup remains
/// observable by the root user-stop response, but cannot by itself prove the Turn has stopped.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentRunCancellationOutcome {
    #[default]
    NoEffect,
    ResourcesOnly,
    TurnTerminationConfirmed,
}

impl AgentRunCancellationOutcome {
    pub(crate) fn turn_termination_confirmed(self) -> bool {
        self == Self::TurnTerminationConfirmed
    }

    pub(crate) fn any_effect(self) -> bool {
        self != Self::NoEffect
    }

    pub(super) fn record_turn_termination(&mut self, confirmed: bool) {
        if confirmed {
            *self = Self::TurnTerminationConfirmed;
        }
    }

    pub(super) fn record_resource_cleanup(&mut self, affected: bool) {
        if affected && *self == Self::NoEffect {
            *self = Self::ResourcesOnly;
        }
    }
}

impl DeletionLifecycleState {
    pub(super) fn contains_input(&self, input: &AgentChatInput) -> bool {
        agent_input_project_id(input).is_some_and(|id| self.projects.contains(id))
            || agent_input_conversation_id(input).is_some_and(|id| self.conversations.contains(id))
    }
}

impl ConversationMutationGuard {
    fn acquire(
        lifecycle: Arc<Mutex<DeletionLifecycleState>>,
        conversation_id: &str,
        project_id: Option<&str>,
    ) -> Result<Self, String> {
        let mut state = lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        if state.conversations.contains(conversation_id)
            || project_id.is_some_and(|project_id| state.projects.contains(project_id))
        {
            return Err(format!(
                "conversation `{conversation_id}` already has a destructive mutation in progress"
            ));
        }

        state.conversations.insert(conversation_id.to_string());
        if let Some(project_id) = project_id {
            state.projects.insert(project_id.to_string());
        }
        drop(state);

        Ok(Self {
            lifecycle,
            conversation_id: conversation_id.to_string(),
            project_id: project_id.map(ToString::to_string),
            retain_conversation_tombstone: false,
        })
    }

    fn retain_conversation_tombstone(&mut self) {
        self.retain_conversation_tombstone = true;
    }
}

impl Drop for ConversationMutationGuard {
    fn drop(&mut self) {
        let mut state = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(project_id) = self.project_id.as_deref() {
            state.projects.remove(project_id);
        }
        if !self.retain_conversation_tombstone {
            state.conversations.remove(&self.conversation_id);
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum FileEffectScope {
    Project(String),
    Conversation(String),
}

#[derive(Debug, Default)]
struct FileEffectState {
    active: HashMap<FileEffectScope, HashMap<String, usize>>,
    unsettled: HashMap<FileEffectScope, HashSet<(String, String)>>,
}

#[derive(Debug)]
pub(super) struct FileEffectGuard {
    tracker: Arc<FileEffectTracker>,
    scopes: Vec<FileEffectScope>,
    run_id: String,
    effect_id: String,
    effects_started: bool,
    durably_settled: bool,
}

impl FileEffectTracker {
    pub(super) fn restore_unsettled(
        &self,
        project_id: Option<&str>,
        conversation_id: Option<&str>,
        run_id: &str,
        effect_id: &str,
    ) {
        let scopes = file_effect_scopes(project_id, conversation_id);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        for scope in scopes {
            state
                .unsettled
                .entry(scope)
                .or_default()
                .insert((run_id.to_string(), effect_id.to_string()));
        }
    }

    pub(super) fn register(
        self: &Arc<Self>,
        project_id: Option<&str>,
        conversation_id: Option<&str>,
        run_id: &str,
        effect_id: &str,
    ) -> FileEffectGuard {
        let scopes = file_effect_scopes(project_id, conversation_id);
        if !scopes.is_empty() {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            for scope in &scopes {
                *state
                    .active
                    .entry(scope.clone())
                    .or_default()
                    .entry(run_id.to_string())
                    .or_default() += 1;
            }
        }
        FileEffectGuard {
            tracker: Arc::clone(self),
            scopes,
            run_id: run_id.to_string(),
            effect_id: effect_id.to_string(),
            effects_started: false,
            durably_settled: false,
        }
    }

    fn active_run_ids(&self, scope: &FileEffectScope) -> Vec<String> {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .active
            .get(scope)
            .map(|runs| runs.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn wait_until_idle(&self, scope: &FileEffectScope, timeout: Duration) -> bool {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let (state, wait) = self
            .idle
            .wait_timeout_while(state, timeout, |state| {
                state.active.get(scope).is_some_and(|runs| !runs.is_empty())
            })
            .unwrap_or_else(|error| error.into_inner());
        !wait.timed_out() || state.active.get(scope).is_none_or(|runs| runs.is_empty())
    }

    fn unsettled_effect_ids(&self, scope: &FileEffectScope) -> Vec<String> {
        let mut effect_ids = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .unsettled
            .get(scope)
            .map(|effects| {
                effects
                    .iter()
                    .map(|(run_id, effect_id)| format!("{run_id}/{effect_id}"))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        effect_ids.sort();
        effect_ids.truncate(8);
        effect_ids
    }

    #[cfg(test)]
    pub(super) fn active_run_ids_for_conversation(&self, conversation_id: &str) -> Vec<String> {
        self.active_run_ids(&FileEffectScope::Conversation(conversation_id.to_string()))
    }

    #[cfg(test)]
    pub(super) fn unsettled_effect_ids_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Vec<String> {
        self.unsettled_effect_ids(&FileEffectScope::Conversation(conversation_id.to_string()))
    }
}

fn file_effect_scopes(
    project_id: Option<&str>,
    conversation_id: Option<&str>,
) -> Vec<FileEffectScope> {
    let mut scopes = Vec::with_capacity(2);
    if let Some(project_id) = project_id {
        scopes.push(FileEffectScope::Project(project_id.to_string()));
    }
    if let Some(conversation_id) = conversation_id {
        scopes.push(FileEffectScope::Conversation(conversation_id.to_string()));
    }
    scopes
}

impl FileEffectGuard {
    pub(super) fn mark_effects_started(&mut self) {
        self.effects_started = true;
    }

    pub(super) fn mark_durably_settled(&mut self) {
        self.durably_settled = true;
        let mut state = self
            .tracker
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for scope in &self.scopes {
            if let Some(effects) = state.unsettled.get_mut(scope) {
                effects.remove(&(self.run_id.clone(), self.effect_id.clone()));
                if effects.is_empty() {
                    state.unsettled.remove(scope);
                }
            }
        }
    }
}

impl Drop for FileEffectGuard {
    fn drop(&mut self) {
        let mut state = self
            .tracker
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for scope in &self.scopes {
            if let Some(runs) = state.active.get_mut(scope) {
                if let Some(count) = runs.get_mut(&self.run_id) {
                    *count = count.saturating_sub(1);
                    if *count == 0 {
                        runs.remove(&self.run_id);
                    }
                }
                if runs.is_empty() {
                    state.active.remove(scope);
                }
            }
            if self.effects_started && !self.durably_settled {
                state
                    .unsettled
                    .entry(scope.clone())
                    .or_default()
                    .insert((self.run_id.clone(), self.effect_id.clone()));
            }
        }
        drop(state);
        self.tracker.idle.notify_all();
    }
}

#[cfg(test)]
static PROJECT_DELETION_FAILURES: Mutex<Vec<String>> = Mutex::new(Vec::new());

#[cfg(test)]
pub(super) fn inject_project_deletion_failure(project_id: &str) {
    PROJECT_DELETION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push(project_id.to_string());
}

#[cfg(test)]
fn take_project_deletion_failure(project_id: &str) -> Option<String> {
    let mut failures = PROJECT_DELETION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures
        .iter()
        .position(|candidate| candidate == project_id)?;
    failures.swap_remove(index);
    Some(format!(
        "injected project deletion failure for {project_id}"
    ))
}

impl AgentService {
    #[cfg(test)]
    pub fn cancel_run(&self, run_id: &str) -> bool {
        self.cancel_run_checked(run_id).unwrap_or(false)
    }

    /// Cancels one user-owned root Run and its exact active descendant Runs.
    ///
    /// Unlike the legacy boolean wrapper, this entry point preserves storage failures so an RPC
    /// caller cannot mistake a best-effort process interruption for a durably committed tree
    /// stop. When the durable fence fails, the local root Run is still interrupted to honour the
    /// user's immediate stop intent, but the error remains authoritative for the response.
    pub fn cancel_run_checked(&self, run_id: &str) -> Result<bool, AgentServiceError> {
        // Resolve the owner once. Runtime teardown removes several in-memory indexes, so a second
        // lookup after authorization could otherwise lose the Conversation and skip the durable
        // descendant stop while the same request is in flight.
        let conversation_id = self.conversation_id_for_run(run_id);
        if !self
            .authorize_resolved_user_run_write(conversation_id.as_deref())
            .map_err(AgentServiceError::from)?
        {
            return Ok(false);
        }
        // `agent.cancelRun` is the explicit user stop boundary. Capture the authoritative
        // conversation binding before cancelling the worker, because worker teardown removes the
        // ActiveRunControl. This is intentionally separate from `cancel_run_internal`: deletion,
        // shutdown, provider failure, and a cancelled `command_session` observation must not gain
        // this user-authorized process termination semantic by accident.
        // Commit the run-scoped durable fence before signalling the root token. Spawn/Followup
        // commit under the same SQLite write boundary, so either their Wake is swept here or they
        // observe this stop and refuse to create new work.
        let tree_stop = match conversation_id.as_deref() {
            Some(conversation_id) => {
                match self.begin_descendant_agent_tree_stop(run_id, conversation_id) {
                    Ok(tree_stop) => tree_stop,
                    Err(error) => {
                        // A failed fence cannot truthfully be reported as a successful tree stop.
                        // Still interrupt process-local root work as an immediate best effort; the
                        // durable error is returned to the caller so it can surface/retry safely.
                        self.cancel_exact_run_execution(run_id, Some(conversation_id));
                        return Err(error);
                    }
                }
            }
            None => None,
        };
        let cancellation = self.cancel_exact_run_execution(run_id, conversation_id.as_deref());
        let cancelled_agent_tree = if let Some(tree_stop) = tree_stop {
            let mut first = self.cancel_agent_tree_wake_batch(tree_stop.cancellation);
            // Repeat once after delivering runtime cancellation to accelerate convergence and to
            // refresh an exact Wake status which raced the first frozen snapshot. The durable
            // fence, rather than this repeat, owns correctness for later scheduling commits.
            let second = match self.reinforce_agent_tree_stop_for_run_checked(run_id) {
                Ok(second) => second,
                Err(error) => {
                    first.errors.push(error.to_string());
                    AgentTreeWakeCancellationProgress::default()
                }
            };
            first.errors.extend(second.errors);
            if first.lost_ownership || second.lost_ownership {
                first.errors.push(
                    "子 Agent 的持久化执行身份在停止期间连续变化，无法确认整棵任务树已经停止。"
                        .to_string(),
                );
            }
            if !first.errors.is_empty() {
                return Err(AgentServiceError::from(first.errors.join("；")));
            }
            first.affected || second.affected
        } else {
            false
        };
        Ok(cancellation.any_effect() || cancelled_agent_tree)
    }

    /// Propagates an explicit user stop from a human-driven root Turn to every currently queued
    /// or admitted descendant Turn. The durable repository owns the claim/admission boundary;
    /// this Host layer owns process-local cancellation tokens, pending approvals, and command
    /// Sessions. Agent identities and completed history remain reusable by a later explicit Turn.
    fn begin_descendant_agent_tree_stop(
        &self,
        root_run_id: &str,
        root_conversation_id: &str,
    ) -> Result<Option<mycopilot_core::AgentTreeRunStopCancellation>, AgentServiceError> {
        let root = match self
            .storage
            .get_agent_node_by_conversation(root_conversation_id)
        {
            Ok(Some(root)) if root.parent_agent_id.is_none() => root,
            Ok(_) => return Ok(None),
            Err(error) => return Err(AgentServiceError::from(error.to_string())),
        };
        self.storage
            .begin_agent_tree_run_stop_by_root_agent(&root.agent_id, root_run_id)
            .map_err(|error| AgentServiceError::from(error.to_string()))
    }

    pub(crate) fn agent_tree_run_is_stopped(&self, run_id: &str) -> Result<bool, String> {
        self.storage
            .get_agent_tree_run_stop(run_id)
            .map(|stop| stop.is_some())
            .map_err(|error| error.to_string())
    }

    /// Repeats the durable sweep after an authoritative collaboration Tool crosses its commit
    /// boundary. Only a tree already fenced by an explicit user root-stop can use this path, so a
    /// model-issued single-Agent interrupt never gains accidental cascade semantics.
    pub(crate) fn reinforce_agent_tree_stop_for_run(&self, run_id: &str) -> bool {
        match self.reinforce_agent_tree_stop_for_run_checked(run_id) {
            Ok(progress) if progress.errors.is_empty() && !progress.lost_ownership => {
                progress.affected
            }
            Ok(progress) => {
                let detail = if progress.errors.is_empty() {
                    "子 Agent 的持久化执行身份连续变化".to_string()
                } else {
                    progress.errors.join("; ")
                };
                eprintln!("failed to reinforce stopped Agent tree: {}", detail);
                false
            }
            Err(error) => {
                eprintln!("failed to reinforce stopped Agent tree: {error}");
                false
            }
        }
    }

    fn reinforce_agent_tree_stop_for_run_checked(
        &self,
        run_id: &str,
    ) -> Result<AgentTreeWakeCancellationProgress, AgentServiceError> {
        let reinforced = match self.storage.reinforce_agent_tree_run_stop(run_id) {
            Ok(Some(reinforced)) => reinforced,
            Ok(None) => return Ok(AgentTreeWakeCancellationProgress::default()),
            Err(error) => return Err(AgentServiceError::from(error.to_string())),
        };
        Ok(self.cancel_agent_tree_wake_batch(reinforced.cancellation))
    }

    fn cancel_agent_tree_wake_batch(
        &self,
        batch: mycopilot_core::AgentTreeWakeCancellationBatch,
    ) -> AgentTreeWakeCancellationProgress {
        let mut progress = AgentTreeWakeCancellationProgress {
            affected: !batch.cancelled_before_admission.is_empty(),
            lost_ownership: false,
            errors: Vec::new(),
        };
        for wake in batch.active_wakes {
            match self.cancel_agent_tree_active_wake(wake) {
                Ok(wake_progress) => {
                    progress.affected |= wake_progress.affected;
                    progress.lost_ownership |= wake_progress.lost_ownership;
                    progress.errors.extend(wake_progress.errors);
                }
                Err(error) => progress.errors.push(error.to_string()),
            }
        }

        if progress.affected || progress.lost_ownership {
            if let Some(dispatcher) = self
                .collaboration_dispatcher
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                dispatcher.notify_work_available();
            }
        }
        progress
    }

    fn cancel_agent_tree_active_wake(
        &self,
        mut wake: mycopilot_core::ActiveAgentTreeWake,
    ) -> Result<AgentTreeWakeCancellationProgress, AgentServiceError> {
        for attempt in 0..2 {
            let runtime_interrupt = self.interrupt_agent_wake_run(&wake.run_id);
            if matches!(&runtime_interrupt, Ok(outcome) if outcome.any_effect()) {
                return Ok(AgentTreeWakeCancellationProgress {
                    affected: true,
                    lost_ownership: false,
                    errors: Vec::new(),
                });
            }

            match self.storage.settle_tree_stopped_active_wake(&wake) {
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::Interrupted(
                    settlement,
                )) => {
                    crate::application::agent_wait::shared_agent_wait_notifications()
                        .notify_caller(&settlement.result_message.recipient_agent_id);
                    self.release_conversation_turn_if_current(&wake.conversation_id, &wake.run_id);
                    self.release_turn_concurrency_permit(&wake.run_id);
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: true,
                        lost_ownership: false,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::AlreadyTerminal) => {
                    self.release_conversation_turn_if_current(&wake.conversation_id, &wake.run_id);
                    self.release_turn_concurrency_permit(&wake.run_id);
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: true,
                        lost_ownership: false,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::LostOwnership) => {
                    if attempt == 0 {
                        let current = self
                            .storage
                            .get_agent_wake(&wake.wake_id)
                            .map_err(|error| AgentServiceError::from(error.to_string()))?;
                        let Some(current) = current else {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        };
                        if current.run_id.as_deref() != Some(wake.run_id.as_str())
                            || current.assistant_message_id.as_deref()
                                != Some(wake.assistant_message_id.as_str())
                            || current.agent_id != wake.agent_id
                        {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        }
                        if current.status.is_terminal() {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: true,
                                lost_ownership: false,
                                errors: Vec::new(),
                            });
                        }
                        let Some(claim_token) = current.claim_token else {
                            return Ok(AgentTreeWakeCancellationProgress {
                                affected: false,
                                lost_ownership: true,
                                errors: Vec::new(),
                            });
                        };
                        wake.status = current.status;
                        wake.claim_token = claim_token;
                        continue;
                    }
                    return Ok(AgentTreeWakeCancellationProgress {
                        affected: false,
                        lost_ownership: true,
                        errors: Vec::new(),
                    });
                }
                Ok(mycopilot_core::AgentTreeStoppedWakeSettlementOutcome::UnsafeActiveAction {
                    count,
                }) => {
                    let runtime_error = runtime_interrupt
                        .err()
                        .map(|error| format!("；运行时中断错误：{error}"))
                        .unwrap_or_default();
                    return Err(AgentServiceError::from(format!(
                        "子 Agent Run {} 仍有 {count} 条未确认的审批或外部执行，无法安全伪装为已中断{runtime_error}",
                        wake.run_id
                    )));
                }
                Err(error) => {
                    let runtime_error = runtime_interrupt
                        .err()
                        .map(|error| format!("；运行时中断错误：{error}"))
                        .unwrap_or_default();
                    return Err(AgentServiceError::from(format!(
                        "无法持久化结算已停止的子 Agent Run {}：{error}{runtime_error}",
                        wake.run_id
                    )));
                }
            }
        }
        unreachable!("tree-stop cancellation retry loop always returns")
    }

    /// Cancels one exact Agent Run, including an adopted command Session only when the caller has
    /// already resolved the Run's Conversation at an authorized explicit-stop boundary.
    ///
    /// Root user-stop and trusted child-Wake interruption share this primitive. Authorization,
    /// durable tree fencing, pending-approval settlement, and Wake bookkeeping remain with their
    /// respective callers.
    pub(super) fn cancel_exact_run_execution(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
    ) -> AgentRunCancellationOutcome {
        let mut outcome = self.cancel_run_internal(run_id);
        let interrupted_sessions = conversation_id.map_or(0, |conversation_id| {
            self.command_sessions
                .interrupt_origin_run(conversation_id, run_id)
        });
        outcome.record_resource_cleanup(interrupted_sessions > 0);
        outcome
    }

    pub(super) fn cancel_run_internal(&self, run_id: &str) -> AgentRunCancellationOutcome {
        self.retire_builtin_capability_run(run_id);
        let runtime_token_signalled = {
            let cancellations = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(token) = cancellations.get(run_id) {
                token.cancel();
                true
            } else {
                false
            }
        };
        // Internal cancellation only reaches Sessions which have not durably transferred
        // ownership. Explicit user cancellation adds its process boundary in `cancel_run` after
        // resolving the active conversation identity; all other cancellation causes keep adopted
        // Sessions alive.
        let cancelled_sessions = self.command_sessions.cancel_pre_handoff_for_run(run_id);
        let cancelled_processes = self.process_runs.cancel_run(run_id);
        let mut outcome = AgentRunCancellationOutcome::NoEffect;
        outcome.record_resource_cleanup(cancelled_sessions > 0 || cancelled_processes > 0);
        outcome.record_turn_termination(runtime_token_signalled);
        outcome
    }

    fn retire_builtin_capability_run(&self, run_id: &str) {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            // The coordinator atomically settles process-only risk waiters/tombstones and then
            // delegates all task-grant retirement to the shared runtime authority.
            coordinator.cancel_run(run_id);
        } else if let Some(runtime) = self.builtin_capabilities.as_ref() {
            let _ = runtime.revoke_run_grants(run_id);
        }
    }

    fn cancel_runs_for_destructive_mutation(
        &self,
        run_ids: &[String],
        automation_run_ids: &HashSet<String>,
    ) -> Result<(), String> {
        for run_id in run_ids {
            if automation_run_ids.contains(run_id) {
                self.cancel_automation_agent_run(run_id)?;
            } else {
                self.cancel_run_internal(run_id);
            }
        }
        Ok(())
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        // Agent-bound projects are deletable: after this layer drains live execution and file
        // effects, StorageService removes every owned Agent tree in the same deletion transaction.
        // A graph-presence precheck here would bypass that authoritative cascade entirely.
        // Marking and effect registration use the same lock. Therefore every effect is either
        // already represented in `file_effects`, or observes the marker and never starts.
        {
            let mut lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !lifecycle.projects.insert(project_id.to_string()) {
                return Err(format!(
                    "project `{project_id}` already has a deletion in progress"
                ));
            }
        }

        let automation_run_ids = match self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_project(project_id)
        {
            Ok(run_ids) => run_ids.into_iter().collect::<HashSet<_>>(),
            Err(error) => {
                self.deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .projects
                    .remove(project_id);
                return Err(error);
            }
        };
        let mut automation_wait_run_ids = automation_run_ids.iter().cloned().collect::<Vec<_>>();
        automation_wait_run_ids.sort();
        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.project_id.as_deref() == Some(project_id))
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scope = FileEffectScope::Project(project_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        if let Err(error) = self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)
        {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .projects
                .remove(project_id);
            return Err(error);
        }
        // A handed-off command is no longer owned by its Agent Run. Project deletion is an
        // explicit process-lifecycle boundary and therefore terminates those Sessions directly.
        self.command_sessions.terminate_project(project_id);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&automation_wait_run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
            return Err(format!(
                "project `{project_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        #[cfg(test)]
        let result = if let Some(error) = take_project_deletion_failure(project_id) {
            Err(error)
        } else {
            self.storage.delete_project(project_id)
        };
        #[cfg(not(test))]
        let result = self.storage.delete_project(project_id);
        if result.is_ok() {
            self.invalidate_all_conversation_context_states();
            {
                let mut pending_actions = self
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending_actions.retain(|_, record| {
                    !agent_input_belongs_to_project(&record.agent_input, project_id)
                });
            }
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .retain(|_, state| state.context.project_id.as_deref() != Some(project_id));
        }
        if result.is_err() {
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .projects
                .remove(project_id);
        }
        result
    }

    pub(super) fn register_file_effect(
        &self,
        agent_input: &AgentChatInput,
        run_id: &str,
        effect_id: &str,
    ) -> AgentResult<FileEffectGuard> {
        let project_id = agent_input_project_id(agent_input);
        let conversation_id = agent_input_conversation_id(agent_input);
        let lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if lifecycle.contains_input(agent_input) {
            return Err(AgentError::cancelled());
        }
        // Register while the lifecycle lock is held so neither deletion path can insert its
        // marker between the state check and the active-effect count increment.
        let guard = self
            .file_effects
            .register(project_id, conversation_id, run_id, effect_id);
        drop(lifecycle);
        Ok(guard)
    }

    #[cfg(test)]
    pub(super) fn unsettled_file_effect_ids_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Vec<String> {
        self.file_effects
            .unsettled_effect_ids(&FileEffectScope::Conversation(conversation_id.to_string()))
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        // Root Agent conversations follow the same graph-aware storage transaction as project
        // deletion. Authorization above still prevents direct deletion of a child conversation.
        let project_id = self
            .storage
            .load_conversation(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let mut mutation_guard = ConversationMutationGuard::acquire(
            Arc::clone(&self.deletion_lifecycle),
            conversation_id,
            project_id.as_deref(),
        )?;

        let automation_run_ids = self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_conversation(conversation_id)?
            .into_iter()
            .collect::<HashSet<_>>();
        let mut automation_wait_run_ids = automation_run_ids.iter().cloned().collect::<Vec<_>>();
        automation_wait_run_ids.sort();

        let mut run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.conversation_id == conversation_id)
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        let scope = FileEffectScope::Conversation(conversation_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        // Agent cancellation deliberately does not reach handed-off Sessions; conversation
        // deletion does, and waits below for their terminal file-effect settlement.
        self.command_sessions
            .terminate_conversation(conversation_id);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&automation_wait_run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "conversation `{conversation_id}` was not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        let result = self.storage.delete_conversation(conversation_id);
        if result.is_ok() {
            self.invalidate_conversation_context_state(conversation_id);
            self.pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, record| {
                    record.snapshot.conversation_id.as_deref() != Some(conversation_id)
                });
            self.usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|_, state| state.context.conversation_id != conversation_id);
            let mut traces = self
                .trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for run_id in &run_ids {
                traces.remove(run_id);
            }
            // Conversation identifiers are immutable for this process lifetime. Retaining the
            // tombstone prevents a stale worker from recreating effects after durable deletion;
            // the temporary project marker is still released by the guard.
            mutation_guard.retain_conversation_tombstone();
        }
        result
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        self.authorize_user_conversation_write(conversation_id)
            .map_err(|error| error.to_string())?;
        if message_ids.is_empty() {
            return Ok(());
        }

        let project_id = self
            .storage
            .load_conversation(conversation_id)?
            .and_then(|conversation| conversation.project_id);
        let _mutation_guard = ConversationMutationGuard::acquire(
            Arc::clone(&self.deletion_lifecycle),
            conversation_id,
            project_id.as_deref(),
        )?;
        let message_id_set = message_ids.iter().cloned().collect::<HashSet<_>>();
        let automation_run_ids = self
            .storage
            .list_nonterminal_automation_agent_run_ids_for_messages(conversation_id, message_ids)?
            .into_iter()
            .collect::<HashSet<_>>();

        let (mut run_ids, mut retired_run_ids) = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let run_ids = usage_contexts
                .iter()
                .filter(|(_, state)| state.context.conversation_id == conversation_id)
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>();
            let retired_run_ids = usage_contexts
                .iter()
                .filter(|(_, state)| {
                    state.context.conversation_id == conversation_id
                        && message_id_set.contains(&state.context.assistant_message_id)
                })
                .map(|(run_id, _)| run_id.clone())
                .collect::<HashSet<_>>();
            (run_ids, retired_run_ids)
        };
        // An explicitly approved process is registered before its worker is spawned. Include
        // message-owned pending records so cancellation and the process guard cover that narrow
        // approved-to-worker-start interval even when the ephemeral usage context is absent.
        run_ids.extend({
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                })
                .map(|record| record.snapshot.run_id.clone())
                .collect::<Vec<_>>()
        });
        let scope = FileEffectScope::Conversation(conversation_id.to_string());
        run_ids.extend(self.file_effects.active_run_ids(&scope));
        run_ids.extend(automation_run_ids.iter().cloned());
        run_ids.sort();
        run_ids.dedup();
        self.cancel_runs_for_destructive_mutation(&run_ids, &automation_run_ids)?;
        self.command_sessions
            .terminate_messages(conversation_id, &message_id_set);

        if !self
            .file_effects
            .wait_until_idle(&scope, FILE_EFFECT_DRAIN_TIMEOUT)
        {
            return Err(format!(
                "messages were not deleted because an active file-producing action did not finish execution and durable settlement within {} ms; cancellation remains requested and deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }
        let unsettled_effects = self.file_effects.unsettled_effect_ids(&scope);
        if !unsettled_effects.is_empty() {
            return Err(format!(
                "messages were not deleted because file-producing actions have effects without a confirmed durable terminal receipt: {}",
                unsettled_effects.join(", ")
            ));
        }
        if !self.wait_until_runs_inactive(&run_ids, FILE_EFFECT_DRAIN_TIMEOUT) {
            return Err(format!(
                "messages were not deleted because cancelled agent runs did not reach a safe terminal boundary within {} ms; deletion may be retried",
                FILE_EFFECT_DRAIN_TIMEOUT.as_millis()
            ));
        }

        // Check resumable approvals only after draining. A successfully settled action can still
        // be momentarily `approved`/`executing` in memory after its atomic audit+target+trace
        // receipt committed; rejecting it before the drain can strand that continuation forever.
        // At this point the marker prevents new work, all runs are inactive, and the effect tracker
        // proved there is no unreceipted side effect. A genuinely `pending` action remains
        // user-resumable and must be cancelled/rejected before its owner message can be removed.
        let pending_blockers = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let mut blockers = pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                        && record.snapshot.status == PendingActionStatus::Pending
                })
                .map(|record| record.snapshot.action_id.clone())
                .collect::<Vec<_>>();
            blockers.sort();
            blockers.truncate(8);
            blockers
        };
        if !pending_blockers.is_empty() {
            return Err(format!(
                "messages were not deleted because they own pending actions; cancel or reject these actions first: {}",
                pending_blockers.join(", ")
            ));
        }

        self.storage
            .delete_chat_messages(conversation_id, message_ids)?;
        retired_run_ids.extend({
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let run_ids = pending_actions
                .values()
                .filter(|record| {
                    record.snapshot.conversation_id.as_deref() == Some(conversation_id)
                        && record
                            .snapshot
                            .assistant_message_id
                            .as_ref()
                            .is_some_and(|message_id| message_id_set.contains(message_id))
                })
                .map(|record| record.snapshot.run_id.clone())
                .collect::<HashSet<_>>();
            pending_actions.retain(|_, record| {
                record.snapshot.conversation_id.as_deref() != Some(conversation_id)
                    || record
                        .snapshot
                        .assistant_message_id
                        .as_ref()
                        .is_none_or(|message_id| !message_id_set.contains(message_id))
            });
            run_ids
        });
        if !retired_run_ids.is_empty() {
            self.usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|run_id, _| !retired_run_ids.contains(run_id));
            self.trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .retain(|run_id, _| !retired_run_ids.contains(run_id));
        }
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    fn wait_until_runs_inactive(&self, run_ids: &[String], timeout: Duration) -> bool {
        if run_ids.is_empty() {
            return true;
        }
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let cancellation_active = {
                let cancellations = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                run_ids
                    .iter()
                    .any(|run_id| cancellations.contains_key(run_id))
            };
            let process_active = run_ids
                .iter()
                .any(|run_id| self.process_runs.has_active_run(run_id));
            if !cancellation_active && !process_active {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub async fn shutdown_active_runs(&self, timeout: Duration) -> (usize, bool) {
        let active_runs = {
            let cancellations = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cancellations
                .iter()
                .map(|(run_id, token)| (run_id.clone(), token.clone()))
                .collect::<Vec<_>>()
        };

        for (run_id, token) in &active_runs {
            token.cancel();
            self.command_sessions.cancel_pre_handoff_for_run(run_id);
            self.process_runs.cancel_run(run_id);
        }

        // Process shutdown and Agent Run retirement advance independently under one deadline.
        // The process lane closes start admission, terminates every group, and waits for durable
        // Session settlement even when there are no active model runs.
        let command_sessions = self.command_sessions.clone();
        let (session_result_tx, session_result_rx) = std::sync::mpsc::sync_channel(1);
        let _session_worker = std::thread::Builder::new()
            .name("agent-command-session-shutdown".to_string())
            .spawn(move || {
                let _ = session_result_tx.send(command_sessions.shutdown(timeout));
            });

        let deadline = tokio::time::Instant::now() + timeout;
        let mut sessions_settled = false;
        let mut active_runs_settled = active_runs.is_empty();
        loop {
            let active_run_count = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len();
            active_runs_settled |= active_run_count == 0;
            if let Ok(settled) = session_result_rx.try_recv() {
                sessions_settled = settled;
            }
            if active_runs_settled && sessions_settled {
                return (active_runs.len(), false);
            }
            if tokio::time::Instant::now() >= deadline {
                if !active_runs_settled {
                    let remaining_run_ids = self
                        .cancellations
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>();
                    self.persist_forced_cancelled_runs(&remaining_run_ids);
                }
                return (active_runs.len(), true);
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn cancelled_terminal_projection_from_latest_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        unresolved_tool_error: &str,
        terminal_error: Option<&str>,
    ) -> Result<TerminalConversationTraceProjection, String> {
        let snapshot = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned()
            .unwrap_or_default();
        match terminal_error {
            Some(terminal_error) => cancelled_conversation_trace_from_snapshot_with_terminal_error(
                snapshot,
                run_id,
                conversation_id,
                assistant_message_id,
                unresolved_tool_error,
                terminal_error,
            ),
            None => cancelled_conversation_trace_from_snapshot(
                snapshot,
                run_id,
                conversation_id,
                assistant_message_id,
                unresolved_tool_error,
            ),
        }
    }

    pub(super) fn persist_forced_cancelled_runs(&self, run_ids: &[String]) {
        const REASON: &str = "Core shutdown timed out while cancelling the active run.";
        let contexts = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            run_ids
                .iter()
                .filter_map(|run_id| {
                    usage_contexts
                        .get(run_id)
                        .map(|state| state.context.clone())
                })
                .collect::<Vec<_>>()
        };
        let completed_at = now_ms();
        for context in contexts {
            let terminal = match self.cancelled_terminal_projection_from_latest_snapshot(
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                REASON,
                Some(REASON),
            ) {
                Ok(terminal) => terminal,
                Err(error) => {
                    // Shutdown must not manufacture model history from a rejected in-memory
                    // projection. Leave the already durable in-progress trace untouched; the
                    // current-schema startup reconciler closes it with its exact durable model
                    // context before any new run is admitted.
                    eprintln!("failed to close forced cancelled conversation trace: {error}");
                    continue;
                }
            };
            let usage_record = self.prepare_run_usage_record(
                &context.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(REASON.to_string()),
            );
            let persisted = self.finalize_turn_with_human_root_notification(
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                AgentRunStatus::Cancelled,
                "",
                status_for_run(AgentRunStatus::Cancelled),
                run_status_label(AgentRunStatus::Cancelled),
                &terminal.trace,
                Some(&terminal.model_context_items),
                context.started_at,
                completed_at,
                usage_record.as_ref(),
                None,
            );
            if persisted.is_ok() {
                self.finish_persisted_run_usage(&context.run_id, AgentRunStatus::Cancelled);
                self.retire_builtin_capability_run(&context.run_id);
                self.invalidate_conversation_context_state(&context.conversation_id);
            } else if let Err(error) = persisted {
                // The transaction is all-or-nothing. A later startup retires the unchanged
                // in-progress trace through the same strict trace/model-context boundary.
                eprintln!("failed to persist forced cancelled conversation trace: {error}");
            }
        }
    }

    pub(super) fn register_cancellation(&self, run_id: &str, token: AgentCancellationToken) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.insert(run_id.to_string(), token);
    }

    pub(super) fn unregister_cancellation(&self, run_id: &str) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.remove(run_id);
    }

    pub(super) fn unregister_cancellation_if_current(
        &self,
        run_id: &str,
        token: &AgentCancellationToken,
    ) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cancellations
            .get(run_id)
            .is_some_and(|current| current.shares_state_with(token))
        {
            cancellations.remove(run_id);
        }
    }

    pub(super) fn is_agent_input_scope_deleting(&self, agent_input: &AgentChatInput) -> bool {
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_input(agent_input)
    }

    pub(super) fn is_project_deleting(&self, project_id: Option<&str>) -> bool {
        let Some(project_id) = project_id else {
            return false;
        };
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .projects
            .contains(project_id)
    }

    pub(super) fn is_conversation_deleting(&self, conversation_id: Option<&str>) -> bool {
        let Some(conversation_id) = conversation_id else {
            return false;
        };
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .conversations
            .contains(conversation_id)
    }

    pub(super) fn persist_final_assistant_output(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        self.persist_final_assistant_output_inner(
            conversation_id,
            assistant_message_id,
            output,
            None,
            collaboration_cutoff,
        )
    }

    pub(super) fn persist_final_assistant_output_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        model_context_items: &[ConversationModelContextItem],
    ) -> Result<(), String> {
        self.persist_final_assistant_output_inner(
            conversation_id,
            assistant_message_id,
            output,
            Some(model_context_items),
            None,
        )
    }

    fn persist_final_assistant_output_inner(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        model_context_items: Option<&[ConversationModelContextItem]>,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        let completed_at = now_ms();
        if matches!(
            output.status,
            AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
        ) {
            // Runtime cancellation may carry a terminal Trace produced from its private recorder,
            // but AgentChatOutput does not carry the paired model-context projection. When the
            // caller has not already supplied that pair, close the latest published Host snapshot
            // and commit its Trace and model context together. Mixing the Runtime terminal Trace
            // with the older durable context can leave an unresolved ToolCall and strand the Turn
            // InProgress. Bounded root shutdown cancellation uses the same projection.
            // Provider finish reasons (for example `tool_calls`) describe the previous model
            // response. They are not cancellation diagnostics and must never become a visible
            // terminal error or the error text of a synthetically settled ToolResult.
            const CANCELLATION_TOOL_RESULT_ERROR: &str = "Agent run was cancelled.";
            let cancelled_projection =
                if output.status == AgentRunStatus::Cancelled && model_context_items.is_none() {
                    Some(self.cancelled_terminal_projection_from_latest_snapshot(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                        CANCELLATION_TOOL_RESULT_ERROR,
                        None,
                    )?)
                } else {
                    None
                };
            let trace = cancelled_projection
                .as_ref()
                .map(|terminal| terminal.trace.clone())
                .or_else(|| output.conversation_turn_trace.clone())
                .or_else(|| {
                    (output.status == AgentRunStatus::Cancelled).then(|| {
                        cancelled_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                        )
                    })
                })
                .unwrap_or_else(|| match output.status {
                    AgentRunStatus::Completed => completed_conversation_trace_without_items(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                    ),
                    AgentRunStatus::Cancelled => unreachable!(),
                    AgentRunStatus::Failed => failed_conversation_trace_without_items(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                        output
                            .finish_reason
                            .as_deref()
                            .unwrap_or("Agent run failed."),
                    ),
                    _ => unreachable!(),
                });
            let model_context_items = model_context_items.or_else(|| {
                cancelled_projection
                    .as_ref()
                    .map(|terminal| terminal.model_context_items.as_slice())
            });
            let usage_error = match output.status {
                AgentRunStatus::Failed => trace.terminal_error.clone(),
                AgentRunStatus::Completed | AgentRunStatus::Cancelled => None,
                _ => unreachable!(),
            };
            let usage_record = self.prepare_run_usage_record(
                &output.run_id,
                output.status,
                output.usage.clone(),
                usage_error,
            );
            let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
            self.finalize_turn_with_human_root_notification(
                &output.run_id,
                conversation_id,
                assistant_message_id,
                output.status,
                if output.status == AgentRunStatus::Cancelled {
                    ""
                } else {
                    &output.content
                },
                status_for_run(output.status),
                run_status_label(output.status),
                &trace,
                model_context_items,
                completed_at,
                completed_at,
                usage_record.as_ref(),
                collaboration_cutoff,
            )?;
            replace_output_usage(output, cumulative_usage);
            self.finish_persisted_run_usage(&output.run_id, output.status);
            self.retire_builtin_capability_run(&output.run_id);
            return Ok(());
        }
        if output.status == AgentRunStatus::WaitingForApproval {
            let [pending_action] = output.proposed_actions.as_slice() else {
                return Err(
                    "WaitingForApproval output must identify exactly one pending action"
                        .to_string(),
                );
            };
            let pending_action_storage_id =
                pending_action_storage_id(&output.run_id, &action_id_for_action(pending_action));
            // ApprovalRequired stages this segment's Usage before publishing the pending action.
            // Reuse that cumulative state without adding the same provider response twice.
            let usage_record =
                self.prepare_run_usage_record(&output.run_id, output.status, None, None);
            let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
            let persisted = self
                .storage
                .persist_waiting_for_approval_if_run_in_progress(
                    conversation_id,
                    assistant_message_id,
                    &output.run_id,
                    &pending_action_storage_id,
                    &output.content,
                    status_for_run(output.status),
                    completed_at,
                    usage_record.as_ref(),
                )?;
            replace_output_usage(output, cumulative_usage);
            match persisted {
                AgentWaitingForApprovalPersistenceOutcome::Persisted
                | AgentWaitingForApprovalPersistenceOutcome::PendingActionAdvanced => {}
                AgentWaitingForApprovalPersistenceOutcome::TurnTerminal => {
                    // A cancellation terminalized the exact Turn while this Runtime segment was
                    // retiring. Its durable state is authoritative; discard the stale
                    // process-local Usage/Trace snapshot instead of recreating WaitingForApproval.
                    self.discard_usage_context(&output.run_id);
                }
            }
            return Ok(());
        }
        self.persist_run_usage(&output.run_id, output.status, output.usage.clone(), None)?;
        let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
        replace_output_usage(output, cumulative_usage);
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            &output.content,
            status_for_run(output.status),
            completed_at,
        )
    }

    pub(super) fn persist_assistant_error(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_error_with_model_context(
            conversation_id,
            assistant_message_id,
            message,
            usage,
            conversation_turn_trace,
            None,
        )
    }

    pub(super) fn persist_assistant_error_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_failure_projection(
            conversation_id,
            assistant_message_id,
            message,
            Some("error"),
            message,
            usage,
            conversation_turn_trace,
            model_context_items,
        )
    }

    pub(super) fn persist_assistant_model_request_interruption(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_model_request_interruption_with_model_context(
            conversation_id,
            assistant_message_id,
            diagnostic_message,
            usage,
            conversation_turn_trace,
            None,
        )
    }

    pub(super) fn persist_assistant_model_request_interruption_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        // The failed sampling attempt never committed a model turn. Keep its diagnostic in usage
        // and trace audit data, while settling the visible assistant message as an empty prefix.
        self.persist_assistant_failure_projection(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            diagnostic_message,
            usage,
            conversation_turn_trace,
            model_context_items,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_assistant_failure_projection(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        persisted_content: &str,
        message_status: Option<&str>,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        let run_id = self.find_usage_run_id(conversation_id, assistant_message_id);
        let fallback_usage = usage.clone();
        let completed_at = now_ms();
        let usage_record = run_id.as_deref().and_then(|run_id| {
            self.prepare_run_usage_record(
                run_id,
                AgentRunStatus::Failed,
                usage,
                Some(diagnostic_message.to_string()),
            )
        });
        self.finalize_turn_with_human_root_notification(
            &conversation_turn_trace.run_id,
            conversation_id,
            assistant_message_id,
            AgentRunStatus::Failed,
            persisted_content,
            message_status,
            run_status_label(AgentRunStatus::Failed),
            conversation_turn_trace,
            model_context_items,
            completed_at,
            completed_at,
            usage_record.as_ref(),
            None,
        )?;
        let cumulative_usage = run_id
            .as_deref()
            .and_then(|run_id| self.preview_cumulative_run_usage(run_id, None))
            .or(fallback_usage);
        if let Some(run_id) = run_id {
            self.finish_persisted_run_usage(&run_id, AgentRunStatus::Failed);
            self.retire_builtin_capability_run(&run_id);
        }
        Ok(cumulative_usage)
    }
}

fn replace_output_usage(output: &mut AgentChatOutput, cumulative_usage: Option<AgentUsage>) {
    let usage = cumulative_usage.or_else(|| output.usage.clone());
    output.usage = usage.clone();
    for event in &mut output.events {
        if let AgentEvent::Done {
            run_id,
            usage: event_usage,
            ..
        } = event
        {
            if run_id == &output.run_id {
                *event_usage = usage.clone();
            }
        }
    }
}

#[cfg(test)]
mod tree_stop_batch_tests {
    use super::*;

    #[test]
    fn one_invalid_wake_does_not_prevent_later_runtime_cancellation() {
        let fixture = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("tree-stop-batch.sqlite")).unwrap());
        let service = AgentService::new(storage);
        let later_cancellation = AgentCancellationToken::new();
        service.register_cancellation("run-later", later_cancellation.clone());

        let progress =
            service.cancel_agent_tree_wake_batch(mycopilot_core::AgentTreeWakeCancellationBatch {
                root_agent_id: "agent-root".to_string(),
                root_conversation_id: "conversation-root".to_string(),
                cancelled_before_admission: Vec::new(),
                active_wakes: vec![
                    mycopilot_core::ActiveAgentTreeWake {
                        agent_id: String::new(),
                        conversation_id: String::new(),
                        wake_id: String::new(),
                        run_id: String::new(),
                        assistant_message_id: String::new(),
                        claim_token: String::new(),
                        status: mycopilot_core::AgentWakeStatus::Running,
                    },
                    mycopilot_core::ActiveAgentTreeWake {
                        agent_id: "agent-later".to_string(),
                        conversation_id: "conversation-later".to_string(),
                        wake_id: "wake-later".to_string(),
                        run_id: "run-later".to_string(),
                        assistant_message_id: "assistant-later".to_string(),
                        claim_token: "claim-later".to_string(),
                        status: mycopilot_core::AgentWakeStatus::Running,
                    },
                ],
            });

        assert!(later_cancellation.is_cancelled());
        assert!(progress.affected);
        assert_eq!(progress.errors.len(), 1);
    }
}
