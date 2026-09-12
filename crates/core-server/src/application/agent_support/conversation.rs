use super::*;

pub(crate) fn active_rewrite_turn_output(
    storage: &StorageService,
    rewrite: &mycopilot_core::storage::conversation_turn_rewrite_repository::ConversationTurnRewriteRecord,
) -> Result<AgentConversationTurnOutput, String> {
    let mut output = serde_json::from_str::<AgentConversationTurnOutput>(&rewrite.response_json)
        .map_err(|_| "edit_turn_replay_corrupt: stored response is invalid".to_string())?;
    let conversation = storage
        .load_conversation(&rewrite.conversation_id)?
        .ok_or_else(|| {
            "edit_turn_replay_corrupt: replacement conversation is missing".to_string()
        })?;
    output.user_message = conversation
        .messages
        .iter()
        .find(|message| message.id == rewrite.replacement_user_message_id)
        .cloned()
        .ok_or_else(|| {
            "edit_turn_replay_corrupt: replacement user message is missing".to_string()
        })?;
    output.assistant_message = conversation
        .messages
        .iter()
        .find(|message| message.id == rewrite.replacement_assistant_message_id)
        .cloned()
        .ok_or_else(|| {
            "edit_turn_replay_corrupt: replacement assistant message is missing".to_string()
        })?;
    Ok(output)
}

pub(crate) struct PreparedConversationTurn {
    pub(crate) output: AgentConversationTurnOutput,
    pub(crate) agent_input: AgentChatInput,
    pub(crate) usage_context: AgentRunUsageContext,
    /// Exact, run-scoped access to sibling resources of the activated Skill
    /// revisions. This authority is host-only and is deliberately not
    /// serialized into `AgentChatInput`.
    pub(crate) skill_resources:
        Option<std::sync::Arc<mycopilot_core::skills::SkillResourceSession>>,
}

pub(crate) enum PreparedConversationTurnOutcome {
    Prepared(Box<PreparedConversationTurn>),
    Replayed(Box<AgentConversationTurnOutput>),
}

#[derive(Debug, Clone)]
pub(crate) struct HumanConversationTurnRewrite {
    pub(crate) request_id: String,
    pub(crate) request_fingerprint: String,
    pub(crate) source_user_message_id: String,
    pub(crate) source_assistant_message_id: String,
}

enum ConversationTurnInputSource {
    Human,
    HumanResponse(Box<mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission>),
    ExistingAgentProjection {
        collaboration_identity: Box<AgentCollaborationIdentity>,
        model_snapshot: Box<AgentModelSelectionSnapshot>,
        reasoning_effort_snapshot: Option<mycopilot_core::ReasoningEffort>,
        wake_admission: Box<mycopilot_core::TrustedAgentWakeTurnAdmission>,
    },
}

#[derive(Clone, Copy)]
enum TurnReservationMode {
    #[cfg(test)]
    PrepareOnly,
    CommitDurableLease,
}

#[cfg(test)]
pub(crate) fn prepare_conversation_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    let (existing, expected_revision) = match normalized_optional(input.conversation_id.as_deref())
    {
        Some(conversation_id) => storage.load_conversation_for_turn(&conversation_id)?,
        None => (None, None),
    };
    match prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::Human,
        TurnReservationMode::PrepareOnly,
        existing,
        expected_revision,
        None,
        None,
        None,
    )? {
        PreparedConversationTurnOutcome::Prepared(prepared) => Ok(*prepared),
        PreparedConversationTurnOutcome::Replayed(_) => {
            Err("prepare-only Turn cannot replay an edit request"
                .to_string()
                .into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_reserved_human_response_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
    response: mycopilot_core::storage::human_interaction_repository::HumanInteractionAsyncTurnAdmission,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    match prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::HumanResponse(Box::new(response)),
        TurnReservationMode::CommitDurableLease,
        existing,
        expected_revision,
        None,
        None,
        None,
    )? {
        PreparedConversationTurnOutcome::Prepared(prepared) => Ok(*prepared),
        PreparedConversationTurnOutcome::Replayed(_) => {
            Err("human response cannot replay a rewrite".to_string().into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_reserved_human_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
    automation_admission: Option<
        &mycopilot_core::storage::automation_repository::AutomationRunAdmissionInput,
    >,
    automation_execution_context: Option<AgentAutomationExecutionContext>,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    match prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::Human,
        TurnReservationMode::CommitDurableLease,
        existing,
        expected_revision,
        None,
        automation_admission,
        automation_execution_context,
    )? {
        PreparedConversationTurnOutcome::Prepared(prepared) => Ok(*prepared),
        PreparedConversationTurnOutcome::Replayed(_) => {
            Err("ordinary human Turn cannot replay an edit request"
                .to_string()
                .into())
        }
    }
}

pub(crate) fn prepare_reserved_human_rewrite_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
    rewrite: HumanConversationTurnRewrite,
) -> Result<PreparedConversationTurnOutcome, AgentServiceError> {
    prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::Human,
        TurnReservationMode::CommitDurableLease,
        existing,
        expected_revision,
        Some(rewrite),
        None,
        None,
    )
}

/// Prepares a child Turn from the already-acknowledged Mailbox projection. The task remains the
/// one durable `role=user` projection created by the collaboration transaction; this function
/// appends only the pending assistant and carries the Host-authenticated identity in RunContext.
#[allow(clippy::too_many_arguments)]
pub(crate) fn prepare_agent_wake_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    spawn: &mycopilot_core::ChildAgentSpawnRecord,
    assistant_message_id: String,
    run_id: &str,
    existing: ChatConversationRecord,
    expected_revision: i64,
    wake_admission: mycopilot_core::TrustedAgentWakeTurnAdmission,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    let model_snapshot = spawn
        .agent
        .model_snapshot
        .clone()
        .ok_or_else(|| "子 Agent 缺少创建时模型快照，不能启动可信 Wake。".to_string())?;
    let input = AgentConversationTurnInput {
        conversation_id: Some(spawn.agent.conversation_id.clone()),
        project_id: spawn.agent.project_id.clone(),
        model_id: model_snapshot.model_config_id.clone(),
        context_window_indicator_enabled: true,
        content: spawn.collaboration_identity.entrusted_task.clone(),
        attachments: Vec::new(),
        skills: Vec::new(),
        title: None,
        user_message_id: Some(spawn.task_message.projection_message_id.clone()),
        assistant_message_id: Some(assistant_message_id),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        // Required construction placeholder only. Atomic Turn admission replaces this with the
        // direct-parent/ancestor durable inheritance result before RunContext is created.
        permissions: AgentPermissions::default(),
    };
    match prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::ExistingAgentProjection {
            collaboration_identity: Box::new(spawn.collaboration_identity.clone()),
            model_snapshot: Box::new(model_snapshot),
            reasoning_effort_snapshot: spawn.agent.reasoning_effort_snapshot,
            wake_admission: Box::new(wake_admission),
        },
        TurnReservationMode::CommitDurableLease,
        Some(existing),
        Some(expected_revision),
        None,
        None,
        None,
    )? {
        PreparedConversationTurnOutcome::Prepared(prepared) => Ok(*prepared),
        PreparedConversationTurnOutcome::Replayed(_) => {
            Err("trusted Agent Wake cannot replay a human edit request"
                .to_string()
                .into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn prepare_conversation_turn_from_source(
    storage: &StorageService,
    skills_service: &SkillsService,
    mut input: AgentConversationTurnInput,
    run_id: &str,
    source: ConversationTurnInputSource,
    reservation: TurnReservationMode,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
    rewrite: Option<HumanConversationTurnRewrite>,
    automation_admission: Option<
        &mycopilot_core::storage::automation_repository::AutomationRunAdmissionInput,
    >,
    automation_execution_context: Option<AgentAutomationExecutionContext>,
) -> Result<PreparedConversationTurnOutcome, AgentServiceError> {
    if rewrite.is_some() && automation_admission.is_some() {
        return Err("an automation Turn cannot be admitted as a rewrite"
            .to_string()
            .into());
    }
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err("消息内容不能为空。".to_string().into());
    }

    let model_id = input.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err("modelId 不能为空。".to_string().into());
    }

    let settings_snapshot = storage
        .load_model_settings_snapshot_for_model(&model_id, false)?
        .ok_or_else(|| "请先配置模型 API。".to_string())?;
    let settings = settings_snapshot.settings;

    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
        .ok_or_else(|| "所选模型配置已不存在，请重新选择模型。".to_string())?;
    let model_label = model.display_label();
    if !model.enabled {
        return Err(format!("模型未启用：{model_label}").into());
    }
    let provider_connection_revision = settings_snapshot
        .provider_connection_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| format!("模型 {model_label} 的 Provider 连接身份缺失。"))?;
    let provider_protocol_revision = settings_snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| format!("模型 {model_label} 的 Provider Protocol 身份缺失。"))?;
    if let ConversationTurnInputSource::ExistingAgentProjection { model_snapshot, .. } = &source {
        if model_snapshot.model_config_id != model.id
            || model_snapshot.provider_connection_revision != provider_connection_revision
            || model_snapshot.provider_protocol_revision != provider_protocol_revision
        {
            return Err("可信 Wake 的模型连接或协议已在 Agent 创建后发生变化。"
                .to_string()
                .into());
        }
    }
    let context_window_tokens = model.effective_context_window_tokens();
    // Resolve the complete pair once and carry it through the run. Model-level credentials
    // take priority; otherwise both values come from global settings. This prevents a URL
    // from one provider from ever being combined with a token from another.
    let connection = settings.effective_connection_for(&model)?;
    let provider_dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let provider_profile_config = model
        .resolved_provider_profile_config(provider_dialect)
        .map_err(|error| format!("模型 {model_label} 的 Provider Profile 无效：{error}"))?;
    if let ConversationTurnInputSource::ExistingAgentProjection {
        reasoning_effort_snapshot: Some(expected),
        ..
    } = &source
    {
        let frozen_effort_matches = match expected {
            mycopilot_core::ReasoningEffort::ProviderDefault => {
                provider_profile_config.provider_reasoning_effort()
                    == mycopilot_core::ProviderReasoningEffort::ProviderDefault
            }
            mycopilot_core::ReasoningEffort::High => {
                provider_profile_config.provider_reasoning_effort()
                    == mycopilot_core::ProviderReasoningEffort::High
            }
            mycopilot_core::ReasoningEffort::Max => {
                provider_profile_config.provider_reasoning_effort()
                    == mycopilot_core::ProviderReasoningEffort::Max
            }
        };
        if provider_profile_config.reasoning_mode() != mycopilot_core::ReasoningMode::Enabled
            || !frozen_effort_matches
        {
            return Err(format!(
                "可信 Wake 的 reasoning effort 与 Agent 创建快照不一致：{expected:?}"
            )
            .into());
        }
    }
    let provider_protocol_key = ProviderProtocolKey::new(
        provider_dialect,
        &provider_profile_config,
        model.provider_model_id.clone(),
        Some(provider_protocol_revision.clone()),
    )
    .map_err(|error| format!("模型 {model_label} 的 Provider Protocol 无效：{error}"))?;
    if !model.supports_image
        && input
            .attachments
            .iter()
            .any(|attachment| attachment.kind == AgentInputAttachmentKind::Image)
    {
        return Err(format!(
            "当前模型「{model_label}」不支持图片输入，请切换支持图片的模型后再发送。"
        )
        .into());
    }

    let mut prompt_preferences = match input.prompt_preferences.clone() {
        Some(preferences) => preferences,
        None => agent_prompt_preferences_from_record(storage.load_agent_prompt_preferences()?),
    };
    prompt_preferences.automation_execution_context = automation_execution_context;

    let timestamp = now_ms();
    let conversation_id = normalized_optional(input.conversation_id.as_deref())
        .unwrap_or_else(|| create_id("conversation"));
    let user_message_id = normalized_optional(input.user_message_id.as_deref())
        .unwrap_or_else(|| create_id("message"));
    let assistant_message_id = normalized_optional(input.assistant_message_id.as_deref())
        .unwrap_or_else(|| create_id("message"));

    if matches!(
        &source,
        ConversationTurnInputSource::ExistingAgentProjection { .. }
    ) && existing.is_none()
    {
        return Err("可信 Wake 的子 Agent Conversation 不存在。"
            .to_string()
            .into());
    }
    let resolved_project_id = resolve_conversation_project_id(
        existing.as_ref(),
        normalized_optional(input.project_id.as_deref()),
    )?;
    let project = resolve_project(storage, resolved_project_id.as_deref())?;
    let inherited_workspace = match &source {
        ConversationTurnInputSource::ExistingAgentProjection { wake_admission, .. } => Some(
            storage
                .load_agent_workspace_for_wake(&wake_admission.wake_id)?
                .ok_or_else(|| "可信 Wake 缺少冻结工作区。".to_string())?,
        ),
        _ => None,
    };
    let prepared_workspace = match inherited_workspace {
        Some(workspace) => workspace,
        None => project
            .as_ref()
            .map(mycopilot_core::workspace::capture_project_workspace)
            .transpose()?,
    };
    // Skills always use only the primary of the same snapshot as this task tree.
    let skill_root =
        mycopilot_core::workspace::WorkspaceResolver::from_context(prepared_workspace.as_ref())
            .available_primary()?;
    let workspace = prepared_workspace
        .as_ref()
        .and_then(|workspace| workspace.project_id.as_deref().zip(skill_root.as_deref()));
    let prepared_skills =
        activate_selected_skills(storage, skills_service, workspace, &input.skills)?;
    let skill_discovery =
        prepare_enabled_skill_discovery(storage, skills_service, workspace, context_window_tokens)?;

    let mut conversation = existing.unwrap_or_else(|| ChatConversationRecord {
        id: conversation_id.clone(),
        project_id: resolved_project_id.clone(),
        model_id: Some(model_id.clone()),
        title: input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| create_conversation_title(&content)),
        messages: Vec::new(),
        created_at: timestamp,
        updated_at: timestamp,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    });

    conversation.project_id = resolved_project_id.clone();
    conversation.model_id = Some(model_id.clone());
    if let Some(rewrite) = &rewrite {
        if let Some(title) = input
            .title
            .as_deref()
            .map(str::trim)
            .filter(|title| !title.is_empty())
        {
            let source = conversation
                .messages
                .iter()
                .find(|message| message.id == rewrite.source_user_message_id)
                .ok_or_else(|| "edit_turn_title_source_missing: 编辑源消息不存在。".to_string())?;
            let source_is_first_user = conversation
                .messages
                .iter()
                .find(|message| message.role == "user")
                .is_some_and(|message| message.id == source.id);
            if !source_is_first_user
                || conversation.title != create_conversation_title(&source.content)
            {
                return Err(
                    "edit_turn_title_not_automatic: 手工标题不能由编辑重发覆盖。"
                        .to_string()
                        .into(),
                );
            }
            conversation.title = title.to_string();
        }
    }
    conversation.updated_at = timestamp;

    let history_traces = storage.list_conversation_turn_traces(&conversation_id)?;
    let history_model_context = storage.list_conversation_model_context_logs(&conversation_id)?;
    let context_compaction_summary =
        storage.get_active_context_compaction_summary(&conversation_id)?;
    let mut history_excluded_message_ids = storage
        .list_trace_bound_agent_projection_message_ids(&conversation_id)
        .map_err(|error| error.to_string())?;
    history_excluded_message_ids.push(user_message_id.clone());
    history_excluded_message_ids.push(assistant_message_id.clone());
    if let Some(rewrite) = &rewrite {
        history_excluded_message_ids.push(rewrite.source_user_message_id.clone());
        history_excluded_message_ids.push(rewrite.source_assistant_message_id.clone());
    }
    let history_excluded_refs = history_excluded_message_ids
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let history_messages = conversation_history_messages_with_model_context(
        &conversation,
        &history_traces,
        &history_model_context,
        context_compaction_summary.as_ref(),
        &history_excluded_refs,
    )?;

    let user_message = match &source {
        ConversationTurnInputSource::Human | ConversationTurnInputSource::HumanResponse(_) => {
            ChatMessageRecord {
                human_interaction_response: None,
                id: user_message_id.clone(),
                role: "user".to_string(),
                content: content.clone(),
                created_at: timestamp,
                status: Some("sent".to_string()),
                attachments: message_attachments_from_input(&input.attachments, timestamp),
                agent_run_json: None,
                ui_state_json: None,
            }
        }
        ConversationTurnInputSource::ExistingAgentProjection {
            collaboration_identity,
            ..
        } => {
            if collaboration_identity.conversation_id != conversation_id
                || collaboration_identity.entrusted_task != content
            {
                return Err("可信 Wake 的协作身份与任务投影不一致。".to_string().into());
            }
            let projected = conversation
                .messages
                .iter()
                .position(|message| message.id == user_message_id)
                .ok_or_else(|| "可信 Wake 的父任务 Conversation 投影不存在。".to_string())?;
            if projected + 1 != conversation.messages.len() {
                return Err("该可信 Wake 的父任务投影已经启动过 Turn，不能重复执行。"
                    .to_string()
                    .into());
            }
            let projected = conversation.messages[projected].clone();
            if projected.role != "user"
                || projected.status.as_deref() != Some("sent")
                || projected.content != content
            {
                return Err("可信 Wake 的父任务 Conversation 投影已损坏。"
                    .to_string()
                    .into());
            }
            projected
        }
    };
    let user_message = if matches!(&source, ConversationTurnInputSource::HumanResponse(_)) {
        conversation
            .messages
            .iter()
            .find(|message| message.id == user_message_id)
            .cloned()
            .unwrap_or(user_message)
    } else {
        user_message
    };
    let assistant_created_at = timestamp.max(user_message.created_at.saturating_add(1));
    let assistant_message = ChatMessageRecord {
        human_interaction_response: None,
        id: assistant_message_id.clone(),
        role: "assistant".to_string(),
        // Lifecycle labels belong to structured run state; message content is model-authored only.
        content: String::new(),
        created_at: assistant_created_at,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };

    let turn_output = AgentConversationTurnOutput {
        run_id: run_id.to_string(),
        event_name: AGENT_EVENT_NAME.to_string(),
        conversation_id: conversation_id.clone(),
        user_message_id: user_message_id.clone(),
        assistant_message_id: assistant_message_id.clone(),
        user_message: user_message.clone(),
        assistant_message: assistant_message.clone(),
        activated_skills: prepared_skills.summaries.clone(),
        skill_activation_revision: prepared_skills.revision.clone(),
    };

    let mut model_input_projection_ids = history_messages
        .iter()
        .filter_map(|message| message.message_id.clone())
        .collect::<Vec<_>>();
    model_input_projection_ids.push(user_message_id.clone());
    let preloaded_agent_message_ids = storage
        .filter_preloaded_agent_message_ids(&conversation_id, &model_input_projection_ids)
        .map_err(|error| error.to_string())?;

    if matches!(
        &source,
        ConversationTurnInputSource::Human | ConversationTurnInputSource::HumanResponse(_)
    ) {
        upsert_message(&mut conversation.messages, user_message.clone());
    }
    upsert_message(&mut conversation.messages, assistant_message.clone());
    let mut prepared_rewrite_attachments = if rewrite.is_some() {
        Some(storage.prepare_conversation_turn_rewrite_attachments(
            &conversation_id,
            &user_message_id,
            resolved_project_id.as_deref(),
            &input.attachments,
            timestamp,
        )?)
    } else {
        None
    };
    match reservation {
        #[cfg(test)]
        TurnReservationMode::PrepareOnly => {
            storage.save_conversation(conversation)?;
        }
        TurnReservationMode::CommitDurableLease => {
            let initial_trace = mycopilot_core::ConversationTraceSnapshot::default()
                .in_progress_trace(run_id, &conversation_id, &assistant_message_id);
            let trusted_wake = match &source {
                ConversationTurnInputSource::Human
                | ConversationTurnInputSource::HumanResponse(_) => None,
                ConversationTurnInputSource::ExistingAgentProjection { wake_admission, .. } => {
                    Some(wake_admission.as_ref())
                }
            };
            let permission_source = match &source {
                ConversationTurnInputSource::Human
                | ConversationTurnInputSource::HumanResponse(_) => {
                    mycopilot_core::AgentTurnPermissionSource::HostAuthenticatedRoot(
                        input.permissions,
                    )
                }
                ConversationTurnInputSource::ExistingAgentProjection { .. } => {
                    mycopilot_core::AgentTurnPermissionSource::InheritTrustedAncestors
                }
            };
            let effective_permissions = if let Some(rewrite) = &rewrite {
                let response_json = serde_json::to_string(&turn_output)
                    .map_err(|error| format!("无法持久化编辑重发响应：{error}"))?;
                let admission = mycopilot_core::storage::conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission {
                    request_id: rewrite.request_id.clone(),
                    request_fingerprint: rewrite.request_fingerprint.clone(),
                    conversation_id: conversation_id.clone(),
                    source_user_message_id: rewrite.source_user_message_id.clone(),
                    source_assistant_message_id: rewrite.source_assistant_message_id.clone(),
                    replacement_user_message_id: user_message_id.clone(),
                    replacement_assistant_message_id: assistant_message_id.clone(),
                    run_id: run_id.to_string(),
                    response_json,
                    created_at: assistant_created_at,
                };
                let admission_result = storage.rewrite_conversation_turn_and_begin_turn(
                    conversation,
                    expected_revision,
                    permission_source,
                    &preloaded_agent_message_ids,
                    &initial_trace,
                    assistant_created_at,
                    now_ms().max(assistant_created_at),
                    &admission,
                    prepared_rewrite_attachments
                        .as_ref()
                        .expect("rewrite attachments were prepared"),
                );
                let (_, permissions, outcome) = match admission_result {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        if let Some(prepared) = prepared_rewrite_attachments.take() {
                            storage
                                .discard_prepared_conversation_turn_rewrite_attachments(prepared);
                        }
                        return Err(error.into());
                    }
                };
                if let mycopilot_core::storage::service::ConversationTurnRewriteBeginOutcome::Replayed(record) = outcome {
                    if let Some(prepared) = prepared_rewrite_attachments.take() {
                        storage.discard_prepared_conversation_turn_rewrite_attachments(prepared);
                    }
                    let output = active_rewrite_turn_output(storage, &record)?;
                    return Ok(PreparedConversationTurnOutcome::Replayed(Box::new(output)));
                }
                permissions
            } else if let ConversationTurnInputSource::HumanResponse(response) = &source {
                storage
                    .save_human_interaction_conversation_and_begin_turn(
                        conversation,
                        expected_revision,
                        permission_source,
                        &preloaded_agent_message_ids,
                        &initial_trace,
                        assistant_created_at,
                        now_ms().max(assistant_created_at),
                        response,
                    )?
                    .1
            } else if let Some(automation_admission) = automation_admission {
                match storage
                    .save_automation_conversation_and_begin_turn_with_preloaded_agent_messages(
                        conversation,
                        expected_revision,
                        permission_source,
                        &preloaded_agent_message_ids,
                        &initial_trace,
                        assistant_created_at,
                        now_ms().max(assistant_created_at),
                        automation_admission,
                    )
                {
                    Ok((_, permissions, _)) => permissions,
                    Err(error)
                        if error
                            == mycopilot_core::storage::automation_repository::AUTOMATION_PERMISSION_DISABLED_AT_ADMISSION =>
                    {
                        return Err(AgentServiceError::structured(
                            mycopilot_core::storage::automation_repository::AUTOMATION_PERMISSION_DISABLED_MESSAGE,
                            serde_json::json!({ "code": "permission_disabled" }),
                        ));
                    }
                    Err(error) => return Err(error.into()),
                }
            } else {
                storage
                    .save_conversation_and_begin_turn_with_preloaded_agent_messages(
                        conversation,
                        expected_revision,
                        trusted_wake,
                        permission_source,
                        &preloaded_agent_message_ids,
                        &initial_trace,
                        assistant_created_at,
                        now_ms().max(assistant_created_at),
                    )?
                    .1
            };
            // Child authority is resolved only inside durable Turn admission. The placeholder on
            // AgentConversationTurnInput never reaches RunContext or a model/tool boundary.
            input.permissions = effective_permissions;
            // Admission, not the timing of preparation/settings reads, decides the mode. Wakes
            // inherit the originating tree, including after the parent has already completed.
            prompt_preferences.context_profile = storage
                .load_agent_context_profile_for_run(run_id)?
                .ok_or_else(|| "已接受的 Run 缺少冻结的上下文模式。".to_string())?;
            let admitted_workspace = storage
                .load_agent_workspace_for_run(run_id)?
                .ok_or_else(|| "已接受的 Run 缺少冻结工作区。".to_string())?;
            if admitted_workspace != prepared_workspace {
                return Err("准备任务期间工作区发生变化，请重新发送消息。"
                    .to_string()
                    .into());
            }
            #[cfg(test)]
            if automation_admission.is_some_and(|admission| {
                crate::application::agent::take_automation_post_admission_preparation_failure(
                    &admission.automation_run_id,
                )
            }) {
                return Err(
                    "injected post-admission Automation Turn preparation failure"
                        .to_string()
                        .into(),
                );
            }
        }
    }
    if matches!(
        &source,
        ConversationTurnInputSource::Human | ConversationTurnInputSource::HumanResponse(_)
    ) && rewrite.is_none()
    {
        storage.save_input_attachments(
            &conversation_id,
            &user_message_id,
            resolved_project_id.as_deref(),
            &input.attachments,
            timestamp,
        )?;
    }
    let attachment_library = storage
        .build_attachment_library_context(&conversation_id, resolved_project_id.as_deref())?;
    let model_capabilities = ModelCapabilities {
        image_input: model.supports_image,
    };
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.clone()),
        project_id: resolved_project_id.clone(),
        workspace: prepared_workspace,
        attachment_library: Some(attachment_library),
        permissions: input.permissions,
        collaboration_identity: match &source {
            ConversationTurnInputSource::Human | ConversationTurnInputSource::HumanResponse(_) => {
                None
            }
            ConversationTurnInputSource::ExistingAgentProjection {
                collaboration_identity,
                ..
            } => Some(collaboration_identity.as_ref().clone()),
        },
    };
    // Admission only reads history. The first actual sampling boundary atomically commits the
    // baseline and capability projections from the same snapshot used for schemas and guidance.
    let world_state_records = load_conversation_world_state(storage, &conversation_id)?;

    let mut agent_messages = history_messages;
    agent_messages.push(AgentChatMessage {
        conversation_completion_covered: false,
        message_id: Some(user_message_id.clone()),
        role: "user".to_string(),
        content,
        created_at: Some(user_message.created_at),
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    });
    storage.project_agent_messages_for_model(&conversation_id, &mut agent_messages)?;
    let provider_usage_semantics =
        mycopilot_core::resolve_provider_runtime_capabilities(&provider_protocol_key)
            .map_err(|error| error.to_string())?
            .usage();
    let mut agent_input = AgentChatInput {
        context_image_attachments: Vec::new(),
        api_url: connection.api_url,
        api_token: connection.api_token,
        provider_configuration_revision: Some(provider_protocol_revision),
        provider_connection_revision: Some(provider_connection_revision),
        search_connection_revision: Some(settings_snapshot.search_connection_revision),
        provider_profile_config: Some(provider_profile_config),
        provider_protocol_key: Some(provider_protocol_key),
        model_config_id: Some(model.id.clone()),
        model: model.provider_model_id.clone(),
        model_capabilities,
        api_style: Some(provider_dialect.api_style()),
        context_window_tokens: Some(context_window_tokens),
        context_window_indicator_enabled: input.context_window_indicator_enabled,
        max_tokens: input.max_tokens,
        temperature: input.temperature,
        stream: Some(true),
        context: Some(run_context),
        search_config: Some(AgentSearchConfig {
            mode: search_mode_from_storage(&settings.search_mode),
            tavily_api_key: None,
        }),
        prompt_preferences: Some(prompt_preferences),
        approval_decision: None,
        tool_continuation: None,
        attachments: input.attachments,
        resume_checkpoint: None,
        assistant_message_id: Some(assistant_message_id.clone()),
        context_compaction_summary,
        world_state_records,
        skill_activation: prepared_skills.runtime,
        skill_discovery: skill_discovery.clone(),
        messages: agent_messages,
    };

    hydrate_context_image_attachments(storage, &mut agent_input)?;

    let skill_resources = match (prepared_skills.resources, skill_discovery.as_ref()) {
        (Some(resources), _) => Some(resources),
        (None, Some(_)) => Some(std::sync::Arc::new(
            mycopilot_core::skills::SkillResourceSession::empty(),
        )),
        (None, None) => None,
    };

    Ok(PreparedConversationTurnOutcome::Prepared(Box::new(
        PreparedConversationTurn {
            skill_resources,
            usage_context: AgentRunUsageContext {
                conversation_id: conversation_id.clone(),
                assistant_message_id: assistant_message_id.clone(),
                run_id: run_id.to_string(),
                project_id: resolved_project_id.clone(),
                model_id: model.id.clone(),
                model_name: model.display_name.clone(),
                provider_usage_semantics,
                input_price: Some(model.input_price.clone()),
                cached_input_price: Some(model.effective_cached_input_price().to_string()),
                output_price: Some(model.output_price.clone()),
                started_at: timestamp,
            },
            output: turn_output,
            agent_input,
        },
    )))
}

#[cfg(test)]
pub(crate) fn conversation_history_messages(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    excluded_message_ids: &[&str],
) -> Result<Vec<AgentChatMessage>, String> {
    conversation_history_messages_with_compaction(conversation, traces, None, excluded_message_ids)
}

#[cfg(test)]
pub(crate) fn conversation_history_messages_with_compaction(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    compaction_summary: Option<&mycopilot_core::ContextCompactionSummary>,
    excluded_message_ids: &[&str],
) -> Result<Vec<AgentChatMessage>, String> {
    conversation_history_messages_with_model_context(
        conversation,
        traces,
        &[],
        compaction_summary,
        excluded_message_ids,
    )
}

pub(crate) fn conversation_history_messages_with_model_context(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    model_context_logs: &[mycopilot_core::ConversationModelContextLog],
    compaction_summary: Option<&mycopilot_core::ContextCompactionSummary>,
    excluded_message_ids: &[&str],
) -> Result<Vec<AgentChatMessage>, String> {
    let traces = traces
        .iter()
        .map(|trace| (trace.assistant_message_id.as_str(), trace))
        .collect::<std::collections::HashMap<_, _>>();
    let model_context_logs = model_context_logs
        .iter()
        .map(|log| (log.assistant_message_id.as_str(), log.items.as_slice()))
        .collect::<std::collections::HashMap<_, _>>();
    let covered_boundary = compaction_summary.and_then(|summary| {
        conversation
            .messages
            .iter()
            .position(|message| message.id == summary.covered_through.message_id())
            .map(|index| (index, &summary.covered_through))
    });
    let completion_is_covered = |message_id: &str| {
        covered_boundary.is_some_and(|(boundary_index, cursor)| {
            if conversation.messages[..boundary_index].iter().any(|message| message.id == message_id) {
                return true;
            }
            cursor.message_id() == message_id
                && match cursor {
                    ContextJournalCursor::Message { .. } => true,
                    ContextJournalCursor::TraceItem { sequence, .. } => {
                        traces.get(message_id).is_some_and(|trace| {
                            trace.items.iter().any(|item| {
                                item.sequence() == *sequence
                                    && matches!(
                                        item,
                                        mycopilot_core::ConversationTurnTraceItem::BackendState {
                                            placement: mycopilot_core::ConversationBackendStatePlacement::AfterMessage,
                                            ..
                                        }
                                    )
                            })
                        })
                    }
                }
        })
    };
    // Mid-run compaction may advance through the current user message to summarize later closed
    // tool exchanges. Keep that latest instruction exact beside the summary while the assistant
    // trace is still active. On later turns it is no longer special and the normal summary
    // boundary applies.
    let retained_active_user_index = covered_boundary.and_then(|(boundary_index, cursor)| {
        let boundary_is_active_trace = matches!(cursor, ContextJournalCursor::TraceItem { .. })
            && traces.get(cursor.message_id()).is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            });
        boundary_is_active_trace.then(|| {
            conversation.messages[..boundary_index]
                .iter()
                .rposition(|message| message.role == "user")
        })?
    });
    let projected = conversation
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let trace = traces.get(message.id.as_str()).copied().cloned();
            let mut model_context_items = model_context_logs
                .get(message.id.as_str())
                .copied()
                .unwrap_or_default()
                .to_vec();
            let Some((boundary_index, cursor)) = covered_boundary else {
                return Some((message, trace, model_context_items));
            };
            if index < boundary_index {
                if retained_active_user_index == Some(index) {
                    return Some((message, trace, model_context_items));
                }
                // Vision payloads are not text-summary replacements. Keep only their immutable
                // material rows, never old assistant completion or unrelated runtime facts.
                let mut trace = trace?;
                trace.items.retain(is_retained_context_image_material);
                model_context_items.retain(|item| {
                    trace
                        .items
                        .iter()
                        .any(|event| event.sequence() == item.sequence)
                });
                return (!trace.items.is_empty()).then_some((
                    message,
                    Some(trace),
                    model_context_items,
                ));
            }
            if index > boundary_index {
                return Some((message, trace, model_context_items));
            }
            match cursor {
                ContextJournalCursor::Message { .. } => {
                    let mut trace = trace?;
                    trace.items.retain(|item| {
                        is_retained_context_image_material(item)
                            || matches!(
                            item,
                            mycopilot_core::ConversationTurnTraceItem::BackendState {
                                placement:
                                    mycopilot_core::ConversationBackendStatePlacement::AfterMessage,
                                ..
                            }
                        )
                    });
                    model_context_items.retain(|item| {
                        trace
                            .items
                            .iter()
                            .any(|event| event.sequence() == item.sequence)
                    });
                    (!trace.items.is_empty()).then_some((message, Some(trace), model_context_items))
                }
                ContextJournalCursor::TraceItem { sequence, .. } => {
                    let mut trace = trace?;
                    // The durable trace is an audit log, while this projection is model input.
                    // Host-only lifecycle rows can legitimately reference ToolCalls already
                    // covered by the active summary, so retaining those rows would turn a valid
                    // full trace into an invalid standalone suffix. The complete trace is still
                    // validated below before this model-visible suffix is accepted.
                    trace.items.retain(|item| {
                        (item.sequence() > *sequence && item.is_model_visible())
                            || is_retained_context_image_material(item)
                    });
                    model_context_items.retain(|item| {
                        trace
                            .items
                            .iter()
                            .any(|event| event.sequence() == item.sequence)
                    });
                    let has_uncovered_completion =
                        trace.terminal_status.is_terminal() && !completion_is_covered(&message.id);
                    (!trace.items.is_empty() || has_uncovered_completion).then_some((
                        message,
                        Some(trace),
                        model_context_items,
                    ))
                }
            }
        })
        .filter(|message| {
            !excluded_message_ids
                .iter()
                .any(|excluded_id| message.0.id == *excluded_id)
        })
        .filter(|(message, trace, _)| {
            if message.status.as_deref() != Some("pending") {
                return true;
            }
            trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            })
        })
        .filter(|(message, _, _)| matches!(message.role.as_str(), "user" | "assistant"))
        .collect::<Vec<_>>();
    let mut history = Vec::with_capacity(projected.len());
    for (message, trace, conversation_model_context_items) in projected {
        if message.role == "assistant" {
            let Some(trace) = trace.as_ref() else {
                return Err(format!(
                    "conversation_history_corrupt: assistant message `{}` is missing its trace",
                    message.id
                ));
            };
            let raw_trace = traces.get(message.id.as_str()).copied().ok_or_else(|| {
                format!(
                    "conversation_history_corrupt: assistant message `{}` is missing its trace",
                    message.id
                )
            })?;
            if raw_trace.conversation_id != conversation.id
                || raw_trace.assistant_message_id != message.id
            {
                return Err(format!(
                    "conversation_history_corrupt: assistant message `{}` has mismatched trace identity",
                    message.id
                ));
            }
            raw_trace.validate().map_err(|_| {
                format!(
                    "conversation_history_corrupt: assistant message `{}` has an invalid trace",
                    message.id
                )
            })?;
            if raw_trace != trace {
                // A compacted trace suffix is a model-facing projection, not the original audit
                // record. It must remain internally valid after audit-only lifecycle rows are
                // removed, but the strict corruption decision above always uses the complete
                // trace. Uncompacted traces avoid a redundant second validation here.
                trace.validate().map_err(|_| {
                    format!(
                        "conversation_history_corrupt: assistant message `{}` has an invalid projected trace",
                        message.id
                    )
                })?;
            }
            trace
                .validate_complete_model_context(&conversation_model_context_items)
                .map_err(|_| {
                    format!(
                        "conversation_history_corrupt: assistant message `{}` has incomplete model context",
                        message.id
                    )
                })?;
            if (message.status.as_deref() == Some("pending"))
                != (trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress)
            {
                return Err(format!(
                    "conversation_history_corrupt: assistant message `{}` status disagrees with its trace",
                    message.id
                ));
            }
        } else if trace.is_some() || !conversation_model_context_items.is_empty() {
            return Err(format!(
                "conversation_history_corrupt: user message `{}` contains assistant trace state",
                message.id
            ));
        }
        let completion_covered = message.role == "assistant" && completion_is_covered(&message.id);
        let content = if completion_covered
            || trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            }) {
            String::new()
        } else {
            message.content.clone()
        };
        if content.trim().is_empty() && trace.is_none() {
            continue;
        }
        history.push(AgentChatMessage {
            conversation_completion_covered: completion_covered,
            message_id: Some(message.id.clone()),
            role: message.role.clone(),
            content,
            created_at: Some(message.created_at),
            conversation_turn_trace: trace,
            conversation_model_context_items,
        });
    }
    Ok(history)
}

/// Rehydrates historical image bytes only from Host-owned immutable references. The request
/// keeps the transient historical image field out of the resume-input allowlist. Existing
/// checkpoint image payloads retain their policy and are revalidated through these references.
pub(crate) fn hydrate_context_image_attachments(
    storage: &StorageService,
    input: &mut AgentChatInput,
) -> Result<(), String> {
    let mut refs = input
        .messages
        .iter()
        .flat_map(|message| &message.conversation_model_context_items)
        .flat_map(|item| item.images.iter().cloned())
        .collect::<Vec<_>>();
    refs.extend(
        input
            .messages
            .iter()
            .filter_map(|message| message.conversation_turn_trace.as_ref())
            .flat_map(|trace| &trace.items)
            .flat_map(|item| match item {
                mycopilot_core::ConversationTurnTraceItem::ContextMaterial { images, .. } => {
                    images.as_slice()
                }
                _ => &[],
            })
            .cloned(),
    );
    if let Some(checkpoint) = &input.resume_checkpoint {
        refs.extend(
            checkpoint
                .conversation_trace_items
                .iter()
                .flat_map(|item| match item {
                    mycopilot_core::ConversationTurnTraceItem::ContextMaterial {
                        images, ..
                    } => images.as_slice(),
                    _ => &[],
                })
                .cloned(),
        );
        refs.extend(
            checkpoint
                .conversation_model_context_items
                .iter()
                .flat_map(|item| item.images.iter().cloned()),
        );
        refs.extend(
            checkpoint
                .context_items
                .iter()
                .flat_map(|item| item.context_image_refs.iter().cloned()),
        );
    }
    if refs.is_empty() {
        input.context_image_attachments.clear();
        return Ok(());
    }
    let conversation_id = input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
        .ok_or_else(|| {
            "context_image_scope_missing: image history needs a conversation".to_string()
        })?;
    input.context_image_attachments =
        storage.load_context_image_attachments(conversation_id, &refs)?;
    Ok(())
}

fn is_retained_context_image_material(item: &mycopilot_core::ConversationTurnTraceItem) -> bool {
    matches!(item, mycopilot_core::ConversationTurnTraceItem::ContextMaterial { images, .. }
        if !images.is_empty())
}
