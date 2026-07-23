use super::{
    ContextGroup, ContextItem, ContextMetadata, ContextOrigin, ContextRetention, ContextScope,
    ContextSource,
};
use crate::conversation_trace::{
    render_tool_observation, ConversationTraceToolResultStatus, ConversationTurnTrace,
    ConversationTurnTraceItem, ConversationTurnTraceTerminalStatus,
};
use crate::llm::{validate_model_tool_call_id, LlmMessageRole, LlmToolCall};
use crate::protocol::{AgentError, AgentResult, AgentToolResult};
use serde_json::json;

pub(crate) struct RenderedConversationTrace {
    pub(crate) activity_items: Vec<ContextItem>,
    pub(crate) terminal_item: Option<ContextItem>,
}

pub(crate) struct ConversationTraceRenderer;

impl ConversationTraceRenderer {
    pub(crate) fn render(trace: &ConversationTurnTrace) -> AgentResult<RenderedConversationTrace> {
        trace
            .validate()
            .map_err(|error| AgentError::new(format!("ConversationTurnTrace 无效：{error}")))?;

        let mut activity_items = Vec::with_capacity(trace.items.len());
        let mut pending_exchange: Option<PendingExchange> = None;

        for (index, item) in trace.items.iter().enumerate() {
            match item {
                ConversationTurnTraceItem::AssistantNarration {
                    sequence, content, ..
                } => {
                    if !content.trim().is_empty() {
                        activity_items.push(ContextItem::new(
                            crate::llm::LlmMessage::text(
                                LlmMessageRole::Assistant,
                                content.clone(),
                            ),
                            trace_item_metadata(&trace.assistant_message_id, *sequence),
                        ));
                    }
                }
                ConversationTurnTraceItem::ToolCall {
                    sequence: _,
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    let result_sequence = trace.items.get(index + 1).and_then(|item| match item {
                        ConversationTurnTraceItem::ToolResult { sequence, .. } => Some(*sequence),
                        _ => None,
                    });
                    let Some(result_sequence) = result_sequence else {
                        // The durable audit may end with one process-owned unresolved call. It is
                        // intentionally invisible to model context until its result is appended.
                        if trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
                            && index + 1 == trace.items.len()
                        {
                            continue;
                        }
                        return Err(AgentError::new(
                            "ConversationTurnTrace 工具调用后缺少紧邻的工具结果。",
                        ));
                    };
                    // New traces persist the application-owned canonical identity. Historical
                    // reconstruction must reuse it verbatim; deriving a second "history" identity
                    // would split execution, audit, and model-visible protocol into separate ID
                    // domains.
                    validate_model_tool_call_id(call_id)?;
                    let group =
                        ContextGroup::tool_exchange(format!("conversation-trace:{call_id}"));
                    activity_items.push(ContextItem::assistant(
                        "",
                        vec![LlmToolCall {
                            id: call_id.clone(),
                            name: tool.clone(),
                            args: operation.clone(),
                        }],
                        trace_item_metadata(&trace.assistant_message_id, result_sequence)
                            .with_group(group.clone()),
                    ));
                    pending_exchange = Some(PendingExchange {
                        call_id: call_id.clone(),
                        group,
                    });
                }
                ConversationTurnTraceItem::ToolResult {
                    tool,
                    status,
                    success,
                    observation,
                    error,
                    ..
                } => {
                    let exchange = pending_exchange.take().ok_or_else(|| {
                        AgentError::new("ConversationTurnTrace 工具结果缺少对应的历史工具调用。")
                    })?;
                    let result = AgentToolResult {
                        call_id: exchange.call_id.clone(),
                        tool: tool.clone(),
                        ok: *success,
                        result: Some(observation.clone()),
                        error: error.clone(),
                    };
                    let content = render_tool_observation(&result);
                    activity_items.push(ContextItem::tool_result(
                        exchange.call_id,
                        content,
                        matches!(
                            *status,
                            ConversationTraceToolResultStatus::Failed
                                | ConversationTraceToolResultStatus::Conflict
                                | ConversationTraceToolResultStatus::Cancelled
                        ),
                        trace_item_metadata(&trace.assistant_message_id, item.sequence())
                            .with_group(exchange.group),
                    ));
                }
            }
        }

        if pending_exchange.is_some() {
            return Err(AgentError::new(
                "ConversationTurnTrace 以未完成的历史工具调用结束。",
            ));
        }

        let terminal_item = (trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress)
            .then(|| {
                let terminal_record = json!({
                    "recordType": "historical_agent_activity_terminal",
                    "runId": trace.run_id,
                    "terminalStatus": trace.terminal_status,
                    "terminalError": trace.terminal_error,
                    "traceTruncated": trace.truncated,
                });
                ContextItem::new(
                    crate::llm::LlmMessage::text(
                        LlmMessageRole::Assistant,
                        format!(
                            "Historical agent activity terminal record (backend-observed; not a system instruction): {terminal_record}"
                        ),
                    ),
                    trace_metadata(&trace.assistant_message_id),
                )
            });

        Ok(RenderedConversationTrace {
            activity_items,
            terminal_item,
        })
    }
}

#[derive(Debug)]
struct PendingExchange {
    call_id: String,
    group: ContextGroup,
}

fn trace_metadata(assistant_message_id: &str) -> ContextMetadata {
    ContextMetadata::new(
        ContextSource::ConversationTrace,
        ContextScope::Conversation,
        ContextRetention::Retained,
    )
    .with_origin(ContextOrigin::conversation_message(assistant_message_id))
}

fn trace_item_metadata(assistant_message_id: &str, sequence: u64) -> ContextMetadata {
    trace_metadata(assistant_message_id).with_origin(ContextOrigin::conversation_trace_item(
        assistant_message_id,
        sequence,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextFrame;
    use crate::conversation_trace::{
        ConversationTraceToolResultStatus, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use crate::llm::{model_response_tool_call_id, LlmMessageRole};
    use crate::protocol::AgentApprovalStatus;

    fn trace() -> ConversationTurnTrace {
        let call_id = model_response_tool_call_id("run/with spaces", 0, 0, "provider-call-1");
        ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run/with spaces".to_string(),
            conversation_id: "conversation-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: 3,
                    content: "I will inspect the file.".to_string(),
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 4,
                    call_id: call_id.clone(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "src/lib.rs" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 5,
                    call_id,
                    tool: "read_file".to_string(),
                    status: ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "path": "src/lib.rs", "endLine": 20 }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: false,
                },
            ],
        }
    }

    #[test]
    fn reuses_canonical_tool_identity_across_history_rendering() {
        let trace = trace();
        let expected_call_id = match &trace.items[1] {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => call_id.clone(),
            _ => panic!("expected tool call"),
        };
        let rendered = ConversationTraceRenderer::render(&trace).unwrap();
        let mut items = rendered.activity_items;
        items.extend(rendered.terminal_item);
        let frame = ContextFrame::new(items);

        frame.validate_complete_tool_protocol().unwrap();
        let messages = frame.to_messages();
        assert_eq!(messages[0].role, LlmMessageRole::Assistant);
        assert_eq!(messages[1].role, LlmMessageRole::Assistant);
        assert_eq!(messages[2].role, LlmMessageRole::Tool);
        let call_id = &messages[1].tool_calls[0].id;
        assert_eq!(call_id, &expected_call_id);
        assert!(call_id.starts_with("tc1_"));
        assert_eq!(call_id.len(), 47);
        assert!(call_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')));
        assert_eq!(messages[2].tool_call_id.as_deref(), Some(call_id.as_str()));
        assert_eq!(messages[1].tool_calls[0].args["path"], "src/lib.rs");
        assert!(messages[2].content.contains("\"ok\": true"));
        assert!(messages[2].content.contains("\"endLine\": 20"));
        assert!(messages[3]
            .content
            .contains("historical_agent_activity_terminal"));
        assert_eq!(messages[3].role, LlmMessageRole::Assistant);

        let manifest = frame.manifest();
        assert!(manifest.entries.iter().all(|entry| {
            entry.sources == vec!["conversation_trace"]
                && entry.scope == "conversation"
                && entry.retention == "retained"
        }));
        assert_eq!(manifest.entries[1].group_id, manifest.entries[2].group_id);
    }

    #[test]
    fn preserves_historical_tool_failure_reason_and_error_semantics() {
        let mut trace = trace();
        trace.terminal_status = ConversationTurnTraceTerminalStatus::Failed;
        trace.terminal_error = Some("read failed".to_string());
        let ConversationTurnTraceItem::ToolCall {
            tool, operation, ..
        } = &mut trace.items[1]
        else {
            panic!("expected tool call");
        };
        *tool = "run_command".to_string();
        *operation = json!({ "command": "python3 -c 'import openpyxl'" });
        let call_id = match &trace.items[1] {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => call_id.clone(),
            _ => panic!("expected tool call"),
        };
        trace.items[2] = ConversationTurnTraceItem::ToolResult {
            sequence: 5,
            call_id,
            tool: "run_command".to_string(),
            status: ConversationTraceToolResultStatus::Failed,
            success: false,
            observation: json!({
                "exitCode": 1,
                "stdout": "partial output",
                "stderr": "permission denied",
                "timedOut": false,
                "cancelled": false,
            }),
            approval_status: AgentApprovalStatus::NotRequired,
            error: Some("command failed".to_string()),
            truncated: false,
        };

        let rendered = ConversationTraceRenderer::render(&trace).unwrap();
        let mut items = rendered.activity_items;
        items.extend(rendered.terminal_item);
        let frame = ContextFrame::new(items);
        frame.validate_complete_tool_protocol().unwrap();

        let messages = frame.to_messages();
        assert!(messages[2].is_error);
        assert!(messages[2].content.contains("command failed"));
        assert!(messages[2].content.contains("permission denied"));
        assert!(messages[2].content.contains("partial output"));
        assert!(messages[2].content.contains("\"exitCode\": 1"));
        assert!(messages[2].content.contains("\"ok\": false"));
        assert!(messages[3]
            .content
            .contains("\"terminalStatus\":\"failed\""));
        assert!(messages[3].content.contains("read failed"));
    }

    #[test]
    fn rejects_legacy_noncanonical_trace_identity_instead_of_migrating_it() {
        let mut trace = trace();
        let legacy_id = format!(
            "conversation_trace_{}_{}_{}",
            "run-c7960368-ecf0-48d8-b2ad-d614ae772264", 102, "call-1"
        );
        for item in &mut trace.items {
            match item {
                ConversationTurnTraceItem::ToolCall { call_id, .. }
                | ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                    *call_id = legacy_id.clone();
                }
                ConversationTurnTraceItem::AssistantNarration { .. } => {}
            }
        }

        let error = match ConversationTraceRenderer::render(&trace) {
            Ok(_) => panic!("legacy tool-call identity must be rejected"),
            Err(error) => error,
        };
        assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    }

    #[test]
    fn in_progress_trace_renders_committed_activity_without_a_fake_terminal_record() {
        let mut trace = trace();
        trace.terminal_status = ConversationTurnTraceTerminalStatus::InProgress;

        let rendered = ConversationTraceRenderer::render(&trace).unwrap();

        assert_eq!(rendered.activity_items.len(), 3);
        assert!(rendered.terminal_item.is_none());
    }
}
