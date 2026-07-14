use super::{
    format_message_created_at, ContextCompactionSummary, ContextFrame, ContextItem,
    ContextMetadata, ContextOrigin, ContextRetention, ContextScope, ContextSource,
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
    pub(crate) compaction_summary: Option<ContextCompactionSummary>,
    pub(crate) messages: Vec<AgentChatMessage>,
    pub(crate) attachments: ContextAttachments,
}

pub(crate) struct ContextAssembler;

impl ContextAssembler {
    pub(crate) fn assemble(input: ContextAssemblyInput) -> AgentResult<ContextFrame> {
        let has_compaction_summary = input.compaction_summary.is_some();
        let normalized = normalize_messages(input.messages)?;
        let current_turn_index = normalized
            .iter()
            .rposition(|message| message.role == "user");
        let has_attachment_text = !input.attachments.text.trim().is_empty();
        let has_attachment_images = !input.attachments.images.is_empty();

        if normalized.is_empty() && !has_compaction_summary {
            return Err(AgentError::new("没有可发送的对话内容。"));
        }
        if !has_compaction_summary && !normalized.iter().any(|message| message.role != "system") {
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
        if let Some(summary) = input.compaction_summary {
            summary.validate()?;
            items.push(ContextItem::new(
                LlmMessage::text(LlmMessageRole::Assistant, summary.render_for_context()?),
                ContextMetadata::new(
                    ContextSource::ConversationSummary,
                    ContextScope::Conversation,
                    ContextRetention::Retained,
                )
                .with_origin(ContextOrigin::compaction_summary(summary.id)),
            ));
        }

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

            let llm_message = LlmMessage::text(role, render_message_content(&message)?);
            let mut metadata = ContextMetadata::new(
                if is_current_turn {
                    ContextSource::CurrentTurn
                } else {
                    ContextSource::ConversationHistory
                },
                ContextScope::Conversation,
                ContextRetention::Retained,
            );
            if let Some(message_id) = message.message_id {
                metadata = metadata.with_origin(ContextOrigin::conversation_message(message_id));
            }

            if !llm_message.content.trim().is_empty() {
                items.push(ContextItem::new(llm_message, metadata));
            }
            if is_current_turn && (has_attachment_text || has_attachment_images) {
                let mut attachment_message =
                    LlmMessage::text(LlmMessageRole::User, input.attachments.text.clone());
                attachment_message
                    .images
                    .extend(attachment_images.take().unwrap_or_default());
                items.push(ContextItem::new(
                    attachment_message,
                    ContextMetadata::new(
                        ContextSource::InputAttachment,
                        ContextScope::Run,
                        ContextRetention::Retained,
                    ),
                ));
            }
            if let Some(trace) = trace {
                items.extend(trace.terminal_item);
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

fn render_message_content(message: &AgentChatMessage) -> AgentResult<String> {
    let Some(created_at) = message.created_at else {
        return Ok(message.content.clone());
    };
    if message.content.trim().is_empty() {
        return Ok(String::new());
    }
    let timestamp = format_message_created_at(created_at)?;
    Ok(format!(
        "[Message created at: {timestamp}]\n{}",
        message.content
    ))
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
                message_id: message.message_id,
                role: role.to_string(),
                content: content.to_string(),
                created_at: message.created_at,
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
    use crate::ContextJournalCursor;
    use serde_json::json;

    fn compaction_summary() -> ContextCompactionSummary {
        let covered_through = ContextJournalCursor::message("assistant-old");
        let prefix = crate::ContextCompactionPrefix {
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-revision-1".to_string(),
            covered_through: covered_through.clone(),
            previous_summary: None,
            source_items: vec![crate::ContextCompactionSourceItem::Message {
                cursor: covered_through.clone(),
                role: "assistant".to_string(),
                content: "The user requested an old task and the agent completed it.".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            }],
        };
        ContextCompactionSummary {
            schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
            id: "summary-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            source_revision: "source-revision-1".to_string(),
            previous_summary_id: None,
            covered_through,
            content: "The user requested an old task and the agent completed it.".to_string(),
            continuity: crate::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
            generation: crate::ContextCompactionGeneration::test(),
            source_input_tokens: 100,
            summary_input_tokens: 20,
            continuity_input_tokens: 30,
            replacement_input_tokens: 50,
            created_at: 1,
        }
    }

    fn message(role: &str, content: &str) -> AgentChatMessage {
        AgentChatMessage {
            message_id: None,
            role: role.to_string(),
            content: content.to_string(),
            created_at: None,
            conversation_turn_trace: None,
        }
    }

    fn traced_assistant(content: &str) -> AgentChatMessage {
        AgentChatMessage {
            message_id: Some("assistant-previous".to_string()),
            role: "assistant".to_string(),
            content: content.to_string(),
            created_at: None,
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
            compaction_summary: None,
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
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].role, LlmMessageRole::System);
        assert_eq!(messages[1].content, "old question");
        assert_eq!(messages[3].role, LlmMessageRole::User);
        assert_eq!(messages[3].content, "current question");
        assert_eq!(messages[4].content, "attachment body");
        assert_eq!(messages[4].images.len(), 1);

        let manifest = frame.manifest();
        assert_eq!(manifest.entries[0].sources, vec!["backend_system_prompt"]);
        assert_eq!(manifest.entries[1].sources, vec!["conversation_history"]);
        assert_eq!(manifest.entries[3].sources, vec!["current_turn"]);
        assert_eq!(manifest.entries[3].scope, "conversation");
        assert_eq!(manifest.entries[3].retention, "retained");
        assert_eq!(manifest.entries[4].sources, vec!["input_attachment"]);
        assert_eq!(manifest.entries[4].scope, "run");
        assert_eq!(manifest.entries[4].retention, "retained");
        assert_eq!(manifest.entries[4].image_base64_bytes, 3);
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains("current question"));
        assert!(!serialized.contains("attachment body"));
    }

    #[test]
    fn renders_message_creation_time_with_explicit_local_offset() {
        let mut timestamped = message("user", "historical question");
        timestamped.created_at = Some(0);
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: None,
            messages: vec![timestamped],
            attachments: ContextAttachments::default(),
        })
        .unwrap();

        let expected = format!(
            "[Message created at: {}]\nhistorical question",
            format_message_created_at(0).unwrap()
        );
        assert_eq!(frame.to_messages()[1].content, expected);
    }

    #[test]
    fn normalizes_supported_messages_and_rejects_unknown_roles() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: None,
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
            compaction_summary: None,
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
            compaction_summary: None,
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
        let mut historical_assistant = traced_assistant("");
        historical_assistant.created_at = Some(0);
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: None,
            messages: vec![
                message("user", "do the work"),
                historical_assistant,
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
        assert!(!messages
            .iter()
            .any(|message| message.content.starts_with("[Message created at:")));
    }

    #[test]
    fn project_scope_is_reserved_in_the_manifest_vocabulary() {
        assert_eq!(ContextScope::Project.as_str(), "project");
    }

    #[test]
    fn assembles_compaction_summary_before_uncovered_tail() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: Some(compaction_summary()),
            messages: vec![message("user", "continue from the summary")],
            attachments: ContextAttachments::default(),
        })
        .unwrap();

        let messages = frame.to_messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, LlmMessageRole::System);
        assert_eq!(messages[1].role, LlmMessageRole::Assistant);
        assert!(messages[1].content.contains("old task"));
        assert!(messages[1].content.contains("semantic summary is lossy"));
        assert!(messages[1]
            .content
            .contains("follow this block are newer and authoritative"));
        assert!(messages[1]
            .content
            .contains("BEGIN_UNTRUSTED_CONTINUITY_RECORDS_JSON"));
        assert_eq!(messages[2].content, "continue from the summary");
        assert_eq!(
            frame.manifest().entries[1].sources,
            vec!["conversation_summary"]
        );
    }

    #[test]
    fn summary_only_context_is_valid_after_covering_the_latest_completed_turn() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: Some(compaction_summary()),
            messages: Vec::new(),
            attachments: ContextAttachments::default(),
        })
        .unwrap();

        assert_eq!(frame.to_messages().len(), 2);
    }
}
