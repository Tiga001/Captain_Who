use crate::protocol::{AgentError, AgentResult};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

const TIMING_METADATA_OPEN_TAG: &str = "<backend_conversation_timing>";
const TIMING_METADATA_CLOSE_TAG: &str = "</backend_conversation_timing>";

/// Tracks the one assistant timestamp that must travel with the following user message.
///
/// Provider APIs do not expose a portable message-timestamp field. Keeping timing metadata on
/// user messages preserves chronology without teaching the model that assistant replies should
/// begin with a backend marker. The tracker is also shared by full assembly and incremental
/// durable-context updates so the same logical message always has the same wire representation.
#[derive(Debug, Clone, Default)]
pub(crate) struct ConversationTimingTracker {
    previous_assistant_created_at: Option<String>,
}

impl ConversationTimingTracker {
    pub(crate) fn render_user_message(
        &mut self,
        content: &str,
        created_at: Option<i64>,
    ) -> AgentResult<String> {
        if content.trim().is_empty() {
            return Ok(String::new());
        }

        let user_created_at = created_at.map(format_message_created_at).transpose()?;
        let rendered = render_user_message_with_timing(
            content,
            self.previous_assistant_created_at.as_deref(),
            user_created_at.as_deref(),
        );
        self.previous_assistant_created_at = None;
        Ok(rendered)
    }

    pub(crate) fn observe_assistant(&mut self, created_at: Option<i64>) -> AgentResult<()> {
        self.previous_assistant_created_at =
            created_at.map(format_message_created_at).transpose()?;
        Ok(())
    }
}

fn render_user_message_with_timing(
    content: &str,
    previous_assistant_created_at: Option<&str>,
    user_created_at: Option<&str>,
) -> String {
    let mut fields = Vec::with_capacity(2);
    if let Some(created_at) = previous_assistant_created_at {
        fields.push(format!(
            "previous_assistant_message_created_at: {created_at}"
        ));
    }
    if let Some(created_at) = user_created_at {
        fields.push(format!("user_message_created_at: {created_at}"));
    }
    if fields.is_empty() {
        return content.to_string();
    }

    format!(
        "{TIMING_METADATA_OPEN_TAG}\n{}\n{TIMING_METADATA_CLOSE_TAG}\n{content}",
        fields.join("\n")
    )
}

pub(crate) fn format_message_created_at(created_at: i64) -> AgentResult<String> {
    let timestamp = message_created_at(created_at)?;
    let local_offset = UtcOffset::local_offset_at(timestamp).unwrap_or(UtcOffset::UTC);
    format_message_created_at_with_offset(timestamp, local_offset)
}

fn message_created_at(created_at: i64) -> AgentResult<OffsetDateTime> {
    if created_at < 0 {
        return Err(AgentError::new("消息创建时间无效。"));
    }

    OffsetDateTime::from_unix_timestamp_nanos(i128::from(created_at) * 1_000_000)
        .map_err(|_| AgentError::new("消息创建时间超出可支持范围。"))
}

fn format_message_created_at_with_offset(
    timestamp: OffsetDateTime,
    offset: UtcOffset,
) -> AgentResult<String> {
    timestamp
        .to_offset(offset)
        .format(&Rfc3339)
        .map_err(|error| AgentError::new(format!("无法格式化消息创建时间：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_milliseconds_with_the_selected_local_offset() {
        let timestamp = message_created_at(1_234).unwrap();
        let china_standard_time = UtcOffset::from_hms(8, 0, 0).unwrap();
        assert_eq!(
            format_message_created_at_with_offset(timestamp, china_standard_time).unwrap(),
            "1970-01-01T08:00:01.234+08:00"
        );
        assert!(format_message_created_at(-1).is_err());
    }

    #[test]
    fn carries_assistant_time_on_the_following_user_message_only() {
        let mut tracker = ConversationTimingTracker::default();
        let first_user = tracker.render_user_message("first", Some(1_000)).unwrap();
        assert!(first_user.contains("user_message_created_at:"));
        assert!(!first_user.contains("previous_assistant_message_created_at:"));

        tracker.observe_assistant(Some(2_000)).unwrap();
        let follow_up = tracker
            .render_user_message("follow up", Some(3_000))
            .unwrap();
        assert!(follow_up.contains("previous_assistant_message_created_at:"));
        assert!(follow_up.contains("user_message_created_at:"));
        assert!(follow_up.ends_with("follow up"));

        let next_user = tracker.render_user_message("next", None).unwrap();
        assert_eq!(next_user, "next");
    }
}
