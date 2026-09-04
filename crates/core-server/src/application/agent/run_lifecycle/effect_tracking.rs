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
