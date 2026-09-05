fn clone_summary_chain_for_history(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
) -> Result<HashMap<String, String>, String> {
    clone_summary_chain_core(
        connection,
        history.source_conversation_id,
        &history.target.id,
        history.target.created_at,
        history.summaries,
        history.message_id_map,
        history.id_replacements,
    )
}

pub(crate) fn clone_child_snapshot_summary_chain(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    target_created_at: i64,
    summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
    message_id_map: &HashMap<String, String>,
    id_replacements: &HashMap<String, String>,
) -> Result<(), String> {
    clone_summary_chain_core(
        connection,
        source_conversation_id,
        target_conversation_id,
        target_created_at,
        summaries,
        message_id_map,
        id_replacements,
    )
    .map(|_| ())
}

fn clone_summary_chain_core(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    target_created_at: i64,
    summaries: &[context_compaction_repository::ContextCompactionSummaryVersion],
    message_id_map: &HashMap<String, String>,
    id_replacements: &HashMap<String, String>,
) -> Result<HashMap<String, String>, String> {
    if summaries.is_empty() {
        return Ok(HashMap::new());
    }
    let mut summary_id_map = HashMap::new();
    let mut latest_summary_id = None;
    for version in summaries {
        let source = &version.summary;
        let summary_id = id_replacements
            .get(&source.id)
            .cloned()
            .unwrap_or_else(|| new_id("context-summary"));
        let covered_through = remap_cursor(&source.covered_through, message_id_map)?;
        let continuity = remap_continuity(&source.continuity, message_id_map, id_replacements)?;
        let previous_summary_id = source
            .previous_summary_id
            .as_ref()
            .map(|previous| mapped_id(&summary_id_map, previous, "上一版摘要"))
            .transpose()?;
        let source_revision = context_compaction_repository::source_revision_for_cursor(
            connection,
            target_conversation_id,
            &covered_through,
        )
        .map_err(|error| error.to_string())?;
        let summary = ContextCompactionSummary {
            schema_version: source.schema_version,
            id: summary_id.clone(),
            conversation_id: target_conversation_id.to_string(),
            source_revision,
            previous_summary_id,
            covered_through,
            content: source.content.clone(),
            continuity,
            generation: source.generation.clone(),
            source_input_tokens: source.source_input_tokens,
            summary_input_tokens: source.summary_input_tokens,
            continuity_input_tokens: source.continuity_input_tokens,
            uncovered_tail_input_tokens: source.uncovered_tail_input_tokens,
            replacement_input_tokens: source.replacement_input_tokens,
            created_at: source.created_at,
        };
        context_compaction_repository::insert_summary(connection, &summary)
            .map_err(|error| error.to_string())?;
        context_compaction_repository::insert_summary_lineage(
            connection,
            &summary,
            &context_compaction_repository::ContextCompactionSummaryLineage {
                introduced_by_assistant_message_id: mapped_id(
                    message_id_map,
                    &version.lineage.introduced_by_assistant_message_id,
                    "摘要生成回复",
                )?,
                source_conversation_id: Some(source_conversation_id.to_string()),
                source_summary_id: Some(source.id.clone()),
            },
        )
        .map_err(|error| error.to_string())?;
        summary_id_map.insert(source.id.clone(), summary_id.clone());
        latest_summary_id = Some(summary_id);
    }
    context_compaction_repository::set_active_summary_head(
        connection,
        target_conversation_id,
        latest_summary_id
            .as_deref()
            .expect("non-empty child snapshot summary chain has a head"),
        summaries.len() as u64,
        target_created_at,
    )
    .map_err(|error| error.to_string())?;
    Ok(summary_id_map)
}

fn clone_context_compaction_receipts_for_history(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
    summary_id_map: &HashMap<String, String>,
) -> Result<(), String> {
    for copy in history.compaction_receipts {
        let source = &copy.source_receipt;
        let target_assistant_message_id = mapped_id(
            history.message_id_map,
            &source.assistant_message_id,
            "上下文压缩所属消息",
        )?;
        let target_covered_through =
            remap_cursor(&source.plan.covered_through, history.message_id_map)?;
        let target_previous_summary_id = source
            .plan
            .previous_summary_id
            .as_ref()
            .map(|summary_id| mapped_id(summary_id_map, summary_id, "上一版上下文压缩摘要"))
            .transpose()?;
        let target_source_revision = context_compaction_repository::source_revision_for_cursor(
            connection,
            &history.target.id,
            &target_covered_through,
        )
        .map_err(|error| error.to_string())?;

        let mut receipt = source.clone();
        receipt.operation_id = copy.target_operation_id.clone();
        receipt.run_id = copy.target_run_id.clone();
        receipt.conversation_id = history.target.id.clone();
        receipt.assistant_message_id = target_assistant_message_id.clone();
        receipt.plan.context_revision = target_source_revision.clone();
        receipt.plan.persistent_revision = target_source_revision.clone();
        receipt.plan.previous_summary_id = target_previous_summary_id;
        receipt.plan.covered_through = target_covered_through;
        if receipt.source_revision.is_some() {
            receipt.source_revision = Some(target_source_revision.clone());
        }
        receipt.summary_id = source
            .summary_id
            .as_ref()
            .map(|summary_id| mapped_id(summary_id_map, summary_id, "上下文压缩摘要"))
            .transpose()?;
        if let Some(result) = receipt.result.as_mut() {
            result.summary_id = receipt
                .summary_id
                .clone()
                .ok_or_else(|| "上下文压缩 receipt 的结果缺少目标摘要。".to_string())?;
        }
        receipt.generation_observation_id = copy.target_observation_id.clone();

        let observation = copy
            .source_observation
            .as_ref()
            .zip(copy.target_observation_id.as_ref())
            .map(|(source_observation, target_observation_id)| {
                let mut target = source_observation.clone();
                target.id = target_observation_id.clone();
                target.run_id = copy.target_run_id.clone();
                target.conversation_id = Some(history.target.id.clone());
                target.assistant_message_id = Some(target_assistant_message_id.clone());
                target.operation_id = Some(copy.target_operation_id.clone());
                if let Some(estimate) = target.estimate.as_mut() {
                    estimate.context_revision = target_source_revision.clone();
                    estimate.persistent_revision = target_source_revision.clone();
                }
                target
            });
        if copy.source_observation.is_some() != observation.is_some() {
            return Err("上下文压缩 receipt 的目标观测映射不完整。".to_string());
        }
        if let Some(observation) = &observation {
            receipt
                .validate_generation_observation(observation)
                .map_err(|error| error.to_string())?;
        }
        receipt.validate().map_err(|error| error.to_string())?;
        record_cloned_compaction_receipt(connection, &receipt, observation.as_ref())?;
        if let Some(mut operation) = crate::storage::manual_context_compaction_repository::get(
            connection,
            &source.conversation_id,
            Some(&source.operation_id),
        )? {
            // Copy the historical divider and boundary identity; billing remains exclusively
            // owned by the source operation and is deliberately absent from the child.
            if operation.status == "completed" {
                operation.operation_id = receipt.operation_id.clone();
                operation.request_id = receipt.operation_id.clone();
                operation.conversation_id = history.target.id.clone();
                operation.assistant_message_id = Some(target_assistant_message_id.clone());
                operation.covered_through_message_id = operation
                    .covered_through_message_id
                    .as_ref()
                    .map(|id| mapped_id(history.message_id_map, id, "手动压缩显示边界"))
                    .transpose()?;
                operation.summary_id = receipt.summary_id.clone();
                crate::storage::manual_context_compaction_repository::insert_cloned_completed(
                    connection, &operation,
                )?;
            }
        }
    }
    Ok(())
}

fn record_cloned_compaction_receipt(
    connection: &Connection,
    receipt: &ContextCompactionReceipt,
    observation: Option<&ModelRequestObservation>,
) -> Result<(), String> {
    let mut planned = receipt.clone();
    planned.status = ContextCompactionReceiptStatus::InProgress;
    planned.stage = ContextCompactionReceiptStage::Planned;
    planned.source_revision = None;
    planned.generation_observation_id = None;
    planned.summary_id = None;
    planned.result = None;
    planned.error = None;
    planned.updated_at = planned.started_at;
    planned.completed_at = None;
    planned.validate().map_err(|error| error.to_string())?;
    context_compaction_receipt_repository::record_receipt_in_connection(connection, &planned, None)
        .map_err(|error| error.to_string())?;
    context_compaction_receipt_repository::record_receipt_in_connection(
        connection,
        receipt,
        observation,
    )
    .map_err(|error| error.to_string())?;
    Ok(())
}

fn clone_provider_transition_receipts_for_history(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
    summary_id_map: &HashMap<String, String>,
) -> Result<(), String> {
    for copy in history.provider_transition_receipts {
        let source_receipt = &copy.source_receipt;
        let source_summary_id = source_receipt
            .summary_id
            .as_deref()
            .ok_or_else(|| "Provider transition receipt 缺少摘要。".to_string())?;
        let target_summary_id = mapped_id(
            summary_id_map,
            source_summary_id,
            "Provider transition 摘要",
        )?;
        let target_assistant_message_id = mapped_id(
            history.message_id_map,
            &source_receipt.assistant_message_id,
            "Provider transition 所属消息",
        )?;
        let target_covered_through =
            remap_cursor(&source_receipt.plan.covered_through, history.message_id_map)?;
        let target_previous_summary_id = source_receipt
            .plan
            .previous_summary_id
            .as_ref()
            .map(|summary_id| {
                mapped_id(
                    summary_id_map,
                    summary_id,
                    "上一版 Provider transition 摘要",
                )
            })
            .transpose()?;
        let target_source_revision = context_compaction_repository::source_revision_for_cursor(
            connection,
            &history.target.id,
            &target_covered_through,
        )
        .map_err(|error| error.to_string())?;

        let mut receipt = source_receipt.clone();
        receipt.operation_id = copy.target_operation_id.clone();
        receipt.run_id = copy.target_run_id.clone();
        receipt.conversation_id = history.target.id.clone();
        receipt.assistant_message_id = target_assistant_message_id.clone();
        receipt.plan.context_revision = target_source_revision.clone();
        receipt.plan.persistent_revision = target_source_revision.clone();
        receipt.plan.previous_summary_id = target_previous_summary_id;
        receipt.plan.covered_through = target_covered_through;
        receipt.source_revision = Some(target_source_revision.clone());
        receipt.generation_observation_id = Some(copy.target_observation_id.clone());
        receipt.summary_id = Some(target_summary_id.clone());
        receipt
            .result
            .as_mut()
            .ok_or_else(|| "Provider transition receipt 缺少应用结果。".to_string())?
            .summary_id = target_summary_id;

        let mut observation = copy.source_observation.clone();
        observation.id = copy.target_observation_id.clone();
        observation.run_id = copy.target_run_id.clone();
        observation.conversation_id = Some(history.target.id.clone());
        observation.assistant_message_id = Some(target_assistant_message_id);
        observation.operation_id = Some(copy.target_operation_id.clone());
        if let Some(estimate) = observation.estimate.as_mut() {
            estimate.context_revision = target_source_revision.clone();
            estimate.persistent_revision = target_source_revision;
        }
        receipt
            .validate_generation_observation(&observation)
            .map_err(|error| error.to_string())?;
        receipt.validate().map_err(|error| error.to_string())?;

        // Planned and terminal rows stay inside the caller's fork transaction, so readers can
        // never observe a half-cloned receipt while recursive forks retain a valid boundary.
        record_cloned_compaction_receipt(connection, &receipt, Some(&observation))?;
    }
    Ok(())
}

fn clone_world_state_records_for_history(
    connection: &Connection,
    history: ConversationHistoryForkPlanRef<'_>,
    summary_id_map: &HashMap<String, String>,
) -> Result<(), String> {
    let Some(first) = history.world_state_records.first() else {
        return Ok(());
    };
    let source_base_summary_id = first.base_summary_id.as_deref();
    let target_base_summary_id = source_base_summary_id
        .map(|summary_id| mapped_id(summary_id_map, summary_id, "World State 基础摘要"))
        .transpose()?;

    for (index, entry) in history.world_state_records.iter().enumerate() {
        if entry.conversation_id != history.source_conversation_id
            || entry.epoch_generation != first.epoch_generation
            || entry.base_summary_id.as_deref() != source_base_summary_id
            || entry.record.epoch_id() != first.record.epoch_id()
            || entry.record.sequence() != index as u64
        {
            return Err("待克隆的 active World State epoch 不是连续且一致的前缀。".to_string());
        }
        let target_anchor = entry
            .effective_before_message_id
            .as_ref()
            .map(|message_id| mapped_id(history.message_id_map, message_id, "World State anchor"))
            .transpose()?;
        let outcome = world_state_repository::append_record_in_connection(
            connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id: &history.target.id,
                epoch_generation: 1,
                base_summary_id: target_base_summary_id.as_deref(),
                effective_before_message_id: target_anchor.as_deref(),
                record: &entry.record,
                created_at: entry.created_at,
            },
        )
        .map_err(|error| error.to_string())?;
        if outcome != world_state_repository::ConversationWorldStateAppendOutcome::Inserted {
            return Err("新任务 World State 出现意外的幂等写入。".to_string());
        }
    }
    Ok(())
}

fn rewrite_world_state_records(
    entries: &mut [world_state_repository::ConversationWorldStateJournalEntry],
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    let Some(first) = entries.first() else {
        return Ok(());
    };
    let source_initial = match &first.record {
        WorldStateRecord::Full(source_initial) => source_initial.clone(),
        WorldStateRecord::Diff(_) => {
            return Err("待克隆的 World State journal 缺少 initial full snapshot。".to_string());
        }
    };
    let target_epoch_id = new_id("world-state-epoch");
    let mut world_replacements = replacements.clone();
    world_replacements.insert(source_initial.epoch_id.clone(), target_epoch_id.clone());
    let mut source_current = source_initial;
    let mut target_current =
        rewrite_world_state_snapshot(&source_current, &target_epoch_id, &world_replacements)?;
    entries[0].record = WorldStateRecord::Full(target_current.clone());

    for entry in entries.iter_mut().skip(1) {
        let WorldStateRecord::Diff(source_diff) = &entry.record else {
            return Err(
                "待克隆的 World State journal 在初始记录后包含 full snapshot。".to_string(),
            );
        };
        source_current = WorldStateReducer::fold(source_current, std::slice::from_ref(source_diff))
            .map_err(|error| format!("无法折叠待克隆的 World State diff：{error}"))?;
        let target_next =
            rewrite_world_state_snapshot(&source_current, &target_epoch_id, &world_replacements)?;
        let target_diff = WorldStateDiff::between(&target_current, &target_next)
            .map_err(|error| format!("无法重建复制后的 World State diff：{error}"))?;
        entry.record = WorldStateRecord::Diff(target_diff);
        target_current = target_next;
    }
    Ok(())
}

fn rewrite_world_state_snapshot(
    source: &WorldStateSnapshot,
    target_epoch_id: &str,
    replacements: &HashMap<String, String>,
) -> Result<WorldStateSnapshot, String> {
    let sections = source
        .sections
        .iter()
        .map(|section| {
            let mut state = section.state.clone();
            rewrite_exact_ids(&mut state, replacements);
            rewrite_history_open_tokens(&mut state, replacements)?;
            let model_projection = section
                .model_projection
                .as_ref()
                .map(|projection| {
                    let mut projection = projection.clone();
                    rewrite_exact_ids(&mut projection, replacements);
                    rewrite_history_open_tokens(&mut projection, replacements)?;
                    Ok::<_, String>(projection)
                })
                .transpose()?;
            WorldStateSectionEnvelope::new(
                section.id.clone(),
                section.lifetime,
                section.visibility,
                state,
                model_projection,
            )
            .map_err(|error| format!("复制后的 World State section 无效：{error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    WorldStateSnapshot::new(target_epoch_id, source.sequence, sections)
        .map_err(|error| format!("复制后的 World State snapshot 无效：{error}"))
}

fn remap_continuity(
    source: &ContextContinuitySnapshot,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ContextContinuitySnapshot, String> {
    let remap_refs = |refs: &[ContextHistoryRef]| {
        refs.iter()
            .map(|reference| remap_history_ref(reference, message_id_map, replacements))
            .collect::<Result<Vec<_>, String>>()
    };
    let snapshot = ContextContinuitySnapshot {
        schema_version: source.schema_version,
        covered_through: remap_cursor(&source.covered_through, message_id_map)?,
        task_evidence_refs: remap_refs(&source.task_evidence_refs)?,
        unresolved_failure_refs: remap_refs(&source.unresolved_failure_refs)?,
        approval_refs: remap_refs(&source.approval_refs)?,
        important_decision_refs: remap_refs(&source.important_decision_refs)?,
        recent_refs: remap_refs(&source.recent_refs)?,
        archived_counts: source.archived_counts.clone(),
    };
    snapshot.validate().map_err(|error| error.to_string())?;
    Ok(snapshot)
}

fn remap_history_ref(
    reference: &ContextHistoryRef,
    message_id_map: &HashMap<String, String>,
    replacements: &HashMap<String, String>,
) -> Result<ContextHistoryRef, String> {
    match reference {
        ContextHistoryRef::Message { message_id } => Ok(ContextHistoryRef::message(mapped_id(
            message_id_map,
            message_id,
            "Continuity 消息引用",
        )?)),
        ContextHistoryRef::TraceItem {
            assistant_message_id,
            sequence,
        } => Ok(ContextHistoryRef::trace_item(
            mapped_id(
                message_id_map,
                assistant_message_id,
                "Continuity trace 引用",
            )?,
            *sequence,
        )),
        ContextHistoryRef::Archive { archive_ref } => Ok(ContextHistoryRef::archive(mapped_id(
            replacements,
            archive_ref,
            "Continuity Archive 引用",
        )?)),
    }
}

fn rewrite_trace_items(
    trace: &mut ConversationTurnTrace,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    let mut value = serde_json::to_value(&trace.items)
        .map_err(|error| format!("无法序列化历史工具轨迹：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    rewrite_history_open_tokens(&mut value, replacements)?;
    trace.items = serde_json::from_value(value)
        .map_err(|error| format!("无法重建复制后的历史工具轨迹：{error}"))?;
    trace.validate().map_err(|error| error.to_string())
}

fn rewrite_model_context_items(
    items: &mut Vec<ConversationModelContextItem>,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    let mut value = serde_json::to_value(&*items)
        .map_err(|error| format!("无法序列化历史模型上下文：{error}"))?;
    rewrite_exact_ids(&mut value, replacements);
    rewrite_history_open_tokens(&mut value, replacements)?;
    *items = serde_json::from_value(value)
        .map_err(|error| format!("无法重建复制后的历史模型上下文：{error}"))?;
    for item in items {
        item.validate()
            .map_err(|error| format!("复制后的历史模型上下文无效：{error}"))?;
    }
    Ok(())
}

pub(crate) fn rewrite_history_open_tokens(
    value: &mut Value,
    replacements: &HashMap<String, String>,
) -> Result<(), String> {
    match value {
        Value::String(current) => {
            *current = rewritten_history_open_tokens(current, replacements)?;
        }
        Value::Array(values) => {
            for value in values {
                rewrite_history_open_tokens(value, replacements)?;
            }
        }
        Value::Object(values) => {
            for value in values.values_mut() {
                rewrite_history_open_tokens(value, replacements)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
    Ok(())
}

fn rewritten_history_open_tokens(
    value: &str,
    replacements: &HashMap<String, String>,
) -> Result<String, String> {
    let prefix = conversation_history_open::HISTORY_OPEN_PREFIX;
    let Some(first_match) = value.find(prefix) else {
        return Ok(value.to_string());
    };
    let mut rewritten = String::with_capacity(value.len());
    rewritten.push_str(&value[..first_match]);
    let mut cursor = first_match;
    while cursor < value.len() {
        let Some(relative_start) = value[cursor..].find(prefix) else {
            rewritten.push_str(&value[cursor..]);
            break;
        };
        let start = cursor.saturating_add(relative_start);
        rewritten.push_str(&value[cursor..start]);
        let encoded_start = start.saturating_add(prefix.len());
        let encoded_len = value[encoded_start..]
            .bytes()
            .take_while(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            .count();
        if encoded_len == 0 {
            rewritten.push_str(prefix);
            cursor = encoded_start;
            continue;
        }
        let end = encoded_start.saturating_add(encoded_len);
        let token = &value[start..end];
        if let Some(remapped) = conversation_history_open::remap_history_open(token, replacements)?
        {
            rewritten.push_str(&remapped);
        } else {
            rewritten.push_str(token);
        }
        cursor = end;
    }
    Ok(rewritten)
}

fn remap_cursor(
    cursor: &ContextJournalCursor,
    message_id_map: &HashMap<String, String>,
) -> Result<ContextJournalCursor, String> {
    match cursor {
        ContextJournalCursor::Message { message_id } => Ok(ContextJournalCursor::message(
            mapped_id(message_id_map, message_id, "摘要消息游标")?,
        )),
        ContextJournalCursor::TraceItem {
            assistant_message_id,
            sequence,
        } => Ok(ContextJournalCursor::trace_item(
            mapped_id(message_id_map, assistant_message_id, "摘要 trace 游标")?,
            *sequence,
        )),
    }
}

fn mapped_id(
    mapping: &HashMap<String, String>,
    source: &str,
    label: &str,
) -> Result<String, String> {
    mapping
        .get(source)
        .cloned()
        .ok_or_else(|| format!("{label}未包含在分叉快照中：{source}"))
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", Uuid::new_v4())
}

fn database_error(error: rusqlite::Error) -> String {
    format!("本地数据库操作失败：{error}")
}
