/// Open non-blocking questions are the only human-interaction state a fork inherits: the exact
/// frozen question/option text with branch-local owner identities. Answers, deliveries and
/// resume material stay with their source conversation.
fn collect_inherited_open_async_questions(
    connection: &Connection,
    source_conversation_id: &str,
    target_conversation_id: &str,
    message_id_map: &HashMap<String, String>,
    run_id_map: &HashMap<String, String>,
    tool_call_id_map: &HashMap<String, String>,
    global_id_replacements: &HashMap<String, String>,
) -> Result<Vec<ForkHumanInteractionRequest>, ConversationForkError> {
    let mut statement = connection
        .prepare(
            "SELECT request_id,run_id,assistant_message_id,tool_call_id,policy_revision,questions_json,created_at,updated_at\n             FROM human_interaction_requests\n             WHERE conversation_id=?1 AND mode='async' AND status='open'\n             ORDER BY sequence",
        )
        .map_err(database_error)?;
    let rows = statement
        .query_map([source_conversation_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(database_error)?;
    let mut copied = Vec::new();
    for row in rows {
        let (
            source_request_id,
            source_run_id,
            source_assistant_message_id,
            source_tool_call_id,
            policy_revision,
            questions_json,
            created_at,
            updated_at,
        ) = row.map_err(database_error)?;
        let Some(target_assistant_message_id) = message_id_map
            .get(&source_assistant_message_id)
            .cloned()
        else {
            // The question was opened beyond the copied boundary; nothing of it is inherited.
            continue;
        };
        // A question that cannot be certified against this snapshot is skipped rather than
        // dragging the whole fork down: the branch simply does not inherit it.
        let Some(target_run_id) = run_id_map
            .get(&source_run_id)
            .cloned()
            .or_else(|| global_id_replacements.get(&source_run_id).cloned())
        else {
            continue;
        };
        let Some(target_tool_call_id) = tool_call_id_map
            .get(&source_tool_call_id)
            .cloned()
            .or_else(|| global_id_replacements.get(&source_tool_call_id).cloned())
        else {
            continue;
        };
        copied.push(ForkHumanInteractionRequest {
            source_request_id,
            target_request_id: Uuid::new_v4().to_string(),
            target_agent_id: root_agent_id_for_conversation(target_conversation_id),
            target_run_id,
            target_assistant_message_id,
            target_tool_call_id,
            policy_revision,
            questions_json,
            created_at,
            updated_at,
        });
    }
    Ok(copied)
}

#[allow(clippy::too_many_arguments)]
fn build_single_conversation_fork_plan_at_point(
    connection: &Connection,
    request_id: &str,
    source_conversation_id: &str,
    fork_point: &ConversationForkPoint,
    created_at: i64,
    allow_member_source: bool,
    target_conversation_id: Option<&str>,
    source_message_limit: Option<usize>,
    history_cutoff_at: Option<i64>,
    receipt_cutoff_at: Option<i64>,
    global_id_replacements: &HashMap<String, String>,
) -> Result<ConversationForkPlan, ConversationForkError> {
    validate_fork_point_input(request_id, source_conversation_id, fork_point)?;
    let source_agent =
        agent_graph_repository::get_agent_node_by_conversation(connection, source_conversation_id)
            .map_err(|error| ConversationForkError::Other(error.to_string()))?;
    let snapshot_authorized = source_agent.is_some();
    let source_root = match source_agent {
        None => None,
        Some(node) if node.parent_agent_id.is_some() => {
            if !allow_member_source {
                return Err(ConversationForkError::Other(
                    "用户只能从根 Agent Conversation 继续新任务；子 Agent 保持只读。".to_string(),
                ));
            }
            ensure_no_active_conversation_turn(connection, source_conversation_id)?;
            None
        }
        Some(node) if node.lifecycle != AgentLifecycle::Active => {
            return Err(ConversationForkError::Other(
                "根 Agent 当前不可用，无法继续新任务。".to_string(),
            ));
        }
        Some(node) => {
            ensure_no_active_conversation_turn(connection, source_conversation_id)?;
            Some(node)
        }
    };
    ensure_no_active_command_sessions(connection, source_conversation_id)?;
    let manual_running = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM manual_context_compaction_operations WHERE conversation_id=?1 AND status='running')",
        [source_conversation_id], |row| row.get::<_, bool>(0),
    ).map_err(database_error)?;
    if manual_running {
        return Err("上下文仍在压缩，结束后才能创建分支。".to_string().into());
    }
    let source = chat_repository::get_active_conversation(connection, source_conversation_id)
        .map_err(database_error)?
        .ok_or_else(|| "原任务不存在。".to_string())?;
    let active_chain =
        context_compaction_repository::list_active_summary_chain(connection, &source.id)
            .map_err(|error| error.to_string())?;
    let resolved = resolve_fork_point(connection, &source, fork_point, &active_chain)?;
    let cutoff = source
        .messages
        .iter()
        .position(|message| message.id == resolved.assistant_message_id)
        .ok_or_else(|| "所选回复不属于原任务。".to_string())?;
    let cutoff_message = &source.messages[cutoff];
    if cutoff_message.role != "assistant" {
        return Err("只能从 assistant 回复继续新任务。".to_string().into());
    }
    let source_positions = source
        .messages
        .iter()
        .enumerate()
        .map(|(position, message)| (message.id.clone(), position))
        .collect::<HashMap<_, _>>();
    let message_limit = source_message_limit.unwrap_or(cutoff);
    if message_limit < cutoff || message_limit >= source.messages.len() {
        return Err("成员对话的可见消息边界无效。".to_string().into());
    }
    let source_messages = source.messages[..=message_limit].to_vec();
    let target_conversation_id = target_conversation_id
        .map(str::to_string)
        .unwrap_or_else(|| new_id("conversation"));
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
    let snapshot_origins = if snapshot_authorized {
        fork_snapshot_origins(connection, &source, &source_messages, &message_id_map)?
    } else {
        Vec::new()
    };

    let source_message_ids = source_messages
        .iter()
        .map(|message| message.id.clone())
        .collect::<Vec<_>>();
    let mut summaries = match resolved.summary_id.as_deref() {
        Some(summary_id) => summaries_visible_through_transition_boundary(
            active_chain,
            &source_positions,
            cutoff,
            summary_id,
        )?,
        None => summaries_visible_at_assistant_reply(
            connection,
            &source.id,
            active_chain,
            &source_positions,
            cutoff,
        )?,
    };
    if let Some(cutoff_at) = history_cutoff_at {
        summaries = summaries_visible_at_time(connection, &source.id, summaries, cutoff_at)?;
    }
    // Provider-transition receipts are part of the visible timeline, not disposable audit noise.
    // Copy every transition whose summary is visible at the selected fork point so a later fork
    // of the child conversation retains the same semantic history and UI boundary.
    let provider_transition_receipts =
        collect_visible_provider_transition_receipts(connection, &source.id, &summaries)?;
    let summary_covered_runtime_tool_calls = summaries
        .last()
        .map(|version| {
            context_compaction_repository::covered_runtime_tool_calls_through_cursor(
                connection,
                &source.id,
                &version.summary.covered_through,
            )
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let summary_covered_projections = summaries
        .last()
        .map(|version| {
            context_compaction_repository::covered_provider_projection_cursors_through_cursor(
                connection,
                &source.id,
                &version.summary.covered_through,
            )
        })
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or_default();
    let source_adaptation = conversation_context_adaptation_repository::get(connection, &source.id)
        .map_err(database_error)?;
    let selected_summary_ids = summaries
        .iter()
        .map(|version| version.summary.id.as_str())
        .collect::<HashSet<_>>();
    let released_state_outside_summary =
        provider_continuation_repository::has_released_for_messages_outside_summary_coverage(
            connection,
            &source.id,
            &source_message_ids,
            &summary_covered_projections,
            &summary_covered_runtime_tool_calls,
        )
        .map_err(database_error)?;
    let source_adaptation_boundary_selected = source_adaptation
        .as_ref()
        .and_then(|requirement| requirement.resolved_summary_id.as_deref())
        .is_some_and(|summary_id| selected_summary_ids.contains(summary_id));
    // An exact manual boundary is a validated Provider-neutral summary of the entire selected
    // history. A later Provider transition's adaptation marker belongs after that cutoff and
    // cannot make this earlier, fully covered snapshot require another paid compaction.
    // Released replay outside the selected summary still forces adaptation below.
    let manual_boundary_covers_selected_history = matches!(
        fork_point,
        ConversationForkPoint::ManualCompactionBoundary { .. }
    ) && summaries.last().is_some_and(|version| {
        Some(version.summary.id.as_str()) == resolved.summary_id.as_deref()
            && source_messages.last().is_some_and(|message| {
                version.summary.covered_through == ContextJournalCursor::message(&message.id)
            })
    });
    let requires_context_adaptation = released_state_outside_summary
        || (!manual_boundary_covers_selected_history
            && source_adaptation.as_ref().is_some_and(|requirement| {
                requirement.is_required() || !source_adaptation_boundary_selected
            }));
    let contains_released_provider_history =
        provider_continuation_repository::has_released_for_messages(
            connection,
            &source.id,
            &source_message_ids,
        )
        .map_err(database_error)?;
    let adaptation_source_summary_id = (!requires_context_adaptation)
        .then(|| {
            source_adaptation
                .as_ref()
                .and_then(|requirement| requirement.resolved_summary_id.clone())
                .filter(|summary_id| selected_summary_ids.contains(summary_id.as_str()))
                .or_else(|| {
                    contains_released_provider_history
                        .then(|| summaries.last().map(|version| version.summary.id.clone()))
                        .flatten()
                })
        })
        .flatten();
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

    let mut traces = Vec::new();
    let backend_event_cutoff = match history_cutoff_at {
        Some(cutoff) => cutoff,
        None => authoritative_fork_cutoff_at(connection, &source.id, fork_point)?,
    };
    let mut run_id_map = HashMap::new();
    let mut tool_call_id_map = HashMap::new();
    for message in &source_messages {
        let trace = conversation_trace_repository::get_trace_for_message(connection, &message.id)
            .map_err(database_error)?;
        ensure_settled_assistant(message, trace.as_ref())?;
        if let Some(mut trace) = trace {
            trace.items.retain(|item| match item {
                crate::ConversationTurnTraceItem::BackendState {
                    placement,
                    created_at,
                    ..
                } => {
                    // Member AssistantReply is a synthetic selector inside the root's snapshot;
                    // its postlude remains visible up to the root's authoritative time boundary.
                    *created_at <= backend_event_cutoff
                        && !(message.id == resolved.assistant_message_id
                            && !allow_member_source
                            && matches!(fork_point, ConversationForkPoint::AssistantReply { .. })
                            && *placement == crate::ConversationBackendStatePlacement::AfterMessage)
                }
                _ => true,
            });
            if agent_run_id(message.agent_run_json.as_deref())
                .as_deref()
                .is_some_and(|run_id| run_id != trace.run_id)
            {
                return Err("历史回复的运行标识与后端工具轨迹不一致。"
                    .to_string()
                    .into());
            }
            let new_run_id = global_id_replacements
                .get(&trace.run_id)
                .cloned()
                .unwrap_or_else(|| new_id("run"));
            run_id_map.insert(trace.run_id.clone(), new_run_id.clone());
            for (item_index, item) in trace.items.iter().enumerate() {
                let call_id = match item {
                    crate::ConversationTurnTraceItem::ToolCall { call_id, .. }
                    | crate::ConversationTurnTraceItem::ToolResult { call_id, .. }
                    | crate::ConversationTurnTraceItem::CommandSessionLifecycle {
                        call_id, ..
                    } => Some(call_id),
                    _ => None,
                };
                if let Some(call_id) = call_id {
                    let target_call_id = global_id_replacements
                        .get(call_id)
                        .or_else(|| tool_call_id_map.get(call_id))
                        .cloned()
                        .unwrap_or_else(|| {
                            crate::llm::model_response_tool_call_id(
                                &new_run_id,
                                item_index,
                                0,
                                call_id,
                            )
                        });
                    insert_global_replacement(&mut tool_call_id_map, call_id, &target_call_id)?;
                }
            }
            let (trace_created_at, committed_at) = trace_times(connection, &message.id)?;
            let mut model_context_items =
                conversation_model_context_repository::get_log_for_message(connection, &message.id)
                    .map_err(database_error)?
                    .map(|log| log.items)
                    .unwrap_or_default();
            model_context_items.retain(|item| {
                trace
                    .items
                    .iter()
                    .any(|event| event.sequence() == item.sequence)
            });
            traces.push(ForkTrace {
                source_run_id: trace.run_id.clone(),
                trace: ConversationTurnTrace {
                    schema_version: trace.schema_version,
                    run_id: new_run_id,
                    conversation_id: target_conversation_id.clone(),
                    assistant_message_id: mapped_id(&message_id_map, &message.id, "消息")?,
                    terminal_status: trace.terminal_status,
                    terminal_error: trace.terminal_error,
                    truncated: trace.truncated,
                    items: trace.items,
                },
                model_context_items,
                created_at: trace_created_at,
                committed_at,
            });
        } else if let Some(run_id) = agent_run_id(message.agent_run_json.as_deref()) {
            let target_run_id = global_id_replacements
                .get(&run_id)
                .cloned()
                .unwrap_or_else(|| new_id("run"));
            run_id_map.entry(run_id).or_insert(target_run_id);
        }
    }
    let compaction_receipts = collect_visible_context_compaction_receipts(
        connection,
        &source.id,
        &message_id_map,
        &run_id_map,
        &summaries,
        receipt_cutoff_at,
    )?;

    // A recursive fork needs every visible turn diff, not only the boundary turn.
    let mut turn_diffs = turn_diff_repository::list_fork_copies_for_messages(
        connection,
        &source.id,
        &source_message_ids,
    )
    .map_err(database_error)?;
    for turn_diff in &mut turn_diffs {
        if turn_diff.record.identity.conversation_id != source.id
            || source.project_id.as_deref() != Some(turn_diff.record.identity.project_id.as_str())
        {
            return Err("历史文件变更证据的任务或项目归属不一致。"
                .to_string()
                .into());
        }
        turn_diff.record.identity.conversation_id = target_conversation_id.clone();
        turn_diff.record.identity.assistant_message_id = mapped_id(
            &message_id_map,
            &turn_diff.record.identity.assistant_message_id,
            "文件变更证据所属消息",
        )?;
        turn_diff.record.identity.run_id = mapped_id(
            &run_id_map,
            &turn_diff.record.identity.run_id,
            "文件变更证据所属运行",
        )?;
    }

    // Open non-blocking questions inherit branch-local request identities. Keep this mapping out
    // of the generic identity walker: a request id is rewritten only inside a ToolResult that can
    // be authenticated against a `request_user_input_async` ToolCall in the same durable record.
    let human_interaction_requests = if source_root.is_some() {
        collect_inherited_open_async_questions(
            connection,
            &source.id,
            &target_conversation_id,
            &message_id_map,
            &run_id_map,
            &tool_call_id_map,
            global_id_replacements,
        )?
    } else {
        Vec::new()
    };
    let human_request_id_replacements = human_interaction_requests
        .iter()
        .map(|request| {
            (
                request.source_request_id.clone(),
                request.target_request_id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut file_changes = Vec::new();
    let mut file_change_id_map = HashMap::new();
    let mut file_observation_id_map = HashMap::new();
    for (source_run_id, target_run_id) in &run_id_map {
        for source_change in
            file_change_repository::list_file_changes_for_run(connection, source_run_id)
                .map_err(database_error)?
        {
            if source_change.conversation_id != source.id
                || source_change.project_id != source.project_id
                || source_change.run_id != *source_run_id
                || source_change.source_tool_name != "apply_patch"
            {
                return Err("文件变更事务的任务、项目、运行或工具归属不一致。"
                    .to_string()
                    .into());
            }
            if history_cutoff_at.is_some_and(|cutoff| source_change.created_at > cutoff) {
                continue;
            }
            if history_cutoff_at.is_some_and(|cutoff| source_change.updated_at > cutoff) {
                return Err(ConversationForkError::Other(
                    "文件变更事务在分叉点后发生过不可版本化的变化，无法生成精确历史快照。"
                        .to_string(),
                ));
            }
            let target_transaction_id = global_id_replacements
                .get(&source_change.id)
                .cloned()
                .unwrap_or_else(|| new_id("file-change"));
            let mut history = file_change_repository::load_file_change_history_snapshot(
                connection,
                &source_change.id,
                history_cutoff_at,
            )
            .map_err(database_error)?;
            if history.operations.len() as u64 != source_change.mutation_count
                || history.operations.len() as u64 != source_change.next_mutation_index
                || history.operations.len() as u64 != source_change.draft_revision
                || history
                    .operations
                    .iter()
                    .enumerate()
                    .any(|(index, operation)| operation.mutation_index != index as u64)
                || history.chunks.iter().any(|chunk| {
                    history
                        .operations
                        .get(chunk.mutation_index as usize)
                        .is_none_or(|operation| operation.action != "append")
                })
            {
                return Err(ConversationForkError::Other(
                    "文件变更事务在分叉点处的 mutation 快照与主记录不一致。".to_string(),
                ));
            }

            let target_source_tool_call_id = mapped_id(
                &tool_call_id_map,
                &source_change.source_tool_call_id,
                "文件变更 begin Tool Call",
            )?;
            let target_observation_id = global_id_replacements
                .get(&source_change.observation_id)
                .cloned()
                .unwrap_or_else(|| format!("fobs_{}", Uuid::new_v4().simple()));
            file_observation_id_map.insert(
                source_change.observation_id.clone(),
                target_observation_id.clone(),
            );
            let mut change_replacements = global_id_replacements.clone();
            change_replacements.extend(run_id_map.clone());
            change_replacements.extend(tool_call_id_map.clone());
            change_replacements.insert(source.id.clone(), target_conversation_id.clone());
            change_replacements.insert(source_change.id.clone(), target_transaction_id.clone());
            change_replacements.insert(
                source_change.observation_id.clone(),
                target_observation_id.clone(),
            );
            let target_source_tool_arguments_digest = remapped_file_change_call_digest(
                &traces,
                &source_change.source_tool_call_id,
                &source_change.source_tool_arguments_digest,
                &source_change.source_tool_name,
                &change_replacements,
            )?;
            for chunk in &mut history.chunks {
                chunk.transaction_id = target_transaction_id.clone();
            }
            for operation in &mut history.operations {
                operation.source_tool_arguments_digest = remapped_file_change_call_digest(
                    &traces,
                    &operation.source_tool_call_id,
                    &operation.source_tool_arguments_digest,
                    &source_change.source_tool_name,
                    &change_replacements,
                )?;
                operation.transaction_id = target_transaction_id.clone();
                operation.source_tool_call_id = mapped_id(
                    &tool_call_id_map,
                    &operation.source_tool_call_id,
                    "文件变更 mutation Tool Call",
                )?;
                let mut receipt = serde_json::from_str::<
                    crate::file_change::FileChangeMutationReceipt,
                >(&operation.receipt_json)
                .map_err(|_| {
                    ConversationForkError::Other("文件变更 mutation receipt 无效。".to_string())
                })?;
                receipt.validate().map_err(|_| {
                    ConversationForkError::Other("文件变更 mutation receipt 无效。".to_string())
                })?;
                receipt.transaction_id = target_transaction_id.clone();
                operation.receipt_json = serde_json::to_string(&receipt).map_err(|_| {
                    ConversationForkError::Other("无法复制文件变更 mutation receipt。".to_string())
                })?;
            }

            let (target_observation_id, target_observation_json) = remap_file_change_observation(
                &source_change,
                &target_conversation_id,
                target_run_id,
                &target_observation_id,
                &tool_call_id_map,
            )?;
            file_change_id_map.insert(source_change.id.clone(), target_transaction_id.clone());
            file_changes.push(ForkFileChange {
                target: AgentFileChangeRecord {
                    schema_version: source_change.schema_version,
                    id: target_transaction_id,
                    conversation_id: target_conversation_id.clone(),
                    project_id: source.project_id.clone(),
                    run_id: target_run_id.clone(),
                    source_tool_name: source_change.source_tool_name,
                    source_tool_call_id: target_source_tool_call_id,
                    source_tool_arguments_digest: target_source_tool_arguments_digest,
                    permission_revision: source_change.permission_revision,
                    tool_set_revision: source_change.tool_set_revision,
                    provider_wire_revision: source_change.provider_wire_revision,
                    observation_id: target_observation_id,
                    observation_json: target_observation_json,
                    file_path: source_change.file_path,
                    operation: source_change.operation,
                    strategy: source_change.strategy,
                    status: source_change.status,
                    base_revision: source_change.base_revision,
                    base_content: source_change.base_content,
                    content: source_change.content,
                    draft_revision: source_change.draft_revision,
                    next_mutation_index: source_change.next_mutation_index,
                    additions: source_change.additions,
                    deletions: source_change.deletions,
                    line_count: source_change.line_count,
                    byte_count: source_change.byte_count,
                    mutation_count: source_change.mutation_count,
                    stats_final: source_change.stats_final,
                    summary: source_change.summary,
                    final_action_id: source_change
                        .final_action_id
                        .as_deref()
                        .map(|call_id| {
                            mapped_id(&tool_call_id_map, call_id, "文件变更最终 Tool Call")
                        })
                        .transpose()?,
                    final_action_arguments_digest: source_change.final_action_arguments_digest,
                    final_permission_revision: source_change.final_permission_revision,
                    final_tool_set_revision: source_change.final_tool_set_revision,
                    final_provider_wire_revision: source_change.final_provider_wire_revision,
                    created_at: source_change.created_at,
                    updated_at: source_change.updated_at,
                    expires_at: source_change.expires_at,
                },
                history,
            });
        }
    }

    let mut guidance_id_map = HashMap::new();
    let mut guidances = Vec::new();
    for message in &source_messages {
        for guidance in
            guidance_repository::list_guidances_for_assistant_message(connection, &message.id)
                .map_err(database_error)?
        {
            if guidance.status != AgentGuidanceStatus::Applied {
                continue;
            }
            let target_guidance_id = global_id_replacements
                .get(&guidance.guidance_id)
                .cloned()
                .unwrap_or_else(|| new_id("guidance"));
            let target_run_id = mapped_id(&run_id_map, &guidance.run_id, "引导所属运行")?;
            let target_assistant_message_id = mapped_id(
                &message_id_map,
                &guidance.assistant_message_id,
                "引导所属消息",
            )?;
            let target_attachment_ids = guidance
                .attachment_ids
                .iter()
                .map(|attachment_id| mapped_id(&attachment_id_map, attachment_id, "引导附件"))
                .collect::<Result<Vec<_>, _>>()?;
            let applied_trace_sequence = guidance
                .applied_trace_sequence
                .ok_or_else(|| "已应用引导缺少 trace 顺序。".to_string())?;
            guidance_id_map.insert(guidance.guidance_id.clone(), target_guidance_id.clone());
            guidances.push(ForkGuidance {
                record: AgentRunGuidanceRecord {
                    guidance_id: target_guidance_id,
                    client_message_id: guidance.client_message_id,
                    run_id: target_run_id,
                    conversation_id: target_conversation_id.clone(),
                    assistant_message_id: target_assistant_message_id,
                    content: guidance.content,
                    status: AgentGuidanceStatus::Queued,
                    attachment_ids: target_attachment_ids,
                    applied_trace_sequence: None,
                    terminal_reason: None,
                    created_at: guidance.created_at,
                    updated_at: guidance.updated_at,
                },
                applied_trace_sequence,
            });
        }
    }

    let mut archive_id_map = HashMap::new();
    let mut archives = Vec::new();
    for trace in &traces {
        for item in &trace.trace.items {
            let archive = match item {
                crate::ConversationTurnTraceItem::ToolResult { archive, .. }
                | crate::ConversationTurnTraceItem::CommandSessionLifecycle { archive, .. } => {
                    archive
                }
                _ => continue,
            };
            let Some(source_archive_ref) = archive.archive_ref.as_deref() else {
                continue;
            };
            if archive_id_map.contains_key(source_archive_ref) {
                continue;
            }
            let mut copy = conversation_history_archive_repository::load_fork_copy(
                connection,
                &source.id,
                source_archive_ref,
                &target_conversation_id,
                &trace.trace.assistant_message_id,
            )
            .map_err(database_error)?;
            if let Some(target_archive_ref) = global_id_replacements.get(source_archive_ref) {
                copy.target_archive_ref = target_archive_ref.clone();
            }
            archive_id_map.insert(
                source_archive_ref.to_string(),
                copy.target_archive_ref.clone(),
            );
            archives.push(copy);
        }
    }

    let mut replacements = global_id_replacements.clone();
    replacements.extend(message_id_map.clone());
    replacements.extend(run_id_map.clone());
    replacements.extend(tool_call_id_map.clone());
    replacements.extend(attachment_id_map.clone());
    replacements.extend(guidance_id_map);
    replacements.extend(file_change_id_map);
    replacements.extend(file_observation_id_map);
    replacements.extend(archive_id_map);
    replacements.insert(source.id.clone(), target_conversation_id.clone());
    for version in &summaries {
        let target_summary_id = new_id("context-summary");
        insert_global_replacement(&mut replacements, &version.summary.id, &target_summary_id)?;
    }
    for copy in &compaction_receipts {
        insert_global_replacement(
            &mut replacements,
            &copy.source_receipt.operation_id,
            &copy.target_operation_id,
        )?;
        insert_global_replacement(
            &mut replacements,
            &copy.source_receipt.run_id,
            &copy.target_run_id,
        )?;
        if let (Some(source_observation_id), Some(target_observation_id)) = (
            copy.source_receipt.generation_observation_id.as_deref(),
            copy.target_observation_id.as_deref(),
        ) {
            insert_global_replacement(
                &mut replacements,
                source_observation_id,
                target_observation_id,
            )?;
        }
    }
    for copy in &provider_transition_receipts {
        insert_global_replacement(
            &mut replacements,
            &copy.source_receipt.operation_id,
            &copy.target_operation_id,
        )?;
        insert_global_replacement(
            &mut replacements,
            &copy.source_receipt.run_id,
            &copy.target_run_id,
        )?;
        insert_global_replacement(
            &mut replacements,
            &copy.source_observation.id,
            &copy.target_observation_id,
        )?;
    }
    let action_audits = remap_terminal_file_change_action_audits(
        connection,
        &source.id,
        &target_conversation_id,
        &source_message_ids,
        &message_id_map,
        &run_id_map,
        &tool_call_id_map,
        &traces,
        &mut replacements,
    )?;
    for fork_trace in &mut traces {
        rewrite_trace_items(
            &mut fork_trace.trace,
            &replacements,
            &human_request_id_replacements,
        )?;
        rewrite_model_context_items(
            &mut fork_trace.model_context_items,
            &replacements,
            &human_request_id_replacements,
        )?;
    }
    let target_messages = source_messages
        .iter()
        .map(|message| {
            clone_message(
                message,
                &message_id_map,
                &replacements,
                &human_request_id_replacements,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;

    let attachments = source_attachments
        .into_iter()
        .map(|source_attachment| {
            let target_attachment_id =
                mapped_id(&attachment_id_map, &source_attachment.id, "附件")?;
            let target_message_id = mapped_id(
                &message_id_map,
                &source_attachment.message_id,
                "附件所属消息",
            )?;
            Ok(ForkAttachmentCopy {
                target: AttachmentRecord {
                    id: target_attachment_id,
                    conversation_id: target_conversation_id.clone(),
                    message_id: target_message_id,
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

    let mut world_state_records = world_state_records_visible_at_cutoff(
        connection,
        &source.id,
        &source_positions,
        &if message_limit > cutoff {
            ContextJournalCursor::message(&source.messages[message_limit].id)
        } else {
            resolved.world_state_cutoff.clone()
        },
        &summaries,
        history_cutoff_at,
    )?;
    rewrite_world_state_records(&mut world_state_records, &replacements)?;
    let provider_continuation_mappings = if requires_context_adaptation {
        // The exact old payload was already released, so a partial clone of whatever happens to
        // remain replayable would create a misleading mixed snapshot and unnecessarily depend on
        // Vault availability. The backend-only marker below forces a tool-free full-prefix
        // compaction before this fork can send anything.
        Vec::new()
    } else {
        let runtime_tool_call_id_map = Arc::new(tool_call_id_map.clone());
        provider_continuation_repository::list_replayable_for_conversation(connection, &source.id)
            .map_err(database_error)?
            .into_iter()
            .filter_map(|record| {
                let target_assistant_message_id =
                    message_id_map.get(&record.assistant_message_id)?.clone();
                Some((record, target_assistant_message_id))
            })
            .map(|(record, target_assistant_message_id)| {
                let source_ref = ProviderContinuationRef::parse(
                    crate::protocol::PROVIDER_CONTINUATION_REF_VERSION,
                    record.continuation_id.clone(),
                )?;
                let target_run_id = mapped_id(
                    &run_id_map,
                    &record.run_id,
                    "Provider continuation 所属运行",
                )?;
                Ok(ProviderContinuationForkMapping {
                    source_ref,
                    source_record: record.clone(),
                    source_conversation_id: source.id.clone(),
                    source_assistant_message_id: record.assistant_message_id.clone(),
                    source_run_id: record.run_id.clone(),
                    request_index: record.request_index,
                    target_conversation_id: target_conversation_id.clone(),
                    target_assistant_message_id,
                    target_run_id,
                    runtime_tool_call_id_map: Arc::clone(&runtime_tool_call_id_map),
                })
            })
            .collect::<Result<Vec<_>, String>>()?
    };

    let collaboration_root = source_root.map(|source_root| CollaborationRootFork {
        source_agent_id: source_root.agent_id,
        target_root: EnsureRootAgentInput {
            agent_id: root_agent_id_for_conversation(&target_conversation_id),
            conversation_id: target_conversation_id.clone(),
            creation_request_id: root_agent_creation_request_id(&target_conversation_id),
            task_name: ROOT_AGENT_TASK_NAME.to_string(),
        },
    });

    Ok(ConversationForkPlan {
        request_id: request_id.to_string(),
        source_conversation_id: source.id,
        source_message_id: resolved.assistant_message_id,
        source_fork_point: fork_point.clone(),
        target: ChatConversationRecord {
            id: target_conversation_id,
            project_id: source.project_id,
            model_id: resolved.model_id.or(source.model_id),
            title: source.title,
            messages: target_messages,
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        },
        attachments,
        archives,
        traces,
        action_audits,
        turn_diffs,
        guidances,
        file_changes,
        summaries,
        compaction_receipts,
        provider_transition_receipts,
        world_state_records,
        provider_continuation_mappings,
        requires_context_adaptation,
        adaptation_source_summary_id,
        message_id_map,
        collaboration_root,
        snapshot_origins,
        human_interaction_requests,
        #[cfg(test)]
        run_id_map,
        id_replacements: replacements,
        members: Vec::new(),
    })
}
