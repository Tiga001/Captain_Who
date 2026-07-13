use crate::protocol::{AgentError, AgentResult};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

pub(crate) fn format_message_created_at(created_at: i64) -> AgentResult<String> {
    if created_at < 0 {
        return Err(AgentError::new("消息创建时间无效。"));
    }

    OffsetDateTime::from_unix_timestamp_nanos(i128::from(created_at) * 1_000_000)
        .map_err(|_| AgentError::new("消息创建时间超出可支持范围。"))?
        .format(&Rfc3339)
        .map_err(|error| AgentError::new(format!("无法格式化消息创建时间：{error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_unix_milliseconds_as_stable_utc_rfc3339() {
        assert_eq!(
            format_message_created_at(1_234).unwrap(),
            "1970-01-01T00:00:01.234Z"
        );
        assert!(format_message_created_at(-1).is_err());
    }
}
