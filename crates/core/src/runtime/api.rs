use super::*;

pub type AgentEventEmitter = Arc<dyn Fn(AgentEvent) + Send + Sync + 'static>;
pub type AgentConversationTraceObserver = Arc<
    dyn Fn(ConversationTraceSnapshot) -> AgentResult<Option<AgentContextBaseline>>
        + Send
        + Sync
        + 'static,
>;
pub type AgentModelRequestObserver = Arc<dyn Fn(ModelRequestObservation) + Send + Sync + 'static>;
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
    pub(super) context_compaction_services: Option<AgentContextCompactionServices>,
    pub(super) skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
    pub(super) skill_activation_resolver: Option<AgentSkillActivationResolver>,
    pub(super) office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    pub(super) command_runtime_profile_resolver:
        Option<Arc<dyn crate::command::CommandRuntimeProfileResolver>>,
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

    /// Supplies the trusted approval-time resolver for model-visible Artifact Runtime profiles.
    /// Runtime bindings stay outside model input until the host has verified and frozen them.
    pub fn with_command_runtime_profile_resolver(
        mut self,
        resolver: Arc<dyn crate::command::CommandRuntimeProfileResolver>,
    ) -> Self {
        self.command_runtime_profile_resolver = Some(resolver);
        self
    }
}

/// Extracts exact resource-bearing Skill selections from the versioned Skill extension state in
/// an approval checkpoint. Hosts use this before resuming so model-activated resource authority is
/// restored from immutable package revisions rather than mutable installation receipts. The host
/// must additionally prove that selections not present in the explicit activation belong to the
/// same run's frozen discovery snapshot before opening package bytes.
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
    let mut state = create_conversation_context_state(input)?;
    state
        .snapshot_with_skill_overlays(
            AgentContextWindowPhase::Idle,
            skill_discovery.as_ref(),
            skill_activation.as_ref(),
        )
        .map(Some)
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
    let PreparedRuntimeCapabilities {
        tool_definitions, ..
    } = prepare_runtime_capabilities(input, run_id, &[], true, None)?;
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
    let system_prompt = build_system_prompt(
        input.context.as_ref(),
        input.prompt_preferences.as_ref(),
        tool_definitions,
    );
    let material = serde_json::to_vec(&json!({
        "schemaVersion": 1,
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
