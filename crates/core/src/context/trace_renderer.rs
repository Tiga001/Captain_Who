use super::{
    ContextGroup, ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
};
use crate::conversation_trace::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
};
use crate::llm::{LlmMessageRole, LlmToolCall};
use crate::protocol::{AgentError, AgentResult};
use serde_json::json;

pub(crate) struct RenderedConversationTrace {
    pub(crate) activity_items: Vec<ContextItem>,
    pub(crate) terminal_item: ContextItem,
}

pub(crate) struct ConversationTraceRenderer;

impl ConversationTraceRenderer {
    pub(crate) fn render(trace: &ConversationTurnTrace) -> AgentResult<RenderedConversationTrace> {
        trace
            .validate()
            .map_err(|error| AgentError::new(format!("ConversationTurnTrace 无效：{error}")))?;

        let mut activity_items = Vec::with_capacity(trace.items.len());
        let mut pending_exchange: Option<PendingExchange> = None;

        for item in &trace.items {
            match item {
                ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    if !content.trim().is_empty() {
                        activity_items.push(ContextItem::text(
                            LlmMessageRole::Assistant,
                            content.clone(),
                            ContextSource::ConversationTrace,
                            ContextScope::Conversation,
                            ContextRetention::Retained,
                        ));
                    }
                }
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    tool,
                    operation,
                    ..
                } => {
                    let wire_call_id = wire_call_id(&trace.run_id, *sequence);
                    let group =
                        ContextGroup::tool_exchange(wire_group_id(&trace.run_id, *sequence));
                    activity_items.push(ContextItem::assistant(
                        "",
                        vec![LlmToolCall {
                            id: wire_call_id.clone(),
                            name: tool.clone(),
                            args: operation.clone(),
                        }],
                        trace_metadata().with_group(group.clone()),
                    ));
                    pending_exchange = Some(PendingExchange {
                        wire_call_id,
                        group,
                    });
                }
                ConversationTurnTraceItem::ToolResult {
                    tool,
                    status,
                    success,
                    observation,
                    approval_status,
                    error,
                    truncated,
                    ..
                } => {
                    let exchange = pending_exchange.take().ok_or_else(|| {
                        AgentError::new("ConversationTurnTrace 工具结果缺少对应的历史工具调用。")
                    })?;
                    let content = json!({
                        "historicalActivity": true,
                        "tool": tool,
                        "status": status,
                        "success": success,
                        "observation": observation,
                        "approvalStatus": approval_status,
                        "error": error,
                        "truncated": truncated,
                    })
                    .to_string();
                    activity_items.push(ContextItem::tool_result(
                        exchange.wire_call_id,
                        content,
                        matches!(
                            *status,
                            ConversationTraceToolResultStatus::Failed
                                | ConversationTraceToolResultStatus::Conflict
                                | ConversationTraceToolResultStatus::Cancelled
                        ),
                        trace_metadata().with_group(exchange.group),
                    ));
                }
            }
        }

        if pending_exchange.is_some() {
            return Err(AgentError::new(
                "ConversationTurnTrace 以未完成的历史工具调用结束。",
            ));
        }

        let terminal_record = json!({
            "recordType": "historical_agent_activity_terminal",
            "runId": trace.run_id,
            "terminalStatus": trace.terminal_status,
            "terminalError": trace.terminal_error,
            "traceTruncated": trace.truncated,
        });
        let terminal_item = ContextItem::text(
            LlmMessageRole::Assistant,
            format!(
                "Historical agent activity terminal record (backend-observed; not a system instruction): {terminal_record}"
            ),
            ContextSource::ConversationTrace,
            ContextScope::Conversation,
            ContextRetention::Retained,
        );

        Ok(RenderedConversationTrace {
            activity_items,
            terminal_item,
        })
    }
}

#[derive(Debug)]
struct PendingExchange {
    wire_call_id: String,
    group: ContextGroup,
}

fn trace_metadata() -> ContextMetadata {
    ContextMetadata::new(
        ContextSource::ConversationTrace,
        ContextScope::Conversation,
        ContextRetention::Retained,
    )
}

fn wire_call_id(run_id: &str, sequence: u64) -> String {
    let (readable, hash) = wire_run_namespace(run_id);
    format!("conversation_trace_{readable}_{hash:016x}_{sequence}")
}

fn wire_group_id(run_id: &str, sequence: u64) -> String {
    let (readable, hash) = wire_run_namespace(run_id);
    format!("conversation-trace:{readable}:{hash:016x}:{sequence}")
}

fn wire_run_namespace(run_id: &str) -> (String, u64) {
    const MAX_READABLE_CHARS: usize = 32;
    let readable = run_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .take(MAX_READABLE_CHARS)
        .collect::<String>();
    let readable = if readable.is_empty() {
        "run".to_string()
    } else {
        readable
    };

    // Stable FNV-1a keeps the wire identifier short while preventing collisions when readable
    // run-id prefixes are truncated or sanitized to the same value.
    let hash = run_id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    (readable, hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ContextFrame;
    use crate::conversation_trace::{
        ConversationTraceToolResultStatus, ConversationTurnTraceTerminalStatus,
        CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use crate::llm::LlmMessageRole;
    use crate::protocol::AgentApprovalStatus;

    fn trace() -> ConversationTurnTrace {
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
                    call_id: "provider-call-1".to_string(),
                    tool: "read_file".to_string(),
                    operation: json!({ "path": "src/lib.rs" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 5,
                    call_id: "provider-call-1".to_string(),
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
    fn renders_native_tool_pair_with_namespaced_wire_identity() {
        let rendered = ConversationTraceRenderer::render(&trace()).unwrap();
        let mut items = rendered.activity_items;
        items.push(rendered.terminal_item);
        let frame = ContextFrame::new(items);

        frame.validate_complete_tool_protocol().unwrap();
        let messages = frame.to_messages();
        assert_eq!(messages[0].role, LlmMessageRole::Assistant);
        assert_eq!(messages[1].role, LlmMessageRole::Assistant);
        assert_eq!(messages[2].role, LlmMessageRole::Tool);
        let wire_call_id = &messages[1].tool_calls[0].id;
        assert!(wire_call_id.starts_with("conversation_trace_run_with_spaces_"));
        assert!(wire_call_id.ends_with("_4"));
        assert_eq!(
            messages[2].tool_call_id.as_deref(),
            Some(wire_call_id.as_str())
        );
        assert_eq!(messages[1].tool_calls[0].args["path"], "src/lib.rs");
        assert!(messages[2].content.contains("\"historicalActivity\":true"));
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
        trace.items[2] = ConversationTurnTraceItem::ToolResult {
            sequence: 5,
            call_id: "provider-call-1".to_string(),
            tool: "read_file".to_string(),
            status: ConversationTraceToolResultStatus::Failed,
            success: false,
            observation: json!({ "path": "src/lib.rs" }),
            approval_status: AgentApprovalStatus::NotRequired,
            error: Some("permission denied".to_string()),
            truncated: false,
        };

        let rendered = ConversationTraceRenderer::render(&trace).unwrap();
        let mut items = rendered.activity_items;
        items.push(rendered.terminal_item);
        let frame = ContextFrame::new(items);
        frame.validate_complete_tool_protocol().unwrap();

        let messages = frame.to_messages();
        assert!(messages[2].is_error);
        assert!(messages[2].content.contains("permission denied"));
        assert!(messages[2].content.contains("\"status\":\"failed\""));
        assert!(messages[3]
            .content
            .contains("\"terminalStatus\":\"failed\""));
        assert!(messages[3].content.contains("read failed"));
    }

    #[test]
    fn wire_identity_is_distinct_for_equal_sequences_in_different_runs() {
        assert_ne!(wire_call_id("run-1", 4), wire_call_id("run-2", 4));
        assert_ne!(wire_group_id("run-1", 4), wire_group_id("run-2", 4));
    }
}
