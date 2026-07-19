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

pub(super) fn conversation_trace_from_input_checkpoint(
    input: &AgentChatInput,
) -> ConversationTraceRecorder {
    let Some(checkpoint) = input.resume_checkpoint.as_ref() else {
        return ConversationTraceRecorder::default();
    };
    let mut recorder = ConversationTraceRecorder::from_checkpoint(
        checkpoint.conversation_trace_items.clone(),
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    if let Some(continuation) = input.tool_continuation.as_ref() {
        recorder.record_tool_call(&continuation.call);
        recorder.record_tool_result(&continuation.call, &continuation.result);
    }
    recorder
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
