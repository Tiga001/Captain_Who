use super::*;

pub(super) fn publish_trace_recorder_snapshot(
    recorder: &ConversationTraceRecorder,
    observer: Option<&AgentConversationTraceObserver>,
) -> AgentResult<Option<AgentContextBaseline>> {
    if let Some(observer) = observer {
        return observer(recorder.snapshot());
    }
    Ok(None)
}

pub(super) fn publish_trace_snapshot(
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    observer: Option<&AgentConversationTraceObserver>,
) -> AgentResult<Option<AgentContextBaseline>> {
    if observer.is_none() {
        return Ok(None);
    }
    let recorder = recorder.lock().unwrap_or_else(|error| error.into_inner());
    publish_trace_recorder_snapshot(&recorder, observer)
}

pub(super) fn finalize_runtime_trace(
    result: AgentResult<AgentChatOutput>,
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
) -> AgentResult<AgentChatOutput> {
    let Some(conversation_id) = conversation_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return result;
    };
    let Some(assistant_message_id) = assistant_message_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return result;
    };

    match result {
        Ok(mut output) => {
            let terminal_status = match output.status {
                AgentRunStatus::Completed => Some(ConversationTurnTraceTerminalStatus::Completed),
                AgentRunStatus::Failed => Some(ConversationTurnTraceTerminalStatus::Failed),
                AgentRunStatus::Cancelled => Some(ConversationTurnTraceTerminalStatus::Cancelled),
                AgentRunStatus::Idle
                | AgentRunStatus::Running
                | AgentRunStatus::WaitingForApproval => None,
            };
            if let Some(terminal_status) = terminal_status {
                output.conversation_turn_trace = Some(finish_trace(
                    recorder,
                    run_id,
                    conversation_id,
                    assistant_message_id,
                    terminal_status,
                    None,
                ));
            }
            Ok(output)
        }
        Err(error) => {
            let message = error.to_string();
            let trace = finish_trace(
                recorder,
                run_id,
                conversation_id,
                assistant_message_id,
                ConversationTurnTraceTerminalStatus::Failed,
                Some(&message),
            );
            Err(error.with_conversation_turn_trace(trace))
        }
    }
}

pub(super) fn conversation_trace_checkpoint_prefix_from_input(
    input: &AgentChatInput,
) -> ConversationTraceRecorder {
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return ConversationTraceRecorder::default();
    };
    ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.conversation_model_context_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    )
}

pub(super) fn conversation_trace_from_input_checkpoint(
    input: &AgentChatInput,
    model_tool_result_gate: &ModelToolResultGate,
    archive_metadata: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<ConversationTraceRecorder> {
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return Ok(ConversationTraceRecorder::default());
    };
    let snapshot = match input.tool_continuation.as_ref() {
        Some(continuation) => {
            // `pending_action_id` is the typed checkpoint marker for an MCP approval. The live
            // continuation still carries the bounded authoritative result for the next model
            // request, but the trace observer must see the exact same safe projection that was
            // atomically committed with the pending-action settlement. Otherwise the first
            // resumed trace publication attempts to rewrite the already committed ToolResult
            // and correctly fails the append-only prefix check.
            //
            // Deliberately do not infer MCP identity from the provider-visible tool name.
            let is_mcp = checkpoint.pending_action_id.is_some();
            let durable_result = if is_mcp {
                crate::tools::mcp_tool_result_persistence_projection(&continuation.result)
            } else {
                continuation.result.clone()
            };
            let projected =
                crate::tools::model_projection_for_persisted_continuation(&durable_result);
            // The Host archive contains this same safe projection, not the raw MCP result. Keep
            // its non-secret trace identity so resumed publications remain an exact prefix.
            let durable_archive = archive_metadata.clone();
            let model_observation = finalize_model_tool_observation(
                model_tool_result_gate,
                &continuation.call.id,
                !continuation.result.ok,
                &projected,
                &durable_archive,
            )?;
            conversation_trace_snapshot_from_checkpoint_and_continuation_with_projection(
                checkpoint,
                &continuation.call,
                &durable_result,
                input.assistant_message_id.as_deref(),
                &model_observation,
                durable_archive,
            )
            .map_err(AgentError::new)?
        }
        None => ConversationTraceSnapshot {
            items: checkpoint.conversation_trace_items.clone(),
            model_context_items: checkpoint.conversation_model_context_items.clone(),
            next_sequence: checkpoint.next_conversation_trace_sequence,
            truncated: checkpoint.conversation_trace_truncated,
        },
    };
    Ok(
        ConversationTraceRecorder::from_checkpoint_with_model_context(
            snapshot.items,
            snapshot.model_context_items,
            snapshot.next_sequence,
            snapshot.truncated,
        ),
    )
}

pub(super) fn attach_failed_runtime_trace(
    error: AgentError,
    recorder: &ConversationTraceRecorder,
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
) -> AgentError {
    let (Some(conversation_id), Some(assistant_message_id)) = (
        conversation_id
            .map(str::trim)
            .filter(|value| !value.is_empty()),
        assistant_message_id
            .map(str::trim)
            .filter(|value| !value.is_empty()),
    ) else {
        return error;
    };
    let message = error.to_string();
    let trace = recorder.finish(
        run_id,
        conversation_id,
        assistant_message_id,
        ConversationTurnTraceTerminalStatus::Failed,
        Some(&message),
    );
    debug_assert!(trace.validate().is_ok());
    error.with_conversation_turn_trace(trace)
}

pub(super) fn finish_trace(
    recorder: &Arc<Mutex<ConversationTraceRecorder>>,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    terminal_status: ConversationTurnTraceTerminalStatus,
    terminal_error: Option<&str>,
) -> ConversationTurnTrace {
    let trace = recorder
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .finish(
            run_id,
            conversation_id,
            assistant_message_id,
            terminal_status,
            terminal_error,
        );
    debug_assert!(trace.validate().is_ok());
    trace
}
