use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentContextCompactionAuditInput,
    AgentContextCompactionAuditOutput, AgentContextWindowSnapshotInput,
    AgentContextWindowSnapshotOutput, AgentConversationTurnInput, AgentConversationTurnOutput,
    AgentFileDraftContentPage, AgentFileWriteDiffPage, AgentServiceError, PendingActionStatus,
    PendingAgentActionSnapshot,
};
use crate::skills_adapter::activate_selected_skills;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mycopilot_core::artifact_runtime::{ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider};
use mycopilot_core::command::{
    run_authorized_command_with_artifact_runtime, AgentCommandExecutionResult,
    CommandAuthorizationSource, CommandExecutionError, CommandRunGuard, CommandRunState,
};
use mycopilot_core::file_write::{
    file_draft_snapshot, file_write_action_approval_status, file_write_approval_route,
    file_write_authorized, file_write_diff, proposed_action_uses_file_write_policy,
    FileWriteApprovalRoute, FileWriteAuthorizationSource,
};
use mycopilot_core::office::{
    resolve_office_engine, OfficeCliDiscoveryOptions, OfficeEngine, OfficeEngineError,
    OfficeExecutionResult,
};
use mycopilot_core::skills::{
    execute_skill_python_script, SkillMaterializationDestination, SkillMaterializationError,
    SkillMaterializationRequest, SkillMaterializationStatus, SkillPackageUri,
    SkillResourceMaterializer, SkillResourcePath, SkillResourceSession, SkillResourceUri,
    SkillScriptRuntimeError, SkillSelection, SkillTemplateTreeMaterializationRequest,
    SkillsService,
};
use mycopilot_core::storage::agent_action_audit_repository::{
    AgentActionAuditExecutionClaimOutcome, AgentActionAuditFinalizationOutcome,
};
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentPendingActionRecord, AgentUsageRecordInsert,
};
use mycopilot_core::storage::pending_action_repository::PendingActionStoreOutcome;
use mycopilot_core::storage::service::{AgentPendingActionSettlementInspection, StorageService};
use mycopilot_core::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_context_configuration_revision,
    conversation_trace_snapshot_from_checkpoint_and_continuation,
    create_conversation_context_state, failed_conversation_trace_without_items,
    inspect_context_window, next_run_id, send_chat_with_host_services, AgentApprovalDecision,
    AgentApprovalDecisionStatus, AgentApprovalStatus, AgentCancellationToken, AgentChatInput,
    AgentChatOutput, AgentContextBaseline, AgentContextCompactionCommitOutcome,
    AgentContextCompactionCommitRequest, AgentContextCompactionGenerationOutput,
    AgentContextCompactionGenerationRequest, AgentContextCompactionModelGenerator,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionServices, AgentContextWindowPhase,
    AgentContextWindowSnapshot, AgentConversationContextState, AgentConversationTraceObserver,
    AgentError, AgentEvent, AgentEventEmitter, AgentHostActionExecutor, AgentModelRequestObserver,
    AgentPatchResult, AgentProposedAction, AgentResult, AgentRunCheckpoint, AgentRunContext,
    AgentRunStatus, AgentRuntimeHostServices, AgentSearchConfig, AgentSkillMaterializationRequest,
    AgentSkillMaterializationResult, AgentSkillMaterializationResultStatus,
    AgentSkillScriptRequest, AgentSkillScriptResult, AgentToolCall, AgentToolContinuation,
    AgentToolResult, AgentUsage, AgentUsageClearInput, AgentUsageClearOutput,
    AgentUsageSummaryInput, AgentUsageSummaryOutput, ContextJournalCursor,
    ConversationTraceSnapshot, ConversationTurnTrace, ConversationTurnTraceTerminalStatus,
};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

mod action_execution;
mod approval;
mod completion;
mod context_compaction;
mod context_window;
mod pending_action_store;
mod run_lifecycle;
mod turn;
mod usage;

use action_execution::*;
use completion::*;
use pending_action_store::*;
use run_lifecycle::{DeletionLifecycleState, FileEffectTracker};

#[cfg(test)]
use context_compaction::validate_compaction_model_visible_boundary;
#[cfg(test)]
use run_lifecycle::inject_project_deletion_failure;

pub(crate) const AGENT_EVENT_NAME: &str = "agent.event";

pub(crate) const THINKING_PLACEHOLDER: &str = "正在思考...";

pub(crate) static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

const MAX_CONVERSATION_CONTEXT_STATE_CACHE_ENTRIES: usize = 32;

type ContextCompactionGenerationFuture = Pin<
    Box<dyn Future<Output = AgentResult<AgentContextCompactionGenerationOutput>> + Send + 'static>,
>;

type ContextCompactionSummaryGenerator = Arc<
    dyn Fn(
            AgentContextCompactionGenerationRequest,
            AgentCancellationToken,
        ) -> ContextCompactionGenerationFuture
        + Send
        + Sync,
>;

pub type CoreServerNotificationSender = UnboundedSender<Value>;

struct ConversationContextStateEntry {
    state: AgentConversationContextState,
    configuration_revision: String,
    active_run_id: Option<String>,
    active_assistant_message_id: Option<String>,
    committed_trace_items: usize,
    terminal: bool,
    last_access: u64,
}

struct ConversationContextStateUpdate {
    baseline: AgentContextBaseline,
    snapshot: Option<AgentContextWindowSnapshot>,
}

#[derive(Clone)]
pub struct AgentService {
    storage: Arc<StorageService>,
    skills: Arc<SkillsService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    trace_snapshots: Arc<Mutex<HashMap<String, ConversationTraceSnapshot>>>,
    conversation_context_states: Arc<Mutex<HashMap<String, ConversationContextStateEntry>>>,
    conversation_context_state_clock: Arc<AtomicU64>,
    context_compaction_summary_generator: Option<ContextCompactionSummaryGenerator>,
    office_engine: Arc<dyn OfficeEngine>,
    artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
    process_runs: CommandRunState,
    deletion_lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    file_effects: Arc<FileEffectTracker>,
}

impl AgentService {
    pub fn try_new(storage: Arc<StorageService>) -> Result<Self, String> {
        storage
            .reconcile_interrupted_pending_agent_actions(now_ms())
            .map_err(|error| format!("failed to reconcile interrupted pending actions: {error}"))?;
        let pending_actions = load_persisted_pending_actions(&storage)?;
        let office_engine = resolve_default_office_engine();
        let artifact_runtime = resolve_default_artifact_runtime();
        let file_effects = Arc::new(FileEffectTracker::default());
        for effect in storage.list_unsettled_file_effects().map_err(|error| {
            format!("failed to restore unsettled file-producing effects: {error}")
        })? {
            file_effects.restore_unsettled(
                effect.project_id.as_deref(),
                Some(&effect.conversation_id),
                &effect.run_id,
                &effect.action_id,
            );
        }
        Ok(Self {
            storage,
            skills: Arc::new(SkillsService::new()),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(pending_actions)),
            usage_contexts: Arc::new(Mutex::new(HashMap::new())),
            trace_snapshots: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_states: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_state_clock: Arc::new(AtomicU64::new(1)),
            context_compaction_summary_generator: None,
            office_engine,
            artifact_runtime,
            process_runs: CommandRunState::default(),
            deletion_lifecycle: Arc::new(Mutex::new(DeletionLifecycleState::default())),
            file_effects,
        })
    }

    #[cfg(test)]
    pub fn new(storage: Arc<StorageService>) -> Self {
        Self::try_new(storage).expect("agent service test fixture must initialize")
    }

    pub fn with_skills_service(mut self, skills: Arc<SkillsService>) -> Self {
        self.skills = skills;
        self
    }

    /// Probes the same Office engine instance used by Agent tools and actions.
    ///
    /// The probe may launch the provider executable and must therefore be
    /// called from a blocking worker when reached through an async server.
    pub fn get_office_engine_status(&self) -> mycopilot_core::office::OfficeEngineStatus {
        self.office_engine.status(AgentCancellationToken::new())
    }

    #[cfg(test)]
    pub fn with_office_engine(mut self, office_engine: Arc<dyn OfficeEngine>) -> Self {
        self.office_engine = office_engine;
        self
    }

    /// Reopens the immutable resource revisions captured in persisted run
    /// metadata. This is used after an approval continuation or process
    /// restart; it never follows the current installation receipt.
    fn restore_skill_resource_session(
        &self,
        input: &AgentChatInput,
    ) -> AgentResult<Option<Arc<SkillResourceSession>>> {
        let Some(activation) = input.skill_activation.as_ref() else {
            return Ok(None);
        };
        let selections = activation
            .skills
            .iter()
            .filter(|skill| skill.resources.is_some())
            .map(|skill| {
                SkillSelection::parse(skill.id.clone(), skill.revision.clone()).map_err(|error| {
                    AgentError::new(format!(
                        "cannot restore activated Skill resource identity `{}`: {error}",
                        skill.id
                    ))
                })
            })
            .collect::<AgentResult<Vec<_>>>()?;
        if selections.is_empty() {
            return Ok(None);
        }
        self.skills
            .restore_resource_session(&selections)
            .map(Arc::new)
            .map(Some)
            .map_err(|error| {
                AgentError::new(format!(
                    "cannot restore activated Skill resource snapshot: {error}"
                ))
            })
    }

    #[cfg(test)]
    fn with_context_compaction_summary_generator(
        mut self,
        generator: ContextCompactionSummaryGenerator,
    ) -> Self {
        self.context_compaction_summary_generator = Some(generator);
        self
    }
}

fn resolve_default_office_engine() -> Arc<dyn OfficeEngine> {
    let options = if let Some(path) = std::env::var_os("MYCOPILOT_OFFICECLI_PATH") {
        OfficeCliDiscoveryOptions::new().with_configured_executable(path)
    } else if let Some(directory) = std::env::var_os("MYCOPILOT_OFFICE_COMPONENTS_DIR") {
        OfficeCliDiscoveryOptions::new().with_application_resources_dir(directory)
    } else {
        OfficeCliDiscoveryOptions::new().allow_path_fallback(cfg!(debug_assertions))
    };
    resolve_office_engine(&options)
}

fn resolve_default_artifact_runtime() -> Option<Arc<ArtifactRuntimeProvider>> {
    let options = if let Some(directory) = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_DIR") {
        ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(directory)
    } else if let Some(directory) = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_COMPONENTS_DIR") {
        ArtifactRuntimeDiscoveryOptions::new().with_application_resources_dir(directory)
    } else {
        // The managed runtime is an optional application component. Native Office tools and
        // ordinary commands must remain available when it has not been prepared or packaged.
        return None;
    };

    match ArtifactRuntimeProvider::discover(&options) {
        Ok(provider) => Some(Arc::new(provider)),
        Err(error) => {
            eprintln!("managed Artifact Runtime is unavailable: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests;
