fn attach_message_guidance_timelines(
    connection: &rusqlite::Connection,
    conversations: &mut [ChatConversationRecord],
) -> Result<(), String> {
    for conversation in conversations {
        let traces = conversation_trace_repository::list_trace_records_for_conversation(
            connection,
            &conversation.id,
        )
        .map_err(storage_error)?;
        let traces = traces
            .into_iter()
            .map(|record| (record.trace.assistant_message_id.clone(), record))
            .collect::<HashMap<_, _>>();
        let command_sessions = agent_command_session_repository::list_sessions_for_conversation(
            connection,
            &conversation.id,
            agent_command_session_repository::MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION,
        )
        .map_err(storage_error)?;
        let guidances =
            guidance_repository::list_guidances_for_conversation(connection, &conversation.id)
                .map_err(storage_error)?;
        let mut guidances_by_message = HashMap::<String, Vec<AgentRunGuidanceRecord>>::new();
        for guidance in guidances {
            guidances_by_message
                .entry(guidance.assistant_message_id.clone())
                .or_default()
                .push(guidance);
        }
        let guidance_attachments =
            attachment_repository::list_guidance_attachments_for_conversation(
                connection,
                &conversation.id,
            )
            .map_err(storage_error)?
            .into_iter()
            .map(|attachment| (attachment.id.clone(), attachment))
            .collect::<HashMap<_, _>>();
        let mcp_actions = pending_action_repository::list_mcp_actions_for_conversation(
            connection,
            &conversation.id,
        )
        .map_err(storage_error)?;
        let mut mcp_actions_by_message_and_run =
            HashMap::<(String, String), Vec<AgentPendingActionRecord>>::new();
        for action in mcp_actions {
            let Some(assistant_message_id) = action.assistant_message_id.clone() else {
                continue;
            };
            mcp_actions_by_message_and_run
                .entry((assistant_message_id, action.run_id.clone()))
                .or_default()
                .push(action);
        }

        for message in &mut conversation.messages {
            if message.role != "assistant" {
                continue;
            }
            let guidances = guidances_by_message.remove(&message.id).unwrap_or_default();
            let trace_record = traces.get(&message.id);
            let trace = trace_record.map(|record| &record.trace);
            if guidances.is_empty() && trace_record.is_none() {
                continue;
            }
            let mcp_actions = trace
                .and_then(|trace| {
                    mcp_actions_by_message_and_run.get(&(message.id.clone(), trace.run_id.clone()))
                })
                .map(Vec::as_slice)
                .unwrap_or_default();
            message.agent_run_json = Some(project_guidance_timeline(
                message.agent_run_json.as_deref(),
                trace_record,
                &guidances,
                &command_sessions,
                &guidance_attachments,
                mcp_actions,
                message.created_at,
            )?);
        }
    }
    Ok(())
}

fn project_guidance_timeline(
    existing_run_json: Option<&str>,
    trace_record: Option<&conversation_trace_repository::ConversationTurnTraceRecord>,
    guidances: &[AgentRunGuidanceRecord],
    command_sessions: &[agent_command_session_repository::AgentCommandSessionRecord],
    guidance_attachments: &HashMap<String, AttachmentRecord>,
    mcp_actions: &[AgentPendingActionRecord],
    fallback_started_at: i64,
) -> Result<String, String> {
    let trace = trace_record.map(|record| &record.trace);
    let trace_completed_at = trace_record.and_then(|record| record.completed_at);
    let (expected_run_id, fallback_status, fallback_completed_at) = trace
        .map(|trace| {
            let status = match trace.terminal_status {
                crate::ConversationTurnTraceTerminalStatus::InProgress => "running",
                crate::ConversationTurnTraceTerminalStatus::Completed => "completed",
                crate::ConversationTurnTraceTerminalStatus::Failed => "failed",
                crate::ConversationTurnTraceTerminalStatus::Cancelled => "cancelled",
            };
            (
                trace.run_id.as_str(),
                status,
                (trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::InProgress)
                    .then_some(
                        trace_completed_at
                            .unwrap_or(fallback_started_at)
                            .max(fallback_started_at),
                    ),
            )
        })
        .or_else(|| {
            guidances
                .first()
                .map(|guidance| (guidance.run_id.as_str(), "running", None))
        })
        .ok_or_else(|| "guidance projection has no current run identity".to_string())?;
    if guidances
        .iter()
        .any(|guidance| guidance.run_id != expected_run_id)
    {
        return Err("guidance projection has inconsistent run identity".to_string());
    }

    let existing_run = if let Some(raw) = existing_run_json {
        let value = serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|error| format!("current AgentRun projection is invalid JSON: {error}"))?;
        let mut run = value
            .as_object()
            .cloned()
            .ok_or_else(|| "current AgentRun projection must be an object".to_string())?;
        chat_repository::discard_legacy_collaboration_activities(&mut run);
        // A malformed or incomplete presentation must never be merged field-by-field. A durable
        // Trace may defer only Tool identity coherence because every Trace-owned Timeline item is
        // discarded and rebuilt below; Guidance without a Trace has no such authority.
        let is_safe = if trace.is_some() {
            chat_repository::current_agent_run_projection_is_safe_for_trace_rebuild(
                &run,
                expected_run_id,
            )
        } else {
            chat_repository::current_agent_run_projection_is_safe(&run, expected_run_id)
        };
        is_safe.then_some(run)
    } else {
        None
    };
    let mut run = if let Some(run) = existing_run {
        run
    } else {
        let canonical = chat_repository::canonical_agent_run_lifecycle_projection(
            None,
            expected_run_id,
            fallback_status,
            fallback_started_at,
            fallback_completed_at.unwrap_or(fallback_started_at),
            fallback_completed_at,
        )
        .map_err(storage_error)?;
        serde_json::from_str::<serde_json::Value>(&canonical)
            .map_err(|error| format!("decode canonical AgentRun projection: {error}"))?
            .as_object()
            .cloned()
            .expect("canonical AgentRun projection is an object")
    };
    // Recover the terminal reason from the backend-owned trace, not a renderer snapshot.
    // RuntimeError codes are durable and intentionally excluded from model context.
    let terminal_interruption = trace
        .filter(|trace| trace.terminal_status == crate::ConversationTurnTraceTerminalStatus::Failed)
        .and_then(|trace| {
            trace.items.iter().rev().find_map(|item| match item {
                ConversationTurnTraceItem::RuntimeError {
                    code: Some(code), ..
                } => crate::AgentModelRequestInterruptionReason::from_trace_code(code),
                _ => None,
            })
        });
    if let Some(reason) = terminal_interruption {
        run.insert(
            "interruption".to_string(),
            serde_json::json!({"reason": reason.as_str()}),
        );
        run.remove("error");
    }
    project_durable_mcp_invocations(&mut run, trace, mcp_actions)?;
    let mcp_trace_anchors = mcp_trace_anchors(&run)?;
    let existing_timeline = run
        .remove("timeline")
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| "current AgentRun timeline must be an array".to_string())?;
    let terminal_trace_error = trace.and_then(|trace| {
        trace
            .terminal_status
            .is_terminal()
            .then_some(trace.terminal_error.as_deref())
            .flatten()
    });
    let trace_is_authoritative = trace.is_some();
    let guidance_is_authoritative = trace_is_authoritative || !guidances.is_empty();
    let mut presentation_suffix = existing_timeline
        .into_iter()
        .filter(|item| {
            !timeline_item_is_rebuilt_from_durable_state(
                item,
                trace_is_authoritative,
                guidance_is_authoritative,
                terminal_trace_error.is_some(),
            )
        })
        .collect::<Vec<_>>();
    // A durable Trace is the ordered authority for the work performed during a Turn. Renderer-only
    // items have no position in that sequence, so they form a presentation suffix (most notably
    // the final answer) instead of being prepended ahead of the reconstructed work. With no Trace,
    // preserve the existing presentation order and append journal-only Guidance as before.
    let mut timeline = if trace_is_authoritative {
        Vec::new()
    } else {
        std::mem::take(&mut presentation_suffix)
    };
    let mut emitted_mcp_invocations = HashSet::new();
    let existing_tool_calls = run
        .remove("toolCalls")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let existing_tool_results = run
        .remove("toolResults")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut tool_calls = if trace_is_authoritative {
        existing_tool_calls
            .into_iter()
            .map(|value| {
                let call = serde_json::from_value::<AgentToolCall>(value).map_err(|error| {
                    format!("decode existing Renderer ToolCall for Trace projection: {error}")
                })?;
                serde_json::to_value(crate::tools::renderer_call_projection_from_trace(&call))
                    .map_err(|error| {
                        format!("encode existing Renderer ToolCall for Trace projection: {error}")
                    })
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        existing_tool_calls
    };
    let mut tool_results = if trace_is_authoritative {
        existing_tool_results
            .into_iter()
            .map(|value| {
                let result = serde_json::from_value::<AgentToolResult>(value).map_err(|error| {
                    format!("decode existing Renderer ToolResult for Trace projection: {error}")
                })?;
                serde_json::to_value(crate::tools::renderer_result_projection_from_trace(&result))
                    .map_err(|error| {
                        format!("encode existing Renderer ToolResult for Trace projection: {error}")
                    })
            })
            .collect::<Result<Vec<_>, String>>()?
    } else {
        existing_tool_results
    };
    let mut projected_command_sessions = run
        .remove("commandSessions")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut activated_skills = run
        .remove("activatedSkills")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut skill_activation_revision = run.remove("skillActivationRevision");
    let mut projected_tool_call_ids = tool_calls
        .iter()
        .filter_map(|call| call.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let mut projected_tool_result_ids = tool_results
        .iter()
        .filter_map(|result| result.get("callId").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();

    if let Some(trace) = trace {
        let mut emitted_terminal_error = false;
        run.insert("runId".to_string(), trace.run_id.clone().into());
        match trace.terminal_status {
            crate::ConversationTurnTraceTerminalStatus::InProgress => {}
            crate::ConversationTurnTraceTerminalStatus::Completed => {
                run.insert("status".to_string(), "completed".into());
                run.insert(
                    "completedAt".to_string(),
                    fallback_completed_at
                        .expect("terminal Trace has a projected completion time")
                        .into(),
                );
            }
            crate::ConversationTurnTraceTerminalStatus::Failed => {
                run.insert("status".to_string(), "failed".into());
                run.insert(
                    "completedAt".to_string(),
                    fallback_completed_at
                        .expect("terminal Trace has a projected completion time")
                        .into(),
                );
            }
            crate::ConversationTurnTraceTerminalStatus::Cancelled => {
                run.insert("status".to_string(), "cancelled".into());
                run.insert(
                    "completedAt".to_string(),
                    fallback_completed_at
                        .expect("terminal Trace has a projected completion time")
                        .into(),
                );
            }
        }
        for item in &trace.items {
            match item {
                ConversationTurnTraceItem::AssistantNarration {
                    sequence, content, ..
                } => timeline.push(serde_json::json!({
                    "id": format!("trace-message-{sequence}"),
                    "type": "message",
                    "content": content,
                    "traceSequence": sequence,
                })),
                ConversationTurnTraceItem::UserGuidance {
                    sequence,
                    guidance_id,
                    client_message_id,
                    content,
                    attachments,
                    folder_references,
                    created_at,
                    ..
                } => {
                    let mut projection = serde_json::json!({
                        "id": format!("user-guidance-{client_message_id}"),
                        "type": "user_guidance",
                        "guidanceId": guidance_id,
                        "clientMessageId": client_message_id,
                        "content": content,
                        "attachments": attachments,
                        "status": "applied",
                        "createdAt": created_at,
                        "sequence": sequence,
                        "traceSequence": sequence,
                    });
                    if !folder_references.is_empty() {
                        projection["folderReferences"] = serde_json::to_value(
                            crate::model_folder_references(folder_references),
                        )
                        .map_err(|error| format!("serialize guidance folder references: {error}"))?;
                    }
                    timeline.push(projection);
                }
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    provenance,
                    operation,
                    approval_status,
                    ..
                } => {
                    if let Some(invocation_id) = mcp_trace_anchors.get(call_id) {
                        if emitted_mcp_invocations.insert(invocation_id.clone()) {
                            timeline.push(serde_json::json!({
                                "id": format!("mcp-invocation-{invocation_id}"),
                                "type": "mcp_tool_call",
                                "invocationId": invocation_id,
                                "traceSequence": sequence,
                            }));
                        }
                    } else {
                        if projected_tool_call_ids.insert(call_id.clone()) {
                            let call = crate::tools::renderer_call_projection_from_trace(
                                &AgentToolCall {
                                    id: call_id.clone(),
                                    tool: tool.clone(),
                                    args: operation.clone(),
                                    approval_status: *approval_status,
                                    reason: None,
                                },
                            );
                            tool_calls.push(serde_json::to_value(call).map_err(|error| {
                                format!("encode reconstructed Renderer ToolCall: {error}")
                            })?);
                        }
                        timeline.push(serde_json::json!({
                            "id": format!("tool-call-{call_id}"),
                            "type": "tool_call",
                            "callId": call_id,
                            "identity": provenance,
                            "traceSequence": sequence,
                        }));
                    }
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    success,
                    observation,
                    error,
                    ..
                } => {
                    if tool == "skills_activate" && *success {
                        if let Some((skill, activation_revision)) =
                            project_activated_skill_from_trace_result(observation)
                        {
                            let skill_id = skill
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .expect("projected activated Skill has an id");
                            if let Some(existing) = activated_skills.iter_mut().find(|existing| {
                                existing.get("id").and_then(serde_json::Value::as_str)
                                    == Some(skill_id)
                            }) {
                                *existing = skill;
                            } else {
                                activated_skills.push(skill);
                            }
                            skill_activation_revision = Some(activation_revision.into());
                        }
                    }
                    if !mcp_trace_anchors.contains_key(call_id)
                        && projected_tool_result_ids.insert(call_id.clone())
                    {
                        let result = crate::tools::renderer_result_projection_from_trace(
                            &AgentToolResult {
                                exact_archive_file: None,
                                call_id: call_id.clone(),
                                tool: tool.clone(),
                                ok: *success,
                                result: Some(observation.clone()),
                                error: error.clone(),
                            },
                        );
                        tool_results.push(serde_json::to_value(result).map_err(|error| {
                            format!("encode reconstructed Renderer ToolResult: {error}")
                        })?);
                    }
                }
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence,
                    phase,
                    operation_id,
                    outcome,
                } => match phase {
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Started => {
                        timeline.push(serde_json::json!({
                            "id": format!("context-compaction-{operation_id}"),
                            "type": "context_compaction",
                            "operationId": operation_id,
                            "status": "running",
                            "traceSequence": sequence,
                        }));
                    }
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished => {
                        let status = match outcome.expect("validated compaction finish has outcome") {
                            crate::protocol::AgentContextCompactionEventOutcome::Applied => "applied",
                            crate::protocol::AgentContextCompactionEventOutcome::Skipped => "skipped",
                            crate::protocol::AgentContextCompactionEventOutcome::Failed => "failed",
                            crate::protocol::AgentContextCompactionEventOutcome::Cancelled => "cancelled",
                        };
                        if let Some(existing) = timeline.iter_mut().find(|item| {
                            item.get("type").and_then(serde_json::Value::as_str)
                                == Some("context_compaction")
                                && item.get("operationId").and_then(serde_json::Value::as_str)
                                    == Some(operation_id.as_str())
                        }) {
                            existing["status"] = status.into();
                        }
                    }
                },
                ConversationTurnTraceItem::RuntimeError {
                    sequence, message, code, ..
                } => {
                    if terminal_interruption.is_some() && (
                        trace.terminal_error.as_deref() == Some(message)
                        || code.as_deref().and_then(crate::AgentModelRequestInterruptionReason::from_trace_code).is_some()
                    ) {
                        emitted_terminal_error = true;
                        continue;
                    }
                    emitted_terminal_error |= trace.terminal_error.as_deref() == Some(message);
                    timeline.push(serde_json::json!({
                        "id": format!("trace-error-{sequence}"),
                        "type": "error",
                        "message": message,
                        "traceSequence": sequence,
                    }));
                }
                ConversationTurnTraceItem::ContextMaterial { .. }
                | ConversationTurnTraceItem::AgentMailboxDelivery { .. }
                | ConversationTurnTraceItem::CommandSessionLifecycle { .. }
                | ConversationTurnTraceItem::BackendState { .. } => {}
            }
        }
        if let Some(terminal_error) = trace
            .terminal_status
            .is_terminal()
            .then_some(trace.terminal_error.as_deref())
            .flatten()
            .filter(|_| !emitted_terminal_error && terminal_interruption.is_none())
        {
            timeline.push(serde_json::json!({
                "id": "terminal-error",
                "type": "error",
                "message": terminal_error,
            }));
        }
    }

    for record in command_sessions.iter().filter(|record| {
        trace.is_some_and(|trace| {
            record.snapshot.assistant_message_id == trace.assistant_message_id
                && record.snapshot.origin_run_id == trace.run_id
                && record.snapshot.status.is_terminal()
        })
    }) {
        let snapshot = &record.snapshot;
        let mut projection = serde_json::json!({
            "callId": snapshot.call_id,
            "status": snapshot.status,
            "startedAt": snapshot.started_at,
            "latestSequence": snapshot.latest_sequence,
            "outputTruncated": snapshot.output_truncated,
            "outputs": snapshot.outputs,
        });
        let object = projection
            .as_object_mut()
            .expect("command Session projection is an object");
        if let Some(ended_at) = snapshot.ended_at {
            object.insert("endedAt".to_string(), ended_at.into());
        }
        if let Some(exit_code) = snapshot.exit_code {
            object.insert("exitCode".to_string(), exit_code.into());
        }
        if let Some(artifact_observation) = &snapshot.artifact_observation {
            object.insert(
                "artifactObservation".to_string(),
                serde_json::to_value(artifact_observation)
                    .map_err(|error| format!("serialize command artifact observation: {error}"))?,
            );
        }
        projected_command_sessions.insert(snapshot.call_id.clone(), projection);
    }

    for guidance in guidances {
        if !matches!(
            guidance.status,
            crate::AgentGuidanceStatus::Queued | crate::AgentGuidanceStatus::Abandoned
        ) {
            continue;
        }
        let mut attachments = Vec::with_capacity(guidance.attachment_ids.len());
        for attachment_id in &guidance.attachment_ids {
            let attachment = guidance_attachments.get(attachment_id).ok_or_else(|| {
                format!(
                    "guidance `{}` references missing attachment `{attachment_id}`",
                    guidance.guidance_id
                )
            })?;
            attachments.push(serde_json::json!({
                "id": attachment.id,
                "kind": attachment.kind,
                "name": attachment.original_name,
                "mimeType": attachment.mime_type,
                "sizeBytes": attachment.size_bytes,
            }));
        }
        let folder_references = crate::deserialize_folder_references_from_storage(
            &guidance.folder_references_json,
        )
        .map_err(|error| {
            format!(
                "guidance `{}` has invalid folder references: {error}",
                guidance.guidance_id
            )
        })?;
        let folder_references = crate::model_folder_references(&folder_references);
        run.insert("runId".to_string(), guidance.run_id.clone().into());
        let mut projection = if guidance.status == crate::AgentGuidanceStatus::Abandoned {
            serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "rejected",
                "rejectionCode": "run_interrupted",
                "error": guidance.terminal_reason,
                "recoverable": true,
                "createdAt": guidance.created_at,
            })
        } else {
            serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "queued",
                "createdAt": guidance.created_at,
            })
        };
        if !folder_references.is_empty() {
            projection["folderReferences"] = serde_json::to_value(folder_references)
                .map_err(|error| format!("serialize guidance folder references: {error}"))?;
        }
        timeline.push(projection);
    }

    if trace_is_authoritative {
        timeline.extend(presentation_suffix);
    }

    run.insert("timeline".to_string(), timeline.into());
    run.insert("toolCalls".to_string(), tool_calls.into());
    run.insert("toolResults".to_string(), tool_results.into());
    if !projected_command_sessions.is_empty() {
        run.insert(
            "commandSessions".to_string(),
            projected_command_sessions.into(),
        );
    }
    if !activated_skills.is_empty() {
        run.insert("activatedSkills".to_string(), activated_skills.into());
    }
    if let Some(skill_activation_revision) = skill_activation_revision {
        run.insert(
            "skillActivationRevision".to_string(),
            skill_activation_revision,
        );
    }
    run.entry("startedAt".to_string())
        .or_insert_with(|| fallback_started_at.into());
    for field in [
        "toolDefinitions",
        "approvals",
        "fileChangeProposals",
        "fileChanges",
        "webSearchActivities",
        "readActivities",
    ] {
        run.entry(field.to_string())
            .or_insert_with(|| serde_json::json!([]));
    }
    run.entry("messageStreamCheckpoints".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if trace.is_some_and(|trace| {
        trace.terminal_status == crate::ConversationTurnTraceTerminalStatus::Completed
    }) {
        // A stale Renderer checkpoint can predate the terminal commit. The durable narration
        // above has already been rebuilt; never append its final-answer stream as a second body.
        chat_repository::settle_completed_message_streams(&mut run);
    }
    if !run.contains_key("status") {
        let status = trace
            .map(|trace| match trace.terminal_status {
                crate::ConversationTurnTraceTerminalStatus::InProgress => "running",
                crate::ConversationTurnTraceTerminalStatus::Completed => "completed",
                crate::ConversationTurnTraceTerminalStatus::Failed => "failed",
                crate::ConversationTurnTraceTerminalStatus::Cancelled => "cancelled",
            })
            .unwrap_or("running");
        run.insert("status".to_string(), status.into());
    }

    serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| format!("serialize guidance timeline: {error}"))
}

fn timeline_item_is_rebuilt_from_durable_state(
    item: &serde_json::Value,
    trace_is_authoritative: bool,
    guidance_is_authoritative: bool,
    has_terminal_trace_error: bool,
) -> bool {
    // Timeline ids are Renderer presentation identities, not persistence identities. A committed
    // stream keeps ids such as `message-stream-*`, so id-prefix checks duplicate it on reload.
    // `traceSequence` is the durable ordering anchor and must be projected exactly once.
    if trace_is_authoritative
        && item
            .get("traceSequence")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    {
        return true;
    }

    match item.get("type").and_then(serde_json::Value::as_str) {
        // These are views over durable Trace/MCP state. Older live projections may predate a
        // traceSequence, so their type is also authoritative once the Trace exists.
        Some("tool_call" | "mcp_tool_call" | "context_compaction") => trace_is_authoritative,
        // Guidance can be queued in its journal before it receives a Trace sequence. Rebuild all
        // Guidance from that journal/Trace pair so an in-run user insertion cannot appear twice.
        Some("user_guidance") => guidance_is_authoritative,
        // A terminal Trace error has one canonical projection. Host-only errors remain untouched
        // when the Trace has no terminal error of its own.
        Some("error") => trace_is_authoritative && has_terminal_trace_error,
        _ => false,
    }
}

fn project_activated_skill_from_trace_result(
    observation: &serde_json::Value,
) -> Option<(serde_json::Value, &str)> {
    let status = observation.get("status")?.as_str()?;
    if !matches!(status, "activated" | "alreadyActivated") {
        return None;
    }
    let revision = observation.get("activationRevision")?.as_str()?;
    let skill = observation.get("skill")?;
    let id = skill.get("id")?.as_str()?;
    let name = skill.get("name")?.as_str()?;
    let skill_revision = skill.get("revision")?.as_str()?;
    let source = skill.get("source")?.as_str()?;
    let (source_kind, source_id) = source.split_once(':')?;
    if !matches!(source_kind, "workspace" | "bundled" | "installed") || source_id.trim().is_empty()
    {
        return None;
    }
    Some((
        serde_json::json!({
            "id": id,
            "name": name,
            "revision": skill_revision,
            "source": { "kind": source_kind, "id": source_id },
        }),
        revision,
    ))
}

fn project_durable_mcp_invocations(
    run: &mut serde_json::Map<String, serde_json::Value>,
    trace: Option<&ConversationTurnTrace>,
    rows: &[AgentPendingActionRecord],
) -> Result<(), String> {
    let Some(trace) = trace else {
        return Ok(());
    };
    let existing_call_ids = mcp_trace_anchors(run)?;
    let mut projected = run
        .get("mcpInvocations")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .ok_or_else(|| "current AgentRun MCP invocations must be an array".to_string())?;

    for item in &trace.items {
        let ConversationTurnTraceItem::ToolCall {
            call_id,
            tool,
            provenance: crate::AgentToolIdentity::Mcp { provenance },
            ..
        } = item
        else {
            continue;
        };
        if existing_call_ids.contains_key(call_id) {
            continue;
        }
        let matching = rows
            .iter()
            .filter(|row| row.tool_call_id.as_deref() == Some(call_id.as_str()))
            .collect::<Vec<_>>();
        let [row] = matching.as_slice() else {
            continue;
        };
        // Terminal automatic journals deliberately scrub this public action. If no prior safe
        // projection exists, trace provenance alone cannot recover the one-time invocation UUID
        // or historical display name, so leave the item generic instead of inventing facts.
        let Ok(crate::AgentProposedAction::McpToolCall { approval }) =
            serde_json::from_str::<crate::AgentProposedAction>(&row.action_json)
        else {
            continue;
        };
        let identity = &approval.identity;
        let expected_storage_id = format!(
            "v2:{}:{}:{}",
            identity.run_id.len(),
            identity.run_id,
            identity.action_id
        );
        if row.action_id != expected_storage_id
            || row.run_id != trace.run_id
            || identity.call_id != *call_id
            || identity.provenance != *provenance
            || approval.call.id != *call_id
            || approval.call.tool != *tool
            || approval.summary.server_id != provenance.server_id
            || approval.summary.scope != provenance.scope
            || approval.summary.raw_tool_name != provenance.raw_tool_name
            || approval.summary.model_tool_name != provenance.model_tool_name
            || !approval.summary.external
        {
            continue;
        }
        let terminal_result = trace.items.iter().find_map(|candidate| {
            let ConversationTurnTraceItem::ToolResult {
                call_id: result_call_id,
                observation,
                success,
                ..
            } = candidate
            else {
                return None;
            };
            (result_call_id == call_id).then_some((observation, *success))
        });
        let duration_ms = u64::try_from(row.updated_at.saturating_sub(row.created_at)).unwrap_or(0);
        let lifecycle = if let Some((observation, success)) = terminal_result {
            project_terminal_mcp_trace_lifecycle(observation, success, duration_ms)
        } else {
            match row.status.as_str() {
                "pending" => Some((
                    "pending_approval",
                    "definitely_not_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                "approved" => Some((
                    "approved",
                    "definitely_not_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                "executing" => Some((
                    "dispatching",
                    "possibly_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                _ => None,
            }
        };
        let Some((state, dispatch, outcome, is_error, error_code, duration, truncated)) = lifecycle
        else {
            continue;
        };
        let mut invocation = serde_json::json!({
            "actionId": identity.action_id,
            "invocationId": identity.invocation_id,
            "callId": identity.call_id,
            "serverId": provenance.server_id,
            "serverDisplayName": approval.summary.server_display_name,
            "scope": provenance.scope,
            "rawToolName": provenance.raw_tool_name,
            "modelToolName": provenance.model_tool_name,
            "external": true,
            "state": state,
            "dispatchCertainty": dispatch,
            "outputTruncated": truncated,
        });
        let object = invocation
            .as_object_mut()
            .expect("MCP invocation projection is an object");
        if let Some(display_reason) = &approval.summary.display_reason {
            object.insert("displayReason".to_string(), display_reason.clone().into());
        }
        if let Some(outcome) = outcome {
            object.insert("outcome".to_string(), outcome.into());
        }
        if let Some(is_error) = is_error {
            object.insert("isError".to_string(), is_error.into());
        }
        if let Some(error_code) = error_code {
            object.insert("errorCode".to_string(), error_code.into());
        }
        if let Some(duration) = duration {
            object.insert("durationMs".to_string(), duration.into());
        }
        projected.push(invocation);
    }
    run.insert("mcpInvocations".to_string(), projected.into());
    Ok(())
}

type ProjectedMcpLifecycle<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<bool>,
    Option<&'a str>,
    Option<u64>,
    bool,
);

fn project_terminal_mcp_trace_lifecycle(
    observation: &serde_json::Value,
    success: bool,
    duration_ms: u64,
) -> Option<ProjectedMcpLifecycle<'_>> {
    let value = observation.as_object()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("mcp_tool")
        || value.get("external").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return None;
    }
    let status = value.get("status")?.as_str()?;
    let outcome = value.get("outcome")?.as_str()?;
    let dispatch = value.get("dispatchCertainty")?.as_str()?;
    let truncated = value
        .get("truncatedAtSource")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        });
    match (status, outcome, dispatch, success) {
        ("completed", "succeeded", "response_received", true) => Some((
            "completed",
            dispatch,
            Some(outcome),
            Some(false),
            None,
            Some(duration_ms),
            truncated,
        )),
        ("completed", "tool_error", "response_received", false) => Some((
            "completed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_error")),
            Some(duration_ms),
            truncated,
        )),
        ("rejected", "rejected", "definitely_not_dispatched", false) => Some((
            "rejected",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_rejected")),
            None,
            false,
        )),
        ("cancelled", "cancelled", "definitely_not_dispatched", false) => Some((
            "cancelled",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.tool_cancelled")),
            None,
            false,
        )),
        ("expired", "expired", "definitely_not_dispatched", false) => Some((
            "expired",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_payload_expired")),
            None,
            false,
        )),
        ("payload_unavailable", "payload_unavailable", "definitely_not_dispatched", false) => {
            Some((
                "payload_unavailable",
                dispatch,
                Some(outcome),
                Some(true),
                Some(code.unwrap_or("mcp.approval_payload_unavailable")),
                None,
                false,
            ))
        }
        ("policy_denied", "policy_denied", "definitely_not_dispatched", false) => Some((
            "policy_denied",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_policy_denied")),
            None,
            false,
        )),
        ("outcome_unknown", "outcome_unknown", "possibly_dispatched", false) => Some((
            "outcome_unknown",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.tool_outcome_unknown")),
            None,
            false,
        )),
        ("failed", "output_too_large", "response_received", false) => Some((
            "failed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_output_too_large")),
            Some(duration_ms),
            true,
        )),
        ("failed", "timed_out", "definitely_not_dispatched", false) => Some((
            "failed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_timeout")),
            Some(duration_ms),
            false,
        )),
        ("failed", "transport_error", "definitely_not_dispatched" | "response_received", false) => {
            Some((
                "failed",
                dispatch,
                Some(outcome),
                Some(true),
                Some(code.unwrap_or("mcp.tool_failed")),
                Some(duration_ms),
                truncated && dispatch == "response_received",
            ))
        }
        _ => None,
    }
}

fn mcp_trace_anchors(
    run: &serde_json::Map<String, serde_json::Value>,
) -> Result<HashMap<String, String>, String> {
    let invocations = run
        .get("mcpInvocations")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "current AgentRun MCP invocations must be an array".to_string())?;

    let mut invocation_ids_by_call_id = HashMap::<String, String>::new();
    let mut call_ids_by_invocation_id = HashMap::<String, String>::new();
    for invocation in invocations {
        let call_id = invocation
            .get("callId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "current MCP invocation is missing callId".to_string())?;
        let invocation_id = invocation
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "current MCP invocation is missing invocationId".to_string())?;
        if invocation_ids_by_call_id
            .insert(call_id.to_string(), invocation_id.to_string())
            .is_some()
        {
            return Err("current MCP invocation callId is duplicated".to_string());
        }
        if call_ids_by_invocation_id
            .insert(invocation_id.to_string(), call_id.to_string())
            .is_some()
        {
            return Err("current MCP invocation identity is duplicated".to_string());
        }
    }
    Ok(invocation_ids_by_call_id)
}

#[cfg(test)]
mod request_owned_projection_tests {
    use super::*;

    #[test]
    fn rebuilding_history_ignores_legacy_owner_cards_without_resetting_the_run() {
        let raw = chat_repository::canonical_agent_run_lifecycle_projection(
            None, "legacy-run", "running", 2, 4, None,
        ).unwrap();
        let mut run: serde_json::Value = serde_json::from_str(&raw).unwrap();
        run["firstResponseAt"] = 3.into();
        run["collaborationTimelineActivities"] = serde_json::json!([{
            "activityId":"old-event", "parentAgentId":"old-parent",
            "parentConversationId":"old-parent-chat"
        }]);
        let trace = conversation_trace_repository::ConversationTurnTraceRecord {
            trace: crate::ConversationTraceSnapshot::default().in_progress_trace(
                "legacy-run", "legacy-chat", "legacy-assistant",
            ),
            completed_at: None,
        };
        let projected = project_guidance_timeline(
            Some(&run.to_string()), Some(&trace), &[], &[], &HashMap::new(), &[], 99,
        ).unwrap();
        let projected: serde_json::Value = serde_json::from_str(&projected).unwrap();
        assert_eq!(projected["runId"], "legacy-run");
        assert_eq!(projected["startedAt"], 2);
        assert_eq!(projected["firstResponseAt"], 3);
        assert_eq!(projected["collaborationTimelineActivities"], serde_json::json!([]));
    }
}
