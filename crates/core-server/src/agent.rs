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
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mycopilot_core::command::{
    run_authorized_command, AgentCommandExecutionResult, CommandAuthorizationSource,
    CommandExecutionError, CommandRunGuard, CommandRunState,
};
use mycopilot_core::file_write::{file_draft_snapshot, file_write_diff};
use mycopilot_core::skills::SkillsService;
use mycopilot_core::storage::models::{
    AgentActionAuditRecord, AgentPendingActionRecord, AgentUsageRecordInsert,
};
use mycopilot_core::storage::pending_action_repository::PendingActionStoreOutcome;
use mycopilot_core::storage::service::StorageService;
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
    AgentRunStatus, AgentRuntimeHostServices, AgentSearchConfig, AgentToolCall,
    AgentToolContinuation, AgentToolResult, AgentUsage, AgentUsageClearInput,
    AgentUsageClearOutput, AgentUsageSummaryInput, AgentUsageSummaryOutput, ContextJournalCursor,
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

#[cfg(test)]
use context_compaction::validate_compaction_model_visible_boundary;

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
    command_runs: CommandRunState,
    deleting_projects: Arc<Mutex<HashSet<String>>>,
}

impl AgentService {
    pub fn try_new(storage: Arc<StorageService>) -> Result<Self, String> {
        storage
            .reconcile_interrupted_pending_agent_actions(now_ms())
            .map_err(|error| format!("failed to reconcile interrupted pending actions: {error}"))?;
        let pending_actions = load_persisted_pending_actions(&storage)?;
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
            command_runs: CommandRunState::default(),
            deleting_projects: Arc::new(Mutex::new(HashSet::new())),
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

    #[cfg(test)]
    fn with_context_compaction_summary_generator(
        mut self,
        generator: ContextCompactionSummaryGenerator,
    ) -> Self {
        self.context_compaction_summary_generator = Some(generator);
        self
    }
}

#[cfg(test)]
mod tests;
