/// Appends one authoritative recovery result to the final unresolved ToolCall of an in-progress
/// trace. The identity and ordering checks make the operation safe to retry during startup.
pub fn conversation_trace_with_recovered_tool_result(
    trace: &ConversationTurnTrace,
    result: &AgentToolResult,
) -> Result<ConversationTurnTrace, String> {
    if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err("only an in-progress conversation trace can accept a recovered result".into());
    }
    let Some(ConversationTurnTraceItem::ToolCall {
        call_id,
        tool,
        operation,
        approval_status,
        ..
    }) = trace.items.last()
    else {
        return Err("conversation trace has no final unresolved ToolCall".into());
    };
    if result.call_id != *call_id || result.tool != *tool {
        return Err("recovered ToolResult identity does not match the unresolved ToolCall".into());
    }
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: tool.clone(),
        args: operation.clone(),
        approval_status: *approval_status,
        reason: operation
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let mut recorder = ConversationTraceRecorder::from_durable_trace(
        trace.items.clone(),
        trace
            .items
            .last()
            .map(ConversationTurnTraceItem::sequence)
            .unwrap_or(0)
            .saturating_add(1),
        trace.truncated,
    );
    recorder.record_tool_result(&call, result);
    let recovered = recorder.snapshot().in_progress_audit_trace(
        &trace.run_id,
        &trace.conversation_id,
        &trace.assistant_message_id,
    );
    recovered.validate()?;
    Ok(recovered)
}

/// Appends a recovered ToolResult to both durable audit and replay-safe model context.
///
/// Startup recovery must advance these projections together. The immutable ToolCall model item
/// proves that the result closes the exact provider/runtime identity persisted before dispatch.
pub fn conversation_trace_snapshot_with_recovered_tool_result(
    trace: &ConversationTurnTrace,
    model_context_items: Vec<ConversationModelContextItem>,
    result: &AgentToolResult,
) -> Result<ConversationTraceSnapshot, String> {
    if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
        return Err("only an in-progress conversation trace can accept a recovered result".into());
    }
    let Some(ConversationTurnTraceItem::ToolCall {
        call_id,
        tool,
        operation,
        approval_status,
        ..
    }) = trace.items.last()
    else {
        return Err("conversation trace has no final unresolved ToolCall".into());
    };
    if result.call_id != *call_id || result.tool != *tool {
        return Err("recovered ToolResult identity does not match the unresolved ToolCall".into());
    }
    let call = AgentToolCall {
        id: call_id.clone(),
        tool: tool.clone(),
        args: operation.clone(),
        approval_status: *approval_status,
        reason: operation
            .get("reason")
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let next_sequence = trace
        .items
        .last()
        .map(ConversationTurnTraceItem::sequence)
        .unwrap_or(0)
        .saturating_add(1);
    let mut recorder =
        ConversationTraceRecorder::from_durable_snapshot(ConversationTraceSnapshot {
            items: trace.items.clone(),
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        });
    let model_result = crate::tools::model_projection_for_persisted_continuation(result);
    let model_observation = crate::context::ModelToolResultGate::new(
        crate::context::ContextTextBudget::heuristic(crate::context::MODEL_TOOL_RESULT_MAX_TOKENS),
    )
    .project(&call.id, !model_result.ok, &model_result, None)
    .content;
    record_model_tool_exchange_with_projection(
        &mut recorder,
        &call,
        result,
        None,
        Some(&model_observation),
        ConversationHistoryArchiveTraceMetadata::default(),
    )?;
    Ok(recorder.snapshot())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerminalConversationTraceProjection {
    pub trace: ConversationTurnTrace,
    pub model_context_items: Vec<ConversationModelContextItem>,
}

pub fn cancelled_conversation_trace_from_checkpoint(
    checkpoint: &AgentRunCheckpoint,
    conversation_id: &str,
    assistant_message_id: &str,
    call: &AgentToolCall,
    result: &AgentToolResult,
    model_observation: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_checkpoint(
        checkpoint,
        conversation_id,
        assistant_message_id,
        call,
        result,
        model_observation,
        ConversationTurnTraceTerminalStatus::Cancelled,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn terminal_conversation_trace_from_checkpoint(
    checkpoint: &AgentRunCheckpoint,
    conversation_id: &str,
    assistant_message_id: &str,
    call: &AgentToolCall,
    result: &AgentToolResult,
    model_observation: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
) -> Result<TerminalConversationTraceProjection, String> {
    if !terminal_status.is_terminal() {
        return Err("terminal conversation projection requires a terminal status".to_string());
    }
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    record_model_tool_exchange_with_projection(
        &mut recorder,
        call,
        result,
        None,
        Some(model_observation),
        ConversationHistoryArchiveTraceMetadata::default(),
    )?;
    let model_context_items = recorder.snapshot().model_context_items;
    let trace = recorder.finish(
        &checkpoint.run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        terminal_error,
    );
    trace.validate_complete_model_context(&model_context_items)?;
    Ok(TerminalConversationTraceProjection {
        trace,
        model_context_items,
    })
}

/// Builds the Host-side approval snapshot with a model observation already finalized by the
/// request's central Tool-result budget gate.
///
/// The Archive pointer and model text enter the append-only snapshot together, preventing a
/// restart from observing a bounded result without its exact recovery route.
pub fn conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
    checkpoint: &AgentRunCheckpoint,
    call: &AgentToolCall,
    result: &AgentToolResult,
    assistant_message_id: Option<&str>,
    model_observation: &str,
    archive: ConversationHistoryArchiveTraceMetadata,
) -> Result<ConversationTraceSnapshot, String> {
    let mut recorder = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let result_sequence = recorder.require_recorded_tool_call(call)?;
    let history_ref = assistant_message_id.map(|assistant_message_id| {
        crate::ContextHistoryRef::trace_item(
            assistant_message_id,
            result_sequence.saturating_add(1),
        )
    });
    record_model_tool_exchange_with_projection(
        &mut recorder,
        call,
        result,
        history_ref.as_ref(),
        Some(model_observation),
        archive,
    )?;
    Ok(recorder.snapshot())
}

fn record_model_tool_exchange_with_projection(
    recorder: &mut ConversationTraceRecorder,
    call: &AgentToolCall,
    result: &AgentToolResult,
    history_ref: Option<&crate::ContextHistoryRef>,
    model_observation: Option<&str>,
    archive: ConversationHistoryArchiveTraceMetadata,
) -> Result<(), String> {
    let durable_result = canonical_tool_result_for_context(result);
    let llm_result = crate::tools::model_projection_for_persisted_continuation(result);
    let model_observation = model_observation
        .map(ToString::to_string)
        .unwrap_or_else(|| render_tool_observation_with_history_ref(&llm_result, history_ref));
    recorder.require_recorded_tool_call(call)?;
    if let Some(sequence) = recorder.record_tool_result_with_archive(call, &durable_result, archive)
    {
        recorder.record_model_message(
            sequence,
            0,
            &LlmMessage::tool_result(call.id.clone(), model_observation, !llm_result.ok),
        )?;
    }
    Ok(())
}

pub fn cancelled_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_snapshot_with_terminal_details(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        reason,
        None,
        None,
    )
}

/// Closes a cancelled snapshot while retaining a distinct terminal failure diagnostic.
///
/// Most user-requested cancellations should use [`cancelled_conversation_trace_from_snapshot`],
/// which deliberately leaves `terminal_error` empty. This explicit variant is reserved for
/// abnormal cancellation paths such as a forced shutdown timeout. `unresolved_tool_error`
/// describes only a ToolCall that had no verifiable result; `terminal_error` describes the Turn.
pub fn cancelled_conversation_trace_from_snapshot_with_terminal_error(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    unresolved_tool_error: &str,
    terminal_error: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_snapshot_with_terminal_details(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        unresolved_tool_error,
        Some(terminal_error),
        None,
    )
}

pub fn terminal_conversation_trace_from_snapshot(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
) -> Result<TerminalConversationTraceProjection, String> {
    terminal_conversation_trace_from_snapshot_with_terminal_details(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        reason,
        Some(reason),
        None,
    )
}

/// Atomically closes the one unresolved ToolCall in a durable snapshot with an authoritative,
/// already-sanitized terminal ToolResult. Sensitive built-in MCP startup reconciliation uses this
/// instead of a generic runtime error so the original call_id receives exactly one typed result
/// while preserving the committed prefix byte-for-byte.
pub fn terminal_conversation_trace_from_snapshot_with_tool_result(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
    result: &AgentToolResult,
) -> Result<TerminalConversationTraceProjection, String> {
    if !terminal_status.is_terminal() {
        return Err("terminal conversation projection requires a terminal status".to_string());
    }
    let mut recorder = ConversationTraceRecorder::from_durable_snapshot(snapshot);
    let call = recorder.unresolved_tool_call().ok_or_else(|| {
        "terminal ToolResult projection requires one unresolved ToolCall".to_string()
    })?;
    if result.call_id != call.id || result.tool != call.tool {
        return Err(
            "terminal ToolResult identity does not match the unresolved ToolCall".to_string(),
        );
    }
    record_model_tool_exchange_with_projection(
        &mut recorder,
        &call,
        result,
        None,
        None,
        ConversationHistoryArchiveTraceMetadata::default(),
    )?;
    let model_context_items = recorder.snapshot().model_context_items;
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        Some(reason),
    );
    trace.validate_complete_model_context(&model_context_items)?;
    Ok(TerminalConversationTraceProjection {
        trace,
        model_context_items,
    })
}

pub(crate) fn terminal_conversation_trace_from_snapshot_with_tool_approval(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    reason: &str,
    terminal_tool_approval_status: Option<AgentApprovalStatus>,
) -> Result<TerminalConversationTraceProjection, String> {
    let terminal_error =
        (terminal_status != ConversationTurnTraceTerminalStatus::Cancelled).then_some(reason);
    terminal_conversation_trace_from_snapshot_with_terminal_details(
        snapshot,
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        reason,
        terminal_error,
        terminal_tool_approval_status,
    )
}

#[allow(clippy::too_many_arguments)]
fn terminal_conversation_trace_from_snapshot_with_terminal_details(
    snapshot: ConversationTraceSnapshot,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    unresolved_tool_error: &str,
    terminal_error: Option<&str>,
    terminal_tool_approval_status: Option<AgentApprovalStatus>,
) -> Result<TerminalConversationTraceProjection, String> {
    if !terminal_status.is_terminal() {
        return Err("terminal conversation projection requires a terminal status".to_string());
    }
    let mut recorder = ConversationTraceRecorder::from_durable_snapshot(snapshot);
    if let Some(mut call) = recorder.unresolved_tool_call() {
        if let Some(approval_status) = terminal_tool_approval_status {
            call.approval_status = approval_status;
        }
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({
                "resultAvailable": false,
                "status": terminal_status.as_str(),
                "terminalStatus": terminal_status,
            })),
            error: Some(unresolved_tool_error.to_string()),
        };
        record_model_tool_exchange_with_projection(
            &mut recorder,
            &call,
            &result,
            None,
            Some(
                if terminal_status == ConversationTurnTraceTerminalStatus::Cancelled {
                    "The Tool Call was cancelled before a verifiable result was available."
                } else {
                    "The Tool Call ended without a verifiable result."
                },
            ),
            ConversationHistoryArchiveTraceMetadata::default(),
        )?;
    }
    let model_context_items = recorder.snapshot().model_context_items;
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        terminal_status,
        terminal_error,
    );
    trace.validate_complete_model_context(&model_context_items)?;
    Ok(TerminalConversationTraceProjection {
        trace,
        model_context_items,
    })
}

pub fn failed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    error: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Failed,
        Some(error),
    )
}

pub fn completed_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Completed,
        None,
    )
}

pub fn cancelled_conversation_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
) -> ConversationTurnTrace {
    terminal_trace_without_items(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Cancelled,
        None,
    )
}

fn terminal_trace_without_items(
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
) -> ConversationTurnTrace {
    let (terminal_error, truncated) = terminal_error
        .map(project_terminal_error)
        .map(|(value, truncated)| (Some(value), truncated))
        .unwrap_or((None, false));
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status,
        terminal_error,
        truncated,
        items: Vec::new(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct ConversationTraceRecorder {
    items: Vec<ConversationTurnTraceItem>,
    model_context_items: Vec<ConversationModelContextItem>,
    next_sequence: u64,
    truncated: bool,
    items_are_durable: bool,
}
