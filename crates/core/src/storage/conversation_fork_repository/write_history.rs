fn clone_message(
    source: &ChatMessageRecord,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ChatMessageRecord, String> {
    let agent_run_json = source
        .agent_run_json
        .as_deref()
        .map(|raw| clone_agent_run_json(raw, replacements))
        .transpose()?;
    Ok(ChatMessageRecord {
        id: mapped_id(message_id_map, &source.id, "消息")?,
        role: source.role.clone(),
        content: source.content.clone(),
        created_at: source.created_at,
        status: source.status.clone(),
        attachments: Vec::new(),
        agent_run_json,
        // Renderer-owned presentation state is opaque to the fork engine.
        ui_state_json: source.ui_state_json.clone(),
    })
}

fn fork_snapshot_origins(
    connection: &Connection,
    source: &ChatConversationRecord,
    source_messages: &[ChatMessageRecord],
    message_id_map: &HashMap<String, String>,
) -> Result<Vec<ForkSnapshotOrigin>, ConversationForkError> {
    let mut origins = Vec::with_capacity(source_messages.len());
    for message in source_messages {
        let target_message_id = mapped_id(message_id_map, &message.id, "消息")?;
        let (original_kind, original_agent_id, original_mailbox_message_id) =
            match message.role.as_str() {
                "assistant" => (None, None, None),
                "user" => {
                    let origin = agent_graph_repository::conversation_message_origin(
                        connection,
                        &source.id,
                        &message.id,
                    )
                    .map_err(|error| ConversationForkError::Other(error.to_string()))?;
                    flattened_snapshot_origin(&origin)?
                }
                _ => {
                    return Err(ConversationForkError::Other(
                        "分叉历史包含不受支持的消息角色。".to_string(),
                    ));
                }
            };
        origins.push(ForkSnapshotOrigin {
            target_message_id,
            source_message_id: message.id.clone(),
            original_kind,
            original_agent_id,
            original_mailbox_message_id,
        });
    }
    Ok(origins)
}

type FlattenedSnapshotOrigin = (Option<&'static str>, Option<String>, Option<String>);

fn flattened_snapshot_origin(
    origin: &ConversationMessageOrigin,
) -> Result<FlattenedSnapshotOrigin, ConversationForkError> {
    match origin {
        ConversationMessageOrigin::Human => Ok((Some("human"), None, None)),
        ConversationMessageOrigin::Agent {
            sender_agent_id,
            source_agent_message_id,
        } => Ok((
            Some("agent"),
            Some(sender_agent_id.clone()),
            Some(source_agent_message_id.clone()),
        )),
        ConversationMessageOrigin::HistoricalSnapshot { original, .. } => match original.as_ref() {
            ConversationMessageOrigin::Human => Ok((Some("human"), None, None)),
            ConversationMessageOrigin::Agent {
                sender_agent_id,
                source_agent_message_id,
            } => Ok((
                Some("agent"),
                Some(sender_agent_id.clone()),
                Some(source_agent_message_id.clone()),
            )),
            ConversationMessageOrigin::HistoricalSnapshot { .. } => Err(
                ConversationForkError::Other("嵌套的历史消息来源无效。".to_string()),
            ),
        },
    }
}

fn clone_agent_run_json(
    raw: &str,
    replacements: &HashMap<String, String>,
) -> Result<String, String> {
    let mut value = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("历史 agent 状态不是有效 JSON：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    rewrite_history_open_tokens(&mut value, replacements)?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| "历史 agent 状态必须是 JSON 对象。".to_string())?;
    object.insert(
        "usage".to_string(),
        json!({
            "inputTokens": 0,
            "outputTokens": 0,
            "outputThinkingTokens": 0,
            "totalTokens": 0,
            "cachedInputTokens": 0,
            "cacheCreationInputTokens": 0,
            "billableRequestCount": 0
        }),
    );
    object.insert("messageStreamCheckpoints".to_string(), json!({}));
    if let Some(state) = object.get_mut("state").and_then(Value::as_object_mut) {
        state.insert("activeRunId".to_string(), Value::Null);
    }
    serde_json::to_string(&value).map_err(|error| format!("无法序列化复制后的 agent 状态：{error}"))
}

fn rewrite_exact_ids(value: &mut Value, replacements: &HashMap<String, String>) {
    match value {
        Value::String(current) => {
            if let Some(replacement) = replacements.get(current) {
                *current = replacement.clone();
            }
        }
        Value::Array(values) => {
            for value in values {
                rewrite_exact_ids(value, replacements);
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_exact_ids(value, replacements);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

fn agent_run_id(raw: Option<&str>) -> Option<String> {
    raw.and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value
                .get("runId")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

fn agent_run_status(raw: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let value = serde_json::from_str::<Value>(raw)
        .map_err(|error| format!("历史 agent 状态不是有效 JSON：{error}"))?;
    match value.get("status") {
        Some(Value::String(status)) => Ok(Some(status.clone())),
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err("历史 agent 状态的 status 字段无效。".to_string()),
    }
}

fn insert_conversation(
    connection: &Connection,
    target: &ChatConversationRecord,
) -> Result<(), String> {
    insert_empty_conversation(connection, target)?;
    for (position, message) in target.messages.iter().enumerate() {
        connection
            .execute(
                "INSERT INTO messages (
                    id, conversation_id, role, content, status, agent_run_json,
                    created_at, position
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    &message.id,
                    &target.id,
                    &message.role,
                    &message.content,
                    &message.status,
                    &message.agent_run_json,
                    message.created_at,
                    position as i64,
                ],
            )
            .map_err(database_error)?;
        chat_repository::update_message_ui_state(
            connection,
            &target.id,
            &message.id,
            message.ui_state_json.as_deref(),
        )
        .map_err(database_error)?;
    }
    Ok(())
}

fn insert_empty_conversation(
    connection: &Connection,
    target: &ChatConversationRecord,
) -> Result<(), String> {
    connection
        .execute(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL)",
            params![
                &target.id,
                &target.project_id,
                &target.model_id,
                &target.title,
                target.created_at,
                target.updated_at,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn insert_root_fork_receipt(
    connection: &Connection,
    plan: &ConversationForkPlan,
    target_message_id: &str,
) -> Result<(), ConversationForkError> {
    connection
        .execute(
            "INSERT INTO conversation_forks (
                request_id, target_conversation_id, source_conversation_id,
                source_message_id, target_message_id, created_at, source_fork_point_json,
                fork_authority, source_root_agent_id, target_root_agent_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &plan.request_id,
                &plan.target.id,
                &plan.source_conversation_id,
                &plan.source_message_id,
                target_message_id,
                plan.target.created_at,
                serde_json::to_string(&plan.source_fork_point)
                    .map_err(|error| format!("无法序列化分叉时间线边界：{error}"))?,
                plan.collaboration_root
                    .as_ref()
                    .map(|_| "collaboration_root")
                    .unwrap_or("legacy"),
                plan.collaboration_root
                    .as_ref()
                    .map(|root| root.source_agent_id.as_str()),
                plan.collaboration_root
                    .as_ref()
                    .map(|root| root.target_root.agent_id.as_str()),
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn insert_member_fork_receipt(
    connection: &Connection,
    plan: &ConversationForkPlan,
    member: &MemberAgentForkPlan,
) -> Result<(), ConversationForkError> {
    let source_root_agent_id = plan
        .collaboration_root
        .as_ref()
        .map(|root| root.source_agent_id.as_str())
        .ok_or_else(|| {
            ConversationForkError::Other("成员 fork 缺少源根 Agent 凭据。".to_string())
        })?;
    let target_root_agent_id = plan
        .collaboration_root
        .as_ref()
        .map(|root| root.target_root.agent_id.as_str())
        .ok_or_else(|| {
            ConversationForkError::Other("成员 fork 缺少目标根 Agent 凭据。".to_string())
        })?;
    connection
        .execute(
            "INSERT INTO agent_member_conversation_forks (
                 root_fork_request_id, source_conversation_id, target_conversation_id,
                 source_root_agent_id, target_root_agent_id,
                 source_member_agent_id, target_member_agent_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &plan.request_id,
                &member.source_agent.conversation_id,
                &member.target_agent.conversation_id,
                source_root_agent_id,
                target_root_agent_id,
                &member.source_agent.agent_id,
                &member.target_agent.agent_id,
                plan.target.created_at,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn insert_snapshot_messages(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
) -> Result<(), ConversationForkError> {
    if history.target.messages.len() != history.snapshot_origins.len() {
        return Err(ConversationForkError::Other(
            "成员 fork 的消息与来源计划数量不一致。".to_string(),
        ));
    }
    for (position, (message, origin)) in history
        .target
        .messages
        .iter()
        .zip(history.snapshot_origins)
        .enumerate()
    {
        if message.id != origin.target_message_id {
            return Err(ConversationForkError::Other(
                "成员 fork 的消息来源身份不一致。".to_string(),
            ));
        }
        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, input_origin_kind,
                     snapshot_source_conversation_id, snapshot_source_message_id,
                     snapshot_original_origin_kind, snapshot_original_agent_id,
                     snapshot_original_mailbox_message_id,
                     agent_run_json, created_at, position
                 ) VALUES (
                     ?1, ?2, ?3, ?4, ?5, 'snapshot', ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13
                 )",
                params![
                    &message.id,
                    &history.target.id,
                    &message.role,
                    &message.content,
                    &message.status,
                    history.source_conversation_id,
                    &origin.source_message_id,
                    origin.original_kind,
                    &origin.original_agent_id,
                    &origin.original_mailbox_message_id,
                    &message.agent_run_json,
                    message.created_at,
                    position as i64,
                ],
            )
            .map_err(database_error)?;
    }
    Ok(())
}

fn apply_history_facts(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
) -> Result<(), ConversationForkError> {
    for archive in history.archives {
        conversation_history_archive_repository::clone_archive_in_connection(connection, archive)
            .map_err(database_error)?;
    }
    for trace in history.traces {
        conversation_trace_repository::commit_trace_in_connection(
            connection,
            &trace.trace,
            trace.created_at,
            trace.committed_at,
        )
        .map_err(database_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            connection,
            &trace.trace.conversation_id,
            &trace.trace.assistant_message_id,
            &trace.model_context_items,
        )
        .map_err(database_error)?;
    }
    for audit in history.action_audits {
        let inserted =
            agent_action_audit_repository::insert_action_audit_record_if_absent(connection, audit)
                .map_err(database_error)?;
        if !inserted {
            return Err(ConversationForkError::Other(
                "分叉后的文件修改审计身份发生冲突。".to_string(),
            ));
        }
    }
    for turn_diff in history.turn_diffs {
        turn_diff_repository::insert_fork_copy(connection, turn_diff).map_err(database_error)?;
    }
    for change in history.file_changes {
        file_change_repository::insert_file_change(connection, &change.target)
            .map_err(database_error)?;
        file_change_repository::insert_file_change_history_snapshot(
            connection,
            &change.target.id,
            &change.history,
        )
        .map_err(database_error)?;
    }
    let summary_id_map = clone_summary_chain_for_history(connection, history)?;
    clone_context_compaction_receipts_for_history(connection, history, &summary_id_map)?;
    clone_provider_transition_receipts_for_history(connection, history, &summary_id_map)?;
    clone_world_state_records_for_history(connection, history, &summary_id_map)?;
    if history.requires_context_adaptation {
        let source_message_id = history.source_message_id.ok_or_else(|| {
            ConversationForkError::Other("Provider continuation 适配缺少源消息边界。".to_string())
        })?;
        conversation_context_adaptation_repository::insert_in_connection(
            connection,
            &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
                conversation_id: history.target.id.clone(),
                reason:
                    conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                        .to_string(),
                source_conversation_id: history.source_conversation_id.to_string(),
                source_message_id: source_message_id.to_string(),
                created_at: history.target.created_at,
                resolved_summary_id: None,
                resolved_at: None,
            },
        )
        .map_err(database_error)?;
    } else if let Some(source_summary_id) = history.adaptation_source_summary_id {
        let source_message_id = history.source_message_id.ok_or_else(|| {
            ConversationForkError::Other("Provider continuation 适配缺少源消息边界。".to_string())
        })?;
        let target_summary_id = mapped_id(
            &summary_id_map,
            source_summary_id,
            "Provider-neutral 适配摘要",
        )?;
        conversation_context_adaptation_repository::insert_in_connection(
            connection,
            &conversation_context_adaptation_repository::ConversationContextAdaptationRequirement {
                conversation_id: history.target.id.clone(),
                reason:
                    conversation_context_adaptation_repository::FORK_RELEASED_PROVIDER_STATE_REASON
                        .to_string(),
                source_conversation_id: history.source_conversation_id.to_string(),
                source_message_id: source_message_id.to_string(),
                created_at: history.target.created_at,
                resolved_summary_id: Some(target_summary_id),
                resolved_at: Some(history.target.created_at),
            },
        )
        .map_err(database_error)?;
    }
    for attachment in history.attachments {
        attachment_repository::save_attachment(connection, &attachment.target)
            .map_err(database_error)?;
    }
    for guidance in history.guidances {
        match guidance_repository::store_guidance_in_connection(connection, &guidance.record)
            .map_err(database_error)?
        {
            guidance_repository::AgentRunGuidanceStoreOutcome::Inserted => {}
            outcome => {
                return Err(format!("克隆用户引导 journal 时发生意外冲突：{outcome:?}").into());
            }
        }
        match guidance_repository::mark_guidance_applied(
            connection,
            &guidance.record.guidance_id,
            guidance.applied_trace_sequence,
            guidance.record.updated_at,
        )
        .map_err(database_error)?
        {
            guidance_repository::AgentRunGuidanceTransitionOutcome::Updated => {}
            outcome => {
                return Err(format!("克隆用户引导 trace 状态时发生意外冲突：{outcome:?}").into());
            }
        }
    }
    Ok(())
}

fn settle_forked_member_lifecycles(
    connection: &Connection,
    plan: &ConversationForkPlan,
) -> Result<(), ConversationForkError> {
    for member in plan.members.iter().rev() {
        if member.source_agent.lifecycle == AgentLifecycle::Active {
            continue;
        }
        let updated_at = plan
            .target
            .created_at
            .max(member.target_agent.updated_at.saturating_add(1));
        let affected = connection
            .execute(
                "UPDATE agent_nodes
                 SET lifecycle = ?1, revision = revision + 1, updated_at = ?2
                 WHERE agent_id = ?3 AND lifecycle = 'active' AND revision = ?4",
                params![
                    member.source_agent.lifecycle.as_str(),
                    updated_at,
                    &member.target_agent.agent_id,
                    member.target_agent.revision,
                ],
            )
            .map_err(database_error)?;
        if affected != 1 {
            return Err(ConversationForkError::Other(
                "成员 Agent 生命周期快照写入不完整。".to_string(),
            ));
        }
    }
    Ok(())
}

fn apply_fork_snapshot_origins(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    origins: &[ForkSnapshotOrigin],
) -> Result<(), ConversationForkError> {
    for origin in origins {
        let affected = connection
            .execute(
                "UPDATE messages
                 SET input_origin_kind = 'snapshot',
                     snapshot_source_conversation_id = ?3,
                     snapshot_source_message_id = ?4,
                     snapshot_original_origin_kind = ?5,
                     snapshot_original_agent_id = ?6,
                     snapshot_original_mailbox_message_id = ?7
                 WHERE conversation_id = ?1 AND id = ?2",
                params![
                    target_conversation_id,
                    &origin.target_message_id,
                    source_conversation_id,
                    &origin.source_message_id,
                    origin.original_kind,
                    &origin.original_agent_id,
                    &origin.original_mailbox_message_id,
                ],
            )
            .map_err(database_error)?;
        if affected != 1 {
            return Err(ConversationForkError::Other(
                "分叉历史消息来源写入不完整。".to_string(),
            ));
        }
    }
    Ok(())
}
