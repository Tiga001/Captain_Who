use super::*;
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
        Ok(AgentSteerEnqueueOutcome::Queued)
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.accepting = false;
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
        || input.content.trim().is_empty()
        || input.created_at < 0
    {
        return Err(AgentError::structured(
            "agent.invalid_steer_input",
            "用户引导缺少有效的身份、正文或创建时间。",
            json!({
                "guidanceIdPresent": !input.guidance_id.trim().is_empty(),
                "clientMessageIdPresent": !input.client_message_id.trim().is_empty(),
                "contentPresent": !input.content.trim().is_empty(),
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
pub type AgentHostActionExecutor = Arc<
    dyn Fn(AgentProposedAction, AgentCancellationToken) -> AgentResult<AgentToolResult>
        + Send
        + Sync
        + 'static,
>;
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
    pub(super) host_executor: Option<AgentHostActionExecutor>,
    pub(super) storage: Option<Arc<StorageService>>,
    pub(super) trace_observer: Option<AgentConversationTraceObserver>,
    pub(super) model_request_observer: Option<AgentModelRequestObserver>,
    pub(super) context_window_observer: Option<AgentContextWindowObserver>,
    pub(super) context_compaction_services: Option<AgentContextCompactionServices>,
    pub(super) skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
    pub(super) skill_activation_resolver: Option<AgentSkillActivationResolver>,
    pub(super) office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    pub(super) image_generation_execution:
        Option<Arc<crate::image_generation::ImageGenerationExecutionService>>,
    pub(super) command_runtime_profile_resolver:
        Option<Arc<dyn crate::command::CommandRuntimeProfileResolver>>,
    pub(super) steer_input: Option<AgentSteerInputQueue>,
}

impl AgentRuntimeHostServices {
    pub fn new() -> Self {
        Self::default()
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

    /// Receives exact aggregate request-capacity accounting after runtime extensions, Skill
    /// instructions, request-only availability context and dynamic Tool schemas are assembled.
    /// No Skill instruction or Tool schema body crosses this observer boundary.
    pub fn with_context_window_observer(mut self, observer: AgentContextWindowObserver) -> Self {
        self.context_window_observer = Some(observer);
        self
    }

    pub fn with_context_compaction(mut self, services: AgentContextCompactionServices) -> Self {
        self.context_compaction_services = Some(services);
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

    /// Supplies the trusted approval-time resolver for model-visible Artifact Runtime profiles.
    /// Runtime bindings stay outside model input until the host has verified and frozen them.
    pub fn with_command_runtime_profile_resolver(
        mut self,
        resolver: Arc<dyn crate::command::CommandRuntimeProfileResolver>,
    ) -> Self {
        self.command_runtime_profile_resolver = Some(resolver);
        self
    }

    pub fn with_steer_input(mut self, input: AgentSteerInputQueue) -> Self {
        self.steer_input = Some(input);
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
pub use super::context_compaction_model::AgentContextCompactionModelGenerator;

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
    let run_context = input.context.clone();
    let mut state = create_conversation_context_state(input)?;
    state
        .snapshot_with_run_overlays(
            AgentContextWindowPhase::Idle,
            run_context.as_ref(),
            skill_discovery.as_ref(),
            skill_activation.as_ref(),
        )
        .map(Some)
}

/// Inspects a context window with the exact Host-projected Skill-gated Tool suffix.
///
/// The projection is intentionally opaque: it binds dynamic schemas to the same trusted provider,
/// permissions and revision-checked Skill resource authority used by a real run. This entry point
/// also accounts for the backend-authored dynamic availability notice; callers must not attempt
/// to approximate the request by passing arbitrary schema lists.
pub fn inspect_context_window_with_tool_projection(
    mut input: AgentChatInput,
    projection: &AgentContextWindowToolProjection,
) -> AgentResult<Option<AgentContextWindowSnapshot>> {
    let skill_discovery = input.skill_discovery.take();
    let skill_activation = input.skill_activation.take();
    let run_context = input.context.clone();
    let mut state = create_conversation_context_state(input)?;
    state
        .snapshot_with_run_overlays_and_tool_projection(
            AgentContextWindowPhase::Idle,
            run_context.as_ref(),
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
    let extension_snapshots = input
        .resume_checkpoint
        .as_ref()
        .map(|checkpoint| checkpoint.extension_snapshots.as_slice())
        .unwrap_or_default();
    let capabilities = prepare_runtime_capabilities_with_skills(
        input,
        "context-window-tool-preview",
        extension_snapshots,
        RuntimeCapabilityServices {
            host_actions_available,
            office_engine: host_services.office_engine.clone(),
            image_generation_execution: host_services.image_generation_execution.clone(),
            skill_activation_resolver: host_services.skill_activation_resolver.clone(),
            skill_resources: host_services.skill_resources.clone(),
        },
    )?;
    Ok(AgentContextWindowToolProjection::new(
        capabilities.initial_tool_set.stable_revision().to_string(),
        capabilities.initial_tool_set.dynamic_revision().to_string(),
        capabilities.initial_tool_set.revision().to_string(),
        capabilities.initial_tool_set.dynamic_definitions().to_vec(),
    ))
}

pub fn conversation_context_configuration_revision(input: &AgentChatInput) -> AgentResult<String> {
    Ok(prepare_conversation_context(input)?.configuration_revision)
}

pub fn create_conversation_context_state(
    input: AgentChatInput,
) -> AgentResult<AgentConversationContextState> {
    let prepared = prepare_conversation_context(&input)?;
    let assembled = assemble_context_preview(
        input.context_compaction_summary.clone(),
        input.messages,
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
    Ok(AgentConversationContextState::new(
        prepared.configuration_revision,
        input.model,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
        detector,
        assembled.frame,
        assembled.timing,
    ))
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
    let system_prompt = build_system_prompt(input.prompt_preferences.as_ref(), tool_definitions);
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 2,
        "model": input.model.trim(),
        "apiStyle": api_style,
        "contextWindowTokens": input.context_window_tokens,
        "reservedOutputTokens": sanitize_max_tokens(input.max_tokens),
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
            attachment_library: None,
            created_at: 10,
        }
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
