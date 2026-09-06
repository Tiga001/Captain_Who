#[derive(Clone, Copy)]
struct OrdinaryProviderContinuationPersistence<'a> {
    capabilities: crate::ProviderRuntimeCapabilities,
    reasoning_mode: crate::ReasoningMode,
    conversation_id: Option<&'a str>,
    request_index: usize,
    protocol: &'a ProviderProtocolKey,
    vault: Option<&'a Arc<crate::ProviderContinuationVault>>,
}

#[allow(clippy::too_many_arguments)]
fn apply_steer_inputs(
    run_id: &str,
    trace_assistant_message_id: Option<&str>,
    mut preceding_assistant_turn: Option<crate::llm::LlmAssistantTurn>,
    provider_continuation_persistence: Option<OrdinaryProviderContinuationPersistence<'_>>,
    inputs: Vec<AgentSteerInput>,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    event_stream: &mut AgentEventStream,
    tool_context: &mut ToolExecutionContext,
    run_context: &mut Option<AgentRunContext>,
) -> AgentResult<Option<AgentContextBaseline>> {
    let attachment_contexts = inputs
        .iter()
        .map(|input| {
            if !input.attachments.is_empty() && input.attachment_library.is_none() {
                return Err(AgentError::structured(
                    "agent.steer_attachment_library_missing",
                    "用户引导附件缺少 Host 构造的附件库快照。",
                    json!({ "guidanceId": input.guidance_id }),
                ));
            }
            build_attachment_context(&input.attachments, input.attachment_library.as_ref())
        })
        .collect::<AgentResult<Vec<_>>>()?;

    let preceding_assistant_content = preceding_assistant_turn
        .as_ref()
        .map(crate::llm::LlmAssistantTurn::visible_text)
        .map(str::trim)
        .filter(|content| !content.is_empty())
        .map(ToString::to_string);
    let expected_next_trace_sequence = preceding_assistant_turn.as_ref().map(|_| {
        conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .next_sequence()
    });
    let expected_preceding_narration_sequence = preceding_assistant_content
        .as_ref()
        .and(expected_next_trace_sequence);
    let expected_steer_boundary_sequence = preceding_assistant_turn
        .as_ref()
        .filter(|_| preceding_assistant_content.is_none())
        .and(expected_next_trace_sequence);
    let mut pending_provider_continuation_handoff = None;
    if let (Some(sequence), Some(persistence)) = (
        expected_next_trace_sequence,
        provider_continuation_persistence,
    ) {
        let turn = preceding_assistant_turn.take().ok_or_else(|| {
            provider_continuation_runtime_error(crate::ProviderContinuationStoreError::InvalidTurn)
        })?;
        let projection = if preceding_assistant_content.is_some() {
            crate::ProviderContinuationProjection::ConversationTraceItem {
                sequence,
                ordinal: 0,
            }
        } else {
            crate::ProviderContinuationProjection::ConversationSteerBoundary {
                guidance_sequence: sequence,
            }
        };
        let (turn, pending) = stage_ordinary_provider_assistant_turn(
            turn,
            persistence.capabilities,
            persistence.reasoning_mode,
            persistence.conversation_id,
            trace_assistant_message_id,
            run_id,
            persistence.request_index,
            persistence.protocol,
            persistence.vault,
            projection,
        )?;
        preceding_assistant_turn = Some(turn);
        pending_provider_continuation_handoff = pending;
        if pending_provider_continuation_handoff.is_some() && trace_observer.is_none() {
            release_pending_provider_continuation(&mut pending_provider_continuation_handoff)?;
            return Err(provider_continuation_runtime_error(
                crate::ProviderContinuationStoreError::InvalidBinding,
            ));
        }
    }
    let mut applied = Vec::with_capacity(inputs.len());
    let preceding_assistant_sequence;
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        preceding_assistant_sequence = match preceding_assistant_content.as_deref() {
            Some(content) => {
                let sequence = recorder
                    .record_narration(content)
                    .map_err(AgentError::new)?;
                if sequence != expected_preceding_narration_sequence {
                    return Err(provider_continuation_runtime_error(
                        crate::ProviderContinuationStoreError::InvalidBinding,
                    ));
                }
                sequence
            }
            None => None,
        };
        for input in &inputs {
            let sequence = recorder
                .record_user_guidance(
                    &input.guidance_id,
                    &input.client_message_id,
                    &input.content,
                    &input.attachments,
                    input.created_at,
                )
                .ok_or_else(|| {
                    AgentError::structured(
                        "agent.invalid_steer_input",
                        "用户引导正文不能为空。",
                        json!({ "guidanceId": input.guidance_id }),
                    )
                })?;
            let (attachments, _) = trace_attachments_from_input(&input.attachments);
            applied.push((input.clone(), attachments, sequence));
        }
    }
    if let Some(expected_sequence) = expected_steer_boundary_sequence {
        if applied.first().map(|(_, _, sequence)| *sequence) != Some(expected_sequence) {
            return Err(provider_continuation_runtime_error(
                crate::ProviderContinuationStoreError::InvalidBinding,
            ));
        }
    }
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for ((input, _, sequence), attachment_context) in
            applied.iter().zip(attachment_contexts.iter())
        {
            let content = input.content.trim();
            let context_content = if attachment_context.text.trim().is_empty() {
                content.to_string()
            } else {
                format!("{content}\n\n{}", attachment_context.text)
            };
            let mut message = LlmMessage::text(LlmMessageRole::User, context_content);
            *message
                .images_mut()
                .expect("user attachment messages support images") =
                attachment_context.images.clone();
            recorder
                .record_model_message(*sequence, 0, &message)
                .map_err(AgentError::new)?;
        }
    }
    let baseline = publish_trace_snapshot(conversation_trace, trace_observer)?;
    // The authoritative storage observer promotes this exact trace projection inside the same
    // transaction that makes its Trace/ModelContext owner durable. Runtime must not promote here:
    // an arbitrary observer can acknowledge an in-memory snapshot without persisting it, and a
    // commit-unknown observer can return an error after the transaction already succeeded.
    // Dropping this handoff leaves the former safely staged for reconciliation and the latter
    // already active through the storage transaction's idempotent exact-projection promotion.
    drop(pending_provider_continuation_handoff.take());

    if let Some(turn) = preceding_assistant_turn {
        let checkpoint_turn = turn.without_raw_continuation_for_checkpoint();
        let provider_projection_sequence =
            preceding_assistant_sequence.or(expected_steer_boundary_sequence);
        active_context.push(
            ContextItem::new(
                LlmMessage::from_assistant_turn(turn),
                with_trace_origin(
                    ContextMetadata::new(
                        ContextSource::ModelResponse,
                        ContextScope::Run,
                        ContextRetention::Retained,
                    ),
                    trace_assistant_message_id,
                    provider_projection_sequence,
                ),
            )
            .with_checkpoint_message(LlmMessage::from_assistant_turn(checkpoint_turn)),
        );
    }
    for ((input, attachments, sequence), attachment_context) in
        applied.into_iter().zip(attachment_contexts)
    {
        if let Some(library) = input.attachment_library {
            replace_runtime_attachment_library(run_context, tool_context, library);
        }
        let content = input.content.trim().to_string();
        let context_content = if attachment_context.text.trim().is_empty() {
            content.clone()
        } else {
            format!("{content}\n\n{}", attachment_context.text)
        };
        let mut message = LlmMessage::text(LlmMessageRole::User, context_content);
        *message
            .images_mut()
            .expect("user attachment messages support images") = attachment_context.images;
        active_context.push(ContextItem::new(
            message,
            with_trace_origin(
                ContextMetadata::new(
                    ContextSource::UserGuidance,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ),
                trace_assistant_message_id,
                Some(sequence),
            ),
        ));
        event_stream.emit(AgentEvent::GuidanceApplied {
            run_id: run_id.to_string(),
            guidance_id: input.guidance_id,
            client_message_id: input.client_message_id,
            content,
            attachments,
            created_at: input.created_at,
            sequence,
        });
    }

    Ok(baseline)
}

fn apply_agent_mailbox_delivery(
    delivery: &AgentSamplingBoundaryDelivery,
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    assistant_message_id: &str,
) -> AgentResult<()> {
    let mut newly_recorded = Vec::new();
    {
        let mut recorder = conversation_trace
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for message in &delivery.messages {
            if let Some(content) = recorder
                .record_agent_mailbox_delivery(
                    message.trace_sequence,
                    &delivery.receipt_id,
                    &message.message_id,
                    &message.sender_agent_id,
                    &message.sender_task_name,
                    &message.sender_task_path,
                    message.kind,
                    &message.content,
                    message.created_at,
                )
                .map_err(AgentError::new)?
            {
                newly_recorded.push((message.clone(), content));
            }
        }
    }
    if newly_recorded.is_empty() {
        return Ok(());
    }
    publish_trace_snapshot(conversation_trace, trace_observer)?;
    for (message, content) in newly_recorded {
        active_context.push(ContextItem::new(
            LlmMessage::text(LlmMessageRole::User, content),
            with_trace_origin(
                ContextMetadata::new(
                    ContextSource::UserGuidance,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ),
                Some(assistant_message_id),
                Some(message.trace_sequence),
            ),
        ));
    }
    Ok(())
}

fn apply_human_interaction_ignored_events(
    events: &[AgentHumanInteractionIgnoredEvent],
    active_context: &mut ContextFrame,
    conversation_trace: &Arc<Mutex<ConversationTraceRecorder>>,
    trace_observer: Option<&AgentConversationTraceObserver>,
    assistant_message_id: &str,
) -> AgentResult<()> {
    let mut newly_recorded = Vec::new();
    {
        let mut recorder = conversation_trace.lock().unwrap_or_else(|error| error.into_inner());
        for event in events {
            crate::human_interaction::validate_human_interaction_id(&event.request_id)
                .map_err(|_| AgentError::new("Invalid ignored question request identity."))?;
            let content = json!({"type":"human_interaction_status", "requestId":event.request_id, "status":"ignored"}).to_string();
            if let Some(content) = recorder.record_backend_state(
                event.trace_sequence, &event.event_id, &content, event.created_at,
                crate::ConversationBackendStatePlacement::Timeline,
            ).map_err(AgentError::new)? {
                newly_recorded.push((event.trace_sequence, content));
            }
        }
    }
    if newly_recorded.is_empty() {
        return Ok(());
    }
    publish_trace_snapshot(conversation_trace, trace_observer)?;
    for (sequence, content) in newly_recorded {
        active_context.push(ContextItem::new(
            LlmMessage::backend_state(content),
            with_trace_origin(
                ContextMetadata::new(ContextSource::BackendState, ContextScope::Run, ContextRetention::Retained),
                Some(assistant_message_id), Some(sequence),
            ),
        ));
    }
    Ok(())
}

fn with_trace_origin(
    metadata: ContextMetadata,
    assistant_message_id: Option<&str>,
    sequence: Option<u64>,
) -> ContextMetadata {
    match assistant_message_id.zip(sequence) {
        Some((assistant_message_id, sequence)) => metadata.with_origin(
            ContextOrigin::conversation_trace_item(assistant_message_id, sequence),
        ),
        None => metadata,
    }
}

fn replace_runtime_attachment_library(
    run_context: &mut Option<AgentRunContext>,
    tool_context: &mut ToolExecutionContext,
    library: crate::protocol::AgentAttachmentLibraryContext,
) {
    let context = run_context.get_or_insert_with(|| AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    context.attachment_library = Some(library.clone());
    tool_context.replace_attachment_library(library);
}

struct ToolResultArchiveRequest<'a> {
    storage: Option<&'a Arc<StorageService>>,
    conversation_id: Option<&'a str>,
    assistant_message_id: Option<&'a str>,
    sequence: Option<u64>,
    raw_result: &'a AgentToolResult,
    archive_result: &'a AgentToolResult,
    model_result: &'a AgentToolResult,
    model_result_for_archive_comparison: &'a AgentToolResult,
    model_tool_result_gate: &'a ModelToolResultGate,
}

fn archive_tool_result(
    request: ToolResultArchiveRequest<'_>,
) -> AgentResult<ConversationHistoryArchiveTraceMetadata> {
    let truncated_at_source = crate::tools::tool_result_truncated_at_source(request.raw_result);
    let gate_truncates = request.model_tool_result_gate.would_truncate_with_source(
        &request.model_result.call_id,
        !request.model_result.ok,
        request.model_result,
        truncated_at_source,
    );
    let exact_preview_truncated = request.raw_result.exact_archive_file.is_some()
        && request
            .raw_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation);
    let mut metadata = ConversationHistoryArchiveTraceMetadata {
        truncated_at_source,
        model_projection_truncated: projection_differs(
            request.archive_result,
            request.model_result_for_archive_comparison,
        ) || gate_truncates
            || exact_preview_truncated,
        archive_projection_truncated: projection_differs(
            request.raw_result,
            request.archive_result,
        ),
        ..Default::default()
    };
    let (Some(storage), Some(conversation_id), Some(assistant_message_id), Some(sequence)) = (
        request.storage,
        request.conversation_id,
        request.assistant_message_id,
        request.sequence,
    ) else {
        return Ok(metadata);
    };
    if let Some(archive) = storage
        .resolve_authoritative_command_archive_metadata(
            conversation_id,
            assistant_message_id,
            request.raw_result,
        )
        .map_err(AgentError::new)?
    {
        if request
            .raw_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation)
        {
            metadata.model_projection_truncated = true;
        }
        metadata.truncated_at_source = archive.truncated_at_source;
        metadata.archive_projection_truncated = archive.archive_projection_truncated;
        metadata.archive_ref = archive.archive_ref;
        metadata.content_hash = archive.content_hash;
        metadata.archived_bytes = archive.archived_bytes;
        metadata.archived_completely = archive.archived_completely;
        return Ok(metadata);
    }
    let archive = if let Some(exact_file) = request
        .raw_result
        .exact_archive_file
        .as_ref()
        .or(request.archive_result.exact_archive_file.as_ref())
    {
        storage.archive_conversation_tool_result_file(ConversationHistoryArchiveFileInput {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            sequence,
            call_id: request.archive_result.call_id.clone(),
            tool: request.archive_result.tool.clone(),
            content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            content_path: exact_file.path().to_path_buf(),
            truncated_at_source: metadata.truncated_at_source,
            model_projection_truncated: metadata.model_projection_truncated,
            archive_projection_truncated: metadata.archive_projection_truncated,
            created_at: now_ms(),
        })
    } else {
        let content = match serde_json::to_string(request.archive_result) {
            Ok(content) => content,
            Err(error) => {
                eprintln!("failed to serialize exact history tool result: {error}");
                return Ok(metadata);
            }
        };
        storage.archive_conversation_tool_result(ConversationHistoryArchiveInput {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            sequence,
            call_id: request.archive_result.call_id.clone(),
            tool: request.archive_result.tool.clone(),
            content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
            content,
            truncated_at_source: metadata.truncated_at_source,
            model_projection_truncated: metadata.model_projection_truncated,
            archive_projection_truncated: metadata.archive_projection_truncated,
            created_at: now_ms(),
        })
    };
    match archive {
        Ok(archive) => {
            metadata.archive_ref = Some(archive.archive_ref);
            metadata.content_hash = Some(archive.content_hash);
            metadata.archived_bytes = Some(archive.total_bytes);
            metadata.archived_completely = Some(archive.archived_completely);
        }
        Err(error) => {
            // Tool settlement remains authoritative even if the auxiliary archive cannot commit.
            // The missing archive identity makes the loss explicit instead of pretending exact
            // recovery is possible.
            eprintln!("failed to store exact history tool result: {error}");
        }
    }
    Ok(metadata)
}

pub(crate) fn finalize_model_tool_observation(
    gate: &ModelToolResultGate,
    call_id: &str,
    is_error: bool,
    model_result: &AgentToolResult,
    archive: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<String> {
    let recovery = model_tool_result_recovery(archive)?;
    let requires_exact_recovery = archive.model_projection_truncated
        && model_result
            .result
            .as_ref()
            .is_some_and(crate::exact_capture::value_has_recoverable_preview_truncation);
    let output = if requires_exact_recovery {
        gate.project_with_required_recovery(call_id, is_error, model_result, recovery.as_ref())
    } else {
        gate.project(call_id, is_error, model_result, recovery.as_ref())
    };
    if output.truncated && !model_observation_has_recovery(&output.content) {
        return Err(AgentError::new(format!(
            "工具 `{}` 的模型结果超过 10K token，但没有可用的分页游标或 Exact History 恢复位置。",
            model_result.tool
        )));
    }
    Ok(output.content)
}

struct FinalizedToolObservations {
    model: String,
    checkpoint: String,
    durable: String,
}

struct ToolResultProjectionLanes<'a> {
    model: &'a AgentToolResult,
    checkpoint: &'a AgentToolResult,
    durable: &'a AgentToolResult,
}

/// Applies the live, private-checkpoint, and durable Model projections at their distinct trust
/// boundaries.
///
/// `results.checkpoint` may contain private resumability authority that the next model request
/// needs after a crash. `results.durable` is the immutable public projection written to the
/// conversation model-context log. They are normally identical to `results.model`; FileChange is
/// the deliberate exception because a successful call adds a run-local successor Observation
/// only after the Host has already committed the canonical ToolResult. MCP remains the other
/// exception: external result bodies are live-only by policy, so both checkpoint and durable
/// replay use the persistence-safe checkpoint projection.
fn finalize_tool_observations(
    gate: &ModelToolResultGate,
    call_id: &str,
    is_error: bool,
    results: ToolResultProjectionLanes<'_>,
    archive: &ConversationHistoryArchiveTraceMetadata,
    durable_replay_uses_checkpoint_projection: bool,
) -> AgentResult<FinalizedToolObservations> {
    let model_observation =
        finalize_model_tool_observation(gate, call_id, is_error, results.model, archive)?;
    let checkpoint_observation = if durable_replay_uses_checkpoint_projection {
        finalize_model_tool_observation(gate, call_id, is_error, results.checkpoint, archive)?
    } else {
        model_observation.clone()
    };
    let durable_observation = if durable_replay_uses_checkpoint_projection {
        checkpoint_observation.clone()
    } else {
        finalize_model_tool_observation(gate, call_id, is_error, results.durable, archive)?
    };
    Ok(FinalizedToolObservations {
        model: model_observation,
        checkpoint: checkpoint_observation,
        durable: durable_observation,
    })
}

fn load_continuation_archive_metadata(
    input: &AgentChatInput,
    storage: Option<&StorageService>,
) -> AgentResult<ConversationHistoryArchiveTraceMetadata> {
    let (
        Some(storage),
        Some(checkpoint),
        Some(continuation),
        Some(conversation_id),
        Some(assistant_message_id),
    ) = (
        storage,
        input.resume_checkpoint.as_ref(),
        input.tool_continuation.as_ref(),
        input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.as_deref()),
        input.assistant_message_id.as_deref(),
    )
    else {
        return Ok(ConversationHistoryArchiveTraceMetadata::default());
    };
    // A Host-owned command Session uses a separate immutable archive sequence so it can commit
    // the complete process spool before the ordinary ToolResult (and before an approval
    // continuation) becomes visible. Resolve that opaque, conversation-scoped ref first instead
    // of assuming the archive sequence must equal the ToolResult trace sequence.
    if let Some(archive) = storage
        .resolve_authoritative_command_archive_metadata(
            conversation_id,
            assistant_message_id,
            &continuation.result,
        )
        .map_err(AgentError::new)?
    {
        return Ok(archive);
    }
    let sequence = continuation_result_sequence(checkpoint, &continuation.call.id);
    let Some(archive) = storage
        .find_conversation_history_archive_for_trace_item(
            conversation_id,
            assistant_message_id,
            sequence,
        )
        .map_err(AgentError::new)?
    else {
        return Ok(ConversationHistoryArchiveTraceMetadata::default());
    };
    if archive.call_id != continuation.call.id
        || archive.call_id != continuation.result.call_id
        || archive.tool != continuation.call.tool
        || archive.tool != continuation.result.tool
        || archive.assistant_message_id != assistant_message_id
        || archive.sequence != sequence
    {
        return Err(AgentError::new(
            "审批续跑的 Exact History Archive 与冻结的工具结果身份不匹配。",
        ));
    }
    Ok(ConversationHistoryArchiveTraceMetadata {
        archive_ref: Some(archive.archive_ref),
        content_hash: Some(archive.content_hash),
        archived_bytes: Some(archive.total_bytes),
        archived_completely: Some(archive.archived_completely),
        truncated_at_source: archive.truncated_at_source,
        model_projection_truncated: archive.model_projection_truncated,
        history_projection_truncated: false,
        archive_projection_truncated: archive.archive_projection_truncated,
    })
}

fn model_tool_result_recovery(
    archive: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<Option<ModelToolResultRecovery>> {
    let mut recovery = ModelToolResultRecovery {
        truncated_at_source: Some(archive.truncated_at_source),
        ..Default::default()
    };
    if archive.archived_completely == Some(true) {
        let archive_ref = archive.archive_ref.as_deref().ok_or_else(|| {
            AgentError::new("完整的工具历史归档缺少 archive ref，无法生成模型续读位置。")
        })?;
        let open =
            crate::storage::conversation_history_open::encode_archive_history_open(archive_ref, 0)
                .map_err(AgentError::new)?;
        recovery.history_open = Some(Value::String(open.clone()));
        recovery.continue_with = Some(json!({
            "tool": "conversation_history",
            "args": {
                "open": open
            }
        }));
    }
    Ok(Some(recovery))
}

fn model_observation_has_recovery(content: &str) -> bool {
    let Ok(Value::Object(object)) = serde_json::from_str::<Value>(content) else {
        return false;
    };
    for field in [
        "continueWith",
        "historyOpen",
        "cursor",
        "nextCursor",
        "nextStartByte",
        "nextStartLine",
        "nextAfterPath",
    ] {
        if object.get(field).is_some_and(|value| !value.is_null()) {
            return true;
        }
    }
    object
        .get("navigation")
        .and_then(Value::as_object)
        .is_some_and(|navigation| navigation.values().any(|value| !value.is_null()))
}

fn projection_differs(left: &AgentToolResult, right: &AgentToolResult) -> bool {
    match (serde_json::to_vec(left), serde_json::to_vec(right)) {
        (Ok(left), Ok(right)) => left != right,
        _ => true,
    }
}

fn auto_executes_builtin_prepared_action(
    permission: crate::AgentBuiltinExecutionPermission,
    call_requires_approval: bool,
    identity: &AgentToolIdentity,
) -> bool {
    permission == crate::AgentBuiltinExecutionPermission::AutoApprove
        && call_requires_approval
        && (matches!(identity, AgentToolIdentity::BuiltinCapability { .. })
            || matches!(
                identity,
                AgentToolIdentity::RuntimeExtension {
                    extension_id,
                    tool_name,
                } if extension_id
                    == crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
                    && tool_name == crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME
            ))
}
