fn resolve_fork_point(
    connection: &Connection,
    source: &ChatConversationRecord,
    fork_point: &ConversationForkPoint,
    active_chain: &[context_compaction_repository::ContextCompactionSummaryVersion],
) -> Result<ResolvedConversationForkPoint, ConversationForkError> {
    match fork_point {
        ConversationForkPoint::Latest {} => {
            let message = source
                .messages
                .last()
                .ok_or_else(|| "当前聊天没有可分支的历史。".to_string())?;
            if message.role != "assistant" {
                return Err("最新位置尚未形成完整回复，暂时无法创建分支。"
                    .to_string()
                    .into());
            }
            Ok(ResolvedConversationForkPoint {
                assistant_message_id: message.id.clone(),
                summary_id: active_chain
                    .last()
                    .map(|version| version.summary.id.clone()),
                model_id: source.model_id.clone(),
            })
        }
        ConversationForkPoint::AssistantReply {
            assistant_message_id,
        } => Ok(ResolvedConversationForkPoint {
            assistant_message_id: assistant_message_id.clone(),
            summary_id: None,
            model_id: assistant_message_model_id(connection, &source.id, assistant_message_id)?,
        }),
        ConversationForkPoint::ManualCompactionBoundary { operation_id } => {
            let (operation, receipt) =
                load_manual_compaction_boundary(connection, &source.id, operation_id)?;
            let version = active_chain
                .iter()
                .find(|version| {
                    Some(version.summary.id.as_str()) == operation.summary_id.as_deref()
                })
                .ok_or_else(|| "该手动压缩摘要已不在当前可分支的历史链中。".to_string())?;
            if version.summary.conversation_id != source.id
                || version.lineage.introduced_by_assistant_message_id
                    != receipt.assistant_message_id
                || version.summary.covered_through != receipt.plan.covered_through
                || receipt.source_revision.as_deref()
                    != Some(version.summary.source_revision.as_str())
            {
                return Err("手动压缩分支边界的摘要身份或历史覆盖范围不一致。"
                    .to_string()
                    .into());
            }
            Ok(ResolvedConversationForkPoint {
                assistant_message_id: receipt.assistant_message_id,
                summary_id: operation.summary_id,
                model_id: operation.model_id,
            })
        }
        ConversationForkPoint::ProviderTransitionBoundary { operation_id } => {
            if !operation_id.starts_with("provider-transition-") {
                return Err("Provider transition 分叉边界无效。".to_string().into());
            }
            let receipt =
                context_compaction_receipt_repository::get_receipt(connection, operation_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "找不到指定的 Provider transition 分叉边界。".to_string())?;
            let summary_id = receipt
                .summary_id
                .as_deref()
                .ok_or_else(|| "指定的 Provider transition 没有已提交摘要。".to_string())?;
            if receipt.operation_id != *operation_id
                || receipt.conversation_id != source.id
                || receipt.status != ContextCompactionReceiptStatus::Applied
                || receipt.stage != ContextCompactionReceiptStage::Completed
                || receipt
                    .result
                    .as_ref()
                    .map(|result| result.summary_id.as_str())
                    != Some(summary_id)
            {
                return Err("Provider transition 分叉边界与已提交记录不一致。"
                    .to_string()
                    .into());
            }
            let version = active_chain
                .iter()
                .find(|version| version.summary.id == summary_id)
                .ok_or_else(|| {
                    "Provider transition 摘要已不在当前受支持的 active 历史链中。".to_string()
                })?;
            if version.summary.conversation_id != source.id
                || version.lineage.introduced_by_assistant_message_id
                    != receipt.assistant_message_id
                || version.summary.covered_through != receipt.plan.covered_through
                || receipt.source_revision.as_deref()
                    != Some(version.summary.source_revision.as_str())
            {
                return Err("Provider transition 摘要身份或历史覆盖范围不匹配。"
                    .to_string()
                    .into());
            }
            let target_model_config_id = receipt
                .model_config_id
                .clone()
                .unwrap_or_else(|| receipt.model.clone());
            Ok(ResolvedConversationForkPoint {
                assistant_message_id: receipt.assistant_message_id,
                summary_id: Some(summary_id.to_string()),
                model_id: Some(target_model_config_id),
            })
        }
    }
}

/// A cloned manual operation has a fresh generic operation ID. Its durable owner and applied
/// receipt establish the boundary identity; an ID prefix cannot establish that identity.
fn load_manual_compaction_boundary(
    connection: &Connection,
    conversation_id: &str,
    operation_id: &str,
) -> Result<
    (
        crate::storage::models::ManualContextCompactionOperation,
        ContextCompactionReceipt,
    ),
    ConversationForkError,
> {
    let operation = crate::storage::manual_context_compaction_repository::get(
        connection,
        conversation_id,
        Some(operation_id),
    )?
    .ok_or_else(|| "找不到指定的手动压缩分支边界。".to_string())?;
    let receipt = context_compaction_receipt_repository::get_receipt(connection, operation_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "手动压缩分支边界缺少已提交记录。".to_string())?;
    if operation.operation_id != operation_id
        || operation.conversation_id != conversation_id
        || operation.status != "completed"
        || operation.phase != "committing"
        || operation
            .model_id
            .as_deref()
            .is_none_or(|model| model.trim().is_empty())
        || operation.summary_id.is_none()
        || operation.assistant_message_id.as_deref() != Some(receipt.assistant_message_id.as_str())
        || operation.covered_through_message_id != operation.assistant_message_id
        || operation.completed_at.is_none()
        || operation.completed_at < receipt.completed_at
        || receipt.operation_id != operation_id
        || receipt.conversation_id != conversation_id
        || receipt.status != ContextCompactionReceiptStatus::Applied
        || receipt.stage != ContextCompactionReceiptStage::Completed
        || receipt.model_config_id != operation.model_id
        || receipt.summary_id != operation.summary_id
        || receipt
            .result
            .as_ref()
            .map(|result| result.summary_id.as_str())
            != operation.summary_id.as_deref()
        || receipt.plan.covered_through
            != ContextJournalCursor::message(&receipt.assistant_message_id)
    {
        return Err("手动压缩分支边界与已完成操作记录不一致。"
            .to_string()
            .into());
    }
    Ok((operation, receipt))
}

fn assistant_message_model_id(
    connection: &Connection,
    conversation_id: &str,
    assistant_message_id: &str,
) -> Result<Option<String>, ConversationForkError> {
    let usage_model = connection
        .query_row(
            "SELECT model_id
             FROM agent_usage_records
             WHERE conversation_id = ?1 AND message_id = ?2",
            params![conversation_id, assistant_message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if let Some(model_id) = usage_model {
        if model_id.trim().is_empty() {
            return Err("回复绑定的模型身份无效。".to_string().into());
        }
        return Ok(Some(model_id));
    }
    let observed_model = connection
        .query_row(
            "SELECT model
             FROM model_request_observations
             WHERE conversation_id = ?1
               AND assistant_message_id = ?2
               AND purpose = 'agent_loop'
             ORDER BY request_index DESC, completed_at DESC, id DESC
             LIMIT 1",
            params![conversation_id, assistant_message_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if observed_model
        .as_deref()
        .is_some_and(|model_id| model_id.trim().is_empty())
    {
        return Err("回复绑定的模型请求身份无效。".to_string().into());
    }
    Ok(observed_model)
}

fn ensure_no_active_command_sessions(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), ConversationForkError> {
    let active_session_count = connection
        .query_row(
            "SELECT COUNT(*)
             FROM agent_command_sessions
             WHERE conversation_id = ?1
               AND status IN ('starting', 'running')",
            [conversation_id],
            |row| row.get::<_, u64>(0),
        )
        .map_err(database_error)?;
    if active_session_count == 0 {
        return Ok(());
    }
    Err(ConversationForkError::ActiveCommandSession {
        conversation_id: conversation_id.to_string(),
        active_session_count,
    })
}

fn ensure_no_active_conversation_turn(
    connection: &Connection,
    conversation_id: &str,
) -> Result<(), ConversationForkError> {
    let active_run_id = connection
        .query_row(
            "SELECT run_id
             FROM conversation_turn_traces
             WHERE conversation_id = ?1 AND terminal_status = 'in_progress'
             LIMIT 1",
            [conversation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(database_error)?;
    if active_run_id.is_some() {
        return Err(ConversationForkError::Other(
            "根 Agent 仍有活跃 Turn，结束后才能继续新任务。".to_string(),
        ));
    }
    Ok(())
}

fn ensure_agent_tree_stable(
    connection: &Connection,
    conversation_ids: &[String],
    agent_ids: &[String],
) -> Result<(), ConversationForkError> {
    for conversation_id in conversation_ids {
        ensure_no_active_conversation_turn(connection, conversation_id)?;
        ensure_no_active_command_sessions(connection, conversation_id)?;
        let has_unsettled_execution = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_pending_actions
                     WHERE conversation_id = ?1
                       AND status IN ('pending', 'approved', 'executing')
                     UNION ALL
                     SELECT 1 FROM context_compaction_receipts
                     WHERE conversation_id = ?1 AND status = 'in_progress'
                     UNION ALL
                     SELECT 1 FROM manual_context_compaction_operations
                     WHERE conversation_id = ?1 AND status = 'running'
                 )",
                [conversation_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if has_unsettled_execution {
            return Err(ConversationForkError::Other(
                "Agent 树仍有未稳定的执行事实，结束后才能继续新任务。".to_string(),
            ));
        }
    }
    for agent_id in agent_ids {
        let has_unsettled_wake = connection
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_wake_requests
                     WHERE agent_id = ?1
                       AND status IN ('queued', 'claimed', 'running', 'waiting_for_approval')
                 )",
                [agent_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(database_error)?;
        if has_unsettled_wake {
            return Err(ConversationForkError::Other(
                "Agent 树仍有未稳定的 Wake 执行，结束后才能继续新任务。".to_string(),
            ));
        }
    }
    Ok(())
}

pub(crate) fn validate_fork_point_input(
    request_id: &str,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
) -> Result<(), String> {
    for (label, value) in [
        ("请求 ID", request_id),
        ("原任务 ID", source_conversation_id),
    ] {
        if value.trim().is_empty() || value.trim() != value || value.chars().count() > 512 {
            return Err(format!("{label}无效。"));
        }
    }
    let (label, value, maximum) = match fork_point {
        ConversationForkPoint::Latest {} => return Ok(()),
        ConversationForkPoint::AssistantReply {
            assistant_message_id,
        } => ("回复 ID", assistant_message_id.as_str(), 512),
        ConversationForkPoint::ProviderTransitionBoundary { operation_id } => (
            "Provider transition operation ID",
            operation_id.as_str(),
            1024,
        ),
        ConversationForkPoint::ManualCompactionBoundary { operation_id } => {
            ("手动压缩 operation ID", operation_id.as_str(), 1024)
        }
    };
    if value.trim().is_empty() || value.trim() != value || value.chars().count() > maximum {
        return Err(format!("{label}无效。"));
    }
    Ok(())
}

fn ensure_settled_assistant(
    message: &ChatMessageRecord,
    trace: Option<&ConversationTurnTrace>,
) -> Result<(), String> {
    if message.role != "assistant" {
        return Ok(());
    }
    let agent_status = agent_run_status(message.agent_run_json.as_deref())?;
    let agent_is_terminal = agent_status
        .as_deref()
        .is_none_or(|status| matches!(status, "idle" | "completed" | "failed" | "cancelled"));
    if trace.is_some_and(|trace| !trace.terminal_status.is_terminal())
        || message.status.as_deref() == Some("pending")
        || !agent_is_terminal
    {
        return Err("这条回复仍在生成，结束后才能在新任务中继续。".to_string());
    }
    Ok(())
}

fn trace_times(connection: &Connection, message_id: &str) -> Result<(i64, i64), String> {
    connection
        .query_row(
            "SELECT created_at, COALESCE(completed_at, updated_at)
             FROM conversation_turn_traces
             WHERE assistant_message_id = ?1",
            [message_id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(database_error)
}

fn summaries_visible_at_assistant_reply(
    connection: &Connection,
    conversation_id: &str,
    chain: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
) -> Result<Vec<context_compaction_repository::ContextCompactionSummaryVersion>, String> {
    let mut visible = Vec::new();
    let mut previous_owner_position = None;
    let mut crossed_cutoff = false;
    for version in chain {
        let owner_position = source_positions
            .get(&version.lineage.introduced_by_assistant_message_id)
            .copied()
            .ok_or_else(|| "摘要链引用了不属于原任务的生成回复。".to_string())?;
        if previous_owner_position.is_some_and(|previous| owner_position < previous) {
            return Err("摘要链的因果顺序无效。".to_string());
        }
        previous_owner_position = Some(owner_position);
        if owner_position > cutoff {
            crossed_cutoff = true;
            continue;
        }
        if owner_position == cutoff {
            let transition = context_compaction_receipt_repository::get_applied_provider_transition_receipt_for_summary(
                connection,
                conversation_id,
                &version.summary.id,
            )
            .map_err(|error| error.to_string())?;
            // A Provider-transition summary owned by this reply is displayed after the reply's
            // Fork action. Once crossed, every later summary belongs after that timeline point,
            // even if it happens to reuse the same owner message.
            let manual = connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM manual_context_compaction_operations WHERE conversation_id = ?1 AND summary_id = ?2 AND status = 'completed')",
                params![conversation_id, &version.summary.id],
                |row| row.get::<_, bool>(0),
            ).map_err(database_error)?;
            if transition.is_some() || manual || crossed_cutoff {
                crossed_cutoff = true;
                continue;
            }
        } else if crossed_cutoff {
            return Err("摘要链跨越分叉边界后又回到了更早轮次。".to_string());
        }
        visible.push(version);
    }
    Ok(visible)
}

fn summaries_visible_through_transition_boundary(
    chain: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
    boundary_summary_id: &str,
) -> Result<Vec<context_compaction_repository::ContextCompactionSummaryVersion>, String> {
    let boundary_index = chain
        .iter()
        .position(|version| version.summary.id == boundary_summary_id)
        .ok_or_else(|| "Provider transition 摘要不在 active 摘要链中。".to_string())?;
    let visible = chain
        .into_iter()
        .take(boundary_index + 1)
        .collect::<Vec<_>>();
    let mut previous_owner_position = None;
    for version in &visible {
        let owner_position = source_positions
            .get(&version.lineage.introduced_by_assistant_message_id)
            .copied()
            .ok_or_else(|| "摘要链引用了不属于原任务的生成回复。".to_string())?;
        if owner_position > cutoff
            || previous_owner_position.is_some_and(|previous| owner_position < previous)
        {
            return Err("Provider transition 摘要链的因果顺序无效。".to_string());
        }
        previous_owner_position = Some(owner_position);
    }
    Ok(visible)
}

fn summaries_visible_at_time(
    connection: &Connection,
    conversation_id: &str,
    summaries: Vec<context_compaction_repository::ContextCompactionSummaryVersion>,
    cutoff_at: i64,
) -> Result<Vec<context_compaction_repository::ContextCompactionSummaryVersion>, String> {
    let receipt_completed_at_by_summary =
        context_compaction_receipt_repository::list_receipts_for_conversation(
            connection,
            conversation_id,
        )
        .map_err(|error| error.to_string())?
        .into_iter()
        .filter_map(|receipt| {
            receipt.summary_id.clone().map(|summary_id| {
                (
                    summary_id,
                    receipt.completed_at.unwrap_or(receipt.updated_at),
                )
            })
        })
        .collect::<HashMap<_, _>>();
    let mut visible = Vec::new();
    for version in summaries {
        if version.summary.created_at > cutoff_at {
            break;
        }
        if receipt_completed_at_by_summary
            .get(&version.summary.id)
            .is_some_and(|completed_at| *completed_at > cutoff_at)
        {
            break;
        }
        visible.push(version);
    }
    Ok(visible)
}

fn collect_visible_provider_transition_receipts(
    connection: &Connection,
    source_conversation_id: &str,
    visible_summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
) -> Result<Vec<ForkProviderTransitionReceipt>, String> {
    let mut receipts = Vec::new();
    for version in visible_summaries {
        let Some(receipt) = context_compaction_receipt_repository::get_applied_provider_transition_receipt_for_summary(
            connection,
            source_conversation_id,
            &version.summary.id,
        )
        .map_err(|error| error.to_string())?
        else {
            continue;
        };
        if receipt.conversation_id != source_conversation_id
            || receipt.assistant_message_id != version.lineage.introduced_by_assistant_message_id
            || receipt.summary_id.as_deref() != Some(version.summary.id.as_str())
            || receipt.plan.previous_summary_id != version.summary.previous_summary_id
            || receipt.plan.covered_through != version.summary.covered_through
            || receipt.source_revision.as_deref() != Some(version.summary.source_revision.as_str())
        {
            return Err(
                "Provider transition receipt 与可见摘要的身份或覆盖范围不一致。".to_string(),
            );
        }
        let observation_id = receipt
            .generation_observation_id
            .as_deref()
            .ok_or_else(|| "Provider transition receipt 缺少模型请求观测。".to_string())?;
        let observation =
            model_request_observation_repository::get_observation(connection, observation_id)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| {
                    "Provider transition receipt 引用的模型请求观测不存在。".to_string()
                })?;
        receipt
            .validate_generation_observation(&observation)
            .map_err(|error| error.to_string())?;
        receipts.push(ForkProviderTransitionReceipt {
            source_receipt: receipt,
            source_observation: observation,
            target_operation_id: new_id("provider-transition"),
            target_run_id: new_id("context-compaction-run"),
            target_observation_id: new_id("model-request-observation"),
        });
    }
    Ok(receipts)
}

fn collect_visible_context_compaction_receipts(
    connection: &Connection,
    source_conversation_id: &str,
    message_id_map: &HashMap<String, String>,
    run_id_map: &HashMap<String, String>,
    visible_summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
    cutoff_at: Option<i64>,
) -> Result<Vec<ForkContextCompactionReceipt>, String> {
    let visible_summary_ids = visible_summaries
        .iter()
        .map(|version| version.summary.id.as_str())
        .collect::<HashSet<_>>();
    let mut copies = Vec::new();
    let mut receipt_run_id_map = run_id_map.clone();
    for receipt in context_compaction_receipt_repository::list_receipts_for_conversation(
        connection,
        source_conversation_id,
    )
    .map_err(|error| error.to_string())?
    {
        if receipt.operation_id.starts_with("provider-transition-")
            || !receipt.status.is_terminal()
            || !message_id_map.contains_key(&receipt.assistant_message_id)
            || cutoff_at
                .is_some_and(|cutoff| receipt.completed_at.unwrap_or(receipt.updated_at) > cutoff)
        {
            continue;
        }
        if receipt.conversation_id != source_conversation_id {
            return Err("上下文压缩 receipt 的 Conversation 身份无效。".to_string());
        }
        remap_cursor(&receipt.plan.covered_through, message_id_map)?;
        if receipt
            .plan
            .previous_summary_id
            .as_deref()
            .is_some_and(|summary_id| !visible_summary_ids.contains(summary_id))
            || receipt
                .summary_id
                .as_deref()
                .is_some_and(|summary_id| !visible_summary_ids.contains(summary_id))
        {
            continue;
        }
        let source_observation = receipt
            .generation_observation_id
            .as_deref()
            .map(|observation_id| {
                model_request_observation_repository::get_observation(connection, observation_id)
                    .map_err(|error| error.to_string())?
                    .ok_or_else(|| "上下文压缩 receipt 引用的模型请求观测不存在。".to_string())
            })
            .transpose()?;
        if let Some(observation) = &source_observation {
            receipt
                .validate_generation_observation(observation)
                .map_err(|error| error.to_string())?;
            if cutoff_at.is_some_and(|cutoff| observation.completed_at > cutoff) {
                continue;
            }
        }
        let target_run_id = receipt_run_id_map
            .entry(receipt.run_id.clone())
            .or_insert_with(|| new_id("context-compaction-run"))
            .clone();
        copies.push(ForkContextCompactionReceipt {
            target_run_id,
            target_operation_id: new_id("context-compaction"),
            target_observation_id: source_observation
                .as_ref()
                .map(|_| new_id("model-request-observation")),
            source_receipt: receipt,
            source_observation,
        });
    }
    Ok(copies)
}

fn world_state_records_visible_at_cutoff(
    connection: &Connection,
    conversation_id: &str,
    source_positions: &HashMap<String, usize>,
    cutoff: usize,
    visible_summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
    cutoff_at: Option<i64>,
) -> Result<Vec<world_state_repository::ConversationWorldStateJournalEntry>, String> {
    let epochs = {
        let mut statement = connection
            .prepare(
                "SELECT epoch_id, base_summary_id, created_at
                 FROM conversation_world_state_epochs
                 WHERE conversation_id = ?1
                 ORDER BY generation DESC",
            )
            .map_err(database_error)?;
        let rows = statement
            .query_map([conversation_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(database_error)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(database_error)?;
        rows
    };
    if epochs.is_empty() {
        return Ok(Vec::new());
    }
    let visible_summary_ids = visible_summaries
        .iter()
        .map(|version| version.summary.id.as_str())
        .collect::<HashSet<_>>();
    let Some((epoch_id, selected_base_summary_id, _)) =
        epochs.into_iter().find(|(_, base_summary_id, created_at)| {
            cutoff_at.is_none_or(|cutoff| *created_at <= cutoff)
                && base_summary_id
                    .as_deref()
                    .is_none_or(|summary_id| visible_summary_ids.contains(summary_id))
        })
    else {
        return Ok(Vec::new());
    };
    let mut entries =
        world_state_repository::list_records_for_epoch(connection, conversation_id, &epoch_id)
            .map_err(|error| error.to_string())?
            .into_iter()
            .map(world_state_repository::ConversationWorldStateJournalEntry::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
    let replacements = conversation_turn_rewrite_repository::replacement_message_ids_by_source(
        connection,
        conversation_id,
    )
    .map_err(database_error)?;
    for entry in &mut entries {
        if let Some(anchor) = entry.effective_before_message_id.as_deref() {
            entry.effective_before_message_id = Some(
                conversation_turn_rewrite_repository::resolve_active_message_id(
                    &replacements,
                    anchor,
                )?,
            );
        }
    }
    let Some(first) = entries.first() else {
        return Err("选中的 World State epoch 没有 initial full snapshot。".to_string());
    };
    let WorldStateRecord::Full(_) = &first.record else {
        return Err("选中的 World State epoch 没有 initial full snapshot。".to_string());
    };
    if first.effective_before_message_id.is_some() {
        return Err("选中的 World State full snapshot 不能绑定消息 anchor。".to_string());
    }
    let base_summary_id = first.base_summary_id.as_deref();
    if base_summary_id != selected_base_summary_id.as_deref() {
        return Err("World State epoch 索引与记录的 summary 边界不一致。".to_string());
    }
    if entries
        .iter()
        .any(|entry| entry.base_summary_id.as_deref() != base_summary_id)
    {
        return Err("选中的 World State epoch 的 summary 边界不一致。".to_string());
    }

    let mut visible = Vec::new();
    let mut crossed_cutoff = false;
    for entry in entries {
        if cutoff_at.is_some_and(|cutoff_at| entry.created_at > cutoff_at) {
            crossed_cutoff = true;
            continue;
        }
        let is_visible =
            match entry.effective_before_message_id.as_deref() {
                None => true,
                Some(message_id) => {
                    source_positions.get(message_id).copied().ok_or_else(|| {
                        format!("World State anchor 不属于原任务消息：{message_id}")
                    })? <= cutoff
                }
            };
        if is_visible {
            if crossed_cutoff {
                return Err("World State diff 顺序跨越分叉边界后又回到可见历史。".to_string());
            }
            visible.push(entry);
        } else {
            crossed_cutoff = true;
        }
    }
    Ok(visible)
}
