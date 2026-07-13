use crate::protocol::{AgentError, AgentResult};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

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
}
