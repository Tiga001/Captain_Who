//! Device-local token counts. No account, conversation, model or request content is retained.
//! The opaque request ID is solely a durable deduplication key, independent of chat retention.
use crate::{
    AgentApiStyle, LocalTokenUsageDay, LocalTokenUsageSummaryInput, LocalTokenUsageSummaryOutput,
    ModelRequestObservation,
};
use rusqlite::{params, Connection, OptionalExtension};
use time::{Date, OffsetDateTime, UtcOffset};

const TIMEZONE: &str = "Asia/Shanghai";
const MAX_RANGE_DAYS: i64 = 3660;

fn invalid(message: impl Into<String>) -> String {
    format!("本机 Token 统计数据无效：{}", message.into())
}

fn parse_count(value: &str) -> Result<u128, String> {
    value
        .parse()
        .map_err(|_| invalid("Token 计数不是有效整数。"))
}

fn parse_date(value: &str) -> Result<Date, String> {
    if value.len() != 10 || !value.is_ascii() {
        return Err(invalid("日期必须为 YYYY-MM-DD。"));
    }
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")
        .expect("static date format");
    let date = Date::parse(value, &format).map_err(|_| invalid("日期必须为 YYYY-MM-DD。"))?;
    if date.year() < 1970 || date.to_string() != value {
        return Err(invalid("日期必须为 1970 年之后的 YYYY-MM-DD。"));
    }
    Ok(date)
}

pub fn validate_range(input: &LocalTokenUsageSummaryInput) -> Result<(), String> {
    let from = parse_date(&input.from)?;
    let to = parse_date(&input.to)?;
    let days = (to - from).whole_days();
    if !(0..MAX_RANGE_DAYS).contains(&days) {
        return Err(invalid("起止日期顺序无效或超过 3660 天。"));
    }
    Ok(())
}

fn beijing_date(timestamp_ms: i64) -> Result<String, String> {
    let timestamp = OffsetDateTime::from_unix_timestamp_nanos(i128::from(timestamp_ms) * 1_000_000)
        .map_err(|_| invalid("请求完成时间超出范围。"))?;
    Ok(timestamp
        .to_offset(UtcOffset::from_hms(8, 0, 0).expect("Beijing offset"))
        .date()
        .to_string())
}

fn complete_token_count(observation: &ModelRequestObservation) -> Option<u128> {
    let usage = observation.actual_usage.as_ref()?;
    // Completion counts already include reasoning for the supported provider contracts.
    // OpenAI cache is an input subset; Anthropic normalized input includes cache read/write.
    match (usage.normalized_input_tokens, usage.raw.output_tokens) {
        (Some(input), Some(output)) => Some(u128::from(input) + u128::from(output)),
        _ if observation.api_style == AgentApiStyle::OpenAiCompatible => {
            usage.raw.total_tokens.map(u128::from)
        }
        // Anthropic's raw total may have been derived from uncached input only. Never guess
        // a complete count from an incomplete breakdown, nor substitute the input estimate.
        _ => None,
    }
}

/// Must share the observation's savepoint/transaction so a crash cannot separate the two facts.
pub(crate) fn record_observation(
    connection: &Connection,
    observation: &ModelRequestObservation,
) -> Result<(), String> {
    let started_at: i64 = connection
        .query_row(
            "SELECT started_at FROM local_token_usage_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    // No historical backfill, including a late replay of an old observation after upgrade.
    if observation.completed_at < started_at {
        return Ok(());
    }
    let date = beijing_date(observation.completed_at)?;
    let count = complete_token_count(observation);
    let count_string = count.map(|value| value.to_string());
    let inserted = connection
        .execute(
            "INSERT INTO local_token_usage_requests(request_id, usage_date, token_count)
             VALUES (?1, ?2, ?3) ON CONFLICT(request_id) DO NOTHING",
            params![observation.id, date, count_string],
        )
        .map_err(|error| error.to_string())?;
    if inserted == 0 {
        return Ok(());
    }
    let existing = connection
        .query_row(
            "SELECT token_count, unreported_request_count FROM local_token_usage_days
             WHERE usage_date = ?1",
            [&date],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    let (existing_count, missing) = existing.unwrap_or_else(|| ("0".into(), 0));
    let next_count = parse_count(&existing_count)?
        .checked_add(count.unwrap_or(0))
        .ok_or_else(|| invalid("累计 Token 数超出范围。"))?;
    let missing = missing
        .checked_add(i64::from(count.is_none()))
        .ok_or_else(|| invalid("缺失用量请求数超出范围。"))?;
    connection
        .execute(
            "INSERT INTO local_token_usage_days(usage_date, token_count, unreported_request_count)
             VALUES (?1, ?2, ?3) ON CONFLICT(usage_date) DO UPDATE SET
             token_count = excluded.token_count,
             unreported_request_count = excluded.unreported_request_count",
            params![date, next_count.to_string(), missing],
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn summary(
    connection: &Connection,
    input: &LocalTokenUsageSummaryInput,
    now_ms: i64,
) -> Result<LocalTokenUsageSummaryOutput, String> {
    validate_range(input)?;
    let today = beijing_date(now_ms)?;
    let started_at = connection
        .query_row(
            "SELECT started_at FROM local_token_usage_metadata WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| error.to_string())?;
    // A single connection is held by StorageService throughout this query. Only daily
    // aggregates are scanned; request IDs and original observations are never sent over IPC.
    let mut statement = connection
        .prepare("SELECT usage_date, token_count, unreported_request_count FROM local_token_usage_days ORDER BY usage_date")
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(|error| error.to_string())?;
    let mut days = Vec::new();
    let mut total = 0_u128;
    let mut peak = 0_u128;
    let mut today_tokens = "0".to_string();
    let mut unreported = 0_u64;
    for row in rows {
        let (date, token_count, missing) = row.map_err(|error| error.to_string())?;
        let count = parse_count(&token_count)?;
        total = total
            .checked_add(count)
            .ok_or_else(|| invalid("累计 Token 数超出范围。"))?;
        peak = peak.max(count);
        unreported = unreported
            .checked_add(missing)
            .ok_or_else(|| invalid("请求数超出范围。"))?;
        if date == today {
            today_tokens = token_count.clone();
        }
        if date >= input.from && date <= input.to {
            days.push(LocalTokenUsageDay { date, token_count });
        }
    }
    Ok(LocalTokenUsageSummaryOutput {
        timezone: TIMEZONE.to_string(),
        started_at,
        days,
        total_tokens: total.to_string(),
        today_tokens,
        peak_daily_tokens: peak.to_string(),
        unreported_request_count: unreported,
    })
}

#[cfg(test)]
mod tests;
