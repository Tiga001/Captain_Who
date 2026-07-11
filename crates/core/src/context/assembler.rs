use super::{
    ContextFrame, ContextGroup, ContextItem, ContextMetadata, ContextRetention, ContextScope,
    ContextSource,
};
use crate::llm::{LlmImage, LlmMessage, LlmMessageRole, LlmToolCall};
use crate::protocol::{AgentChatMessage, AgentError, AgentResult};

#[derive(Debug, Clone, Default)]
pub(crate) struct ContextAttachments {
    pub(crate) text: String,
    pub(crate) images: Vec<LlmImage>,
}

#[derive(Debug, Clone)]
pub(crate) struct ContextToolContinuation {
    pub(crate) call: LlmToolCall,
    pub(crate) observation: String,
    pub(crate) is_error: bool,
}

pub(crate) struct ContextAssemblyInput {
    pub(crate) system_prompt: String,
    pub(crate) messages: Vec<AgentChatMessage>,
    pub(crate) attachments: ContextAttachments,
    pub(crate) approval_observation: Option<String>,
    pub(crate) tool_continuation: Option<ContextToolContinuation>,
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

            items.push(ContextItem::new(llm_message, metadata));
        }

        if input.tool_continuation.is_none() {
            if let Some(observation) = input.approval_observation {
                items.push(ContextItem::text(
                    LlmMessageRole::User,
                    observation,
                    ContextSource::ApprovalDecision,
                    ContextScope::Run,
                    ContextRetention::Retained,
                ));
            }
        }

        if let Some(continuation) = input.tool_continuation {
            let group =
                ContextGroup::tool_exchange(format!("tool-continuation:{}", continuation.call.id));
            let metadata = ContextMetadata::new(
                ContextSource::ToolContinuation,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone());
            items.push(ContextItem::assistant(
                "",
                vec![continuation.call.clone()],
                metadata,
            ));
            items.push(ContextItem::tool_result(
                continuation.call.id,
                continuation.observation,
                continuation.is_error,
                ContextMetadata::new(
                    ContextSource::ToolContinuation,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_source(ContextSource::ToolResult)
                .with_group(group),
            ));
        }

        Ok(ContextFrame::new(items))
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
        if content.is_empty() {
            continue;
        }

        match role {
            "system" | "user" | "assistant" => normalized.push(AgentChatMessage {
                role: role.to_string(),
                content: content.to_string(),
            }),
            _ => return Err(AgentError::new(format!("不支持的消息角色：{role}"))),
        }
    }

    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn message(role: &str, content: &str) -> AgentChatMessage {
        AgentChatMessage {
            role: role.to_string(),
            content: content.to_string(),
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
            approval_observation: Some("approval result".to_string()),
            tool_continuation: None,
        })
        .unwrap();

        let messages = frame.to_messages();
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].role, LlmMessageRole::System);
        assert_eq!(messages[1].content, "old question");
        assert_eq!(messages[3].role, LlmMessageRole::User);
        assert!(messages[3].content.contains("attachment body"));
        assert_eq!(messages[3].images.len(), 1);
        assert_eq!(messages[4].content, "approval result");

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
        assert_eq!(manifest.entries[4].sources, vec!["approval_decision"]);
        assert_eq!(manifest.entries[4].retention, "retained");
        let serialized = serde_json::to_string(&manifest).unwrap();
        assert!(!serialized.contains("current question"));
        assert!(!serialized.contains("attachment body"));
    }

    #[test]
    fn continuation_is_an_atomic_tool_exchange_and_replaces_approval_observation() {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            messages: vec![message("user", "edit file")],
            attachments: ContextAttachments::default(),
            approval_observation: Some("duplicate approval observation".to_string()),
            tool_continuation: Some(ContextToolContinuation {
                call: LlmToolCall {
                    id: "call-1".to_string(),
                    name: "apply_patch".to_string(),
                    args: json!({ "filePath": "src/main.rs" }),
                },
                observation: "tool failed".to_string(),
                is_error: true,
            }),
        })
        .unwrap();

        let messages = frame.to_messages();
        assert_eq!(messages.len(), 4);
        assert!(!messages
            .iter()
            .any(|message| message.content.contains("duplicate approval")));
        assert_eq!(messages[2].role, LlmMessageRole::Assistant);
        assert_eq!(messages[3].role, LlmMessageRole::Tool);

        let manifest = frame.manifest();
        assert_eq!(
            manifest.entries[2].group_id,
            Some("tool-continuation:call-1")
        );
        assert_eq!(manifest.entries[2].group_id, manifest.entries[3].group_id);
        assert_eq!(manifest.entries[2].group_kind, Some("tool_exchange"));
        assert!(manifest.entries[2].tool_argument_character_count > 0);
        assert_eq!(
            manifest.entries[3].sources,
            vec!["tool_continuation", "tool_result"]
        );
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
            approval_observation: None,
            tool_continuation: None,
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
            approval_observation: None,
            tool_continuation: None,
        })
        .unwrap_err();
        assert!(error.to_string().contains("不支持的消息角色"));
    }

    #[test]
    fn project_scope_is_reserved_in_the_manifest_vocabulary() {
        assert_eq!(ContextScope::Project.as_str(), "project");
    }
}
