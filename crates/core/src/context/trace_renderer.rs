use super::{
    ContextGroup, ContextItem, ContextMetadata, ContextOrigin, ContextRetention, ContextScope,
    ContextSource,
};
use crate::conversation_trace::{
    render_tool_observation, render_user_guidance_content, validate_model_context_prefix,
    ConversationModelContextItem, ConversationTraceToolResultStatus, ConversationTurnTrace,
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
    pub(crate) fn render_with_model_context(
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
    ) -> AgentResult<RenderedConversationTrace> {
        if model_context_items.is_empty() {
            return Self::render(trace);
        }
        trace
            .validate()
            .map_err(|error| AgentError::new(format!("ConversationTurnTrace 无效：{error}")))?;
        validate_model_context_prefix(trace, model_context_items)
            .map_err(|error| AgentError::new(format!("模型上下文日志无效：{error}")))?;

        let covered_sequence = model_context_items
            .last()
            .map(|item| item.sequence)
            .expect("non-empty model context prefix");
        let mut activity_items = model_context_items
            .iter()
            .map(|item| model_context_item(item, &trace.assistant_message_id))
            .collect::<AgentResult<Vec<_>>>()?;
        let suffix = ConversationTurnTrace {
            schema_version: trace.schema_version,
            run_id: trace.run_id.clone(),
            conversation_id: trace.conversation_id.clone(),
            assistant_message_id: trace.assistant_message_id.clone(),
            terminal_status: trace.terminal_status,
            terminal_error: trace.terminal_error.clone(),
            truncated: trace.truncated,
            items: trace
                .items
                .iter()
                .filter(|item| item.sequence() > covered_sequence && item.is_model_visible())
                .cloned()
                .collect(),
        };
        let rendered_suffix = Self::render(&suffix)?;
        activity_items.extend(rendered_suffix.activity_items);
        Ok(RenderedConversationTrace {
            activity_items,
            terminal_item: rendered_suffix.terminal_item,
        })
    }

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
                ConversationTurnTraceItem::UserGuidance {
                    sequence,
                    content,
                    attachments,
                    ..
                } => {
                    if pending_exchange.is_some() {
                        return Err(AgentError::new(
                            "ConversationTurnTrace 用户引导不能拆分工具调用与结果。",
                        ));
                    }
                    activity_items.push(ContextItem::new(
                        crate::llm::LlmMessage::text(
                            LlmMessageRole::User,
                            render_user_guidance_content(content, attachments),
                        ),
                        trace_item_metadata(&trace.assistant_message_id, *sequence),
                    ));
                }
                ConversationTurnTraceItem::ToolCall {
                    sequence: _,
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    let result_sequence = trace.items[index + 1..]
                        .iter()
                        .find(|item| item.is_model_visible())
                        .and_then(|item| match item {
                            ConversationTurnTraceItem::ToolResult { sequence, .. } => {
                                Some(*sequence)
                            }
                            _ => None,
                        });
                    let Some(result_sequence) = result_sequence else {
                        // The durable audit may end with one process-owned unresolved call. It is
                        // intentionally invisible to model context until its result is appended.
                        if trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
                            && !trace.items[index + 1..]
                                .iter()
                                .any(ConversationTurnTraceItem::is_model_visible)
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
                    if tool == "todo_update" {
                        pending_exchange = Some(PendingExchange {
                            call_id: call_id.clone(),
                            group: None,
                        });
                        continue;
                    }
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
                        group: Some(group),
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
                    let Some(group) = exchange.group else {
                        continue;
                    };
                    let result = AgentToolResult {
                        exact_archive_file: None,
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
                            .with_group(group),
                    ));
                }
                ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {
                    // Host lifecycle audit is intentionally invisible to model context. Only an
                    // explicit command_session Tool result exposes later output to the model.
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

fn model_context_item(
    item: &ConversationModelContextItem,
    assistant_message_id: &str,
) -> AgentResult<ContextItem> {
    let metadata = trace_item_metadata(assistant_message_id, item.sequence);
    match item.role.as_str() {
        "user" => Ok(ContextItem::new(
            crate::llm::LlmMessage::text(LlmMessageRole::User, item.content.clone()),
            metadata,
        )),
        "assistant" if item.tool_calls.is_empty() => Ok(ContextItem::new(
            crate::llm::LlmMessage::text(LlmMessageRole::Assistant, item.content.clone()),
            metadata,
        )),
        "assistant" => {
            let call = &item.tool_calls[0];
            let group = ContextGroup::tool_exchange(format!("conversation-trace:{}", call.id));
            Ok(ContextItem::assistant(
                item.content.clone(),
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.name.clone(),
                    args: call.args.clone(),
                }],
                metadata.with_group(group),
            ))
        }
        "tool" => {
            let call_id = item
                .tool_call_id
                .as_deref()
                .ok_or_else(|| AgentError::new("模型上下文 tool 消息缺少 call id。"))?;
            let group = ContextGroup::tool_exchange(format!("conversation-trace:{call_id}"));
            Ok(ContextItem::tool_result(
                call_id,
                item.content.clone(),
                item.is_error,
                metadata.with_group(group),
            ))
        }
        _ => Err(AgentError::new("模型上下文日志包含未知消息角色。")),
    }
}

#[derive(Debug)]
struct PendingExchange {
    call_id: String,
    group: Option<ContextGroup>,
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
        ConversationCommandSessionLifecyclePhase, ConversationTraceToolResultStatus,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use crate::llm::{model_response_tool_call_id, LlmMessageRole};
    use crate::protocol::{AgentApprovalStatus, AgentContextCheckpointToolCall};
    use crate::AgentCommandSessionStatus;

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
                    provenance: None,
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
                    archive: Default::default(),
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
        assert!(!messages[2].content.contains("\"ok\""));
        assert!(messages[2].content.contains("\"endLine\":20"));
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
    fn run_scoped_todo_exchange_is_not_replayed_into_later_model_context() {
        let mut trace = trace();
        let ConversationTurnTraceItem::ToolCall {
            tool, operation, ..
        } = &mut trace.items[1]
        else {
            panic!("expected tool call");
        };
        *tool = "todo_update".to_string();
        *operation = json!({
            "items": [{ "id": "todo-1", "title": "Do not carry me", "status": "pending" }]
        });
        let ConversationTurnTraceItem::ToolResult {
            tool, observation, ..
        } = &mut trace.items[2]
        else {
            panic!("expected tool result");
        };
        *tool = "todo_update".to_string();
        *observation = json!({
            "revision": 1,
            "items": [{ "id": "todo-1", "title": "Do not carry me", "status": "pending" }]
        });

        let rendered = ConversationTraceRenderer::render(&trace).unwrap();
        assert_eq!(rendered.activity_items.len(), 1);
        let mut items = rendered.activity_items;
        items.extend(rendered.terminal_item);
        let context = ContextFrame::new(items)
            .to_messages()
            .into_iter()
            .map(|message| message.content)
            .collect::<Vec<_>>()
            .join("\n");
        assert!(!context.contains("todo_update"));
        assert!(!context.contains("Do not carry me"));
    }

    #[test]
    fn command_session_audit_is_not_rendered_or_required_in_model_context_log() {
        let mut trace = trace();
        let call_id = match &trace.items[1] {
            ConversationTurnTraceItem::ToolCall { call_id, .. } => call_id.clone(),
            _ => panic!("expected tool call"),
        };
        match &mut trace.items[1] {
            ConversationTurnTraceItem::ToolCall { tool, .. } => *tool = "run_command".to_string(),
            _ => unreachable!(),
        }
        match &mut trace.items[2] {
            ConversationTurnTraceItem::ToolResult { tool, sequence, .. } => {
                *tool = "run_command".to_string();
                *sequence = 6;
            }
            _ => unreachable!(),
        }
        trace.items.insert(
            2,
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: 5,
                phase: ConversationCommandSessionLifecyclePhase::Started,
                session_id: "cmd_0123456789abcdef0123456789abcdef".to_string(),
                call_id,
                status: AgentCommandSessionStatus::Running,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                archive: Default::default(),
                created_at: 1_000,
            },
        );

        let rendered = ConversationTraceRenderer::render(&trace).unwrap();
        assert_eq!(rendered.activity_items.len(), 3);
        assert!(!ContextFrame::new(rendered.activity_items)
            .to_messages()
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n")
            .contains("cmd_0123456789abcdef0123456789abcdef"));

        let model_context_items = trace
            .items
            .iter()
            .filter(|item| item.is_model_visible())
            .map(|item| match item {
                ConversationTurnTraceItem::AssistantNarration {
                    sequence, content, ..
                } => ConversationModelContextItem {
                    sequence: *sequence,
                    ordinal: 0,
                    role: "assistant".to_string(),
                    content: content.clone(),
                    tool_call_id: None,
                    tool_calls: Vec::new(),
                    is_error: false,
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    operation,
                    ..
                } => ConversationModelContextItem {
                    sequence: *sequence,
                    ordinal: 0,
                    role: "assistant".to_string(),
                    content: String::new(),
                    tool_call_id: None,
                    tool_calls: vec![AgentContextCheckpointToolCall {
                        id: call_id.clone(),
                        name: tool.clone(),
                        args: operation.clone(),
                    }],
                    is_error: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence, call_id, ..
                } => ConversationModelContextItem {
                    sequence: *sequence,
                    ordinal: 0,
                    role: "tool".to_string(),
                    content: "command is running".to_string(),
                    tool_call_id: Some(call_id.clone()),
                    tool_calls: Vec::new(),
                    is_error: false,
                },
                ConversationTurnTraceItem::UserGuidance { .. }
                | ConversationTurnTraceItem::CommandSessionLifecycle { .. } => unreachable!(),
            })
            .collect::<Vec<_>>();
        ConversationTraceRenderer::render_with_model_context(&trace, &model_context_items).unwrap();
    }

    #[test]
    fn exact_model_prefix_wins_over_lossy_trace_projection() {
        let trace = trace();
        let (call_id, tool, args) = match &trace.items[1] {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } => (call_id.clone(), tool.clone(), operation.clone()),
            _ => panic!("expected tool call"),
        };
        let exact_marker = "EXACT_RESULT_BODY_OMITTED_FROM_DURABLE_TRACE";
        let model_items = vec![
            ConversationModelContextItem {
                sequence: 3,
                ordinal: 0,
                role: "assistant".to_string(),
                content: "I will inspect the file.".to_string(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 4,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![AgentContextCheckpointToolCall {
                    id: call_id.clone(),
                    name: tool,
                    args,
                }],
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 5,
                ordinal: 0,
                role: "tool".to_string(),
                content: format!(r#"{{"ok":true,"content":"{exact_marker}"}}"#),
                tool_call_id: Some(call_id),
                tool_calls: Vec::new(),
                is_error: false,
            },
        ];

        let rendered =
            ConversationTraceRenderer::render_with_model_context(&trace, &model_items).unwrap();
        let mut items = rendered.activity_items;
        items.extend(rendered.terminal_item);
        let frame = ContextFrame::new(items);
        frame.validate_complete_tool_protocol().unwrap();
        let messages = frame.to_messages();

        assert!(messages
            .iter()
            .any(|message| message.content.contains(exact_marker)));
        assert_eq!(
            messages
                .iter()
                .map(|message| message.content.matches(exact_marker).count())
                .sum::<usize>(),
            1
        );
    }

    #[test]
    fn exact_model_prefix_cannot_split_a_tool_exchange() {
        let trace = trace();
        let (call_id, tool, args) = match &trace.items[1] {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                tool,
                operation,
                ..
            } => (call_id.clone(), tool.clone(), operation.clone()),
            _ => panic!("expected tool call"),
        };
        let model_items = vec![
            ConversationModelContextItem {
                sequence: 3,
                ordinal: 0,
                role: "assistant".to_string(),
                content: "I will inspect the file.".to_string(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            },
            ConversationModelContextItem {
                sequence: 4,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![AgentContextCheckpointToolCall {
                    id: call_id,
                    name: tool,
                    args,
                }],
                is_error: false,
            },
        ];

        let error = match ConversationTraceRenderer::render_with_model_context(&trace, &model_items)
        {
            Ok(_) => panic!("a model prefix must not split a tool exchange"),
            Err(error) => error,
        };
        assert!(error
            .to_string()
            .contains("cannot end with an unresolved tool call"));
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
            archive: Default::default(),
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
        assert!(messages[2].content.contains("\"exitCode\":1"));
        assert!(!messages[2].content.contains("\"ok\""));
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
                ConversationTurnTraceItem::AssistantNarration { .. }
                | ConversationTurnTraceItem::UserGuidance { .. }
                | ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {}
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
