#[cfg(test)]
const FILE_EFFECT_DRAIN_TIMEOUT: Duration = Duration::from_millis(300);
#[cfg(not(test))]
const FILE_EFFECT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

use super::preparing::{PreparingRegistry, PreparingScope};

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
    project_mutations: HashMap<String, usize>,
    mutations_idle: Arc<Condvar>,
}

/// Fence only the authoritative affected conversations. A project deletion separately fences new
/// work and waits for already registered local mutations, rather than making one conversation's
/// file drain block every conversation in that project.
#[derive(Debug)]
struct ConversationMutationGuard {
    lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    conversation_ids: Vec<String>,
    project_id: Option<String>,
    retain_conversation_tombstone: bool,
    preparation: DeletionPreparationFence,
}

#[derive(Debug)]
struct ProjectMutationGuard {
    lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    project_id: String,
    conversation_ids: Vec<String>,
    retain_project_tombstone: bool,
    preparation: DeletionPreparationFence,
}

#[derive(Debug)]
struct DeletionPreparationFence {
    registry: Arc<PreparingRegistry>,
    scopes: Vec<PreparingScope>,
    retain_tombstone: bool,
}

impl DeletionPreparationFence {
    fn acquire(
        registry: Arc<PreparingRegistry>,
        scopes: Vec<PreparingScope>,
    ) -> Result<Self, String> {
        let mut guard = Self {
            registry,
            scopes: Vec::with_capacity(scopes.len()),
            retain_tombstone: false,
        };
        for scope in scopes {
            guard.registry.begin_delete_scope(&scope)?;
            guard.scopes.push(scope);
        }
        Ok(guard)
    }

    fn wait_idle(&self, timeout: Duration) -> bool {
        let started = Instant::now();
        self.scopes.iter().all(|scope| {
            self.registry
                .wait_scope_idle(scope, timeout.saturating_sub(started.elapsed()))
        })
    }
}

impl Drop for DeletionPreparationFence {
    fn drop(&mut self) {
        if !self.retain_tombstone {
            for scope in &self.scopes {
                self.registry.clear_delete_scope(scope);
            }
        }
    }
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
        preparing: Arc<PreparingRegistry>,
        mut conversation_ids: Vec<String>,
        project_id: Option<&str>,
    ) -> Result<Self, String> {
        conversation_ids.sort();
        conversation_ids.dedup();
        if conversation_ids.is_empty() {
            return Err("Conversation mutation requires a nonempty authoritative scope.".into());
        }
        let mut state = lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        if conversation_ids
            .iter()
            .any(|id| state.conversations.contains(id))
            || project_id.is_some_and(|project_id| state.projects.contains(project_id))
        {
            return Err(format!(
                "conversation `{}` already has a destructive mutation in progress",
                conversation_ids[0]
            ));
        }

        let preparation = DeletionPreparationFence::acquire(
            preparing,
            conversation_ids
                .iter()
                .cloned()
                .map(PreparingScope::Conversation)
                .collect(),
        )?;
        state.conversations.extend(conversation_ids.iter().cloned());
        if let Some(project_id) = project_id {
            *state
                .project_mutations
                .entry(project_id.to_string())
                .or_default() += 1;
        }
        drop(state);

        Ok(Self {
            lifecycle,
            conversation_ids,
            project_id: project_id.map(ToString::to_string),
            retain_conversation_tombstone: false,
            preparation,
        })
    }

    fn retain_conversation_tombstone(&mut self) {
        self.retain_conversation_tombstone = true;
        self.preparation.retain_tombstone = true;
    }

    /// A durable root stop closes descendant creation before the caller re-resolves its full
    /// tree. Install the additional exact scopes under the same writer boundary as admission.
    fn extend_conversations(&mut self, mut conversation_ids: Vec<String>) -> Result<(), String> {
        conversation_ids.retain(|id| !self.conversation_ids.contains(id));
        conversation_ids.sort();
        conversation_ids.dedup();
        if conversation_ids.is_empty() {
            return Ok(());
        }
        let mut state = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if conversation_ids
            .iter()
            .any(|id| state.conversations.contains(id))
        {
            return Err("Agent tree already has a destructive mutation in progress.".into());
        }
        let mut extra = DeletionPreparationFence::acquire(
            Arc::clone(&self.preparation.registry),
            conversation_ids
                .iter()
                .cloned()
                .map(PreparingScope::Conversation)
                .collect(),
        )?;
        state.conversations.extend(conversation_ids.iter().cloned());
        self.preparation.scopes.append(&mut extra.scopes);
        self.conversation_ids.extend(conversation_ids);
        self.conversation_ids.sort();
        Ok(())
    }
}

impl Drop for ConversationMutationGuard {
    fn drop(&mut self) {
        let mut state = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(project_id) = self.project_id.as_deref() {
            if let Some(count) = state.project_mutations.get_mut(project_id) {
                *count -= 1;
                if *count == 0 {
                    state.project_mutations.remove(project_id);
                }
            }
        }
        if !self.retain_conversation_tombstone {
            for conversation_id in &self.conversation_ids {
                state.conversations.remove(conversation_id);
            }
        }
        let idle = Arc::clone(&state.mutations_idle);
        drop(state);
        idle.notify_all();
    }
}

impl ProjectMutationGuard {
    fn acquire(
        lifecycle: Arc<Mutex<DeletionLifecycleState>>,
        preparing: Arc<PreparingRegistry>,
        project_id: &str,
    ) -> Result<Self, String> {
        let mut state = lifecycle.lock().unwrap_or_else(|error| error.into_inner());
        if state.projects.contains(project_id) {
            return Err(format!(
                "project `{project_id}` already has a deletion in progress"
            ));
        }
        let preparation = DeletionPreparationFence::acquire(
            preparing,
            vec![PreparingScope::Project(project_id.to_string())],
        )?;
        state.projects.insert(project_id.to_string());
        drop(state);
        Ok(Self {
            lifecycle,
            project_id: project_id.into(),
            conversation_ids: Vec::new(),
            retain_project_tombstone: false,
            preparation,
        })
    }

    fn wait_local_mutations(&self, timeout: Duration) -> bool {
        let state = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let idle = Arc::clone(&state.mutations_idle);
        let (state, _) = idle
            .wait_timeout_while(state, timeout, |state| {
                state
                    .project_mutations
                    .get(&self.project_id)
                    .copied()
                    .unwrap_or(0)
                    != 0
            })
            .unwrap_or_else(|error| error.into_inner());
        state
            .project_mutations
            .get(&self.project_id)
            .copied()
            .unwrap_or(0)
            == 0
    }

    fn include_conversations(&mut self, mut conversation_ids: Vec<String>) -> Result<(), String> {
        conversation_ids.sort();
        conversation_ids.dedup();
        let mut state = self
            .lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if conversation_ids
            .iter()
            .any(|id| state.conversations.contains(id))
        {
            return Err("Project member already has a destructive mutation in progress; retry after settlement.".into());
        }
        let mut extra = DeletionPreparationFence::acquire(
            Arc::clone(&self.preparation.registry),
            conversation_ids
                .iter()
                .cloned()
                .map(PreparingScope::Conversation)
                .collect(),
        )?;
        state.conversations.extend(conversation_ids.iter().cloned());
        self.preparation.scopes.append(&mut extra.scopes);
        self.conversation_ids = conversation_ids;
        Ok(())
    }

    fn retain_project_tombstone(&mut self) {
        self.retain_project_tombstone = true;
        self.preparation.retain_tombstone = true;
    }
}

impl Drop for ProjectMutationGuard {
    fn drop(&mut self) {
        if !self.retain_project_tombstone {
            let mut state = self
                .lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.projects.remove(&self.project_id);
            for conversation_id in &self.conversation_ids {
                state.conversations.remove(conversation_id);
            }
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

#[cfg(test)]
mod scoped_deletion_tests {
    use super::*;

    #[test]
    fn conversation_mutations_do_not_fence_other_conversations_in_same_project() {
        let lifecycle = Arc::new(Mutex::new(DeletionLifecycleState::default()));
        let preparing = Arc::new(PreparingRegistry::default());
        let first = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["a".into()],
            Some("project"),
        )
        .unwrap();
        let second = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["b".into()],
            Some("project"),
        )
        .unwrap();
        assert!(!lifecycle.lock().unwrap().projects.contains("project"));
        assert_eq!(lifecycle.lock().unwrap().project_mutations["project"], 2);
        let unrelated = preparing
            .register(
                "c",
                Some("project"),
                "run-c",
                "assistant-c",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(unrelated.validate().is_ok());
        drop(first);
        drop(second);
        assert!(lifecycle.lock().unwrap().project_mutations.is_empty());
    }

    #[test]
    fn project_deletion_waits_for_local_mutations_without_holding_registry() {
        let lifecycle = Arc::new(Mutex::new(DeletionLifecycleState::default()));
        let preparing = Arc::new(PreparingRegistry::default());
        let local = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["a".into()],
            Some("project"),
        )
        .unwrap();
        let project = ProjectMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            "project",
        )
        .unwrap();
        assert!(!project.wait_local_mutations(Duration::from_millis(1)));
        assert!(ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["b".into()],
            Some("project"),
        )
        .is_err());
        let unrelated = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["c".into()],
            Some("other"),
        )
        .unwrap();
        drop(local);
        assert!(project.wait_local_mutations(Duration::from_millis(10)));
        drop(project);
        drop(unrelated);
        assert!(!lifecycle.lock().unwrap().projects.contains("project"));
    }

    #[test]
    fn failed_tree_scope_extension_does_not_leave_partial_tombstones() {
        let lifecycle = Arc::new(Mutex::new(DeletionLifecycleState::default()));
        let preparing = Arc::new(PreparingRegistry::default());
        let mut first = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["root".into()],
            None,
        )
        .unwrap();
        let second = ConversationMutationGuard::acquire(
            Arc::clone(&lifecycle),
            Arc::clone(&preparing),
            vec!["child-b".into()],
            None,
        )
        .unwrap();
        assert!(first
            .extend_conversations(vec!["child-a".into(), "child-b".into()])
            .is_err());
        assert!(!lifecycle.lock().unwrap().conversations.contains("child-a"));
        let free = preparing
            .register(
                "child-a",
                None,
                "run-a",
                "assistant-a",
                AgentCancellationToken::new(),
                None,
            )
            .unwrap();
        assert!(free.validate().is_ok());
        drop(second);
        drop(first);
    }
}
