use super::{
    ContextFrame, ContextItem, ContextMetadata, ContextRetention, ContextScope, ContextSource,
    ConversationTraceRenderer,
};
use crate::llm::{LlmImage, LlmMessage, LlmMessageRole};
use crate::protocol::{AgentChatMessage, AgentError, AgentResult};

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextAttachments {
    pub(crate) text: String,
    pub(crate) images: Vec<LlmImage>,
}

pub(crate) struct ContextAssemblyInput {
    pub(crate) system_prompt: String,
    pub(crate) messages: Vec<AgentChatMessage>,
    pub(crate) attachments: ContextAttachments,
}

pub(crate) struct ContextAssembler;

impl ContextAssembler {
    pub(crate) fn assemble(input: ContextAssemblyInput) -> AgentResult<ContextFrame> {
        let mut normalized = normalize_messages(input.messages)?;
        let current_turn_index = normalized
            .iter()
            .rposition(|message| message.role == "user");
        let has_attachment_text = !input.attachments.text.trim().is_empty();
        let has_attachment_images = !input.attachments.images.is_empty();

        if has_attachment_text {
            if let Some(index) = current_turn_index {
                normalized[index].content = format!(
                    "{}\n\n{}",
                    normalized[index].content, input.attachments.text
                );
            }
        }

        if normalized.is_empty() {
            return Err(AgentError::new("没有可发送的对话内容。"));
        }
        if !normalized.iter().any(|message| message.role != "system") {
            return Err(AgentError::new("对话里缺少用户或助手消息。"));
        }

        let mut items = Vec::with_capacity(normalized.len() + 4);
        items.push(ContextItem::text(
            LlmMessageRole::System,
            input.system_prompt,
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ));

        let mut attachment_images = Some(input.attachments.images);
        for (index, message) in normalized.into_iter().enumerate() {
            let role = role_from_str(&message.role)?;
            let is_current_turn = current_turn_index == Some(index);
            let trace = message
                .conversation_turn_trace
                .as_ref()
                .map(ConversationTraceRenderer::render)
                .transpose()?;

            if let Some(trace) = &trace {
                items.extend(trace.activity_items.iter().cloned());
            }

            let mut llm_message = LlmMessage::text(role, message.content);
            let mut metadata = ContextMetadata::new(
                if is_current_turn {
                    ContextSource::CurrentTurn
                } else {
                    ContextSource::ConversationHistory
                },
                ContextScope::Conversation,
                ContextRetention::Retained,
            );

            if is_current_turn && (has_attachment_text || has_attachment_images) {
                metadata = metadata.with_source(ContextSource::InputAttachment);
                llm_message
                    .images
                    .extend(attachment_images.take().unwrap_or_default());
            }

            if !llm_message.content.trim().is_empty() {
                items.push(ContextItem::new(llm_message, metadata));
            }
            if let Some(trace) = trace {
                items.push(trace.terminal_item);
            }
        }

        let frame = ContextFrame::new(items);
        frame.validate_complete_tool_protocol()?;
        Ok(frame)
    }
}

fn role_from_str(role: &str) -> AgentResult<LlmMessageRole> {
    match role {
        "system" => Ok(LlmMessageRole::System),
        "user" => Ok(LlmMessageRole::User),
        "assistant" => Ok(LlmMessageRole::Assistant),
        _ => Err(AgentError::new(format!("不支持的消息角色：{role}"))),
    }
}

fn normalize_messages(messages: Vec<AgentChatMessage>) -> AgentResult<Vec<AgentChatMessage>> {
    let mut normalized = Vec::new();

    for message in messages {
        let role = message.role.trim();
        let content = message.content.trim();
        let trace = message.conversation_turn_trace;
        if content.is_empty() && trace.is_none() {
            continue;
        }
        if trace.is_some() && role != "assistant" {
            return Err(AgentError::new(
                "ConversationTurnTrace 只能附加到 assistant 历史消息。",
            ));
        }

        match role {
            "system" | "user" | "assistant" => normalized.push(AgentChatMessage {
                role: role.to_string(),
                content: content.to_string(),
                conversation_turn_trace: trace,
            }),
            _ => return Err(AgentError::new(format!("不支持的消息角色：{role}"))),
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_trace::{
        ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
        ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
    };
    use crate::protocol::AgentApprovalStatus;
    use serde_json::json;

    fn message(role: &str, content: &str) -> AgentChatMessage {
        AgentChatMessage {
            role: role.to_string(),
            content: content.to_string(),
            conversation_turn_trace: None,
        }
    }

    fn traced_assistant(content: &str) -> AgentChatMessage {
        AgentChatMessage {
            role: "assistant".to_string(),
            content: content.to_string(),
            conversation_turn_trace: Some(ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "run-previous".to_string(),
                conversation_id: "conversation-1".to_string(),
                assistant_message_id: "assistant-previous".to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: vec![
                    ConversationTurnTraceItem::AssistantNarration {
                        sequence: 0,
                        content: "I will update the file.".to_string(),
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolCall {
                        sequence: 1,
                        call_id: "call-1".to_string(),
                        tool: "write_file".to_string(),
                        operation: json!({
                            "filePath": "src/new.rs",
                            "mode": "create"
                        }),
                        approval_status: AgentApprovalStatus::Approved,
                        truncated: false,
                    },
                    ConversationTurnTraceItem::ToolResult {
                        sequence: 2,
                        call_id: "call-1".to_string(),
                        tool: "write_file".to_string(),
                        status: ConversationTraceToolResultStatus::Succeeded,
                        success: true,
                        observation: json!({
                            "filePath": "src/new.rs",
                            "status": "applied",
                            "additions": 4,
                            "deletions": 0
                        }),
                        approval_status: AgentApprovalStatus::Approved,
                        error: None,
                        truncated: false,
                    },
                ],
            }),
        }
    }

    #[test]
    fn assembles_ordered_context_with_provenance() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "backend rules".to_string(),
            messages: vec![
                message("user", "old question"),
                message("assistant", "old answer"),
                message("user", "current question"),
            ],
            attachments: ContextAttachments {
                text: "attachment body".to_string(),
                images: vec![LlmImage {
                    mime_type: "image/png".to_string(),
                    data_base64: "abc".to_string(),
                }],
            },
        })
        .unwrap();

        let messages = frame.to_messages();
        assert_eq!(messages.len(), 4);
        assert_eq!(messages[0].role, LlmMessageRole::System);
        assert_eq!(messages[1].content, "old question");
        assert_eq!(messages[3].role, LlmMessageRole::User);
        assert!(messages[3].content.contains("attachment body"));
        assert_eq!(messages[3].images.len(), 1);

        let manifest = frame.manifest();
        assert_eq!(manifest.entries[0].sources, vec!["backend_system_prompt"]);
        assert_eq!(manifest.entries[1].sources, vec!["conversation_history"]);
        assert_eq!(
            manifest.entries[3].sources,
            vec!["current_turn", "input_attachment"]
        );
        assert_eq!(manifest.entries[3].scope, "conversation");
        assert_eq!(manifest.entries[3].retention, "retained");
        assert_eq!(manifest.entries[3].image_base64_bytes, 3);
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains("current question"));
        assert!(!serialized.contains("attachment body"));
    }

    #[test]
    fn normalizes_supported_messages_and_rejects_unknown_roles() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            messages: vec![
                message(" user ", " hello "),
                message("assistant", " "),
                message("system", "history rules"),
            ],
            attachments: ContextAttachments::default(),
        })
        .unwrap();
        let messages = frame.to_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[1].content, "hello");
        assert_eq!(messages[2].content, "history rules");

        let error = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            messages: vec![message("tool", "result")],
            attachments: ContextAttachments::default(),
        })
        .unwrap_err();
        assert!(error.to_string().contains("不支持的消息角色"));
    }

    #[test]
    fn assembles_conversation_trace_before_final_reply_and_terminal_before_next_user() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            messages: vec![
                message("user", "create a file"),
                traced_assistant("Created src/new.rs."),
                message("user", "what changed?"),
            ],
            attachments: ContextAttachments::default(),
        })
        .unwrap();

        frame.validate_complete_tool_protocol().unwrap();
        let messages = frame.to_messages();
        assert_eq!(messages.len(), 8);
        assert_eq!(messages[1].content, "create a file");
        assert_eq!(messages[2].content, "I will update the file.");
        assert_eq!(messages[3].role, LlmMessageRole::Assistant);
        assert_eq!(messages[3].tool_calls[0].name, "write_file");
        assert_eq!(messages[3].tool_calls[0].args["filePath"], "src/new.rs");
        assert_eq!(messages[4].role, LlmMessageRole::Tool);
        assert_eq!(
            messages[4].tool_call_id.as_deref(),
            Some(messages[3].tool_calls[0].id.as_str())
        );
        assert_eq!(messages[5].content, "Created src/new.rs.");
        assert!(messages[6]
            .content
            .contains("historical_agent_activity_terminal"));
        assert!(messages[6]
            .content
            .contains("\"terminalStatus\":\"completed\""));
        assert_eq!(messages[7].role, LlmMessageRole::User);
        assert_eq!(messages[7].content, "what changed?");

        let manifest = frame.manifest();
        for index in [2, 3, 4, 6] {
            assert_eq!(manifest.entries[index].sources, vec!["conversation_trace"]);
            assert_eq!(manifest.entries[index].scope, "conversation");
            assert_eq!(manifest.entries[index].retention, "retained");
        }
        assert_eq!(manifest.entries[5].sources, vec!["conversation_history"]);
        assert_eq!(manifest.entries[7].sources, vec!["current_turn"]);

        let checkpoint = frame.checkpoint_items().unwrap();
        let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
        restored.validate_complete_tool_protocol().unwrap();
        assert_eq!(
            serde_json::to_value(frame.manifest()).unwrap(),
            serde_json::to_value(restored.manifest()).unwrap()
        );
    }

    #[test]
    fn keeps_trace_when_historical_assistant_final_text_is_empty() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            messages: vec![
                message("user", "do the work"),
                traced_assistant(""),
                message("user", "continue"),
            ],
            attachments: ContextAttachments::default(),
        })
        .unwrap();

        let messages = frame.to_messages();
        assert!(messages
            .iter()
            .any(|message| message.content == "I will update the file."));
        assert!(messages.iter().any(|message| message
            .content
            .contains("historical_agent_activity_terminal")));
        assert!(!messages.iter().any(|message| message.content.is_empty()
            && message.role == LlmMessageRole::Assistant
            && message.tool_calls.is_empty()));
    }

    #[test]
    fn project_scope_is_reserved_in_the_manifest_vocabulary() {
        assert_eq!(ContextScope::Project.as_str(), "project");
    }
}
