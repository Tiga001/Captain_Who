use crate::adapters::agent_skill_installation::AgentSkillInstallationInspectionAdapter;
use crate::adapters::skills_adapter::{
    activate_selected_skills, model_skill_activation_resolver, prepare_enabled_skill_discovery,
};
use crate::application::agent_collaboration::{
    ChildAgentFactory, PersistentAgentSamplingBoundaryInbox,
};
use crate::application::agent_support::*;
pub use crate::application::agent_support::{
    AgentActionExecutionOutput, AgentContextWindowSnapshotInput, AgentContextWindowSnapshotOutput,
    AgentConversationTurnInput, AgentConversationTurnOutput, AgentFileDraftContentPage,
    AgentFileWriteDiffPage, AgentProviderTransitionGetStatusInput,
    AgentProviderTransitionGetStatusOutput, AgentProviderTransitionOperation,
    AgentProviderTransitionPreflightInput, AgentProviderTransitionPreflightOutput,
    AgentProviderTransitionStartInput, AgentServiceError, PendingActionStatus,
    PendingAgentActionSnapshot,
};
use crate::application::mcp::approval_payload_store::{
    McpApprovalStartupInspector, McpApprovalStartupPayloadState,
};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use mycopilot_core::artifact_runtime::{ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeProvider};
use mycopilot_core::command::{
    AgentCommandExecutionResult, CommandAuthorizationSource, CommandRunGuard, CommandRunState,
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
    execute_skill_python_script, SkillInstallationService, SkillInstallationWorkflow,
    SkillMaterializationDestination, SkillMaterializationError, SkillMaterializationRequest,
    SkillMaterializationStatus, SkillPackageUri, SkillResourceMaterializer, SkillResourcePath,
    SkillResourceSession, SkillResourceUri, SkillScriptRuntimeError, SkillSelection,
    SkillTemplateTreeMaterializationRequest, SkillsService,
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
    AgentRunGuidanceTransitionOutcome, McpActionTerminalizationRequest,
    McpAutoActionJournalTerminalOutcome, McpStartupActionTerminalOutcome, StorageService,
};
use mycopilot_core::{
    cancelled_conversation_trace_from_checkpoint, cancelled_conversation_trace_from_snapshot,
    cancelled_conversation_trace_without_items, completed_conversation_trace_without_items,
    conversation_context_configuration_revision,
    conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection,
    conversation_trace_snapshot_with_recovered_tool_result,
    create_conversation_context_state_with_host_services, failed_conversation_trace_without_items,
    inspect_context_window_with_tool_projection, mcp_tool_invocation_event,
    mcp_tool_result_from_approved_invocation, mcp_tool_result_from_rejected_approval,
    mcp_tool_result_size_summary, next_run_id, prepare_context_window_tool_projection,
    project_persisted_continuation_for_archive, project_persisted_continuation_for_model,
    project_persisted_continuation_observation, send_chat_with_host_services,
    terminal_conversation_trace_from_snapshot, AgentApprovalDecision, AgentApprovalDecisionStatus,
    AgentApprovalStatus, AgentCancellationToken, AgentChatInput, AgentChatOutput,
    AgentCollaborationIdentity, AgentContextBaseline, AgentContextCompactionCommitOutcome,
    AgentContextCompactionCommitRequest, AgentContextCompactionGenerationOutput,
    AgentContextCompactionGenerationRequest, AgentContextCompactionModelGenerator,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionServices,
    AgentContextWindowObserver, AgentContextWindowSnapshot, AgentContextWindowToolProjection,
    AgentConversationContextState, AgentConversationTraceObserver, AgentError, AgentEvent,
    AgentEventEmitter, AgentGuidanceStatus, AgentHostActionExecutor, AgentMcpDispatchCertainty,
    AgentMcpInvocationFailureStage, AgentMcpResultSizeSummary, AgentMcpServerScope,
    AgentMcpToolInvocationOutcome, AgentMcpToolInvocationState, AgentModelRequestObserver,
    AgentPatchResult, AgentProposedAction, AgentResult, AgentRunCheckpoint, AgentRunContext,
    AgentRunStatus, AgentRuntimeHostServices, AgentSearchConfig,
    AgentSkillInstallationPrepareExecutor, AgentSkillMaterializationRequest,
    AgentSkillMaterializationResult, AgentSkillMaterializationResultStatus,
    AgentSkillScriptRequest, AgentSkillScriptResult, AgentSteerEnqueueOutcome, AgentSteerInput,
    AgentSteerInputQueue, AgentSteerRunInput, AgentSteerRunOutput, AgentSteerRunRejectionCode,
    AgentSteerRunResultStatus, AgentToolCall, AgentToolContinuation, AgentToolResult, AgentUsage,
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput,
    ContextJournalCursor, ConversationModelContextItem, ConversationTraceSnapshot,
    ConversationTurnTrace, ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
    McpApprovedToolInvocation, McpToolCatalogContext, McpToolInvocationEventUpdate, McpToolInvoker,
    McpToolRuntime, ModelCapabilities, ProviderContinuationVault, ProviderProtocolDialect,
    ProviderProtocolKey,
};
#[cfg(test)]
use mycopilot_core::{
    create_conversation_context_state, ProviderContinuationVaultFactory, ProviderUsageSemantics,
};
use mycopilot_mcp_client::{McpConfigDigest, McpConfigEpoch, McpServerId};
use serde_json::Value;
use tokio::sync::{mpsc::UnboundedSender, Notify};

mod action_execution;
mod approval;
mod command_sessions;
mod completion;
mod context_compaction;
mod context_window;
mod pending_action_store;
mod persisted_resume_input;
mod provider_transition;
mod run_lifecycle;
mod steering;
mod turn;
mod turn_executor;
mod usage;

use action_execution::*;
#[cfg(test)]
use command_sessions::AgentCommandSessionHandoffGuard;
use command_sessions::{
    AgentCommandHandoffOutcome, AgentCommandSessionLaunch, AgentCommandSessionRegistry,
    CommandSessionOwner, StartAgentCommandSession,
};
use completion::*;
use pending_action_store::*;
use persisted_resume_input::*;
use run_lifecycle::{DeletionLifecycleState, FileEffectTracker};
use turn_executor::*;
pub(crate) use turn_executor::{AgentTurnStart, TrustedAgentWakeTurnStart};

#[cfg(test)]
use context_compaction::validate_compaction_trace_boundary;
#[cfg(test)]
use run_lifecycle::inject_project_deletion_failure;

#[cfg(test)]
fn test_provider_continuation_vault(
    storage: Arc<StorageService>,
) -> Result<Arc<ProviderContinuationVault>, String> {
    ProviderContinuationVaultFactory::open_or_provision(
        storage,
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
    )
    .map(Arc::new)
    .map_err(|error| {
        format!(
            "failed to initialize test Provider continuation vault: {}",
            error.code()
        )
    })
}

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

type McpApprovalClock = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Typed Host selector for invalidating approvals owned by one MCP configuration source.
///
/// Scope, configuration identity, Registry revision, and Catalog generation are optional narrowing
/// predicates for source removal and delayed lifecycle events. The stable Server ID always remains
/// the primary identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct McpActionInvalidationTarget {
    pub(crate) server_id: McpServerId,
    pub(crate) scope: Option<AgentMcpServerScope>,
    pub(crate) source_config_digest: Option<McpConfigDigest>,
    pub(crate) source_config_epoch: Option<McpConfigEpoch>,
    pub(crate) prior_to_registry_revision: Option<u64>,
    pub(crate) prior_to_catalog_generation: Option<u64>,
}

impl McpActionInvalidationTarget {
    pub(crate) fn server(server_id: McpServerId) -> Self {
        Self {
            server_id,
            scope: None,
            source_config_digest: None,
            source_config_epoch: None,
            prior_to_registry_revision: None,
            prior_to_catalog_generation: None,
        }
    }

    pub(crate) fn with_scope(mut self, scope: AgentMcpServerScope) -> Self {
        self.scope = Some(scope);
        self
    }

    pub(crate) fn with_source_config_digest(mut self, digest: McpConfigDigest) -> Self {
        self.source_config_digest = Some(digest);
        self
    }

    pub(crate) fn with_source_config_epoch(mut self, epoch: McpConfigEpoch) -> Self {
        self.source_config_epoch = Some(epoch);
        self
    }

    pub(crate) fn prior_to_registry_revision(mut self, revision: u64) -> Self {
        self.prior_to_registry_revision = Some(revision);
        self
    }

    pub(crate) fn prior_to_catalog_generation(mut self, generation: u64) -> Self {
        self.prior_to_catalog_generation = Some(generation);
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct McpActionInvalidationSummary {
    pub(crate) terminalized_before_dispatch: usize,
    pub(crate) terminalized_outcome_unknown: usize,
    pub(crate) payload_invalidation_attempts: usize,
    pub(crate) payload_invalidation_failures: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct McpApprovalExpiryReconciliation {
    pub(crate) cutoff_ms: i64,
    pub(crate) candidates: usize,
    pub(crate) terminalized: usize,
    pub(crate) status_cas_conflicts: usize,
    pub(crate) payload_invalidation_attempts: usize,
    pub(crate) payload_invalidation_failures: usize,
}

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

/// Process-local accelerator for the durable in-progress ConversationTurnTrace admission fact.
/// It is keyed by Conversation (not run) so startup, normal execution, and approval continuation
/// all enforce the same logical-Turn boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveConversationTurn {
    run_id: String,
    assistant_message_id: String,
}

#[derive(Clone)]
pub struct AgentService {
    storage: Arc<StorageService>,
    provider_continuation_vault: Option<Arc<ProviderContinuationVault>>,
    skills: Arc<SkillsService>,
    cancellations: Arc<Mutex<HashMap<String, AgentCancellationToken>>>,
    active_runs: Arc<Mutex<HashMap<String, ActiveRunControl>>>,
    active_conversation_turns: Arc<Mutex<HashMap<String, ActiveConversationTurn>>>,
    /// Process-local wakeup accelerator for consumers which observe durable Turn state.
    ///
    /// The corresponding Conversation trace and pending-action rows remain authoritative. A
    /// subscriber always checks SQLite both before and after registering this notification, so
    /// dropping the map on restart or coalescing notifications cannot lose a completion.
    durable_turn_notifications: Arc<Mutex<HashMap<String, Arc<Notify>>>>,
    turn_concurrency_gate: crate::application::agent_dispatcher::AgentTurnConcurrencyGate,
    active_turn_permits: Arc<
        Mutex<HashMap<String, crate::application::agent_dispatcher::AgentTurnConcurrencyPermit>>,
    >,
    pending_actions: Arc<Mutex<HashMap<String, PendingActionRecord>>>,
    startup_recoverable_mcp_approvals: Arc<Mutex<HashSet<String>>>,
    usage_contexts: Arc<Mutex<HashMap<String, AgentRunUsageState>>>,
    trace_snapshots: Arc<Mutex<HashMap<String, ConversationTraceSnapshot>>>,
    running_context_window_snapshots: Arc<Mutex<HashMap<String, AgentContextWindowSnapshot>>>,
    conversation_context_states: Arc<Mutex<HashMap<String, ConversationContextStateEntry>>>,
    conversation_context_state_clock: Arc<AtomicU64>,
    context_compaction_summary_generator: Option<ContextCompactionSummaryGenerator>,
    conversation_admission: Arc<Mutex<()>>,
    provider_transitions: Arc<Mutex<HashMap<String, String>>>,
    provider_transition_operations: Arc<Mutex<HashMap<String, AgentProviderTransitionOperation>>>,
    office_engine: Arc<dyn OfficeEngine>,
    image_generation_execution: Option<Arc<ImageGenerationExecutionService>>,
    skill_installation_prepare: Option<Arc<dyn AgentSkillInstallationPrepareExecutor>>,
    skill_installation: Option<Arc<AgentSkillInstallationInspectionAdapter>>,
    skill_installation_service: Option<Arc<SkillInstallationService>>,
    skill_installation_workflow: Option<Arc<SkillInstallationWorkflow>>,
    artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
    mcp_tool_invoker: Option<Arc<dyn McpToolInvoker>>,
    mcp_startup_inspector: Option<Arc<dyn McpApprovalStartupInspector>>,
    mcp_approval_clock: McpApprovalClock,
    process_runs: CommandRunState,
    command_sessions: AgentCommandSessionRegistry,
    deletion_lifecycle: Arc<Mutex<DeletionLifecycleState>>,
    file_effects: Arc<FileEffectTracker>,
}

impl AgentService {
    #[cfg(test)]
    pub fn try_new(storage: Arc<StorageService>) -> Result<Self, String> {
        let provider_continuation_vault = test_provider_continuation_vault(Arc::clone(&storage))?;
        Self::try_new_with_startup_reconciliation(storage, true, Some(provider_continuation_vault))
    }

    /// Builds the production service while deferring orphan trace retirement until asynchronous
    /// external execution journals have been reconciled.
    pub(crate) fn try_new_deferred_startup_reconciliation_with_provider_continuation_vault(
        storage: Arc<StorageService>,
        provider_continuation_vault: Option<Arc<ProviderContinuationVault>>,
    ) -> Result<Self, String> {
        Self::try_new_deferred_startup_reconciliation_with_agent_limit(
            storage,
            provider_continuation_vault,
            crate::application::agent_dispatcher::DEFAULT_AGENT_GLOBAL_CONCURRENCY,
        )
    }

    pub(crate) fn try_new_deferred_startup_reconciliation_with_agent_limit(
        storage: Arc<StorageService>,
        provider_continuation_vault: Option<Arc<ProviderContinuationVault>>,
        global_agent_concurrency_limit: usize,
    ) -> Result<Self, String> {
        Self::try_new_with_startup_reconciliation_and_limit(
            storage,
            false,
            provider_continuation_vault,
            global_agent_concurrency_limit,
        )
    }

    #[cfg(test)]
    pub(crate) fn try_new_deferred_startup_reconciliation(
        storage: Arc<StorageService>,
    ) -> Result<Self, String> {
        let provider_continuation_vault = test_provider_continuation_vault(Arc::clone(&storage))?;
        Self::try_new_with_startup_reconciliation(storage, false, Some(provider_continuation_vault))
    }

    #[cfg(test)]
    fn try_new_with_startup_reconciliation(
        storage: Arc<StorageService>,
        reconcile_orphaned_traces: bool,
        provider_continuation_vault: Option<Arc<ProviderContinuationVault>>,
    ) -> Result<Self, String> {
        Self::try_new_with_startup_reconciliation_and_limit(
            storage,
            reconcile_orphaned_traces,
            provider_continuation_vault,
            crate::application::agent_dispatcher::DEFAULT_AGENT_GLOBAL_CONCURRENCY,
        )
    }

    fn try_new_with_startup_reconciliation_and_limit(
        storage: Arc<StorageService>,
        reconcile_orphaned_traces: bool,
        provider_continuation_vault: Option<Arc<ProviderContinuationVault>>,
        global_agent_concurrency_limit: usize,
    ) -> Result<Self, String> {
        let turn_concurrency_gate =
            crate::application::agent_dispatcher::AgentTurnConcurrencyGate::new(
                global_agent_concurrency_limit,
            )
            .map_err(|error| error.to_string())?;
        // Process handles are intentionally not recoverable across Host restarts. Reconcile the
        // operational projection before generic orphaned-run handling so no stale row is ever
        // advertised as controllable by this process.
        let unresolved_command_sessions = storage
            .reconcile_agent_command_sessions_on_startup(now_ms())
            .map_err(|error| format!("failed to reconcile command sessions: {error}"))?;
        storage
            .reconcile_interrupted_pending_agent_actions(now_ms())
            .map_err(|error| format!("failed to reconcile interrupted pending actions: {error}"))?;
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
        let usage_contexts = usage::restore_pending_usage_contexts(&storage, &pending_actions)?;
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
        // A crash can lose the OS process handle after a Running receipt was committed. The
        // Session becomes outcome_unknown, but its command may still be modifying files outside
        // this Host. Preserve the deletion fence until a future explicit reconciliation flow can
        // acknowledge that uncertainty; never infer safety merely from losing process control.
        for session in unresolved_command_sessions {
            file_effects.restore_unsettled(
                session.snapshot.project_id.as_deref(),
                Some(&session.snapshot.conversation_id),
                &session.snapshot.origin_run_id,
                &pending_action_storage_id(
                    &session.snapshot.origin_run_id,
                    &session.snapshot.call_id,
                ),
            );
        }
        let startup_recoverable_mcp_approvals = pending_actions
            .iter()
            .filter(|(_, record)| {
                record.snapshot.status == PendingActionStatus::Approved
                    && matches!(
                        &record.snapshot.action,
                        AgentProposedAction::McpToolCall { approval }
                            if approval.approval_mode
                                == mycopilot_core::AgentMcpApprovalMode::Prompt
                    )
            })
            .map(|(storage_id, _)| storage_id.clone())
            .collect::<HashSet<_>>();
        let command_sessions = AgentCommandSessionRegistry::new(Arc::clone(&storage));
        let service = Self {
            storage,
            provider_continuation_vault,
            skills: Arc::new(SkillsService::new()),
            cancellations: Arc::new(Mutex::new(HashMap::new())),
            active_runs: Arc::new(Mutex::new(HashMap::new())),
            active_conversation_turns: Arc::new(Mutex::new(HashMap::new())),
            durable_turn_notifications: Arc::new(Mutex::new(HashMap::new())),
            turn_concurrency_gate,
            active_turn_permits: Arc::new(Mutex::new(HashMap::new())),
            pending_actions: Arc::new(Mutex::new(pending_actions)),
            startup_recoverable_mcp_approvals: Arc::new(Mutex::new(
                startup_recoverable_mcp_approvals,
            )),
            usage_contexts: Arc::new(Mutex::new(usage_contexts)),
            trace_snapshots: Arc::new(Mutex::new(HashMap::new())),
            running_context_window_snapshots: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_states: Arc::new(Mutex::new(HashMap::new())),
            conversation_context_state_clock: Arc::new(AtomicU64::new(1)),
            context_compaction_summary_generator: None,
            conversation_admission: Arc::new(Mutex::new(())),
            provider_transitions: Arc::new(Mutex::new(HashMap::new())),
            provider_transition_operations: Arc::new(Mutex::new(HashMap::new())),
            office_engine,
            image_generation_execution: None,
            skill_installation_prepare: None,
            skill_installation: None,
            skill_installation_service: None,
            skill_installation_workflow: None,
            artifact_runtime,
            mcp_tool_invoker: None,
            mcp_startup_inspector: None,
            mcp_approval_clock: Arc::new(now_ms),
            process_runs: CommandRunState::default(),
            command_sessions,
            deletion_lifecycle: Arc::new(Mutex::new(DeletionLifecycleState::default())),
            file_effects,
        };
        if reconcile_orphaned_traces {
            service.reconcile_startup_mcp_actions()?;
            service
                .storage
                .reconcile_orphaned_in_progress_conversation_turn_traces(
                    &HashSet::new(),
                    service.mcp_approval_now_ms(),
                )
                .map_err(|error| {
                    format!("failed to reconcile orphaned conversation traces: {error}")
                })?;
        }
        service.restore_durable_conversation_turn_occupancies()?;
        Ok(service)
    }

    #[cfg(test)]
    pub fn new(storage: Arc<StorageService>) -> Self {
        Self::try_new(storage).expect("agent service test fixture must initialize")
    }

    pub(crate) fn fork_conversation_view(
        &self,
        input: mycopilot_core::storage::models::ForkConversationRequest,
    ) -> Result<
        mycopilot_core::storage::models::ChatConversationViewRecord,
        mycopilot_core::storage::conversation_fork_repository::ConversationForkError,
    > {
        match self.provider_continuation_vault.as_deref() {
            Some(vault) => self
                .storage
                .fork_conversation_request_view_with_provider_continuation_vault(input, vault),
            None => self.storage.fork_conversation_request_view(input),
        }
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

    pub(crate) fn with_skill_installation(
        mut self,
        service: Arc<AgentSkillInstallationInspectionAdapter>,
        installation_service: Arc<SkillInstallationService>,
        workflow: Arc<SkillInstallationWorkflow>,
    ) -> Self {
        self.skill_installation_prepare = Some(service.clone());
        self.skill_installation = Some(service);
        self.skill_installation_service = Some(installation_service);
        self.skill_installation_workflow = Some(workflow);
        self
    }

    /// Installs the process-owned MCP invocation boundary for future Agent runs.
    ///
    /// Connections, transport configuration and credentials remain owned by core-server. Each
    /// run freezes only a bounded, protocol-neutral Tool catalog snapshot.
    pub(crate) fn with_mcp_tool_invoker(mut self, invoker: Arc<dyn McpToolInvoker>) -> Self {
        self.mcp_tool_invoker = Some(invoker);
        self
    }

    pub(crate) fn with_mcp_startup_inspector(
        mut self,
        inspector: Arc<dyn McpApprovalStartupInspector>,
    ) -> Self {
        self.mcp_startup_inspector = Some(inspector);
        self
    }

    #[cfg(test)]
    pub(super) fn with_mcp_approval_clock(
        mut self,
        clock: impl Fn() -> i64 + Send + Sync + 'static,
    ) -> Self {
        self.mcp_approval_clock = Arc::new(clock);
        self
    }

    fn mcp_approval_now_ms(&self) -> i64 {
        (self.mcp_approval_clock)()
    }

    /// Applies MCP-specific startup semantics before request admission.
    ///
    /// Pending/approved invocations are recoverable only when an authenticated durable payload is
    /// available. Process-only or missing payloads are definitely not dispatched. An `executing`
    /// row already crossed the durable dispatch boundary, so it is terminalized as outcome unknown
    /// without consulting or consuming the payload.
    pub(crate) fn reconcile_startup_mcp_actions(&self) -> Result<usize, String> {
        let now = self.mcp_approval_now_ms();
        let candidates = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions
                .iter()
                .filter(|(_, record)| {
                    matches!(
                        record.snapshot.action,
                        AgentProposedAction::McpToolCall { .. }
                    )
                })
                .map(|(storage_id, record)| (storage_id.clone(), record.clone()))
                .collect::<Vec<_>>()
        };

        let mut reconciled = 0_usize;
        for (storage_id, record) in candidates {
            let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                continue;
            };
            let terminal_outcome = match record.snapshot.status {
                PendingActionStatus::Executing => {
                    Some(McpStartupActionTerminalOutcome::OutcomeUnknown)
                }
                PendingActionStatus::Pending | PendingActionStatus::Approved => {
                    if approval.approval_mode == mycopilot_core::AgentMcpApprovalMode::Auto {
                        // Auto invocations never become resumable approvals. An `approved` journal
                        // row proves the durable dispatch CAS was not reached, so startup retires
                        // it without invoking the Server or exposing an approval prompt.
                        Some(McpStartupActionTerminalOutcome::PolicyDenied)
                    } else if approval.expires_at <= now {
                        Some(McpStartupActionTerminalOutcome::Expired)
                    } else {
                        match self
                            .mcp_startup_inspector
                            .as_ref()
                            .map(|inspector| inspector.inspect_startup_payload(approval))
                            .unwrap_or(McpApprovalStartupPayloadState::Unavailable)
                        {
                            McpApprovalStartupPayloadState::DurableAvailable => None,
                            McpApprovalStartupPayloadState::Expired => {
                                Some(McpStartupActionTerminalOutcome::Expired)
                            }
                            McpApprovalStartupPayloadState::Unavailable => {
                                Some(McpStartupActionTerminalOutcome::PayloadUnavailable)
                            }
                        }
                    }
                }
                PendingActionStatus::Rejected
                | PendingActionStatus::Cancelled
                | PendingActionStatus::Completed
                | PendingActionStatus::Failed => None,
            };
            let Some(terminal_outcome) = terminal_outcome else {
                continue;
            };
            let expected_status = pending_status_label(record.snapshot.status);
            if self
                .storage
                .terminalize_mcp_agent_action_on_startup(
                    &storage_id,
                    expected_status,
                    terminal_outcome,
                    now,
                )
                .map_err(|_| {
                    format!(
                        "failed to reconcile MCP startup action {}",
                        record.snapshot.action_id
                    )
                })?
            {
                self.pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&storage_id);
                self.startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&storage_id);
                reconciled = reconciled.saturating_add(1);
            }
        }
        self.storage
            .reconcile_mcp_approval_envelopes(now)
            .map_err(|_| "failed to reconcile MCP approval payload envelopes".to_string())?;
        Ok(reconciled)
    }

    /// Expires pre-dispatch MCP approvals against one injected wall-clock snapshot.
    ///
    /// The in-memory lock serializes this tick with AgentService approval transitions. SQLite
    /// still performs the authoritative status CAS so another process or a stale service cannot
    /// retire an action that crossed into `executing`. Payload invalidation runs only after the
    /// durable terminal transition commits.
    pub(crate) fn reconcile_expired_mcp_approvals(
        &self,
    ) -> Result<McpApprovalExpiryReconciliation, String> {
        let now = self.mcp_approval_now_ms();
        let mut summary = McpApprovalExpiryReconciliation {
            cutoff_ms: now,
            ..Default::default()
        };
        let (retired, storage_error) = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let candidates = pending_actions
                .iter()
                .filter_map(|(storage_id, record)| {
                    if !matches!(
                        record.snapshot.status,
                        PendingActionStatus::Pending | PendingActionStatus::Approved
                    ) {
                        return None;
                    }
                    let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action
                    else {
                        return None;
                    };
                    (approval.expires_at <= now).then(|| storage_id.clone())
                })
                .collect::<Vec<_>>();
            summary.candidates = candidates.len();
            let mut retired = Vec::with_capacity(candidates.len());
            let mut storage_error = None;
            for storage_id in candidates {
                let Some(record) = pending_actions.get(&storage_id).cloned() else {
                    continue;
                };
                let expected_status = pending_status_label(record.snapshot.status);
                let changed = match self.storage.terminalize_mcp_agent_action_on_startup(
                    &storage_id,
                    expected_status,
                    McpStartupActionTerminalOutcome::Expired,
                    now,
                ) {
                    Ok(changed) => changed,
                    Err(_) => {
                        storage_error = Some(format!(
                            "failed to expire MCP approval {}",
                            record.snapshot.action_id
                        ));
                        break;
                    }
                };
                if !changed {
                    summary.status_cas_conflicts = summary.status_cas_conflicts.saturating_add(1);
                    continue;
                }
                pending_actions.remove(&storage_id);
                self.startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&storage_id);
                summary.terminalized = summary.terminalized.saturating_add(1);
                retired.push(record);
            }
            (retired, storage_error)
        };

        for record in retired {
            summary.payload_invalidation_attempts =
                summary.payload_invalidation_attempts.saturating_add(1);
            let invalidated = self.mcp_tool_invoker.as_ref().is_some_and(|invoker| {
                let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                    return false;
                };
                invoker
                    .invalidate_prepared_approval(&approval.identity)
                    .is_ok()
            });
            if !invalidated {
                summary.payload_invalidation_failures =
                    summary.payload_invalidation_failures.saturating_add(1);
            }
        }
        if let Some(error) = storage_error {
            return Err(error);
        }
        Ok(summary)
    }

    /// Invalidates every active approval bound to one typed MCP Server/source selector.
    ///
    /// Pending and approved actions are definitely pre-dispatch and receive the requested safe
    /// terminal reason. Executing actions have crossed the durable dispatch boundary and are
    /// always terminalized as outcome-unknown. Durable rows and envelopes commit as one batch;
    /// process-owned payload deletion is attempted once per action after that commit.
    pub(crate) fn invalidate_mcp_actions_for_server(
        &self,
        target: &McpActionInvalidationTarget,
        reason: McpStartupActionTerminalOutcome,
    ) -> Result<McpActionInvalidationSummary, String> {
        let server_id = target.server_id.to_string();
        self.invalidate_mcp_actions_matching(
            reason,
            "failed to atomically invalidate MCP approvals for the selected Server",
            |approval| {
                let provenance = &approval.identity.provenance;
                provenance.server_id == server_id
                    && target
                        .scope
                        .as_ref()
                        .is_none_or(|scope| scope == &provenance.scope)
                    && target
                        .source_config_digest
                        .as_ref()
                        .is_none_or(|digest| digest.as_str() == provenance.config_digest)
                    && target
                        .source_config_epoch
                        .as_ref()
                        .is_none_or(|epoch| epoch.to_string() == provenance.config_epoch)
                    && target
                        .prior_to_registry_revision
                        .is_none_or(|revision| provenance.registry_revision < revision)
                    && target
                        .prior_to_catalog_generation
                        .is_none_or(|generation| provenance.catalog_generation < generation)
            },
        )
    }

    /// Invalidates all active MCP approvals after the Registry subscriber reports a gap.
    ///
    /// A current Registry snapshot cannot identify a configuration source which was removed in a
    /// skipped notification, so narrowing this operation would be unsafe.
    pub(crate) fn invalidate_all_mcp_actions(
        &self,
        reason: McpStartupActionTerminalOutcome,
    ) -> Result<McpActionInvalidationSummary, String> {
        self.invalidate_mcp_actions_matching(
            reason,
            "failed to atomically invalidate MCP approvals after a Registry event gap",
            |_| true,
        )
    }

    fn invalidate_mcp_actions_matching(
        &self,
        reason: McpStartupActionTerminalOutcome,
        storage_error: &'static str,
        matches_approval: impl Fn(&mycopilot_core::AgentMcpToolApproval) -> bool,
    ) -> Result<McpActionInvalidationSummary, String> {
        if !matches!(
            reason,
            McpStartupActionTerminalOutcome::PolicyDenied
                | McpStartupActionTerminalOutcome::PayloadUnavailable
        ) {
            return Err("invalid pre-dispatch MCP invalidation reason".to_string());
        }
        let now = self.mcp_approval_now_ms();
        let selected = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let selected = pending_actions
                .iter()
                .filter_map(|(storage_id, record)| {
                    let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action
                    else {
                        return None;
                    };
                    if !matches_approval(approval) {
                        return None;
                    }
                    let outcome = match record.snapshot.status {
                        PendingActionStatus::Pending | PendingActionStatus::Approved => reason,
                        PendingActionStatus::Executing => {
                            McpStartupActionTerminalOutcome::OutcomeUnknown
                        }
                        PendingActionStatus::Rejected
                        | PendingActionStatus::Cancelled
                        | PendingActionStatus::Completed
                        | PendingActionStatus::Failed => return None,
                    };
                    Some((storage_id.clone(), record.clone(), outcome))
                })
                .collect::<Vec<_>>();
            if selected.is_empty() {
                return Ok(McpActionInvalidationSummary::default());
            }
            let requests = selected
                .iter()
                .map(
                    |(storage_id, record, outcome)| McpActionTerminalizationRequest {
                        action_id: storage_id.clone(),
                        expected_status: pending_status_label(record.snapshot.status).to_string(),
                        outcome: *outcome,
                    },
                )
                .collect::<Vec<_>>();
            self.storage
                .terminalize_mcp_agent_actions(&requests, now)
                .map_err(|_| storage_error.to_string())?;
            for (storage_id, _, _) in &selected {
                pending_actions.remove(storage_id);
            }
            let mut recoverable = self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            for (storage_id, _, _) in &selected {
                recoverable.remove(storage_id);
            }
            selected
        };

        let mut summary = McpActionInvalidationSummary::default();
        for (_, record, outcome) in selected {
            match outcome {
                McpStartupActionTerminalOutcome::OutcomeUnknown => {
                    summary.terminalized_outcome_unknown =
                        summary.terminalized_outcome_unknown.saturating_add(1);
                }
                McpStartupActionTerminalOutcome::PayloadUnavailable
                | McpStartupActionTerminalOutcome::Expired
                | McpStartupActionTerminalOutcome::PolicyDenied
                | McpStartupActionTerminalOutcome::Rejected
                | McpStartupActionTerminalOutcome::Cancelled => {
                    summary.terminalized_before_dispatch =
                        summary.terminalized_before_dispatch.saturating_add(1);
                }
            }
            summary.payload_invalidation_attempts =
                summary.payload_invalidation_attempts.saturating_add(1);
            let invalidated = self.mcp_tool_invoker.as_ref().is_some_and(|invoker| {
                let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                    return false;
                };
                invoker
                    .invalidate_prepared_approval(&approval.identity)
                    .is_ok()
            });
            if !invalidated {
                summary.payload_invalidation_failures =
                    summary.payload_invalidation_failures.saturating_add(1);
            }
            self.process_runs.cancel(&record.storage_id);
            if let Some(token) = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&record.snapshot.run_id)
            {
                token.cancel();
            }
        }
        Ok(summary)
    }

    /// Settles process-bound approvals when the Host is about to discard their payload store.
    ///
    /// A future authenticated durable store reports `DurableAvailable` and is left untouched.
    /// Groups are narrowed by Server, scope and source digest so one configuration source cannot
    /// retire another source's pending approvals.
    pub(crate) fn invalidate_process_bound_mcp_actions_before_shutdown(
        &self,
    ) -> Result<McpActionInvalidationSummary, String> {
        let approvals = {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            pending_actions
                .values()
                .filter_map(|record| {
                    if !matches!(
                        record.snapshot.status,
                        PendingActionStatus::Pending | PendingActionStatus::Approved
                    ) {
                        return None;
                    }
                    let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action
                    else {
                        return None;
                    };
                    Some(approval.as_ref().clone())
                })
                .collect::<Vec<_>>()
        };

        let mut groups = Vec::<(
            McpActionInvalidationTarget,
            Vec<mycopilot_core::AgentMcpToolApproval>,
        )>::new();
        for approval in approvals {
            let provenance = &approval.identity.provenance;
            let server_id = provenance
                .server_id
                .parse::<McpServerId>()
                .map_err(|_| "active MCP approval has an invalid Server identity".to_string())?;
            let source_config_digest = provenance
                .config_digest
                .parse::<McpConfigDigest>()
                .map_err(|_| "active MCP approval has an invalid source digest".to_string())?;
            let target = McpActionInvalidationTarget::server(server_id)
                .with_scope(provenance.scope.clone())
                .with_source_config_digest(source_config_digest);
            match groups
                .iter_mut()
                .find(|(candidate, _)| candidate == &target)
            {
                Some((_, group)) => group.push(approval),
                None => groups.push((target, vec![approval])),
            }
        }

        let mut total = McpActionInvalidationSummary::default();
        for (target, approvals) in groups {
            let survives_restart = approvals.iter().any(|approval| {
                self.mcp_startup_inspector
                    .as_ref()
                    .is_some_and(|inspector| {
                        inspector.inspect_startup_payload(approval)
                            == McpApprovalStartupPayloadState::DurableAvailable
                    })
            });
            if survives_restart {
                continue;
            }
            let summary = self.invalidate_mcp_actions_for_server(
                &target,
                McpStartupActionTerminalOutcome::PayloadUnavailable,
            )?;
            total.terminalized_before_dispatch = total
                .terminalized_before_dispatch
                .saturating_add(summary.terminalized_before_dispatch);
            total.terminalized_outcome_unknown = total
                .terminalized_outcome_unknown
                .saturating_add(summary.terminalized_outcome_unknown);
            total.payload_invalidation_attempts = total
                .payload_invalidation_attempts
                .saturating_add(summary.payload_invalidation_attempts);
            total.payload_invalidation_failures = total
                .payload_invalidation_failures
                .saturating_add(summary.payload_invalidation_failures);
        }
        Ok(total)
    }

    fn capture_mcp_tool_runtime(&self, input: &AgentChatInput) -> Option<McpToolRuntime> {
        let project_id = input
            .context
            .as_ref()
            .and_then(|context| context.project_id.as_deref())
            .map(str::trim)
            .filter(|project_id| !project_id.is_empty())
            .map(str::to_string);
        self.mcp_tool_invoker.as_ref().map(|invoker| {
            McpToolRuntime::capture_for_context(
                Arc::clone(invoker),
                McpToolCatalogContext { project_id },
            )
        })
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
            let model_context_items = self
                .storage
                .get_conversation_model_context_log(&trace.assistant_message_id)?
                .ok_or_else(|| {
                    "failed to recover interrupted image ToolResult: immutable model context is missing"
                        .to_string()
                })?
                .items;
            let recovered = conversation_trace_snapshot_with_recovered_tool_result(
                &trace,
                model_context_items,
                &result,
            )
            .map_err(|error| {
                format!("failed to append interrupted image ToolResult audit: {error}")
            })?;
            let recovered_trace = recovered.in_progress_audit_trace(
                &trace.run_id,
                &trace.conversation_id,
                &trace.assistant_message_id,
            );
            let committed_at = now_ms();
            self.storage
                .append_in_progress_conversation_turn_trace_and_apply_guidances(
                    &recovered_trace,
                    &recovered.model_context_items,
                    committed_at,
                    committed_at,
                )
                .map_err(|error| {
                    format!("failed to persist interrupted image ToolResult audit: {error}")
                })?;
            reconciled += 1;
        }
        Ok(reconciled)
    }

    pub(crate) fn reconcile_startup_orphaned_conversation_traces(&self) -> Result<usize, String> {
        let reconciled = self
            .storage
            .reconcile_orphaned_in_progress_conversation_turn_traces(&HashSet::new(), now_ms())
            .map_err(|error| {
                format!("failed to reconcile orphaned conversation traces: {error}")
            })?;
        self.restore_durable_conversation_turn_occupancies()?;
        Ok(reconciled)
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
