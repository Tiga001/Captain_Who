use super::*;
use mycopilot_core::notification_subject::{
    human_root_notification_subject, NotificationSubject as HumanRootNotificationSubject,
};
use mycopilot_core::storage::notification_repository::NewNotificationEventRecord;
use sha2::{Digest, Sha256};
#[cfg(test)]
use unicode_segmentation::UnicodeSegmentation;

const ORDINARY_NOTIFICATION_TTL_MS: i64 = 24 * 60 * 60 * 1_000;
const ATTENTION_NOTIFICATION_TTL_MS: i64 = 7 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HumanRootNotificationContext {
    pub(super) conversation_id: String,
    pub(super) user_message_id: String,
    pub(super) assistant_message_id: String,
    pub(super) subject: HumanRootNotificationSubject,
}

impl AgentService {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finalize_turn_with_human_root_notification(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        status: AgentRunStatus,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
        collaboration_final_response_boundary: Option<u64>,
    ) -> Result<(), String> {
        let notification = self.human_root_terminal_notification(
            run_id,
            conversation_id,
            assistant_message_id,
            status,
            completed_at,
        )?;
        if let Some(notification) = notification.as_ref() {
            self.storage
                .finalize_chat_message_with_conversation_turn_notification(
                    conversation_id,
                    assistant_message_id,
                    content,
                    message_status,
                    run_status,
                    trace,
                    model_context_items,
                    trace_created_at,
                    completed_at,
                    usage,
                    notification,
                    collaboration_final_response_boundary,
                )
        } else {
            self.storage
                .finalize_chat_message_with_conversation_trace_model_context_and_usage(
                    conversation_id,
                    assistant_message_id,
                    content,
                    message_status,
                    run_status,
                    trace,
                    model_context_items,
                    trace_created_at,
                    completed_at,
                    usage,
                    collaboration_final_response_boundary,
                )
        }
    }

    /// Resolves an ordinary user-owned root Turn from durable identities only.
    ///
    /// Automation intentionally enters the HumanRoot executor, while a trusted child wake owns a
    /// child Conversation. Check both durable owner facts so neither execution source can start
    /// producing ordinary task notifications if a caller is refactored later.
    pub(super) fn human_root_notification_context(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Result<Option<HumanRootNotificationContext>, String> {
        if self
            .storage
            .get_automation_run_by_agent_run_id(run_id)?
            .is_some()
        {
            return Ok(None);
        }
        if self
            .storage
            .get_agent_node_by_conversation(conversation_id)
            .map_err(|error| error.to_string())?
            .is_some_and(|agent| agent.parent_agent_id.is_some())
        {
            return Ok(None);
        }

        let Some(conversation) = self.storage.load_conversation(conversation_id)? else {
            return Ok(None);
        };
        let Some(assistant_index) = conversation
            .messages
            .iter()
            .position(|message| message.id == assistant_message_id && message.role == "assistant")
        else {
            return Ok(None);
        };
        let Some(user_message) = conversation.messages[..assistant_index]
            .iter()
            .rev()
            .find(|message| message.role == "user")
        else {
            return Ok(None);
        };
        if !is_direct_human_origin(
            &self
                .storage
                .conversation_message_origin(conversation_id, &user_message.id)
                .map_err(|error| error.to_string())?,
        ) {
            return Ok(None);
        }
        let attachment_names = user_message
            .attachments
            .iter()
            .map(|attachment| attachment.name.clone())
            .collect::<Vec<_>>();
        Ok(Some(HumanRootNotificationContext {
            conversation_id: conversation_id.to_string(),
            user_message_id: user_message.id.clone(),
            assistant_message_id: assistant_message_id.to_string(),
            subject: human_root_notification_subject(&user_message.content, &attachment_names),
        }))
    }

    pub(super) fn human_root_terminal_notification(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        status: AgentRunStatus,
        occurred_at: i64,
    ) -> Result<Option<NewNotificationEventRecord>, String> {
        let (notification_kind, ttl_ms) = match status {
            AgentRunStatus::Completed => ("task_completed", ORDINARY_NOTIFICATION_TTL_MS),
            AgentRunStatus::Failed => ("task_failed", ATTENTION_NOTIFICATION_TTL_MS),
            AgentRunStatus::Cancelled => ("task_cancelled", ORDINARY_NOTIFICATION_TTL_MS),
            _ => return Ok(None),
        };
        let Some(context) =
            self.human_root_notification_context(run_id, conversation_id, assistant_message_id)?
        else {
            return Ok(None);
        };
        Ok(Some(new_human_root_notification_event(
            notification_kind,
            run_id,
            context,
            None,
            occurred_at,
            ttl_ms,
        )))
    }

    pub(super) fn human_root_approval_notification(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        approval_action_id: &str,
        occurred_at: i64,
    ) -> Result<Option<NewNotificationEventRecord>, String> {
        let Some(context) =
            self.human_root_notification_context(run_id, conversation_id, assistant_message_id)?
        else {
            return Ok(None);
        };
        Ok(Some(new_human_root_notification_event(
            "approval_required",
            run_id,
            context,
            Some(approval_action_id),
            occurred_at,
            ATTENTION_NOTIFICATION_TTL_MS,
        )))
    }
}

fn is_direct_human_origin(origin: &mycopilot_core::ConversationMessageOrigin) -> bool {
    matches!(origin, mycopilot_core::ConversationMessageOrigin::Human)
}

fn new_human_root_notification_event(
    notification_kind: &str,
    run_id: &str,
    context: HumanRootNotificationContext,
    approval_action_id: Option<&str>,
    occurred_at: i64,
    ttl_ms: i64,
) -> NewNotificationEventRecord {
    let HumanRootNotificationContext {
        conversation_id,
        user_message_id,
        assistant_message_id,
        subject,
    } = context;
    NewNotificationEventRecord {
        notification_kind: notification_kind.to_string(),
        source_kind: "human_root".to_string(),
        source_id: run_id.to_string(),
        run_id: Some(run_id.to_string()),
        automation_id: None,
        conversation_id: Some(conversation_id),
        user_message_id: Some(user_message_id),
        assistant_message_id: Some(assistant_message_id),
        approval_action_id: approval_action_id.map(str::to_string),
        subject_kind: subject.kind.to_string(),
        subject_text: subject.text,
        dedupe_key: deterministic_notification_key(notification_kind, run_id, approval_action_id),
        supersession_key: format!("human-root:{run_id}"),
        resource_revision: Some(occurred_at.max(1)),
        occurred_at,
        expires_at: occurred_at.saturating_add(ttl_ms),
    }
}

fn deterministic_notification_key(
    notification_kind: &str,
    run_id: &str,
    approval_action_id: Option<&str>,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"human-root-notification-v1\0");
    digest.update(notification_kind.as_bytes());
    digest.update(b"\0");
    digest.update(run_id.as_bytes());
    if let Some(approval_action_id) = approval_action_id {
        digest.update(b"\0");
        digest.update(approval_action_id.as_bytes());
    }
    format!(
        "human-root:{notification_kind}:sha256:{:x}",
        digest.finalize()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::notification_subject::{
        NOTIFICATION_SUBJECT_MAX_BYTES, PROMPT_EXCERPT_MAX_GRAPHEMES,
    };

    #[test]
    fn normalizes_visible_prompt_without_attachment_transport_text() {
        let subject = human_root_notification_subject(
            "  制定单元设备\n\n技术报告方案  \n\nAttachments: report.docx, diagram.png",
            &["report.docx".to_string(), "diagram.png".to_string()],
        );

        assert_eq!(subject.kind, "prompt_excerpt");
        assert_eq!(subject.text, "制定单元设备 技术报告方案");
    }

    #[test]
    fn recognizes_both_attachment_only_summary_formats_without_leaking_names() {
        for content in ["Attachments: private.xlsx", "附件：private.xlsx"] {
            let subject = human_root_notification_subject(content, &["private.xlsx".to_string()]);
            assert_eq!(
                subject,
                HumanRootNotificationSubject {
                    kind: "attachment_task",
                    text: "附件任务".to_string(),
                }
            );
        }
    }

    #[test]
    fn truncates_by_unicode_grapheme_cluster() {
        let family = "👨‍👩‍👧‍👦";
        let input =
            std::iter::repeat_n(family, PROMPT_EXCERPT_MAX_GRAPHEMES + 1).collect::<String>();
        let subject = human_root_notification_subject(&input, &[]);

        assert!(subject.text.ends_with('…'));
        assert!(subject.text.graphemes(true).count() <= PROMPT_EXCERPT_MAX_GRAPHEMES);
        assert!(subject.text.len() <= NOTIFICATION_SUBJECT_MAX_BYTES);
        assert!(subject
            .text
            .trim_end_matches('…')
            .graphemes(true)
            .all(|grapheme| grapheme == family));
    }

    #[test]
    fn preserves_short_prompt_and_falls_back_for_defensive_empty_input() {
        assert_eq!(
            human_root_notification_subject("修复测试", &[]).text,
            "修复测试"
        );
        assert_eq!(human_root_notification_subject(" \n ", &[]).text, "任务");
    }

    #[test]
    fn structured_event_contains_only_safe_identity_and_stable_keys() {
        let context = HumanRootNotificationContext {
            conversation_id: "conversation-1".to_string(),
            user_message_id: "user-1".to_string(),
            assistant_message_id: "assistant-1".to_string(),
            subject: human_root_notification_subject("修复通知系统", &[]),
        };
        let first = new_human_root_notification_event(
            "task_failed",
            "run-1",
            context.clone(),
            None,
            100,
            ATTENTION_NOTIFICATION_TTL_MS,
        );
        let replay = new_human_root_notification_event(
            "task_failed",
            "run-1",
            context,
            None,
            100,
            ATTENTION_NOTIFICATION_TTL_MS,
        );

        assert_eq!(first, replay);
        assert_eq!(first.source_kind, "human_root");
        assert_eq!(first.subject_text, "修复通知系统");
        assert_eq!(first.approval_action_id, None);
        assert!(!first.dedupe_key.contains("修复通知系统"));
    }

    #[test]
    fn excludes_dispatcher_and_historical_snapshot_inputs() {
        assert!(!is_direct_human_origin(
            &mycopilot_core::ConversationMessageOrigin::Agent {
                sender_agent_id: "child-agent".to_string(),
                source_agent_message_id: "mailbox-message".to_string(),
            }
        ));
        assert!(!is_direct_human_origin(
            &mycopilot_core::ConversationMessageOrigin::HistoricalSnapshot {
                source_conversation_id: "source-conversation".to_string(),
                source_message_id: "source-message".to_string(),
                original: Box::new(mycopilot_core::ConversationMessageOrigin::Human),
            }
        ));
    }
}
