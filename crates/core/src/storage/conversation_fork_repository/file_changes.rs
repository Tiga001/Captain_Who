fn remapped_file_change_call_digest(
    traces: &[ForkTrace],
    source_call_id: &str,
    source_digest: &str,
    expected_tool_name: &str,
    replacements: &HashMap<String, String>,
) -> Result<String, ConversationForkError> {
    let mut matching_operations = traces
        .iter()
        .flat_map(|trace| trace.trace.items.iter())
        .filter_map(|item| match item {
            crate::ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } if call_id == source_call_id && tool == expected_tool_name => Some(operation),
            _ => None,
        });
    let source_operation = matching_operations.next().ok_or_else(|| {
        ConversationForkError::Other("文件变更事务引用的源 Tool Call 不在分叉历史中。".to_string())
    })?;
    if matching_operations.next().is_some() {
        return Err(ConversationForkError::Other(
            "文件变更事务引用了重复的源 Tool Call。".to_string(),
        ));
    }
    let computed_source_digest = crate::file_change::proposal_digest(source_operation)
        .map_err(|_| ConversationForkError::Other("无法验证文件变更 Tool Call。".to_string()))?;
    if computed_source_digest != source_digest {
        return Err(ConversationForkError::Other(
            "文件变更事务的 Tool Call digest 与历史不一致。".to_string(),
        ));
    }
    let mut target_operation = source_operation.clone();
    rewrite_exact_ids(&mut target_operation, replacements);
    crate::file_change::proposal_digest(&target_operation)
        .map_err(|_| ConversationForkError::Other("无法复制文件变更 Tool Call。".to_string()))
}

#[allow(clippy::too_many_arguments)]
fn remap_terminal_file_change_action_audits(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    source_message_ids: &[String],
    message_id_map: &HashMap<String, String>,
    run_id_map: &HashMap<String, String>,
    tool_call_id_map: &HashMap<String, String>,
    traces: &[ForkTrace],
    replacements: &mut HashMap<String, String>,
) -> Result<Vec<AgentActionAuditRecord>, ConversationForkError> {
    let visible_message_ids = source_message_ids.iter().collect::<HashSet<_>>();
    let source_audits =
        agent_action_audit_repository::list_terminal_file_change_action_audits_for_conversation(
            connection,
            source_conversation_id,
        )
        .map_err(database_error)?;
    let mut remapped = Vec::new();
    for audit in source_audits.into_iter().filter(|audit| {
        audit
            .assistant_message_id
            .as_ref()
            .is_some_and(|message_id| visible_message_ids.contains(message_id))
    }) {
        remapped.push(remap_terminal_file_change_action_audit(
            audit,
            source_conversation_id,
            target_conversation_id,
            message_id_map,
            run_id_map,
            tool_call_id_map,
            traces,
            replacements,
        )?);
    }
    Ok(remapped)
}

#[allow(clippy::too_many_arguments)]
fn remap_terminal_file_change_action_audit(
    audit: AgentActionAuditRecord,
    source_conversation_id: &str,
    target_conversation_id: &str,
    message_id_map: &HashMap<String, String>,
    run_id_map: &HashMap<String, String>,
    tool_call_id_map: &HashMap<String, String>,
    traces: &[ForkTrace],
    replacements: &mut HashMap<String, String>,
) -> Result<AgentActionAuditRecord, ConversationForkError> {
    if audit.conversation_id.as_deref() != Some(source_conversation_id)
        || audit.action_type != "file_change"
        || audit.tool_name != "apply_patch"
        || !matches!(
            audit.status.as_str(),
            "completed" | "failed" | "cancelled" | "rejected"
        )
        || audit.completed_at.is_none()
        || audit.command_result_json.is_some()
    {
        return Err(ConversationForkError::Other(
            "文件修改审计的来源身份或终态无效。".to_string(),
        ));
    }
    let source_assistant_message_id = audit
        .assistant_message_id
        .as_deref()
        .ok_or_else(|| ConversationForkError::Other("文件修改审计缺少所属消息。".to_string()))?;
    let target_assistant_message_id = mapped_id(
        message_id_map,
        source_assistant_message_id,
        "文件修改审计所属消息",
    )?;
    let target_run_id = mapped_id(run_id_map, &audit.run_id, "文件修改审计所属运行")?;

    let mut action = serde_json::from_str::<AgentProposedAction>(&audit.action_json)
        .map_err(|_| ConversationForkError::Other("文件修改审计 action 无效。".to_string()))?;
    let AgentProposedAction::FileChange { file_change } = &mut action else {
        return Err(ConversationForkError::Other(
            "文件修改审计 action 类型无效。".to_string(),
        ));
    };
    let source_tool_call_id = file_change.id.clone();
    let source_transaction_id = file_change.transaction_id.clone();
    let source_observation_id = file_change.execution.observation_id.clone();
    if audit.action_id != crate::canonical_pending_action_id(&audit.run_id, &source_tool_call_id)
        || file_change.execution.source_call_id != source_tool_call_id
        || file_change.execution.run_id != audit.run_id
        || file_change.execution.conversation_id != source_conversation_id
        || file_change.execution.transaction.id != source_transaction_id
        || file_change.execution.proposal.transaction_id != source_transaction_id
    {
        return Err(ConversationForkError::Other(
            "文件修改审计与冻结 action 的身份不一致。".to_string(),
        ));
    }
    let target_tool_call_id = mapped_id(
        tool_call_id_map,
        &source_tool_call_id,
        "文件修改审计 Tool Call",
    )?;
    let target_transaction_id = if let Some(target) = replacements.get(&source_transaction_id) {
        target.clone()
    } else {
        let target = new_id("file-change-history");
        insert_global_replacement(replacements, &source_transaction_id, &target)?;
        target
    };
    let target_observation_id = if let Some(target) = replacements.get(&source_observation_id) {
        target.clone()
    } else {
        let target = format!("fobs_{}", Uuid::new_v4().simple());
        insert_global_replacement(replacements, &source_observation_id, &target)?;
        target
    };
    let target_observation_source_tool_call_id = mapped_id(
        tool_call_id_map,
        &file_change.execution.observation.source_tool_call_id,
        "文件修改审计 observation Tool Call",
    )?;
    let target_trace_args_digest = remapped_file_change_call_digest(
        traces,
        &source_tool_call_id,
        &file_change.execution.trace_args_digest,
        "apply_patch",
        replacements,
    )?;
    let staged = file_change.execution.staged_transaction_id.is_some();

    file_change.id = target_tool_call_id.clone();
    file_change.transaction_id = target_transaction_id.clone();
    let execution = file_change.execution.as_mut();
    execution.transaction.id = target_transaction_id.clone();
    execution.proposal.id = target_tool_call_id.clone();
    execution.proposal.transaction_id = target_transaction_id.clone();
    execution.observation_id = target_observation_id.clone();
    execution.observation.observation_id = target_observation_id;
    execution.observation.source_tool_call_id = target_observation_source_tool_call_id;
    execution.observation.conversation_id = target_conversation_id.to_string();
    execution.observation.run_id = target_run_id.clone();
    execution.source_call_id = target_tool_call_id.clone();
    // A forked terminal action is display-only and never replays. Keep the exact private source
    // digest as lineage evidence; only the cloned durable Trace projection receives a new digest.
    execution.trace_args_digest = target_trace_args_digest.clone();
    if staged {
        execution.staged_transaction_id = Some(target_transaction_id.clone());
    }
    execution.conversation_id = target_conversation_id.to_string();
    execution.run_id = target_run_id.clone();
    if let Some(journal) = execution.delete_journal.as_ref() {
        execution.delete_journal = Some(
            journal
                .with_forked_transaction_id(&target_transaction_id)
                .map_err(|_| {
                    ConversationForkError::Other("无法重映射文件修改删除日志。".to_string())
                })?,
        );
    }
    if let Some(receipt) = execution.receipt.as_mut() {
        receipt.transaction_id = target_transaction_id.clone();
    }
    file_change.validate().map_err(|_| {
        ConversationForkError::Other("复制后的文件修改审计 action 无效。".to_string())
    })?;
    let target_action_json = serde_json::to_string(&action).map_err(|_| {
        ConversationForkError::Other("无法序列化复制后的文件修改审计 action。".to_string())
    })?;
    let target_file_change_result_json = remap_file_change_result_json(
        audit.file_change_result_json.as_deref(),
        &source_transaction_id,
        &target_transaction_id,
    )?;
    let target_tool_result_json = remap_file_change_tool_result_json(
        audit.tool_result_json.as_deref(),
        &source_tool_call_id,
        &target_tool_call_id,
        &source_transaction_id,
        &target_transaction_id,
    )?;

    Ok(AgentActionAuditRecord {
        action_id: crate::canonical_pending_action_id(&target_run_id, &target_tool_call_id),
        run_id: target_run_id,
        conversation_id: Some(target_conversation_id.to_string()),
        assistant_message_id: Some(target_assistant_message_id),
        action_type: audit.action_type,
        tool_name: audit.tool_name,
        decision: audit.decision,
        status: audit.status,
        action_json: target_action_json,
        file_change_result_json: target_file_change_result_json,
        command_result_json: None,
        tool_result_json: target_tool_result_json,
        error: audit.error,
        created_at: audit.created_at,
        decided_at: audit.decided_at,
        completed_at: audit.completed_at,
        effective_permissions_json: audit.effective_permissions_json,
        path_scope: audit.path_scope,
        command_cwd_scope: audit.command_cwd_scope,
        blocked_reason: audit.blocked_reason,
        decision_source: audit.decision_source,
    })
}

fn remap_file_change_result_json(
    raw: Option<&str>,
    source_transaction_id: &str,
    target_transaction_id: &str,
) -> Result<Option<String>, ConversationForkError> {
    raw.map(|raw| {
        let mut result = serde_json::from_str::<AgentFileChangeResult>(raw)
            .map_err(|_| ConversationForkError::Other("文件修改审计 result 无效。".to_string()))?;
        if result.transaction_id != source_transaction_id {
            return Err(ConversationForkError::Other(
                "文件修改审计 result 的事务身份不一致。".to_string(),
            ));
        }
        result.transaction_id = target_transaction_id.to_string();
        serde_json::to_string(&result).map_err(|_| {
            ConversationForkError::Other("无法序列化复制后的文件修改 result。".to_string())
        })
    })
    .transpose()
}

fn remap_file_change_tool_result_json(
    raw: Option<&str>,
    source_tool_call_id: &str,
    target_tool_call_id: &str,
    source_transaction_id: &str,
    target_transaction_id: &str,
) -> Result<Option<String>, ConversationForkError> {
    raw.map(|raw| {
        let mut tool_result = serde_json::from_str::<AgentToolResult>(raw).map_err(|_| {
            ConversationForkError::Other("文件修改审计 ToolResult 无效。".to_string())
        })?;
        if tool_result.call_id != source_tool_call_id || tool_result.tool != "apply_patch" {
            return Err(ConversationForkError::Other(
                "文件修改审计 ToolResult 的调用身份不一致。".to_string(),
            ));
        }
        tool_result.call_id = target_tool_call_id.to_string();
        if let Some(value) = tool_result.result.take() {
            let mut result =
                serde_json::from_value::<AgentFileChangeResult>(value).map_err(|_| {
                    ConversationForkError::Other("文件修改审计 ToolResult 的结果无效。".to_string())
                })?;
            if result.transaction_id != source_transaction_id {
                return Err(ConversationForkError::Other(
                    "文件修改审计 ToolResult 的事务身份不一致。".to_string(),
                ));
            }
            result.transaction_id = target_transaction_id.to_string();
            tool_result.result = Some(serde_json::to_value(result).map_err(|_| {
                ConversationForkError::Other("无法序列化复制后的文件修改 ToolResult。".to_string())
            })?);
        }
        serde_json::to_string(&tool_result).map_err(|_| {
            ConversationForkError::Other("无法序列化复制后的文件修改 ToolResult。".to_string())
        })
    })
    .transpose()
}

fn remap_file_change_observation(
    source_change: &AgentFileChangeRecord,
    target_conversation_id: &str,
    target_run_id: &str,
    target_observation_id: &str,
    tool_call_id_map: &HashMap<String, String>,
) -> Result<(String, String), ConversationForkError> {
    let source_observation_id = source_change.observation_id.as_str();
    let source_observation_json = source_change.observation_json.as_str();
    if source_observation_id.is_empty() || source_observation_json.is_empty() {
        return Err(ConversationForkError::Other(
            "文件变更 observation 持久记录不完整。".to_string(),
        ));
    }
    let mut checkpoint = serde_json::from_str::<crate::file_change::FileObservationCheckpoint>(
        source_observation_json,
    )
    .map_err(|_| ConversationForkError::Other("文件变更 observation 持久记录无效。".to_string()))?;
    let observation_matches_operation = match (&source_change.operation, &checkpoint.state) {
        (operation, crate::file_change::FileObservationState::Missing) => operation == "create",
        (operation, crate::file_change::FileObservationState::Existing { revision, .. }) => {
            operation == "update" && source_change.base_revision.as_deref() == Some(revision)
        }
    };
    if !observation_matches_operation {
        return Err(ConversationForkError::Other(
            "文件变更 observation 与 operation/base revision 不一致。".to_string(),
        ));
    }
    let canonical_target = checkpoint.canonical_target.clone();
    checkpoint
        .validate_frozen_binding(
            &source_change.conversation_id,
            &source_change.run_id,
            std::path::Path::new(&canonical_target),
        )
        .map_err(|_| {
            ConversationForkError::Other("文件变更 observation 与源事务不一致。".to_string())
        })?;
    if checkpoint.observation_id != source_observation_id {
        return Err(ConversationForkError::Other(
            "文件变更 observation 标识与源事务不一致。".to_string(),
        ));
    }
    checkpoint.observation_id = target_observation_id.to_string();
    checkpoint.source_tool_call_id = tool_call_id_map
        .get(&checkpoint.source_tool_call_id)
        .cloned()
        .ok_or_else(|| {
            ConversationForkError::Other(
                "文件读取 observation Tool Call 不在分叉历史中。".to_string(),
            )
        })?;
    checkpoint.conversation_id = target_conversation_id.to_string();
    checkpoint.run_id = target_run_id.to_string();
    checkpoint
        .validate_frozen_binding(
            target_conversation_id,
            target_run_id,
            std::path::Path::new(&canonical_target),
        )
        .map_err(|_| {
            ConversationForkError::Other("复制后的文件变更 observation 无效。".to_string())
        })?;
    let target_observation_json = serde_json::to_string(&checkpoint)
        .map_err(|_| ConversationForkError::Other("无法复制文件变更 observation。".to_string()))?;
    Ok((target_observation_id.to_string(), target_observation_json))
}

fn history_from_single_plan(
    plan: ConversationForkPlan,
) -> Result<
    (
        ConversationHistoryForkPlan,
        Vec<ProviderContinuationForkMapping>,
    ),
    ConversationForkError,
> {
    if plan.collaboration_root.is_some() || !plan.members.is_empty() {
        return Err(ConversationForkError::Other(
            "成员历史计划意外包含新的 Agent 树身份。".to_string(),
        ));
    }
    Ok((
        ConversationHistoryForkPlan {
            source_conversation_id: plan.source_conversation_id,
            source_message_id: Some(plan.source_message_id),
            target: plan.target,
            attachments: plan.attachments,
            archives: plan.archives,
            traces: plan.traces,
            action_audits: plan.action_audits,
            turn_diffs: plan.turn_diffs,
            guidances: plan.guidances,
            file_changes: plan.file_changes,
            summaries: plan.summaries,
            compaction_receipts: plan.compaction_receipts,
            provider_transition_receipts: plan.provider_transition_receipts,
            world_state_records: plan.world_state_records,
            requires_context_adaptation: plan.requires_context_adaptation,
            adaptation_source_summary_id: plan.adaptation_source_summary_id,
            message_id_map: plan.message_id_map,
            snapshot_origins: plan.snapshot_origins,
            human_interaction_requests: plan.human_interaction_requests,
            id_replacements: plan.id_replacements,
        },
        plan.provider_continuation_mappings,
    ))
}

fn build_message_only_history_plan(
    connection: &Connection,
    source: &ChatConversationRecord,
    message_limit: Option<usize>,
    target_conversation_id: &str,
    created_at: i64,
    global_id_replacements: &HashMap<String, String>,
) -> Result<ConversationHistoryForkPlan, ConversationForkError> {
    let source_messages = message_limit
        .map(|limit| source.messages[..=limit].to_vec())
        .unwrap_or_default();
    let source_message_ids = source_messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let message_id_map = source_messages
        .iter()
        .map(|message| {
            (
                message.id.clone(),
                global_id_replacements
                    .get(&message.id)
                    .cloned()
                    .unwrap_or_else(|| new_id("message")),
            )
        })
        .collect::<HashMap<_, _>>();
    let source_attachments = attachment_repository::list_message_attachments_for_fork(
        connection,
        &source.id,
        &source_message_ids,
    )
    .map_err(database_error)?;
    let attachment_id_map = source_attachments
        .iter()
        .map(|attachment| {
            (
                attachment.id.clone(),
                global_id_replacements
                    .get(&attachment.id)
                    .cloned()
                    .unwrap_or_else(|| new_id("attachment")),
            )
        })
        .collect::<HashMap<_, _>>();
    let mut replacements = global_id_replacements.clone();
    replacements.extend(message_id_map.clone());
    replacements.extend(attachment_id_map.clone());
    replacements.insert(source.id.clone(), target_conversation_id.to_string());
    let snapshot_origins =
        fork_snapshot_origins(connection, source, &source_messages, &message_id_map)?;
    let mut target_messages = source_messages
        .iter()
        .map(|message| clone_message(message, &message_id_map, &replacements))
        .collect::<Result<Vec<_>, _>>()?;
    for message in &mut target_messages {
        message.ui_state_json = None;
    }
    let attachments = source_attachments
        .into_iter()
        .map(|source_attachment| {
            Ok(ForkAttachmentCopy {
                target: AttachmentRecord {
                    id: mapped_id(&attachment_id_map, &source_attachment.id, "附件")?,
                    conversation_id: target_conversation_id.to_string(),
                    message_id: mapped_id(
                        &message_id_map,
                        &source_attachment.message_id,
                        "附件所属消息",
                    )?,
                    project_id: source.project_id.clone(),
                    kind: source_attachment.kind.clone(),
                    original_name: source_attachment.original_name.clone(),
                    mime_type: source_attachment.mime_type.clone(),
                    size_bytes: source_attachment.size_bytes,
                    storage_rel_path: String::new(),
                    created_at: source_attachment.created_at,
                },
                source: source_attachment,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ConversationHistoryForkPlan {
        source_conversation_id: source.id.clone(),
        source_message_id: source_messages.last().map(|message| message.id.clone()),
        target: ChatConversationRecord {
            id: target_conversation_id.to_string(),
            project_id: source.project_id.clone(),
            model_id: source.model_id.clone(),
            title: source.title.clone(),
            messages: target_messages,
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        },
        attachments,
        archives: Vec::new(),
        traces: Vec::new(),
        action_audits: Vec::new(),
        turn_diffs: Vec::new(),
        guidances: Vec::new(),
        file_changes: Vec::new(),
        summaries: Vec::new(),
        compaction_receipts: Vec::new(),
        provider_transition_receipts: Vec::new(),
        world_state_records: Vec::new(),
        requires_context_adaptation: false,
        adaptation_source_summary_id: None,
        message_id_map,
        snapshot_origins,
        // A message-only member history cannot carry an open question: questions are only ever
        // admitted for root conversations.
        human_interaction_requests: Vec::new(),
        id_replacements: replacements,
    })
}
