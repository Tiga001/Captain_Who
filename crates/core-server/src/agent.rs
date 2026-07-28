use crate::agent_support::*;
pub use crate::agent_support::{
    AgentActionExecutionOutput, AgentContextWindowSnapshotInput, AgentContextWindowSnapshotOutput,
    AgentConversationTurnInput, AgentConversationTurnOutput, AgentFileDraftContentPage,
    AgentFileWriteDiffPage, AgentServiceError, PendingActionStatus, PendingAgentActionSnapshot,
};
use crate::skills_adapter::{
    activate_selected_skills, model_skill_activation_resolver, prepare_enabled_skill_discovery,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use mycopilot_core::artifact_runtime::{ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider};
use mycopilot_core::command::{
    run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer,
    AgentCommandExecutionResult, CommandAuthorizationSource, CommandExecutionError,
    CommandRunGuard, CommandRunState, ProcessOutputObserver,
};
use mycopilot_core::file_input::AgentFileInputExecutionContext;
use mycopilot_core::file_write::{
    file_draft_snapshot, file_write_action_approval_status, file_write_approval_route,
    file_write_authorized, file_write_diff, proposed_action_uses_file_write_policy,
    FileWriteApprovalRoute, FileWriteAuthorizationSource,
};
use mycopilot_core::image_generation::{ImageGenerationExecutionService, ImageGenerationOperation};
use mycopilot_core::office::{
    resolve_office_engine, OfficeCliDiscoveryOptions, OfficeEngine, OfficeEngineError,
    OfficeEngineErrorCode, OfficeEngineRecovery, OfficeExecutionResult,
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
    AgentActionAuditRecord, AgentPendingActionRecord, AgentRunGuidanceRecord,
    AgentUsageRecordInsert,
};
use mycopilot_core::storage::pending_action_repository::PendingActionStoreOutcome;
use mycopilot_core::storage::service::{
    AgentPendingActionSettlementInspection, AgentRunGuidanceStoreOutcome,
    AgentRunGuidanceTransitionOutcome, StorageService,
};
use mycopilot_core::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_context_configuration_revision,
    conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection,
    create_conversation_context_state, failed_conversation_trace_without_items,
    inspect_context_window_with_tool_projection, next_run_id,
    prepare_context_window_tool_projection, project_persisted_continuation_for_archive,
    project_persisted_continuation_for_model, project_persisted_continuation_observation,
    send_chat_with_host_services, terminalize_interrupted_conversation_trace,
    AgentApprovalDecision, AgentApprovalDecisionStatus, AgentApprovalStatus,
    AgentCancellationToken, AgentChatInput, AgentChatOutput, AgentContextBaseline,
    AgentContextCompactionCommitOutcome, AgentContextCompactionCommitRequest,
    AgentContextCompactionGenerationOutput, AgentContextCompactionGenerationRequest,
    AgentContextCompactionModelGenerator, AgentContextCompactionPrepareOutcome,
    AgentContextCompactionServices, AgentContextWindowObserver, AgentContextWindowSnapshot,
    AgentContextWindowToolProjection, AgentConversationContextState,
    AgentConversationTraceObserver, AgentError, AgentEvent, AgentEventEmitter, AgentGuidanceStatus,
    AgentHostActionExecutor, AgentModelRequestObserver, AgentPatchResult, AgentProposedAction,
    AgentResult, AgentRunCheckpoint, AgentRunContext, AgentRunStatus, AgentRuntimeHostServices,
    AgentSearchConfig, AgentSkillMaterializationRequest, AgentSkillMaterializationResult,
    AgentSkillMaterializationResultStatus, AgentSkillScriptRequest, AgentSkillScriptResult,
    AgentSteerEnqueueOutcome, AgentSteerInput, AgentSteerInputQueue, AgentSteerRunInput,
    AgentSteerRunOutput, AgentSteerRunRejectionCode, AgentSteerRunResultStatus, AgentToolCall,
    AgentToolContinuation, AgentToolResult, AgentUsage, AgentUsageClearInput,
    AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput, ContextJournalCursor,
    ConversationModelContextItem, ConversationTraceSnapshot, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus, ModelCapabilities,
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
mod steering;
mod turn;
mod usage;

use action_execution::*;
use completion::*;
use pending_action_store::*;
use run_lifecycle::{DeletionLifecycleState, FileEffectTracker};

#[cfg(test)]
use context_compaction::validate_compaction_trace_boundary;
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

type OfficeEngineResolver = Arc<dyn Fn() -> Arc<dyn OfficeEngine> + Send + Sync + 'static>;

/// A Host-owned, single-flight Office engine binding.
///
/// Office prepared executions are revision-bound security snapshots. A stale
/// engine can therefore be rediscovered before a new operation is prepared,
/// but an already prepared (and possibly approved) operation must never be
/// replayed against the replacement engine.
struct RefreshableOfficeEngine {
    current: Mutex<Arc<dyn OfficeEngine>>,
    resolver: OfficeEngineResolver,
}

impl RefreshableOfficeEngine {
    fn new(resolver: OfficeEngineResolver) -> Self {
        let current = resolver();
        Self {
            current: Mutex::new(current),
            resolver,
        }
    }

    #[cfg(test)]
    fn with_current(current: Arc<dyn OfficeEngine>, resolver: OfficeEngineResolver) -> Self {
        Self {
            current: Mutex::new(current),
            resolver,
        }
    }

    fn current(&self) -> Arc<dyn OfficeEngine> {
        self.current
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Re-resolves at most once for every observed stale engine instance.
    /// Concurrent callers that observed the same instance all receive the
    /// first replacement instead of racing independent discoveries.
    fn refresh_if_current(&self, observed: &Arc<dyn OfficeEngine>) -> Arc<dyn OfficeEngine> {
        let mut current = self
            .current
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !Arc::ptr_eq(&current, observed) {
            return current.clone();
        }
        let replacement = (self.resolver)();
        *current = replacement.clone();
        replacement
    }
}

impl OfficeEngine for RefreshableOfficeEngine {
    fn capabilities(&self) -> mycopilot_core::office::OfficeEngineCapabilities {
        self.current().capabilities()
    }

    fn status(
        &self,
        cancellation: AgentCancellationToken,
    ) -> mycopilot_core::office::OfficeEngineStatus {
        let current = self.current();
        let status = current.status(cancellation.clone());
        if status.error_code.as_deref()
            != Some(OfficeEngineErrorCode::InvalidConfiguration.stable_name())
        {
            return status;
        }
        self.refresh_if_current(&current).status(cancellation)
    }

    fn prepare(
        &self,
        context: &mycopilot_core::office::OfficeExecutionContext,
        request: &mycopilot_core::office::OfficeExecutionRequest,
    ) -> Result<mycopilot_core::office::OfficePreparedExecution, OfficeEngineError> {
        let current = self.current();
        match current.prepare(context, request) {
            Err(error) if error.code() == OfficeEngineErrorCode::InvalidConfiguration => {
                self.refresh_if_current(&current).prepare(context, request)
            }
            result => result,
        }
    }

    fn execute_prepared(
        &self,
        context: &mycopilot_core::office::OfficeExecutionContext,
        prepared: &mycopilot_core::office::OfficePreparedExecution,
        cancellation: AgentCancellationToken,
        action_cancel_flag: Option<Arc<AtomicBool>>,
    ) -> Result<OfficeExecutionResult, OfficeEngineError> {
        let current = self.current();
        match current.execute_prepared(context, prepared, cancellation, action_cancel_flag) {
            Err(error) if error.code() == OfficeEngineErrorCode::InvalidConfiguration => {
                // Discovery is safe here, but execution is deliberately not retried. The
                // replacement has a different revision, so the old frozen plan (including an
                // approved write) no longer carries authority to execute.
                self.refresh_if_current(&current);
                Err(OfficeEngineError::new(
                    OfficeEngineErrorCode::PreconditionFailed,
                    OfficeEngineRecovery::Retry,
                    "The Office engine changed after this operation was prepared. The Host refreshed the engine, but did not replay the stale prepared or approved operation; prepare it again.",
                ))
            }
            result => result,
        }
    }
}

pub type CoreServerNotificationSender = UnboundedSender<Value>;

struct ConversationContextStateEntry {
    state: AgentConversationContextState,
    configuration_revision: String,
    active_run_id: Option<String>,
    active_assistant_message_id: Option<String>,
    committed_activity_items: usize,
    terminal: bool,
    last_access: u64,
}

struct ConversationContextStateUpdate {
    baseline: AgentContextBaseline,
    snapshot: Option<AgentContextWindowSnapshot>,
}

#[derive(Clone)]
struct RunContextToolProjection {
    initial: Option<AgentContextWindowToolProjection>,
}

impl RunContextToolProjection {
    fn new(initial: AgentContextWindowToolProjection) -> Self {
        Self {
            initial: Some(initial),
        }
    }

    #[cfg(test)]
    fn pending() -> Self {
        Self { initial: None }
    }

    fn projection(&self) -> Option<AgentContextWindowToolProjection> {
        self.initial.clone()
    }
}

#[derive(Debug, Clone)]
enum ActiveRunSteerState {
    Accepting,
    Closed {
        code: AgentSteerRunRejectionCode,
        message: String,
    },
}

#[derive(Debug, Clone)]
struct ActiveRunControl {
    conversation_id: String,
    assistant_message_id: String,
    project_id: Option<String>,
    model_capabilities: ModelCapabilities,
    steer_state: ActiveRunSteerState,
    steer_input: AgentSteerInputQueue,
}

#[derive(Clone)]
pub struct AgentService {
    storage: Arc<StorageService>,
    skills: Arc<SkillsService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    trace_snapshots: Arc<Mutex<HashMap<String, ConversationTraceSnapshot>>>,
    running_context_window_snapshots: Arc<Mutex<HashMap<String, AgentContextWindowSnapshot>>>,
    conversation_context_states: Arc<Mutex<HashMap<String, ConversationContextStateEntry>>>,
    conversation_context_state_clock: Arc<AtomicU64>,
    context_compaction_summary_generator: Option<ContextCompactionSummaryGenerator>,
    office_engine: Arc<dyn OfficeEngine>,
    image_generation_execution: Option<Arc<ImageGenerationExecutionService>>,
    artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
    process_runs: CommandRunState,
    deletion_lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    file_effects: Arc<FileEffectTracker>,
}

impl AgentService {
    #[cfg(test)]
    pub fn try_new(storage: Arc<StorageService>) -> Result<Self, String> {
        Self::try_new_with_startup_reconciliation(storage, true)
    }

    /// Builds the production service while deferring orphan trace retirement until asynchronous
    /// external execution journals have been reconciled.
    pub(crate) fn try_new_deferred_startup_reconciliation(
        storage: Arc<StorageService>,
    ) -> Result<Self, String> {
        Self::try_new_with_startup_reconciliation(storage, false)
    }

    fn try_new_with_startup_reconciliation(
        storage: Arc<StorageService>,
        reconcile_orphaned_traces: bool,
    ) -> Result<Self, String> {
        storage
            .reconcile_interrupted_pending_agent_actions(now_ms())
            .map_err(|error| format!("failed to reconcile interrupted pending actions: {error}"))?;
        if reconcile_orphaned_traces {
            storage
                .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), now_ms())
                .map_err(|error| {
                    format!("failed to reconcile orphaned conversation traces: {error}")
                })?;
        }
        let interrupted_guidances = storage
            .list_queued_agent_run_guidances()
            .map_err(|error| format!("failed to list interrupted agent run guidance: {error}"))?;
        let interrupted_run_ids = interrupted_guidances
            .iter()
            .map(|guidance| guidance.run_id.as_str())
            .collect::<HashSet<_>>();
        for run_id in interrupted_run_ids {
            storage
                .abandon_queued_agent_run_guidances(
                    run_id,
                    "Core process restarted before the guidance reached a terminal state.",
                    now_ms(),
                )
                .map_err(|error| {
                    format!("failed to reconcile interrupted agent run guidance: {error}")
                })?;
        }
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
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(pending_actions)),
            usage_contexts: Arc::new(Mutex::new(HashMap::new())),
            trace_snapshots: Arc::new(Mutex::new(HashMap::new())),
            running_context_window_snapshots: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_states: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_state_clock: Arc::new(AtomicU64::new(1)),
            context_compaction_summary_generator: None,
            office_engine,
            image_generation_execution: None,
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

    /// Installs the process-owned image-generation executor into future Agent runs. The service
    /// remains a Host capability so provider credentials and transport details never enter model
    /// input or resumable checkpoints.
    pub fn with_image_generation_execution(
        mut self,
        service: Arc<ImageGenerationExecutionService>,
    ) -> Self {
        self.image_generation_execution = Some(service);
        self
    }

    /// Reconciles durable image ToolCalls with the authoritative image execution journal.
    ///
    /// Production invokes this only after the execution service has terminalized every
    /// interrupted journal entry and before generic orphan trace retirement. It is idempotent:
    /// closed traces are skipped and trace storage enforces an exact append-only prefix.
    pub(crate) async fn reconcile_interrupted_image_generation_tool_audits(
        &self,
    ) -> Result<usize, String> {
        let Some(execution_service) = self.image_generation_execution.as_ref() else {
            return Ok(0);
        };
        let traces = self.storage.list_in_progress_conversation_turn_traces()?;
        let mut reconciled = 0;
        for trace in traces {
            let Some(ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation: args,
                ..
            }) = trace.items.last()
            else {
                continue;
            };
            if tool != "image_generation" {
                continue;
            }
            let operation_name = args
                .get("request")
                .and_then(|request| request.get("operation"))
                .and_then(Value::as_str);
            let operation = match operation_name {
                Some("generate") => ImageGenerationOperation::Generate,
                Some("edit") => ImageGenerationOperation::Edit,
                _ => continue,
            };
            let reason = args
                .get("reason")
                .and_then(Value::as_str)
                .and_then(mycopilot_core::normalize_agent_image_generation_reason)
                .unwrap_or_else(|| "Recover the interrupted image generation audit.".to_string());
            let execution_id =
                mycopilot_core::agent_image_generation_execution_id(&trace.run_id, call_id)
                    .map_err(|error| {
                        format!("failed to derive interrupted image execution identity: {error}")
                    })?;
            let result = match execution_service.inspect_terminal(&execution_id).await {
                Ok(Some(execution)) => {
                    mycopilot_core::agent_image_generation_tool_result_from_execution(
                        call_id,
                        &reason,
                        operation,
                        execution_id.as_str(),
                        execution,
                    )
                }
                Ok(None) => continue,
                Err(error) => {
                    mycopilot_core::agent_image_generation_tool_result_from_service_error(
                        call_id,
                        &reason,
                        operation,
                        execution_id.as_str(),
                        error,
                    )
                }
            };
            let recovered =
                mycopilot_core::conversation_trace_with_recovered_tool_result(&trace, &result)
                    .map_err(|error| {
                        format!("failed to append interrupted image ToolResult audit: {error}")
                    })?;
            let committed_at = now_ms();
            self.storage
                .append_in_progress_conversation_turn_trace(&recovered, committed_at, committed_at)
                .map_err(|error| {
                    format!("failed to persist interrupted image ToolResult audit: {error}")
                })?;
            reconciled += 1;
        }
        Ok(reconciled)
    }

    pub(crate) fn reconcile_startup_orphaned_conversation_traces(&self) -> Result<usize, String> {
        self.storage
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), now_ms())
            .map_err(|error| format!("failed to reconcile orphaned conversation traces: {error}"))
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
        let checkpoint_authority = input
            .resume_checkpoint
            .as_ref()
            .map(mycopilot_core::skill_checkpoint_authority)
            .transpose()?
            .flatten();
        let discovery = match checkpoint_authority.as_ref() {
            Some(authority) => authority.discovery.clone(),
            None => input.skill_discovery.clone(),
        };
        if let Some(discovery) = discovery.as_ref() {
            discovery.validate().map_err(|error| {
                AgentError::new(format!(
                    "cannot restore an invalid frozen Skill discovery catalog: {error}"
                ))
            })?;
        }
        let selections = match checkpoint_authority {
            Some(authority) => authority.resource_selections,
            None => input
                .skill_activation
                .iter()
                .flat_map(|activation| activation.skills.iter())
                .filter(|skill| skill.resources.is_some())
                .map(|skill| {
                    SkillSelection::parse(skill.id.clone(), skill.revision.clone()).map_err(
                        |error| {
                            AgentError::new(format!(
                                "cannot restore activated Skill resource identity `{}`: {error}",
                                skill.id
                            ))
                        },
                    )
                })
                .collect::<AgentResult<Vec<_>>>()?,
        };
        let mut unique =
            std::collections::BTreeMap::<mycopilot_core::skills::SkillId, SkillSelection>::new();
        for selection in selections {
            match unique.get(selection.skill_id()) {
                Some(existing) if existing.expected_revision() != selection.expected_revision() => {
                    return Err(AgentError::new(format!(
                        "cannot restore two revisions of Skill `{}`",
                        selection.skill_id()
                    )));
                }
                Some(_) => {}
                None => {
                    unique.insert(selection.skill_id().clone(), selection);
                }
            }
        }
        let selections = unique.into_values().collect::<Vec<_>>();
        if selections.is_empty() {
            return Ok(discovery.map(|_| Arc::new(SkillResourceSession::empty())));
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
    let mut options = if let Some(path) = std::env::var_os("MYCOPILOT_OFFICECLI_PATH") {
        OfficeCliDiscoveryOptions::new().with_configured_executable(path)
    } else if let Some(directory) = std::env::var_os("MYCOPILOT_OFFICE_COMPONENTS_DIR") {
        OfficeCliDiscoveryOptions::new().with_application_resources_dir(directory)
    } else {
        OfficeCliDiscoveryOptions::new().allow_path_fallback(cfg!(debug_assertions))
    };
    if let Some(directory) = std::env::var_os("MYCOPILOT_OFFICE_RENDERER_DIR") {
        options = options.with_configured_render_runtime_dir(directory);
    }
    if let Ok(executable) = std::env::current_exe() {
        options = options.with_browser_proxy_executable(executable);
    }
    let resolver: OfficeEngineResolver = Arc::new(move || resolve_office_engine(&options));
    Arc::new(RefreshableOfficeEngine::new(resolver))
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
