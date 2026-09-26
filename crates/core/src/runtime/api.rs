use super::*;
use crate::AgentRunCheckpoint;
use std::collections::{HashMap, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentSteerEnqueueOutcome {
    Queued,
    Duplicate,
    Closed,
}

#[derive(Debug)]
struct AgentSteerInputQueueState {
    accepting: bool,
    pending: VecDeque<crate::AgentSteerInput>,
    seen_by_client_message_id: HashMap<String, crate::AgentSteerInput>,
    client_message_id_by_guidance_id: HashMap<String, String>,
}

impl Default for AgentSteerInputQueueState {
    fn default() -> Self {
        Self {
            accepting: true,
            pending: VecDeque::new(),
            seen_by_client_message_id: HashMap::new(),
            client_message_id_by_guidance_id: HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentSteerInputQueue {
    state: Arc<Mutex<AgentSteerInputQueueState>>,
    changed: Arc<tokio::sync::Notify>,
}

pub(crate) enum AgentSteerDrainOrClose {
    Pending(Vec<crate::AgentSteerInput>),
    Closed,
}

impl AgentSteerInputQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether both handles refer to the same runtime steering queue.
    ///
    /// Hosts use this identity check to prevent a stale run-finalizer from removing a newer
    /// continuation queue that reuses the same run id after an approval boundary.
    pub fn is_same_queue(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.state, &other.state)
    }

    pub fn enqueue(&self, input: crate::AgentSteerInput) -> AgentResult<AgentSteerEnqueueOutcome> {
        self.enqueue_with(input, || {})
    }

    /// Enqueues one input and invokes `on_queued` before releasing the queue lock.
    ///
    /// Hosts use this to publish the durable `guidance_queued` acknowledgement before the
    /// runtime can drain the input and publish `guidance_applied`.
    pub fn enqueue_with(
        &self,
        input: crate::AgentSteerInput,
        on_queued: impl FnOnce(),
    ) -> AgentResult<AgentSteerEnqueueOutcome> {
        validate_steer_input(&input)?;
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if !state.accepting {
            return Ok(AgentSteerEnqueueOutcome::Closed);
        }
        if let Some(existing) = state
            .seen_by_client_message_id
            .get(&input.client_message_id)
        {
            return if existing == &input {
                Ok(AgentSteerEnqueueOutcome::Duplicate)
            } else {
                Err(AgentError::structured(
                    "agent.steer_client_message_conflict",
                    "同一个 clientMessageId 不能表示不同的用户引导。",
                    json!({
                        "clientMessageId": input.client_message_id,
                        "existingGuidanceId": existing.guidance_id,
                        "candidateGuidanceId": input.guidance_id,
                    }),
                ))
            };
        }
        if let Some(existing_client_message_id) = state
            .client_message_id_by_guidance_id
            .get(&input.guidance_id)
        {
            return Err(AgentError::structured(
                "agent.steer_guidance_id_conflict",
                "同一个 guidanceId 不能关联不同的 clientMessageId。",
                json!({
                    "guidanceId": input.guidance_id,
                    "existingClientMessageId": existing_client_message_id,
                    "candidateClientMessageId": input.client_message_id,
                }),
            ));
        }

        state
            .client_message_id_by_guidance_id
            .insert(input.guidance_id.clone(), input.client_message_id.clone());
        state
            .seen_by_client_message_id
            .insert(input.client_message_id.clone(), input.clone());
        state.pending.push_back(input);
        on_queued();
        self.changed.notify_waiters();
        Ok(AgentSteerEnqueueOutcome::Queued)
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.accepting = false;
        self.changed.notify_waiters();
    }

    /// Transfers pending inputs and their identities to the next segment of the same Run.
    /// The retiring segment keeps a closed, empty queue, so its Drop guard cannot close the
    /// successor. Hosts serialize this handoff with admission through their Run registry lock.
    pub fn handoff_pending(&self) -> Self {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let successor = Self {
            state: Arc::new(Mutex::new(AgentSteerInputQueueState {
                accepting: state.accepting,
                pending: std::mem::take(&mut state.pending),
                seen_by_client_message_id: std::mem::take(&mut state.seen_by_client_message_id),
                client_message_id_by_guidance_id: std::mem::take(
                    &mut state.client_message_id_by_guidance_id,
                ),
            })),
            changed: Arc::new(tokio::sync::Notify::new()),
        };
        state.accepting = false;
        self.changed.notify_waiters();
        successor
    }

    /// Atomically stops admission and returns every input that was accepted but not yet drained.
    pub fn close_and_take_pending(&self) -> Vec<crate::AgentSteerInput> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.accepting = false;
        state.pending.drain(..).collect()
    }

    pub fn is_accepting(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .accepting
    }

    pub fn pending_len(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .pending
            .len()
    }

    pub(crate) fn drain_pending(&self) -> Vec<crate::AgentSteerInput> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.pending.drain(..).collect()
    }

    /// One-shot notification used by independent Host waits. Durable guidance remains in the
    /// queue; this signal only allows a wait to yield promptly without polling.
    pub async fn changed(&self) {
        self.changed.notified().await;
    }

    pub(crate) fn take_pending_or_close(&self) -> AgentSteerDrainOrClose {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.pending.is_empty() {
            state.accepting = false;
            AgentSteerDrainOrClose::Closed
        } else {
            AgentSteerDrainOrClose::Pending(state.pending.drain(..).collect())
        }
    }
}

fn validate_steer_input(input: &crate::AgentSteerInput) -> AgentResult<()> {
    if input.guidance_id.trim().is_empty()
        || input.client_message_id.trim().is_empty()
        || (input.content.trim().is_empty()
            && input.attachments.is_empty()
            && input.folder_references.is_empty())
        || input.created_at < 0
    {
        return Err(AgentError::structured(
            "agent.invalid_steer_input",
            "用户引导缺少有效的身份、正文、附件或文件夹，或创建时间无效。",
            json!({
                "guidanceIdPresent": !input.guidance_id.trim().is_empty(),
                "clientMessageIdPresent": !input.client_message_id.trim().is_empty(),
                "contentPresent": !input.content.trim().is_empty(),
                "attachmentsPresent": !input.attachments.is_empty(),
                "folderReferencesPresent": !input.folder_references.is_empty(),
                "createdAt": input.created_at,
            }),
        ));
    }
    Ok(())
}

pub type AgentEventEmitter = Arc<dyn Fn(AgentEvent) + Send + Sync + 'static>;
pub type AgentConversationTraceObserver = Arc<
    dyn Fn(ConversationTraceSnapshot) -> AgentResult<Option<AgentContextBaseline>>
        + Send
        + Sync
        + 'static,
>;
pub type AgentModelRequestObserver = Arc<dyn Fn(ModelRequestObservation) + Send + Sync + 'static>;
pub type AgentContextWindowObserver =
    Arc<dyn Fn(AgentContextWindowSnapshot) + Send + Sync + 'static>;

/// Trusted request emitted only at the one boundary immediately before an Agent-loop provider
/// sample is assembled. Implementations must bind durable Mailbox facts to this exact batch before
/// returning any model-visible input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSamplingBoundaryRequest {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
    pub expected_next_trace_sequence: u64,
}

/// A request-boundary projection from the same frozen observations used for tools and guidance.
/// The Host binds this port to an admitted conversation/run; rendered state is never authority.
#[derive(Debug, Clone)]
pub struct AgentConversationWorldStateRequest {
    pub conversation_id: String,
    pub boundary: crate::WorldStateRequestBoundary,
    pub sections: Vec<crate::WorldStateSectionEnvelope>,
}

/// Required durable state writes are separate from best-effort model usage diagnostics.
pub trait AgentConversationWorldStateHost: Send + Sync {
    fn prepare_request(
        &self,
        request: AgentConversationWorldStateRequest,
    ) -> AgentResult<Vec<crate::AnchoredWorldStateRecord>>;

    /// A successful Provider response confirms adoption of the prepared state prefix. An error
    /// here must stop continuation, never retry an already completed Provider request.
    fn mark_request_observed(&self, boundary: &crate::WorldStateRequestBoundary)
        -> AgentResult<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSamplingBoundaryMessage {
    pub trace_sequence: u64,
    pub message_id: String,
    pub sender_agent_id: String,
    pub sender_task_name: String,
    pub sender_task_path: String,
    pub kind: crate::AgentMailboxKind,
    pub content: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentSamplingBoundaryDelivery {
    pub receipt_id: String,
    pub messages: Vec<AgentSamplingBoundaryMessage>,
}

/// Narrow Host port for collaboration delivery. It is intentionally neither a Tool executor nor
/// a ContextAssembler: Core asks once per model batch, and the Host atomically projects/binds the
/// caller's persistent inbox and appends the corresponding Turn trace/model-context prefix.
pub trait AgentSamplingBoundaryInbox: Send + Sync {
    fn bind_for_model_batch(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Option<AgentSamplingBoundaryDelivery>>;
}
pub type AgentHostActionExecutor = Arc<
    dyn Fn(
            AgentProposedAction,
            Option<AgentRunCheckpoint>,
            AgentCancellationToken,
        ) -> AgentResult<AgentToolResult>
        + Send
        + Sync
        + 'static,
>;

/// Default Host-owned quiet wait for a command Session terminal state.
pub const AGENT_COMMAND_SESSION_DEFAULT_WAIT_MS: u64 = 120_000;
/// Default bounded settlement wait after a controlled interrupt.
pub const AGENT_COMMAND_SESSION_INTERRUPT_WAIT_MS: u64 = 5_000;
/// Hard upper bound for Host-controlled Session observation waits.
pub const AGENT_COMMAND_SESSION_MAX_WAIT_MS: u64 = 300_000;
/// Maximum incremental command output admitted by one model Tool call.
///
/// Keeping this below the central Tool-result budget is important: the Host advances the model's
/// Session cursor when it returns this output, so a later generic projection must never silently
/// discard bytes which the model can no longer poll again.
pub const AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCommandSessionAction {
    Poll,
    Interrupt,
}

/// Trusted invocation passed to the Host-owned command Session registry.
///
/// Conversation, run and Tool-call identities are injected by the runtime. They are deliberately
/// absent from the model-authored schema and must be used by the Host for ownership checks and
/// audit attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandSessionExecutionRequest {
    pub conversation_id: String,
    pub run_id: String,
    pub call_id: String,
    pub session_id: String,
    pub action: AgentCommandSessionAction,
    pub wait_ms: u64,
    pub max_output_bytes: usize,
}

/// Ephemeral run controls for one Host-owned command Session observation.
///
/// These controls stay separate from [`AgentCommandSessionExecutionRequest`] so the request
/// remains a stable, comparable identity/audit value. Releasing an observation is not authority
/// to terminate an already handed-off process.
#[derive(Clone, Debug)]
pub struct AgentCommandSessionExecutionControl {
    cancellation_token: AgentCancellationToken,
    steer_input: Option<AgentSteerInputQueue>,
}

impl AgentCommandSessionExecutionControl {
    pub fn new(
        cancellation_token: AgentCancellationToken,
        steer_input: Option<AgentSteerInputQueue>,
    ) -> Self {
        Self {
            cancellation_token,
            steer_input,
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancellation_token.is_cancelled()
    }

    pub fn has_pending_guidance(&self) -> bool {
        self.steer_input
            .as_ref()
            .is_some_and(|queue| queue.pending_len() > 0)
    }
}

/// Incremental output returned from the Host's one authoritative model-read cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommandSessionExecutionOutput {
    pub session_id: String,
    pub status: crate::protocol::AgentCommandSessionStatus,
    pub output: String,
    pub exit_code: Option<i32>,
    pub requested_after_sequence: u64,
    pub first_output_sequence: Option<u64>,
    pub last_output_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub truncated_before: bool,
    pub output_truncated: bool,
    pub outputs: Vec<crate::command::AgentCommandPublishedOutput>,
    /// Terminal-only bounded Office file-effect evidence from the authoritative Host Session.
    pub artifact_observation: Option<crate::protocol::AgentCommandArtifactObservation>,
    /// Opaque, conversation-bound recovery route for the authoritative terminal output Archive.
    /// Running Sessions never expose a route and raw Archive references never cross this boundary.
    pub history_open: Option<String>,
}

/// Narrow Host capability used by the model-facing `command_session` Tool.
///
/// The implementation owns process handles, durable ownership, transcript cursors and
/// conversation isolation. It must serialize interactions for the same Session while permitting
/// unrelated Sessions to progress independently. Core supplies only a validated request and
/// receives a bounded, presentation-safe projection.
pub trait AgentCommandSessionExecutor: Send + Sync {
    fn execute_command_session(
        &self,
        request: AgentCommandSessionExecutionRequest,
        control: AgentCommandSessionExecutionControl,
    ) -> AgentResult<AgentCommandSessionExecutionOutput>;
}

pub type AgentSkillActivationResolver = Arc<
    dyn Fn(&crate::skills::SkillSelection) -> AgentResult<AgentResolvedSkillActivation>
        + Send
        + Sync
        + 'static,
>;

#[derive(Clone)]
pub struct AgentResolvedSkillActivation {
    pub skill: crate::protocol::AgentActivatedSkill,
    pub resources: Arc<crate::skills::SkillResourceSession>,
}

impl std::fmt::Debug for AgentResolvedSkillActivation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentResolvedSkillActivation")
            .field("skill", &self.skill)
            .field("resource_packages", &self.resources.package_uris())
            .finish()
    }
}

/// Optional capabilities supplied by the process that hosts the agent runtime.
///
/// Keeping these dependencies in one value prevents the runtime entry point from growing a new
/// positional parameter for every durable-state or orchestration capability.
#[derive(Clone, Default)]
pub struct AgentRuntimeHostServices {
    pub(super) conversation_world_state: Option<Arc<dyn AgentConversationWorldStateHost>>,
    pub(super) web_search_policy: Option<Arc<dyn crate::WebSearchPolicySource>>,
    pub(super) host_executor: Option<AgentHostActionExecutor>,
    pub(super) storage: Option<Arc<StorageService>>,
    pub(super) trace_observer: Option<AgentConversationTraceObserver>,
    pub(super) model_request_observer: Option<AgentModelRequestObserver>,
    pub(super) context_window_observer: Option<AgentContextWindowObserver>,
    pub(super) context_compaction_services: Option<AgentContextCompactionServices>,
    pub(super) provider_continuation_vault: Option<Arc<crate::ProviderContinuationVault>>,
    pub(super) skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
    pub(super) skill_activation_resolver: Option<AgentSkillActivationResolver>,
    pub(super) office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    pub(super) image_generation_execution:
        Option<Arc<crate::image_generation::ImageGenerationExecutionService>>,
    pub(super) skill_installation_prepare:
        Option<Arc<dyn crate::tools::AgentSkillInstallationPrepareExecutor>>,
    pub(super) skill_installation_commit:
        Option<Arc<dyn crate::tools::AgentSkillInstallationCommitPreparer>>,
    pub(super) mcp_tools: Option<crate::tools::McpToolRuntime>,
    pub(super) builtin_capabilities: Option<crate::BuiltinCapabilityRuntime>,
    pub(super) command_runtime_profile_resolver:
        Option<Arc<dyn crate::command::CommandRuntimeProfileResolver>>,
    pub(super) command_session_executor: Option<Arc<dyn AgentCommandSessionExecutor>>,
    pub(super) steer_input: Option<AgentSteerInputQueue>,
    pub(super) workflow_inbox: Option<Arc<dyn crate::AgentWorkflowInbox>>,
    pub(super) collaboration_inbox: Option<Arc<dyn AgentSamplingBoundaryInbox>>,
    pub(super) agent_collaboration: Option<crate::AgentCollaborationRuntimeServices>,
    pub(super) agent_collaboration_policy: Option<Arc<dyn crate::AgentCollaborationPolicySource>>,
    pub(super) automation_report_sink: Option<Arc<dyn crate::AutomationReportSink>>,
    pub(super) workflow_runtime: Option<Arc<dyn crate::WorkflowRuntimeHost>>,
    pub(super) human_interaction_policy: Option<Arc<dyn HumanInteractionPolicySource>>,
    pub(super) human_interaction_runtime: Option<Arc<dyn AgentHumanInteractionRuntimeHost>>,
    // Read-only capacity projection only. The driver deliberately ignores these flags and
    // requires the real durable runtime port before granting execution.
    pub(super) human_interaction_preview_readiness: Option<(bool, bool)>,
    pub(super) user_input_resume: Option<AgentUserInputResume>,
}

/// Private segment boundary. The Host must atomically save the question, checkpoint and usage
/// before returning success. Neither this payload nor its checkpoint is a Renderer event.
#[derive(Debug, Clone)]
pub struct AgentUserInputSuspension {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub call: AgentToolCall,
    pub questions: crate::human_interaction::HumanInteractionToolInput,
    pub checkpoint: AgentRunCheckpoint,
    pub segment_usage: Option<crate::AgentUsage>,
}

pub trait AgentHumanInteractionRuntimeHost: Send + Sync {
    fn suspend(&self, suspension: AgentUserInputSuspension) -> AgentResult<()>;

    /// Only a Host with durable admission and answer delivery may expose the asynchronous tool.
    /// Synchronous-only hosts keep this closed regardless of the user's question setting.
    fn async_execution_ready(&self) -> bool {
        false
    }

    /// Commit admission before returning the generated request ID. Exact owner/call retries must
    /// resolve to the original batch; a successful return is the sole authoritative ToolResult.
    /// The Host rechecks the current setting in its admission transaction.
    fn accept_async(
        &self,
        _request: AgentAsyncUserInputRequest,
    ) -> AgentResult<AgentAsyncUserInputAccepted> {
        Err(AgentError::new(
            "Asynchronous human questions are unavailable.",
        ))
    }

    /// Atomically bind pending ignored-question facts to this already planned model boundary.
    /// Persist the trace/model projection before returning. Never claim answers, create guidance,
    /// signal a Wake, or schedule inference here. Previously bound events are normal history.
    fn bind_ignored_events(
        &self,
        _request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<AgentHumanInteractionIgnoredEvent>> {
        Ok(Vec::new())
    }
}

/// Native ownership is injected by the runtime; the model authored only `questions`.
#[derive(Debug, Clone)]
pub struct AgentAsyncUserInputRequest {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub call: AgentToolCall,
    pub questions: crate::human_interaction::HumanInteractionToolInput,
}

#[derive(Debug, Clone)]
pub struct AgentAsyncUserInputAccepted {
    pub request_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentHumanInteractionIgnoredEvent {
    pub trace_sequence: u64,
    pub event_id: String,
    pub request_id: String,
    pub created_at: i64,
}

/// A Host-authenticated response already claimed for this exact suspended call. Deliberately
/// has no serde implementation: model input and Renderer RPC cannot manufacture resume authority.
#[derive(Debug, Clone)]
pub struct AgentUserInputResume {
    pub request_id: String,
    pub response_id: String,
    pub checkpoint: AgentRunCheckpoint,
    pub continuation: crate::AgentToolContinuation,
}

/// Live Host-owned policy for human questions. This is not model/Renderer input and is never
/// restored from a checkpoint as authorization. The runtime freezes one snapshot per request;
/// the Host independently checks current policy in the durable admission transaction.
/// Supplying policy does not enable execution: the durable runtime port is also required.
pub trait HumanInteractionPolicySource: Send + Sync {
    fn snapshot(&self) -> AgentResult<crate::human_interaction::HumanInteractionSettings>;
}

impl AgentRuntimeHostServices {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_conversation_world_state(
        mut self,
        host: Arc<dyn AgentConversationWorldStateHost>,
    ) -> Self {
        self.conversation_world_state = Some(host);
        self
    }

    /// Supplies live trusted web settings for every request and new execution. When present,
    /// model/Renderer-provided search configuration cannot replace this authority.
    pub fn with_web_search_policy(mut self, source: Arc<dyn crate::WebSearchPolicySource>) -> Self {
        self.web_search_policy = Some(source);
        self
    }

    pub fn with_workflow_runtime(mut self, host: Arc<dyn crate::WorkflowRuntimeHost>) -> Self {
        self.workflow_runtime = Some(host);
        self
    }

    pub fn with_workflow_inbox(mut self, inbox: Arc<dyn crate::AgentWorkflowInbox>) -> Self {
        self.workflow_inbox = Some(inbox);
        self
    }

    pub fn with_human_interaction_runtime(
        mut self,
        host: Arc<dyn AgentHumanInteractionRuntimeHost>,
    ) -> Self {
        self.human_interaction_runtime = Some(host);
        self
    }

    /// Describes the eventual Host's question handlers without constructing executable ports.
    /// This is consumed only by `prepare_context_window_tool_projection`.
    pub fn with_human_interaction_preview_readiness(
        mut self,
        blocking: bool,
        asynchronous: bool,
    ) -> Self {
        self.human_interaction_preview_readiness = Some((blocking, blocking && asynchronous));
        self
    }

    pub fn with_user_input_resume(mut self, resume: AgentUserInputResume) -> Self {
        self.user_input_resume = Some(resume);
        self
    }

    /// Installs the policy source for a human-owned root run. Capability preparation additionally
    /// excludes child and automation inputs, even if a Host accidentally passes this service.
    pub fn with_human_interaction_policy(
        mut self,
        policy: Arc<dyn HumanInteractionPolicySource>,
    ) -> Self {
        self.human_interaction_policy = Some(policy);
        self
    }

    pub fn with_host_actions(
        mut self,
        host_executor: AgentHostActionExecutor,
        storage: Arc<StorageService>,
    ) -> Self {
        self.host_executor = Some(host_executor);
        self.storage = Some(storage);
        self
    }

    pub fn with_storage(mut self, storage: Arc<StorageService>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_trace_observer(mut self, observer: AgentConversationTraceObserver) -> Self {
        self.trace_observer = Some(observer);
        self
    }

    pub fn with_model_request_observer(mut self, observer: AgentModelRequestObserver) -> Self {
        self.model_request_observer = Some(observer);
        self
    }

    /// Receives exact aggregate request-capacity accounting after runtime extensions, Run World
    /// State, Skill instructions and dynamic Tool schemas are assembled.
    /// No Skill instruction or Tool schema body crosses this observer boundary.
    pub fn with_context_window_observer(mut self, observer: AgentContextWindowObserver) -> Self {
        self.context_window_observer = Some(observer);
        self
    }

    pub fn with_context_compaction(mut self, services: AgentContextCompactionServices) -> Self {
        self.context_compaction_services = Some(services);
        self
    }

    /// Supplies the Host-private encrypted vault used by provider-native Assistant Turn replay.
    /// Raw continuation state never crosses this runtime service boundary.
    pub fn with_provider_continuation_vault(
        mut self,
        vault: Arc<crate::ProviderContinuationVault>,
    ) -> Self {
        self.provider_continuation_vault = Some(vault);
        self
    }

    /// Supplies the exact, run-scoped Skill resource authority resolved by the
    /// host. Resource bytes and managed-store paths remain outside Agent input
    /// and checkpoints.
    pub fn with_skill_resources(
        mut self,
        resources: Arc<crate::skills::SkillResourceSession>,
    ) -> Self {
        self.skill_resources = Some(resources);
        self
    }

    /// Supplies one immutable MCP catalog snapshot for this run boundary.
    ///
    /// The process host owns connections and credentials; the core runtime only receives
    /// provider-neutral descriptors and an invocation capability.
    pub fn with_mcp_tools(mut self, mcp_tools: crate::tools::McpToolRuntime) -> Self {
        self.mcp_tools = Some(mcp_tools);
        self
    }

    /// Supplies Host-owned manifests, policy and process-memory grants for built-in capabilities.
    pub fn with_builtin_capabilities(mut self, runtime: crate::BuiltinCapabilityRuntime) -> Self {
        self.builtin_capabilities = Some(runtime);
        self
    }

    /// Enables the closed, side-effect-free report channel for one automation-owned logical run.
    pub fn with_automation_report_sink(
        mut self,
        sink: Arc<dyn crate::AutomationReportSink>,
    ) -> Self {
        self.automation_report_sink = Some(sink);
        self
    }

    /// Supplies the trusted, host-owned resolver for one exact Skill selection. The model never
    /// receives this capability directly; `skills_activate` can only address selections frozen in
    /// the run's discovery snapshot.
    pub fn with_skill_activation_resolver(
        mut self,
        resolver: AgentSkillActivationResolver,
    ) -> Self {
        self.skill_activation_resolver = Some(resolver);
        self
    }

    /// Supplies the application-owned Office provider used by both read-only
    /// runtime tools and approval-gated Host operations. The provider remains
    /// outside model input and persisted checkpoints.
    pub fn with_office_engine(mut self, engine: Arc<dyn crate::office::OfficeEngine>) -> Self {
        self.office_engine = Some(engine);
        self
    }

    /// Supplies the application-owned image-generation execution service used by the typed Agent
    /// tool. Provider configuration, credentials, transport, and Artifact publication remain
    /// outside model input and persisted checkpoints.
    pub fn with_image_generation_execution(
        mut self,
        service: Arc<crate::image_generation::ImageGenerationExecutionService>,
    ) -> Self {
        self.image_generation_execution = Some(service);
        self
    }

    /// Supplies the process-owned, read-only Skill installation inspection boundary. The service
    /// owns source resolution and opaque preparation references; only its presentation-safe result
    /// crosses into the model-facing Tool result.
    pub fn with_skill_installation_prepare(
        mut self,
        service: Arc<dyn crate::tools::AgentSkillInstallationPrepareExecutor>,
    ) -> Self {
        self.skill_installation_prepare = Some(service);
        self
    }

    pub fn with_skill_installation_commit(
        mut self,
        service: Arc<dyn crate::tools::AgentSkillInstallationCommitPreparer>,
    ) -> Self {
        self.skill_installation_commit = Some(service);
        self
    }

    /// Supplies the trusted approval-time resolver for model-visible Artifact Runtime profiles.
    /// Runtime bindings stay outside model input until the host has verified and frozen them.
    pub fn with_command_runtime_profile_resolver(
        mut self,
        resolver: Arc<dyn crate::command::CommandRuntimeProfileResolver>,
    ) -> Self {
        self.command_runtime_profile_resolver = Some(resolver);
        self
    }

    /// Supplies the Host-owned, conversation-isolated command Session control boundary.
    pub fn with_command_session_executor(
        mut self,
        executor: Arc<dyn AgentCommandSessionExecutor>,
    ) -> Self {
        self.command_session_executor = Some(executor);
        self
    }

    pub fn with_steer_input(mut self, input: AgentSteerInputQueue) -> Self {
        self.steer_input = Some(input);
        self
    }

    pub fn with_collaboration_inbox(mut self, inbox: Arc<dyn AgentSamplingBoundaryInbox>) -> Self {
        self.collaboration_inbox = Some(inbox);
        self
    }

    /// Supplies authenticated collaboration services. Execution and preview retain them only
    /// when the Host's policy bound to this logical run enables collaboration, including resume.
    pub fn with_agent_collaboration(
        mut self,
        services: crate::AgentCollaborationRuntimeServices,
    ) -> Self {
        self.agent_collaboration = Some(services);
        self
    }

    pub fn with_agent_collaboration_policy(
        mut self,
        source: Arc<dyn crate::AgentCollaborationPolicySource>,
    ) -> Self {
        self.agent_collaboration_policy = Some(source);
        self
    }
}

/// Extracts authority-bearing immutable Skill selections from the versioned Skill extension state
/// in an approval checkpoint. Hosts use this before resuming so managed Skill identity and any
/// model-activated resource authority are restored from exact package revisions rather than
/// mutable installation receipts. The host must additionally prove that selections not present in
/// the explicit activation belong to the same run's frozen discovery snapshot before opening a
/// package.
pub fn skill_resource_selections_from_checkpoint(
    checkpoint: &crate::protocol::AgentRunCheckpoint,
) -> AgentResult<Vec<crate::skills::SkillSelection>> {
    Ok(skill_checkpoint_authority(checkpoint)?
        .map(|authority| authority.resource_selections)
        .unwrap_or_default())
}

/// Trusted Skill authority frozen at an approval boundary.
///
/// A resumed run must use this catalog and these immutable package revisions instead of accepting
/// replacement Skill metadata from the continuation payload or current installation receipts.
#[derive(Debug, Clone)]
pub struct AgentSkillCheckpointAuthority {
    pub discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    pub resource_selections: Vec<crate::skills::SkillSelection>,
}

pub fn skill_checkpoint_authority(
    checkpoint: &crate::protocol::AgentRunCheckpoint,
) -> AgentResult<Option<AgentSkillCheckpointAuthority>> {
    Ok(
        super::extensions::checkpoint_authority_from_snapshots(&checkpoint.extension_snapshots)?
            .map(
                |(discovery, resource_selections)| AgentSkillCheckpointAuthority {
                    discovery,
                    resource_selections,
                },
            ),
    )
}

/// Removes the run-scoped discovery catalog from a terminal checkpoint while retaining bounded
/// activation summaries for audit. Unknown Skill extension versions are dropped fail-closed.
pub fn redact_terminal_skill_discovery(checkpoint: &mut crate::protocol::AgentRunCheckpoint) {
    super::extensions::redact_discovery_from_snapshots(&mut checkpoint.extension_snapshots);
}

pub use super::context_compaction::{
    AgentContextCompactionCommitOutcome, AgentContextCompactionCommitRequest,
    AgentContextCompactionGenerationOutput, AgentContextCompactionGenerationRequest,
    AgentContextCompactionPrepareOutcome, AgentContextCompactionPrepareRequest,
    AgentContextCompactionServices,
};
pub use super::context_compaction_model::{
    estimate_provider_transition_compaction_source_tokens, AgentContextCompactionModelGenerator,
};

pub async fn send_chat(input: AgentChatInput) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default().send_chat(input).await
}

pub async fn send_chat_with_events(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
) -> AgentResult<AgentChatOutput> {
    send_chat_with_events_and_cancellation(input, run_id, emitter, AgentCancellationToken::new())
        .await
}

pub async fn send_chat_with_events_and_cancellation(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            None,
        )
        .await
}

pub async fn send_chat_with_host_executor(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
    host_executor: AgentHostActionExecutor,
    storage: Arc<StorageService>,
) -> AgentResult<AgentChatOutput> {
    send_chat_with_host_services(
        input,
        run_id,
        emitter,
        cancellation_token,
        AgentRuntimeHostServices::new().with_host_actions(host_executor, storage),
    )
    .await
}

pub async fn send_chat_with_host_services(
    input: AgentChatInput,
    run_id: String,
    emitter: AgentEventEmitter,
    cancellation_token: AgentCancellationToken,
    host_services: AgentRuntimeHostServices,
) -> AgentResult<AgentChatOutput> {
    AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(run_id),
            Some(emitter),
            cancellation_token,
            Some(host_services),
        )
        .await
}

pub fn next_run_id() -> String {
    generate_run_id()
}

pub fn inspect_context_window(
    mut input: AgentChatInput,
) -> AgentResult<Option<AgentContextWindowSnapshot>> {
    let skill_discovery = input.skill_discovery.take();
    let skill_activation = input.skill_activation.take();
    let mut state = create_conversation_context_state(input)?;
    state
        .snapshot_with_skill_overlays(skill_discovery.as_ref(), skill_activation.as_ref())
        .map(Some)
}

/// Inspects a context window with the exact Host-projected Skill-gated Tool suffix.
///
/// The projection is intentionally opaque: it binds dynamic schemas to the same trusted provider,
/// permissions and revision-checked Skill resource authority used by a real run. This entry point
/// also accounts for the backend-authored Run World State snapshot; callers must not attempt to
/// approximate the request by passing arbitrary schema lists.
pub fn inspect_context_window_with_tool_projection(
    mut input: AgentChatInput,
    projection: &AgentContextWindowToolProjection,
) -> AgentResult<Option<AgentContextWindowSnapshot>> {
    let skill_discovery = input.skill_discovery.take();
    let skill_activation = input.skill_activation.take();
    let mut state = create_conversation_context_state(input)?;
    state
        .snapshot_with_skill_overlays_and_tool_projection(
            skill_discovery.as_ref(),
            skill_activation.as_ref(),
            projection,
        )
        .map(Some)
}

/// Projects the exact initial Skill-gated Tool contract for a context-window preview.
///
/// This helper performs the same permission filtering, provider registration and Host resource
/// authority validation as a real run. It deliberately requires explicit Host services instead
/// of treating the presentation-safe `AgentSkillActivation.resources` hints as authority.
/// `host_actions_available` must describe the execution boundary that the eventual run will use.
/// It can affect approval projection for dynamic Tools, but never the configuration-stable Tool
/// prefix.
pub fn prepare_context_window_tool_projection(
    input: &AgentChatInput,
    host_services: &AgentRuntimeHostServices,
    host_actions_available: bool,
) -> AgentResult<AgentContextWindowToolProjection> {
    let (human_ready, human_async_ready) = host_services
        .human_interaction_runtime
        .as_ref()
        .map(|host| (true, host.async_execution_ready()))
        .or(host_services.human_interaction_preview_readiness)
        .unwrap_or((false, false));
    let extension_snapshots = input
        .resume_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.extension_snapshots.as_slice())
        .unwrap_or_default();
    let FrozenCollaborationServices {
        services: agent_collaboration,
        policy: agent_collaboration_policy,
    } = freeze_collaboration_runtime_services(
        host_services.agent_collaboration.clone(),
        host_services.agent_collaboration_policy.clone(),
    )?;
    let agent_collaboration = match input.resume_checkpoint.as_ref() {
        Some(checkpoint) => restore_collaboration_runtime_services(
            agent_collaboration,
            checkpoint.collaboration_run_snapshot.as_ref(),
        )?,
        None => agent_collaboration,
    };
    let capabilities = prepare_runtime_capabilities_with_skills(
        input,
        "context-window-tool-preview",
        extension_snapshots,
        RuntimeCapabilityServices {
            web_search_policy: host_services.web_search_policy.clone(),
            host_actions_available,
            office_engine: host_services.office_engine.clone(),
            image_generation_execution: host_services.image_generation_execution.clone(),
            skill_installation_prepare: host_services.skill_installation_prepare.clone(),
            skill_installation_commit: host_services.skill_installation_commit.clone(),
            skill_activation_resolver: host_services.skill_activation_resolver.clone(),
            skill_resources: host_services.skill_resources.clone(),
            mcp_tools: host_services.mcp_tools.clone(),
            builtin_capabilities: host_services.builtin_capabilities.clone(),
            agent_collaboration,
            agent_collaboration_policy,
            automation_report_sink: host_services.automation_report_sink.clone(),
            workflow_runtime: host_services.workflow_runtime.clone(),
            human_interaction_policy: host_services.human_interaction_policy.clone(),
            human_interaction_execution_ready: human_ready,
            human_interaction_async_execution_ready: human_async_ready,
        },
    )?;
    let initial_run_world_state = RunWorldStateTracker::new_with_extension_sections(
        "context-window-tool-preview:world-state",
        input,
        &capabilities.initial_tool_set,
        capabilities.runtime_extensions.world_state_sections()?,
    )?
    .snapshot()
    .clone();
    let conversation_sections = MemoryConversationWorldState::new(input)?.preview_sections(
        capabilities
            .runtime_extensions
            .conversation_world_state_sections()?,
    )?;
    let mut capability_context = ContextFrame::new(Vec::new());
    capabilities
        .runtime_extensions
        .contribute_request_context(&ModelRequestContext::agent_work(), &mut capability_context)?;
    Ok(AgentContextWindowToolProjection::new(
        capabilities.initial_tool_set.checkpoint(),
        initial_run_world_state,
        capabilities.initial_tool_set.dynamic_definitions().to_vec(),
    )
    .with_conversation_world_state_sections(conversation_sections)
    .with_capability_context(
        capability_context
            .model_request_items()
            .into_iter()
            .cloned()
            .collect(),
    ))
}

pub fn conversation_context_configuration_revision(input: &AgentChatInput) -> AgentResult<String> {
    Ok(prepare_conversation_context(input)?.configuration_revision)
}

pub fn create_conversation_context_state(
    input: AgentChatInput,
) -> AgentResult<AgentConversationContextState> {
    let prepared = prepare_conversation_context(&input)?;
    let output_budget = resolve_output_budget(&input, prepared.api_style)?;
    let assembled = assemble_context_preview(
        DurableConversationTimeline {
            compaction_summary: input.context_compaction_summary.clone(),
            world_state_records: input.world_state_records.clone(),
            messages: input.messages,
        },
        None,
        None,
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        &prepared.tool_definitions,
    )?;
    let detector = ContextCapacityDetector::for_model(
        &input.model,
        prepared.api_style,
        &prepared.tool_definitions,
    );
    let mut state = AgentConversationContextState::new(
        prepared.configuration_revision,
        input.model,
        input.context_window_tokens,
        output_budget.reserved_output_tokens,
        detector,
        assembled.frame,
        assembled.timing,
    );
    state.hydrate_context_images(&input.context_image_attachments)?;
    Ok(state)
}

/// Rebuilds a durable conversation context and privately restores provider-native Assistant
/// Turns before any context-window measurement is exposed to the Host.
///
/// The frozen Profile and protocol key remain Host-only. Raw continuation state is loaded through
/// `AgentRuntimeHostServices` and attached directly to the in-memory context frame; it never
/// crosses `AgentChatInput`, checkpoint, Trace, or message serialization boundaries.
pub fn create_conversation_context_state_with_host_services(
    input: AgentChatInput,
    conversation_id: &str,
    host_services: &AgentRuntimeHostServices,
) -> AgentResult<AgentConversationContextState> {
    let conversation_id = conversation_id.trim();
    if conversation_id.is_empty() {
        return Err(AgentError::new(
            "Provider continuation preview requires a conversation id.",
        ));
    }
    let input_conversation_id = input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
        .map(str::trim);
    if input_conversation_id != Some(conversation_id) {
        return Err(AgentError::new(
            "Provider continuation preview conversation identity does not match Agent input.",
        ));
    }
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let provider_dialect = crate::provider_profile::ProviderProtocolDialect::from(api_style);
    let provider_profile_config = input
        .provider_profile_config
        .clone()
        .ok_or_else(|| AgentError::new("Provider profile configuration is missing."))?;
    provider_profile_config
        .validate_for_dialect(provider_dialect)
        .map_err(|error| {
            AgentError::new(format!(
                "Provider profile configuration is invalid: {error}"
            ))
        })?;
    let provider_protocol_key = match input.provider_protocol_key.clone() {
        Some(key) => {
            key.validate_against_config(&provider_profile_config)
                .map_err(|error| {
                    AgentError::new(format!("Provider protocol key is invalid: {error}"))
                })?;
            if key.model_id != input.model.trim() {
                return Err(AgentError::new(
                    "Provider protocol key does not match the selected model.",
                ));
            }
            if key.provider_configuration_revision != input.provider_configuration_revision {
                return Err(AgentError::new(
                    "Provider protocol key does not match the selected provider configuration revision.",
                ));
            }
            key
        }
        None => crate::provider_profile::ProviderProtocolKey::new(
            provider_dialect,
            &provider_profile_config,
            input.model.trim(),
            input.provider_configuration_revision.clone(),
        )
        .map_err(|error| AgentError::new(format!("Provider protocol key is invalid: {error}")))?,
    };

    let mut state = create_conversation_context_state(input)?;
    super::hydrate_provider_continuation_history(
        state.provider_hydration_frame_mut(),
        &provider_profile_config,
        &provider_protocol_key,
        Some(conversation_id),
        host_services.storage.as_deref(),
        host_services.provider_continuation_vault.as_deref(),
        super::ProviderContinuationResumeRequirement {
            required_refs: None,
            current_assistant_turn_id: None,
        },
    )?;
    Ok(state)
}

pub(super) struct PreparedConversationContext {
    pub(super) tool_definitions: Vec<AgentToolDefinition>,
    api_style: crate::protocol::AgentApiStyle,
    configuration_revision: String,
}

pub(super) fn prepare_conversation_context(
    input: &AgentChatInput,
) -> AgentResult<PreparedConversationContext> {
    let run_id = "conversation-context-state";
    // Conversation configuration identifies only the stable prompt/tool prefix. Skill discovery
    // and activation are run overlays whose Host-bound resource authority is established when a
    // real Agent run is prepared; preview/revision calculation must neither require that authority
    // nor let a dynamic overlay perturb the stable configuration identity.
    let mut stable_input = input.clone();
    stable_input.skill_discovery = None;
    stable_input.skill_activation = None;
    let PreparedRuntimeCapabilities {
        initial_tool_set, ..
    } = prepare_runtime_capabilities(&stable_input, run_id, &[], true, None)?;
    let tool_definitions = initial_tool_set.stable_definitions().to_vec();
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let configuration_revision = conversation_context_configuration_revision_from_parts(
        input,
        api_style,
        &tool_definitions,
    )?;
    Ok(PreparedConversationContext {
        tool_definitions,
        api_style,
        configuration_revision,
    })
}

pub(super) fn conversation_context_configuration_revision_from_parts(
    input: &AgentChatInput,
    api_style: crate::protocol::AgentApiStyle,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<String> {
    if let Some(identity) = input
        .context
        .as_ref()
        .and_then(|context| context.collaboration_identity.as_ref())
    {
        identity.validate().map_err(|error| {
            AgentError::new(format!("Collaboration identity is invalid: {error}"))
        })?;
    }
    let system_prompt = build_system_prompt_with_collaboration(
        input.prompt_preferences.as_ref(),
        tool_definitions,
        input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref()),
    );
    let provider_dialect = crate::provider_profile::ProviderProtocolDialect::from(api_style);
    let provider_profile_config = input
        .provider_profile_config
        .clone()
        .ok_or_else(|| AgentError::new("Provider profile configuration is missing."))?;
    provider_profile_config
        .validate_for_dialect(provider_dialect)
        .map_err(|error| {
            AgentError::new(format!(
                "Provider profile configuration is invalid: {error}"
            ))
        })?;
    let provider_protocol_key = match input.provider_protocol_key.as_ref() {
        Some(key) => key.clone(),
        None => crate::provider_profile::ProviderProtocolKey::new(
            provider_dialect,
            &provider_profile_config,
            input.model.trim(),
            input.provider_configuration_revision.clone(),
        )
        .map_err(|error| AgentError::new(format!("Provider protocol key is invalid: {error}")))?,
    };
    provider_protocol_key
        .validate_against_config(&provider_profile_config)
        .map_err(|error| AgentError::new(format!("Provider protocol key is invalid: {error}")))?;
    let output_budget = resolve_output_budget(input, api_style)?;
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 5,
        "contextProfile": input.prompt_preferences.as_ref()
            .map(|preferences| preferences.context_profile).unwrap_or_default(),
        "model": input.model.trim(),
        "apiStyle": api_style,
        "providerProfileConfig": provider_profile_config,
        "providerProtocolKey": provider_protocol_key,
        "contextWindowTokens": input.context_window_tokens,
        "requestMaxTokens": output_budget.request_max_tokens,
        "reservedOutputTokens": output_budget.reserved_output_tokens,
        "systemPrompt": system_prompt,
        "toolDefinitions": tool_definitions,
    }))
    .map_err(|error| AgentError::new(format!("无法生成上下文计量配置指纹：{error}")))?;
    Ok(content_revision(&material))
}

#[cfg(test)]
mod steer_input_queue_tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    fn input(guidance_id: &str, client_message_id: &str, content: &str) -> crate::AgentSteerInput {
        crate::AgentSteerInput {
            guidance_id: guidance_id.to_string(),
            client_message_id: client_message_id.to_string(),
            content: content.to_string(),
            attachments: Vec::new(),
            folder_references: Vec::new(),
            attachment_library: None,
            created_at: 10,
        }
    }

    fn attachment_only_input(guidance_id: &str, client_message_id: &str) -> crate::AgentSteerInput {
        let mut input = input(guidance_id, client_message_id, "");
        input.attachments.push(crate::AgentInputAttachment {
            id: "attachment-1".to_string(),
            kind: crate::AgentInputAttachmentKind::File,
            name: "notes.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 5,
            encoding: crate::AgentInputAttachmentEncoding::Managed,
            data: String::new(),
            content_sha256: Some("hash".to_string()),
            truncated: None,
        });
        input
    }

    #[test]
    fn steer_queue_accepts_attachment_only_input_but_rejects_empty_input() {
        let queue = AgentSteerInputQueue::new();
        assert_eq!(
            queue
                .enqueue(attachment_only_input(
                    "guidance-attachment",
                    "client-attachment"
                ))
                .unwrap(),
            AgentSteerEnqueueOutcome::Queued
        );
        assert!(queue
            .enqueue(input("guidance-empty", "client-empty", ""))
            .is_err());
    }

    #[test]
    fn steer_queue_is_fifo_idempotent_and_conflict_safe() {
        let queue = AgentSteerInputQueue::new();
        let first = input("guidance-1", "client-1", "first");
        let second = input("guidance-2", "client-2", "second");

        assert_eq!(
            queue.enqueue(first.clone()).unwrap(),
            AgentSteerEnqueueOutcome::Queued
        );
        assert_eq!(
            queue.enqueue(first.clone()).unwrap(),
            AgentSteerEnqueueOutcome::Duplicate
        );
        assert!(queue
            .enqueue(input("guidance-other", "client-1", "changed"))
            .is_err());
        assert!(queue
            .enqueue(input("guidance-1", "client-other", "changed"))
            .is_err());
        assert_eq!(
            queue.enqueue(second.clone()).unwrap(),
            AgentSteerEnqueueOutcome::Queued
        );

        assert_eq!(queue.drain_pending(), vec![first, second]);
        assert!(queue.is_accepting());
    }

    #[test]
    fn approval_handoff_preserves_fifo_and_identity_without_sharing_close() {
        let queue = AgentSteerInputQueue::new();
        let first = input("guidance-1", "client-1", "first");
        let second = input("guidance-2", "client-2", "second");
        queue.enqueue(first.clone()).unwrap();
        queue.enqueue(second.clone()).unwrap();

        let successor = queue.handoff_pending();
        assert!(!queue.is_same_queue(&successor));
        assert!(!queue.is_accepting());
        assert_eq!(queue.pending_len(), 0);
        assert_eq!(
            successor.enqueue(first.clone()).unwrap(),
            AgentSteerEnqueueOutcome::Duplicate
        );
        assert!(successor
            .enqueue(input("different", "client-1", "changed"))
            .is_err());
        let third = input("guidance-3", "client-3", "third");
        successor.enqueue(third.clone()).unwrap();

        // Both cleanup operations may happen late, after the next Runtime has already started.
        queue.close();
        assert!(queue.close_and_take_pending().is_empty());
        assert!(successor.is_accepting());
        assert_eq!(
            successor.drain_pending(),
            vec![first.clone(), second, third]
        );
        let next = successor.handoff_pending();
        successor.close();
        assert!(next.is_accepting());
        assert_eq!(
            next.enqueue(first).unwrap(),
            AgentSteerEnqueueOutcome::Duplicate
        );
    }

    #[test]
    fn take_pending_or_close_keeps_accepting_only_when_work_was_taken() {
        let queue = AgentSteerInputQueue::new();
        let first = input("guidance-1", "client-1", "first");
        queue.enqueue(first.clone()).unwrap();

        assert!(matches!(
            queue.take_pending_or_close(),
            AgentSteerDrainOrClose::Pending(items) if items == vec![first]
        ));
        assert!(queue.is_accepting());
        assert!(matches!(
            queue.take_pending_or_close(),
            AgentSteerDrainOrClose::Closed
        ));
        assert!(!queue.is_accepting());
        assert_eq!(
            queue
                .enqueue(input("guidance-2", "client-2", "second"))
                .unwrap(),
            AgentSteerEnqueueOutcome::Closed
        );
    }

    #[test]
    fn enqueue_and_terminal_close_race_never_loses_an_accepted_input() {
        for index in 0..100 {
            let queue = AgentSteerInputQueue::new();
            let barrier = Arc::new(Barrier::new(3));
            let enqueue_queue = queue.clone();
            let enqueue_barrier = Arc::clone(&barrier);
            let enqueue = thread::spawn(move || {
                enqueue_barrier.wait();
                enqueue_queue
                    .enqueue(input(
                        &format!("guidance-{index}"),
                        &format!("client-{index}"),
                        "race",
                    ))
                    .unwrap()
            });
            let close_queue = queue.clone();
            let close_barrier = Arc::clone(&barrier);
            let close = thread::spawn(move || {
                close_barrier.wait();
                close_queue.take_pending_or_close()
            });
            barrier.wait();

            let enqueue_outcome = enqueue.join().unwrap();
            let close_outcome = close.join().unwrap();
            match enqueue_outcome {
                AgentSteerEnqueueOutcome::Queued => {
                    let accepted_was_taken = matches!(
                        close_outcome,
                        AgentSteerDrainOrClose::Pending(ref items) if items.len() == 1
                    );
                    let accepted_remains_pending = queue.pending_len() == 1;
                    assert!(accepted_was_taken || accepted_remains_pending);
                }
                AgentSteerEnqueueOutcome::Closed => {
                    assert!(matches!(close_outcome, AgentSteerDrainOrClose::Closed));
                    assert_eq!(queue.pending_len(), 0);
                }
                AgentSteerEnqueueOutcome::Duplicate => {
                    panic!("a fresh race cannot produce a duplicate")
                }
            }
        }
    }
}
