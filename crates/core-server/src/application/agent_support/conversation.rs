use super::*;

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

enum ConversationTurnInputSource {
    Human,
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
    prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::Human,
        TurnReservationMode::PrepareOnly,
        existing,
        expected_revision,
    )
}

pub(crate) fn prepare_reserved_human_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    prepare_conversation_turn_from_source(
        storage,
        skills_service,
        input,
        run_id,
        ConversationTurnInputSource::Human,
        TurnReservationMode::CommitDurableLease,
        existing,
        expected_revision,
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
        // Child authority is a Host policy. It is never inherited from a model-authored payload.
        permissions: AgentPermissions::default(),
    };
    prepare_conversation_turn_from_source(
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
    )
}

#[allow(clippy::too_many_arguments)]
fn prepare_conversation_turn_from_source(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
    source: ConversationTurnInputSource,
    reservation: TurnReservationMode,
    existing: Option<ChatConversationRecord>,
    expected_revision: Option<i64>,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err("消息内容不能为空。".to_string().into());
    }

    let model_id = input.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err("modelId 不能为空。".to_string().into());
    }

    let settings_snapshot = storage
        .load_model_settings_snapshot()?
        .ok_or_else(|| "请先配置模型 API。".to_string())?;
    let settings = settings_snapshot.settings;

    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
        .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
    if !model.enabled {
        return Err(format!("模型未启用：{model_id}").into());
    }
    if let ConversationTurnInputSource::ExistingAgentProjection { model_snapshot, .. } = &source {
        if model_snapshot.model_config_id != model.id {
            return Err("可信 Wake 的模型选择与 Agent 创建快照不一致。"
                .to_string()
                .into());
        }
    }
    let provider_connection_revision = settings_snapshot
        .provider_connection_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| format!("模型 {model_id} 的 Provider 连接身份缺失。"))?;
    let provider_protocol_revision = settings_snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .cloned()
        .ok_or_else(|| format!("模型 {model_id} 的 Provider Protocol 身份缺失。"))?;
    let context_window_tokens = model.effective_context_window_tokens();
    // Resolve the complete pair once and carry it through the run. Model-level credentials
    // take priority; otherwise both values come from global settings. This prevents a URL
    // from one provider from ever being combined with a token from another.
    let connection = settings.effective_connection_for(&model)?;
    let provider_dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let provider_profile_config = model
        .resolved_provider_profile_config(provider_dialect)
        .map_err(|error| format!("模型 {model_id} 的 Provider Profile 无效：{error}"))?;
    if let ConversationTurnInputSource::ExistingAgentProjection {
        reasoning_effort_snapshot: Some(expected),
        ..
    } = &source
    {
        if provider_profile_config.reasoning.mode != mycopilot_core::ReasoningMode::Enabled
            || provider_profile_config.reasoning.effort != *expected
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
        model.id.clone(),
        Some(provider_protocol_revision.clone()),
    )
    .map_err(|error| format!("模型 {model_id} 的 Provider Protocol 无效：{error}"))?;
    if !model.supports_image
        && input
            .attachments
            .iter()
            .any(|attachment| attachment.kind == AgentInputAttachmentKind::Image)
    {
        return Err(format!(
            "当前模型「{}」不支持图片输入，请切换支持图片的模型后再发送。",
            model.display_name
        )
        .into());
    }

    let prompt_preferences = match input.prompt_preferences.clone() {
        Some(preferences) => preferences,
        None => agent_prompt_preferences_from_record(storage.load_agent_prompt_preferences()?),
    };

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
    let workspace_root = project
        .as_ref()
        .and_then(|project| project.path.as_deref())
        .map(PathBuf::from);
    let workspace = project
        .as_ref()
        .zip(workspace_root.as_deref())
        .map(|(project, root)| (project.id.as_str(), root));
    let prepared_skills =
        activate_selected_skills(storage, skills_service, workspace, &input.skills)?;
    let skill_discovery =
        prepare_enabled_skill_discovery(storage, skills_service, context_window_tokens)?;

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
        ConversationTurnInputSource::Human => ChatMessageRecord {
            id: user_message_id.clone(),
            role: "user".to_string(),
            content: content.clone(),
            created_at: timestamp,
            status: Some("sent".to_string()),
            attachments: message_attachments_from_input(&input.attachments, timestamp),
            agent_run_json: None,
            ui_state_json: None,
        },
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
    let assistant_created_at = timestamp.max(user_message.created_at.saturating_add(1));
    let assistant_message = ChatMessageRecord {
        id: assistant_message_id.clone(),
        role: "assistant".to_string(),
        content: THINKING_PLACEHOLDER.to_string(),
        created_at: assistant_created_at,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };

    let mut model_input_projection_ids = history_messages
        .iter()
        .filter_map(|message| message.message_id.clone())
        .collect::<Vec<_>>();
    model_input_projection_ids.push(user_message_id.clone());
    let preloaded_agent_message_ids = storage
        .filter_preloaded_agent_message_ids(&conversation_id, &model_input_projection_ids)
        .map_err(|error| error.to_string())?;

    if matches!(&source, ConversationTurnInputSource::Human) {
        upsert_message(&mut conversation.messages, user_message.clone());
    }
    upsert_message(&mut conversation.messages, assistant_message.clone());
    match reservation {
        #[cfg(test)]
        TurnReservationMode::PrepareOnly => {
            storage.save_conversation(conversation)?;
        }
        TurnReservationMode::CommitDurableLease => {
            let initial_trace = mycopilot_core::ConversationTraceSnapshot::default()
                .in_progress_trace(run_id, &conversation_id, &assistant_message_id);
            let trusted_wake = match &source {
                ConversationTurnInputSource::Human => None,
                ConversationTurnInputSource::ExistingAgentProjection { wake_admission, .. } => {
                    Some(wake_admission.as_ref())
                }
            };
            storage.save_conversation_and_begin_turn_with_preloaded_agent_messages(
                conversation,
                expected_revision,
                trusted_wake,
                &preloaded_agent_message_ids,
                &initial_trace,
                assistant_created_at,
                now_ms().max(assistant_created_at),
            )?;
        }
    }
    if matches!(&source, ConversationTurnInputSource::Human) {
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
        workspace: project.as_ref().map(|project| AgentWorkspaceContext {
            project_id: Some(project.id.clone()),
            display_name: Some(project.name.clone()),
            root_path: project.path.clone(),
        }),
        attachment_library: Some(attachment_library),
        permissions: input.permissions,
        collaboration_identity: match &source {
            ConversationTurnInputSource::Human => None,
            ConversationTurnInputSource::ExistingAgentProjection {
                collaboration_identity,
                ..
            } => Some(collaboration_identity.as_ref().clone()),
        },
    };
    let world_state_records =
        ensure_conversation_world_state(EnsureConversationWorldStateRequest {
            storage,
            conversation_id: &conversation_id,
            effective_before_message_id: &user_message_id,
            context: Some(&run_context),
            prompt_preferences: Some(&prompt_preferences),
            model_id: &model.id,
            model_capabilities,
            active_summary: context_compaction_summary.as_ref(),
            created_at: timestamp,
        })?;

    let mut agent_messages = history_messages;
    agent_messages.push(AgentChatMessage {
        message_id: Some(user_message_id.clone()),
        role: "user".to_string(),
        content,
        created_at: Some(user_message.created_at),
        conversation_turn_trace: None,
        conversation_model_context_items: Vec::new(),
    });
    let goal = storage.load_visible_conversation_goal(&conversation_id)?;

    let provider_usage_semantics =
        mycopilot_core::resolve_provider_runtime_capabilities(&provider_protocol_key)
            .map_err(|error| error.to_string())?
            .usage();
    let agent_input = AgentChatInput {
        api_url: connection.api_url,
        api_token: connection.api_token,
        provider_configuration_revision: Some(provider_protocol_revision),
        provider_connection_revision: Some(provider_connection_revision),
        search_connection_revision: Some(settings_snapshot.search_connection_revision),
        provider_profile_config: Some(provider_profile_config),
        provider_protocol_key: Some(provider_protocol_key),
        model: model.id.clone(),
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
            tavily_api_key: non_empty(settings.tavily_api_key),
        }),
        prompt_preferences: Some(prompt_preferences),
        approval_decision: None,
        tool_continuation: None,
        attachments: input.attachments,
        resume_checkpoint: None,
        assistant_message_id: Some(assistant_message_id.clone()),
        context_compaction_summary,
        goal,
        world_state_records,
        skill_activation: prepared_skills.runtime,
        skill_discovery: skill_discovery.clone(),
        messages: agent_messages,
    };

    let skill_resources = match (prepared_skills.resources, skill_discovery.as_ref()) {
        (Some(resources), _) => Some(resources),
        (None, Some(_)) => Some(std::sync::Arc::new(
            mycopilot_core::skills::SkillResourceSession::empty(),
        )),
        (None, None) => None,
    };

    Ok(PreparedConversationTurn {
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
        output: AgentConversationTurnOutput {
            run_id: run_id.to_string(),
            event_name: AGENT_EVENT_NAME.to_string(),
            conversation_id,
            user_message_id,
            assistant_message_id,
            user_message,
            assistant_message,
            activated_skills: prepared_skills.summaries,
            skill_activation_revision: prepared_skills.revision,
        },
        agent_input,
    })
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
                return (retained_active_user_index == Some(index)).then_some((
                    message,
                    trace,
                    model_context_items,
                ));
            }
            if index > boundary_index {
                return Some((message, trace, model_context_items));
            }
            match cursor {
                ContextJournalCursor::Message { .. } => None,
                ContextJournalCursor::TraceItem { sequence, .. } => {
                    let mut trace = trace?;
                    trace.items.retain(|item| item.sequence() > *sequence);
                    model_context_items.retain(|item| item.sequence > *sequence);
                    let has_uncovered_completion = trace.terminal_status.is_terminal();
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
            if trace.conversation_id != conversation.id || trace.assistant_message_id != message.id
            {
                return Err(format!(
                    "conversation_history_corrupt: assistant message `{}` has mismatched trace identity",
                    message.id
                ));
            }
            trace.validate().map_err(|_| {
                format!(
                    "conversation_history_corrupt: assistant message `{}` has an invalid trace",
                    message.id
                )
            })?;
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
        let content = if trace.as_ref().is_some_and(|trace| {
            trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
        }) || (trace.as_ref().is_some_and(|trace| {
            trace.terminal_status == ConversationTurnTraceTerminalStatus::Cancelled
        }) && message.content.trim() == THINKING_PLACEHOLDER)
        {
            String::new()
        } else {
            message.content.clone()
        };
        if content.trim().is_empty() && trace.is_none() {
            continue;
        }
        history.push(AgentChatMessage {
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
