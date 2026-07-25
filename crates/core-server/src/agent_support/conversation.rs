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

pub(crate) fn prepare_conversation_turn(
    storage: &StorageService,
    skills_service: &SkillsService,
    input: AgentConversationTurnInput,
    run_id: &str,
) -> Result<PreparedConversationTurn, AgentServiceError> {
    let content = input.content.trim().to_string();
    if content.is_empty() {
        return Err("消息内容不能为空。".to_string().into());
    }

    let model_id = input.model_id.trim().to_string();
    if model_id.is_empty() {
        return Err("modelId 不能为空。".to_string().into());
    }

    let settings = storage
        .load_model_settings()?
        .ok_or_else(|| "请先配置模型 API。".to_string())?;

    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_id)
        .cloned()
        .ok_or_else(|| format!("未找到模型配置：{model_id}"))?;
    if !model.enabled {
        return Err(format!("模型未启用：{model_id}").into());
    }
    let context_window_tokens = model.effective_context_window_tokens();
    // Resolve the complete pair once and carry it through the run. Model-level credentials
    // take priority; otherwise both values come from global settings. This prevents a URL
    // from one provider from ever being combined with a token from another.
    let connection = settings.effective_connection_for(&model)?;
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

    let existing = storage.load_conversation(&conversation_id)?;
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
    let context_compaction_summary =
        storage.get_active_context_compaction_summary(&conversation_id)?;
    let history_messages = conversation_history_messages_with_compaction(
        &conversation,
        &history_traces,
        context_compaction_summary.as_ref(),
        &[user_message_id.as_str(), assistant_message_id.as_str()],
    );

    let user_message = ChatMessageRecord {
        id: user_message_id.clone(),
        role: "user".to_string(),
        content: content.clone(),
        created_at: timestamp,
        status: Some("sent".to_string()),
        attachments: message_attachments_from_input(&input.attachments, timestamp),
        agent_run_json: None,
        ui_state_json: None,
    };
    let assistant_message = ChatMessageRecord {
        id: assistant_message_id.clone(),
        role: "assistant".to_string(),
        content: THINKING_PLACEHOLDER.to_string(),
        created_at: timestamp + 1,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    };

    upsert_message(&mut conversation.messages, user_message.clone());
    upsert_message(&mut conversation.messages, assistant_message.clone());
    storage.save_conversation(conversation)?;
    storage.save_input_attachments(
        &conversation_id,
        &user_message_id,
        resolved_project_id.as_deref(),
        &input.attachments,
        timestamp,
    )?;
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
    };
    let world_state_records = ensure_conversation_world_state(
        storage,
        &conversation_id,
        &user_message_id,
        Some(&run_context),
        Some(&prompt_preferences),
        model_capabilities,
        context_compaction_summary.as_ref(),
        timestamp,
    )?;

    let mut agent_messages = history_messages;
    agent_messages.push(AgentChatMessage {
        message_id: Some(user_message_id.clone()),
        role: "user".to_string(),
        content,
        created_at: Some(timestamp),
        conversation_turn_trace: None,
    });

    let agent_input = AgentChatInput {
        api_url: connection.api_url,
        api_token: connection.api_token,
        model: model.id.clone(),
        model_capabilities,
        api_style: None,
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
            input_price: Some(model.input_price.clone()),
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
) -> Vec<AgentChatMessage> {
    conversation_history_messages_with_compaction(conversation, traces, None, excluded_message_ids)
}

pub(crate) fn conversation_history_messages_with_compaction(
    conversation: &ChatConversationRecord,
    traces: &[ConversationTurnTrace],
    compaction_summary: Option<&mycopilot_core::ContextCompactionSummary>,
    excluded_message_ids: &[&str],
) -> Vec<AgentChatMessage> {
    let traces = traces
        .iter()
        .map(|trace| (trace.assistant_message_id.as_str(), trace))
        .collect::<std::collections::HashMap<_, _>>();
    let covered_boundary = compaction_summary.and_then(|summary| {
        conversation
            .messages
            .iter()
            .position(|message| message.id == summary.covered_through.message_id())
            .map(|index| (index, &summary.covered_through))
    });
    conversation
        .messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let trace = traces.get(message.id.as_str()).copied().cloned();
            let Some((boundary_index, cursor)) = covered_boundary else {
                return Some((message, trace));
            };
            if index < boundary_index {
                return None;
            }
            if index > boundary_index {
                return Some((message, trace));
            }
            match cursor {
                ContextJournalCursor::Message { .. } => None,
                ContextJournalCursor::TraceItem { sequence, .. } => {
                    let mut trace = trace?;
                    trace.items.retain(|item| item.sequence() > *sequence);
                    let has_uncovered_completion = trace.terminal_status.is_terminal();
                    (!trace.items.is_empty() || has_uncovered_completion)
                        .then_some((message, Some(trace)))
                }
            }
        })
        .filter(|message| {
            !excluded_message_ids
                .iter()
                .any(|excluded_id| message.0.id == *excluded_id)
        })
        .filter(|(message, trace)| {
            if message.status.as_deref() != Some("pending") {
                return true;
            }
            trace.as_ref().is_some_and(|trace| {
                trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            })
        })
        .filter(|(message, _)| matches!(message.role.as_str(), "user" | "assistant"))
        .filter_map(|(message, trace)| {
            if message.status.as_deref() == Some("error") && trace.is_none() {
                return None;
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
                return None;
            }
            Some(AgentChatMessage {
                message_id: Some(message.id.clone()),
                role: message.role.clone(),
                content,
                created_at: Some(message.created_at),
                conversation_turn_trace: trace,
            })
        })
        .collect()
}
