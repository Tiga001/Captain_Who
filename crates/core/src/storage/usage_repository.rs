use crate::storage::models::AgentUsageRecordInsert;
use crate::{
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageModelSummary, AgentUsageSummaryInput,
    AgentUsageSummaryOutput, AgentUsageSummaryRange,
};
use rusqlite::{params, params_from_iter, types::Value as SqlValue, Connection, OptionalExtension};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

const DELETED_USAGE_ROLLUP_UPDATE_SQL: &str = "
    request_count = agent_deleted_usage_daily_rollups.request_count + excluded.request_count,
    message_count = agent_deleted_usage_daily_rollups.message_count + excluded.message_count,
    unpriced_message_count = agent_deleted_usage_daily_rollups.unpriced_message_count + excluded.unpriced_message_count,
    input_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.input_tokens IS NULL AND excluded.input_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.input_tokens, 0) + COALESCE(excluded.input_tokens, 0)
    END,
    output_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.output_tokens IS NULL AND excluded.output_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.output_tokens, 0) + COALESCE(excluded.output_tokens, 0)
    END,
    output_thinking_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.output_thinking_tokens IS NULL AND excluded.output_thinking_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.output_thinking_tokens, 0) + COALESCE(excluded.output_thinking_tokens, 0)
    END,
    total_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.total_tokens IS NULL AND excluded.total_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.total_tokens, 0) + COALESCE(excluded.total_tokens, 0)
    END,
    cached_input_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.cached_input_tokens IS NULL AND excluded.cached_input_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.cached_input_tokens, 0) + COALESCE(excluded.cached_input_tokens, 0)
    END,
    cache_creation_input_tokens = CASE
        WHEN agent_deleted_usage_daily_rollups.cache_creation_input_tokens IS NULL AND excluded.cache_creation_input_tokens IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.cache_creation_input_tokens, 0) + COALESCE(excluded.cache_creation_input_tokens, 0)
    END,
    estimated_cost = CASE
        WHEN agent_deleted_usage_daily_rollups.estimated_cost IS NULL AND excluded.estimated_cost IS NULL THEN NULL
        ELSE COALESCE(agent_deleted_usage_daily_rollups.estimated_cost, 0.0) + COALESCE(excluded.estimated_cost, 0.0)
    END,
    updated_at = excluded.updated_at
";

pub fn upsert_usage_record(
    connection: &Connection,
    record: &AgentUsageRecordInsert,
) -> rusqlite::Result<()> {
    connection.execute(
        "
        INSERT INTO agent_usage_records (
            id,
            conversation_id,
            message_id,
            run_id,
            project_id,
            model_id,
            model_name,
            started_at,
            completed_at,
            status,
            error,
            created_at,
            input_tokens,
            output_tokens,
            output_thinking_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            billable_request_count,
            input_price,
            output_price,
            estimated_cost
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22)
        ON CONFLICT(conversation_id, message_id) DO UPDATE SET
            run_id = excluded.run_id,
            project_id = excluded.project_id,
            model_id = excluded.model_id,
            model_name = excluded.model_name,
            started_at = excluded.started_at,
            completed_at = excluded.completed_at,
            status = excluded.status,
            error = excluded.error,
            created_at = excluded.created_at,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
            output_thinking_tokens = excluded.output_thinking_tokens,
            total_tokens = excluded.total_tokens,
            cached_input_tokens = excluded.cached_input_tokens,
            cache_creation_input_tokens = excluded.cache_creation_input_tokens,
            billable_request_count = excluded.billable_request_count,
            input_price = excluded.input_price,
            output_price = excluded.output_price,
            estimated_cost = excluded.estimated_cost
        ",
        params![
            &record.id,
            &record.conversation_id,
            &record.message_id,
            &record.run_id,
            &record.project_id,
            &record.model_id,
            &record.model_name,
            record.started_at,
            record.completed_at,
            &record.status,
            &record.error,
            record.created_at,
            optional_u64_to_i64(record.input_tokens),
            optional_u64_to_i64(record.output_tokens),
            optional_u64_to_i64(record.output_thinking_tokens),
            optional_u64_to_i64(record.total_tokens),
            optional_u64_to_i64(record.cached_input_tokens),
            optional_u64_to_i64(record.cache_creation_input_tokens),
            u64_to_i64(record.billable_request_count),
            &record.input_price,
            &record.output_price,
            record.estimated_cost,
        ],
    )?;
    Ok(())
}

pub fn load_usage_record_for_owner(
    connection: &Connection,
    run_id: &str,
    conversation_id: &str,
    message_id: &str,
) -> rusqlite::Result<Option<AgentUsageRecordInsert>> {
    connection
        .query_row(
            "
            SELECT
                id,
                conversation_id,
                message_id,
                run_id,
                project_id,
                model_id,
                model_name,
                started_at,
                completed_at,
                status,
                error,
                created_at,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                billable_request_count,
                input_price,
                output_price,
                estimated_cost
            FROM agent_usage_records
            WHERE run_id = ?1
              AND conversation_id = ?2
              AND message_id = ?3
            ",
            params![run_id, conversation_id, message_id],
            |row| {
                Ok(AgentUsageRecordInsert {
                    id: row.get(0)?,
                    conversation_id: row.get(1)?,
                    message_id: row.get(2)?,
                    run_id: row.get(3)?,
                    project_id: row.get(4)?,
                    model_id: row.get(5)?,
                    model_name: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
                    status: row.get(9)?,
                    error: row.get(10)?,
                    created_at: row.get(11)?,
                    input_tokens: checked_optional_i64_to_u64(row.get(12)?, 12)?,
                    output_tokens: checked_optional_i64_to_u64(row.get(13)?, 13)?,
                    output_thinking_tokens: checked_optional_i64_to_u64(row.get(14)?, 14)?,
                    total_tokens: checked_optional_i64_to_u64(row.get(15)?, 15)?,
                    cached_input_tokens: checked_optional_i64_to_u64(row.get(16)?, 16)?,
                    cache_creation_input_tokens: checked_optional_i64_to_u64(row.get(17)?, 17)?,
                    billable_request_count: checked_i64_to_u64(row.get(18)?, 18)?,
                    input_price: row.get(19)?,
                    output_price: row.get(20)?,
                    estimated_cost: row.get(21)?,
                })
            },
        )
        .optional()
}

pub fn roll_up_deleted_usage_for_conversation(
    connection: &Connection,
    conversation_id: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    roll_up_deleted_usage(
        connection,
        "conversation_id = ?1",
        vec![SqlValue::Text(conversation_id.to_string())],
        now_ms,
    )
}

pub fn roll_up_deleted_usage_for_project(
    connection: &Connection,
    project_id: &str,
    now_ms: i64,
) -> rusqlite::Result<()> {
    roll_up_deleted_usage(
        connection,
        "
        conversation_id IN (
            SELECT id
            FROM conversations
            WHERE project_id = ?1
        )
        ",
        vec![SqlValue::Text(project_id.to_string())],
        now_ms,
    )
}

pub fn roll_up_deleted_usage_for_messages(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
    now_ms: i64,
) -> rusqlite::Result<()> {
    if message_ids.is_empty() {
        return Ok(());
    }
    let message_ids_json = serde_json::to_string(message_ids)
        .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
    roll_up_deleted_usage(
        connection,
        "
        conversation_id = ?1
        AND message_id IN (SELECT value FROM json_each(?2))
        ",
        vec![
            SqlValue::Text(conversation_id.to_string()),
            SqlValue::Text(message_ids_json),
        ],
        now_ms,
    )
}

pub(super) fn roll_up_orphaned_usage(connection: &Connection) -> rusqlite::Result<()> {
    let now_ms = connection.query_row(
        "
        SELECT COALESCE(MAX(created_at), 0)
        FROM agent_usage_records
        WHERE NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE messages.id = agent_usage_records.message_id
              AND messages.conversation_id = agent_usage_records.conversation_id
        )
        ",
        [],
        |row| row.get(0),
    )?;
    roll_up_deleted_usage(
        connection,
        "
        NOT EXISTS (
            SELECT 1
            FROM messages
            WHERE messages.id = agent_usage_records.message_id
              AND messages.conversation_id = agent_usage_records.conversation_id
        )
        ",
        Vec::new(),
        now_ms,
    )
}

fn roll_up_deleted_usage(
    connection: &Connection,
    usage_filter_sql: &str,
    mut usage_filter_values: Vec<SqlValue>,
    now_ms: i64,
) -> rusqlite::Result<()> {
    let day_parameter = usage_filter_values.len() + 1;
    let now_parameter = usage_filter_values.len() + 2;
    let statement = format!(
        "
        INSERT INTO agent_deleted_usage_daily_rollups (
            usage_day,
            model_id,
            model_name,
            request_count,
            message_count,
            unpriced_message_count,
            input_tokens,
            output_tokens,
            output_thinking_tokens,
            total_tokens,
            cached_input_tokens,
            cache_creation_input_tokens,
            estimated_cost,
            created_at,
            updated_at
        )
        SELECT
            CAST(created_at / ?{day_parameter} AS INTEGER) * ?{day_parameter},
            model_id,
            model_name,
            COALESCE(SUM(billable_request_count), 0),
            COUNT(*),
            SUM(CASE
                WHEN estimated_cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL) THEN 1
                ELSE 0
            END),
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(output_thinking_tokens),
            SUM(total_tokens),
            SUM(cached_input_tokens),
            SUM(cache_creation_input_tokens),
            SUM(estimated_cost),
            ?{now_parameter},
            ?{now_parameter}
        FROM agent_usage_records
        WHERE {usage_filter_sql}
        GROUP BY
            CAST(created_at / ?{day_parameter} AS INTEGER) * ?{day_parameter},
            model_id,
            model_name
        ON CONFLICT(usage_day, model_id, model_name) DO UPDATE SET
            {DELETED_USAGE_ROLLUP_UPDATE_SQL}
        "
    );
    usage_filter_values.push(SqlValue::Integer(DAY_MS));
    usage_filter_values.push(SqlValue::Integer(now_ms));
    connection.execute(&statement, params_from_iter(usage_filter_values))?;
    Ok(())
}

pub fn usage_summary(
    connection: &Connection,
    input: &AgentUsageSummaryInput,
    now_ms: i64,
) -> rusqlite::Result<AgentUsageSummaryOutput> {
    let (from, to) = summary_window(input, now_ms);
    let totals = query_usage_totals(connection, from, to)?;
    let models = query_usage_models(connection, from, to)?;

    Ok(AgentUsageSummaryOutput {
        request_count: totals.request_count,
        message_count: totals.message_count,
        unpriced_message_count: totals.unpriced_message_count,
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
        output_thinking_tokens: totals.output_thinking_tokens,
        total_tokens: totals.total_tokens,
        cached_input_tokens: totals.cached_input_tokens,
        cache_creation_input_tokens: totals.cache_creation_input_tokens,
        estimated_cost: totals.estimated_cost,
        models,
    })
}

pub fn clear_usage_records(
    connection: &Connection,
    input: &AgentUsageClearInput,
) -> rusqlite::Result<AgentUsageClearOutput> {
    let changed_records = connection.execute(
        "
        DELETE FROM agent_usage_records
        WHERE (?1 IS NULL OR created_at >= ?1)
          AND (?2 IS NULL OR created_at <= ?2)
        ",
        params![input.from, input.to],
    )?;
    let (rollup_from, rollup_to) = rollup_window(input.from, input.to);
    let changed_rollups = connection.execute(
        "
        DELETE FROM agent_deleted_usage_daily_rollups
        WHERE (?1 IS NULL OR usage_day >= ?1)
          AND (?2 IS NULL OR usage_day <= ?2)
        ",
        params![rollup_from, rollup_to],
    )?;

    Ok(AgentUsageClearOutput {
        deleted_records: (changed_records + changed_rollups) as u64,
    })
}

pub fn estimate_usage_cost(
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    input_price: &str,
    output_price: &str,
) -> Option<f64> {
    let mut cost = 0.0;
    let mut has_usage = false;

    if let Some(tokens) = input_tokens {
        cost += tokens as f64 * parse_price_per_1k(input_price)? / 1000.0;
        has_usage = true;
    }
    if let Some(tokens) = output_tokens {
        cost += tokens as f64 * parse_price_per_1k(output_price)? / 1000.0;
        has_usage = true;
    }

    has_usage.then_some(cost)
}

#[derive(Debug, Default)]
struct UsageTotals {
    request_count: u64,
    message_count: u64,
    unpriced_message_count: u64,
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    output_thinking_tokens: Option<u64>,
    total_tokens: Option<u64>,
    cached_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    estimated_cost: Option<f64>,
}

fn query_usage_totals(
    connection: &Connection,
    from: Option<i64>,
    to: Option<i64>,
) -> rusqlite::Result<UsageTotals> {
    let (rollup_from, rollup_to) = rollup_window(from, to);
    connection.query_row(
        "
        SELECT
            COALESCE(SUM(request_count), 0),
            COALESCE(SUM(message_count), 0),
            COALESCE(SUM(unpriced_message_count), 0),
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(output_thinking_tokens),
            SUM(total_tokens),
            SUM(cached_input_tokens),
            SUM(cache_creation_input_tokens),
            SUM(estimated_cost)
        FROM (
            SELECT
                billable_request_count AS request_count,
                1 AS message_count,
                CASE
                    WHEN estimated_cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL) THEN 1
                    ELSE 0
                END AS unpriced_message_count,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                estimated_cost
            FROM agent_usage_records
            WHERE (?1 IS NULL OR created_at >= ?1)
              AND (?2 IS NULL OR created_at <= ?2)
            UNION ALL
            SELECT
                request_count,
                message_count,
                unpriced_message_count,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                estimated_cost
            FROM agent_deleted_usage_daily_rollups
            WHERE (?3 IS NULL OR usage_day >= ?3)
              AND (?4 IS NULL OR usage_day <= ?4)
        )
        ",
        params![from, to, rollup_from, rollup_to],
        |row| {
            Ok(UsageTotals {
                request_count: i64_to_u64(row.get::<_, i64>(0)?),
                message_count: i64_to_u64(row.get::<_, i64>(1)?),
                unpriced_message_count: i64_to_u64(row.get::<_, i64>(2)?),
                input_tokens: optional_i64_to_u64(row.get(3)?),
                output_tokens: optional_i64_to_u64(row.get(4)?),
                output_thinking_tokens: optional_i64_to_u64(row.get(5)?),
                total_tokens: optional_i64_to_u64(row.get(6)?),
                cached_input_tokens: optional_i64_to_u64(row.get(7)?),
                cache_creation_input_tokens: optional_i64_to_u64(row.get(8)?),
                estimated_cost: row.get(9)?,
            })
        },
    )
}

fn query_usage_models(
    connection: &Connection,
    from: Option<i64>,
    to: Option<i64>,
) -> rusqlite::Result<Vec<AgentUsageModelSummary>> {
    let (rollup_from, rollup_to) = rollup_window(from, to);
    let mut statement = connection.prepare(
        "
        WITH usage_entries AS (
            SELECT
                model_id,
                model_name,
                created_at AS observed_at,
                billable_request_count AS request_count,
                1 AS message_count,
                CASE
                    WHEN estimated_cost IS NULL AND (input_tokens IS NOT NULL OR output_tokens IS NOT NULL) THEN 1
                    ELSE 0
                END AS unpriced_message_count,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                estimated_cost
            FROM agent_usage_records
            WHERE (?1 IS NULL OR created_at >= ?1)
              AND (?2 IS NULL OR created_at <= ?2)
            UNION ALL
            SELECT
                model_id,
                model_name,
                usage_day AS observed_at,
                request_count,
                message_count,
                unpriced_message_count,
                input_tokens,
                output_tokens,
                output_thinking_tokens,
                total_tokens,
                cached_input_tokens,
                cache_creation_input_tokens,
                estimated_cost
            FROM agent_deleted_usage_daily_rollups
            WHERE (?3 IS NULL OR usage_day >= ?3)
              AND (?4 IS NULL OR usage_day <= ?4)
        ),
        aggregated AS (
            SELECT
                model_id,
                COALESCE(SUM(request_count), 0) AS request_count,
                COALESCE(SUM(message_count), 0) AS message_count,
                COALESCE(SUM(unpriced_message_count), 0) AS unpriced_message_count,
                SUM(input_tokens) AS input_tokens,
                SUM(output_tokens) AS output_tokens,
                SUM(output_thinking_tokens) AS output_thinking_tokens,
                SUM(total_tokens) AS total_tokens,
                SUM(cached_input_tokens) AS cached_input_tokens,
                SUM(cache_creation_input_tokens) AS cache_creation_input_tokens,
                SUM(estimated_cost) AS estimated_cost
            FROM usage_entries
            GROUP BY model_id
        )
        SELECT
            aggregated.model_id,
            COALESCE(
                NULLIF((
                    SELECT configured.display_name
                    FROM models AS configured
                    WHERE configured.id = aggregated.model_id
                    LIMIT 1
                ), ''),
                (
                    SELECT historical.model_name
                    FROM usage_entries AS historical
                    WHERE historical.model_id = aggregated.model_id
                    ORDER BY historical.observed_at DESC, historical.model_name DESC
                    LIMIT 1
                ),
                aggregated.model_id
            ) AS model_name,
            EXISTS(
                SELECT 1
                FROM models AS configured
                WHERE configured.id = aggregated.model_id
            ) AS is_configured,
            aggregated.request_count,
            aggregated.message_count,
            aggregated.unpriced_message_count,
            aggregated.input_tokens,
            aggregated.output_tokens,
            aggregated.output_thinking_tokens,
            aggregated.total_tokens,
            aggregated.cached_input_tokens,
            aggregated.cache_creation_input_tokens,
            aggregated.estimated_cost
        FROM aggregated
        ORDER BY COALESCE(aggregated.total_tokens, 0) DESC, model_name ASC
        ",
    )?;

    let models = statement
        .query_map(params![from, to, rollup_from, rollup_to], |row| {
            Ok(AgentUsageModelSummary {
                model_id: row.get(0)?,
                model_name: row.get(1)?,
                is_configured: row.get(2)?,
                request_count: i64_to_u64(row.get::<_, i64>(3)?),
                message_count: i64_to_u64(row.get::<_, i64>(4)?),
                unpriced_message_count: i64_to_u64(row.get::<_, i64>(5)?),
                input_tokens: optional_i64_to_u64(row.get(6)?),
                output_tokens: optional_i64_to_u64(row.get(7)?),
                output_thinking_tokens: optional_i64_to_u64(row.get(8)?),
                total_tokens: optional_i64_to_u64(row.get(9)?),
                cached_input_tokens: optional_i64_to_u64(row.get(10)?),
                cache_creation_input_tokens: optional_i64_to_u64(row.get(11)?),
                estimated_cost: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(models)
}

fn rollup_window(from: Option<i64>, to: Option<i64>) -> (Option<i64>, Option<i64>) {
    // A rollup row is anchored at a UTC day boundary. Select anchors contained by the
    // requested interval so adjacent local-day windows cannot claim the same row twice.
    (from.map(day_start_at_or_after_ms), to.map(day_start_ms))
}

fn day_start_ms(timestamp_ms: i64) -> i64 {
    timestamp_ms.div_euclid(DAY_MS) * DAY_MS
}

fn day_start_at_or_after_ms(timestamp_ms: i64) -> i64 {
    let start = day_start_ms(timestamp_ms);
    if start == timestamp_ms {
        start
    } else {
        start.saturating_add(DAY_MS)
    }
}

fn summary_window(input: &AgentUsageSummaryInput, now_ms: i64) -> (Option<i64>, Option<i64>) {
    match input.range {
        AgentUsageSummaryRange::Last7Days => (Some(now_ms - 7 * DAY_MS), Some(now_ms)),
        AgentUsageSummaryRange::Last30Days => (Some(now_ms - 30 * DAY_MS), Some(now_ms)),
        AgentUsageSummaryRange::All => (None, None),
        AgentUsageSummaryRange::Custom => (input.from, input.to),
    }
}

fn parse_price_per_1k(value: &str) -> Option<f64> {
    let normalized = value.trim().replace(',', "");
    if normalized.is_empty() {
        return None;
    }
    normalized
        .parse::<f64>()
        .ok()
        .filter(|price| price.is_finite() && *price >= 0.0)
}

pub(crate) fn is_valid_price_per_1k(value: &str) -> bool {
    parse_price_per_1k(value).is_some()
}

fn optional_u64_to_i64(value: Option<u64>) -> Option<i64> {
    value.map(u64_to_i64)
}

fn u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn optional_i64_to_u64(value: Option<i64>) -> Option<u64> {
    value.map(i64_to_u64)
}

fn checked_optional_i64_to_u64(value: Option<i64>, column: usize) -> rusqlite::Result<Option<u64>> {
    value
        .map(|value| checked_i64_to_u64(value, column))
        .transpose()
}

fn checked_i64_to_u64(value: i64, column: usize) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(column, value))
}

fn i64_to_u64(value: i64) -> u64 {
    value.max(0) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn in_memory_connection() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn insert_conversation(connection: &Connection, conversation_id: &str) {
        connection
            .execute(
                "
                INSERT INTO conversations (
                    id,
                    project_id,
                    model_id,
                    title,
                    created_at,
                    updated_at
                )
                VALUES (?1, NULL, NULL, 'Usage test', 0, 0)
                ",
                params![conversation_id],
            )
            .unwrap();
    }

    fn upsert_test_usage_record(
        connection: &Connection,
        record: &AgentUsageRecordInsert,
    ) -> rusqlite::Result<()> {
        connection.execute(
            "
            INSERT OR IGNORE INTO messages (
                id, conversation_id, role, content, status,
                agent_run_json, ui_state_json, created_at, position
            ) VALUES (?1, ?2, 'assistant', 'done', 'sent', NULL, NULL, ?3, 0)
            ",
            params![record.message_id, record.conversation_id, record.created_at],
        )?;
        upsert_usage_record(connection, record)
    }

    fn usage_record(
        conversation_id: &str,
        message_id: &str,
        model_name: &str,
        created_at: i64,
        input_tokens: u64,
        estimated_cost: Option<f64>,
    ) -> AgentUsageRecordInsert {
        AgentUsageRecordInsert {
            id: format!("usage-{message_id}"),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
            run_id: format!("run-{message_id}"),
            project_id: None,
            model_id: "provider/model-a".to_string(),
            model_name: model_name.to_string(),
            started_at: Some(created_at - 1),
            completed_at: Some(created_at),
            status: Some("completed".to_string()),
            error: None,
            created_at,
            input_tokens: Some(input_tokens),
            output_tokens: Some(0),
            output_thinking_tokens: None,
            total_tokens: Some(input_tokens),
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: 1,
            input_price: Some("0.01".to_string()),
            output_price: Some("0.02".to_string()),
            estimated_cost,
        }
    }

    #[test]
    fn summarizes_usage_by_range_and_model() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        insert_conversation(&connection, "conversation-2");
        upsert_test_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: Some("project-1".to_string()),
                model_id: "provider/model-a".to_string(),
                model_name: "Model A".to_string(),
                started_at: Some(900),
                completed_at: Some(1_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 1_000,
                input_tokens: Some(1_000),
                output_tokens: Some(500),
                output_thinking_tokens: Some(125),
                total_tokens: Some(1_500),
                cached_input_tokens: Some(100),
                cache_creation_input_tokens: None,
                billable_request_count: 2,
                input_price: Some("0.01".to_string()),
                output_price: Some("0.02".to_string()),
                estimated_cost: Some(0.02),
            },
        )
        .unwrap();
        upsert_test_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-2".to_string(),
                conversation_id: "conversation-2".to_string(),
                message_id: "message-2".to_string(),
                run_id: "run-2".to_string(),
                project_id: None,
                model_id: "model-b".to_string(),
                model_name: "Model B".to_string(),
                started_at: Some(8_900),
                completed_at: Some(9_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 9_000,
                input_tokens: Some(2_000),
                output_tokens: Some(100),
                output_thinking_tokens: Some(25),
                total_tokens: Some(2_100),
                cached_input_tokens: None,
                cache_creation_input_tokens: Some(50),
                billable_request_count: 1,
                input_price: Some("0.01".to_string()),
                output_price: Some("0.02".to_string()),
                estimated_cost: Some(0.022),
            },
        )
        .unwrap();

        let summary = usage_summary(
            &connection,
            &AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::Custom,
                from: Some(0),
                to: Some(5_000),
            },
            10_000,
        )
        .unwrap();

        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.message_count, 1);
        assert_eq!(summary.input_tokens, Some(1_000));
        assert_eq!(summary.output_tokens, Some(500));
        assert_eq!(summary.output_thinking_tokens, Some(125));
        assert_eq!(summary.cached_input_tokens, Some(100));
        assert_eq!(summary.cache_creation_input_tokens, None);
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].model_id, "provider/model-a");
    }

    #[test]
    fn groups_renamed_models_by_stable_identity_and_marks_deleted_configuration() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        insert_conversation(&connection, "conversation-2");
        connection
            .execute(
                "
                INSERT INTO models (
                    id, display_name, supports_image,
                    input_price, output_price, enabled, position, created_at, updated_at
                )
                VALUES ('provider/model-a', 'Current model name', 0,
                        '0.03', '0.04', 1, 0, 0, 0)
                ",
                [],
            )
            .unwrap();
        upsert_test_usage_record(
            &connection,
            &usage_record(
                "conversation-1",
                "message-1",
                "Old model name",
                1_000,
                100,
                Some(1.0),
            ),
        )
        .unwrap();
        upsert_test_usage_record(
            &connection,
            &usage_record(
                "conversation-2",
                "message-2",
                "Renamed model",
                2_000,
                200,
                None,
            ),
        )
        .unwrap();

        let summarize = || {
            usage_summary(
                &connection,
                &AgentUsageSummaryInput {
                    range: AgentUsageSummaryRange::All,
                    from: None,
                    to: None,
                },
                3_000,
            )
            .unwrap()
        };

        let configured = summarize();
        assert_eq!(configured.models.len(), 1);
        assert_eq!(configured.message_count, 2);
        assert_eq!(configured.unpriced_message_count, 1);
        assert_eq!(configured.estimated_cost, Some(1.0));
        assert_eq!(configured.models[0].model_name, "Current model name");
        assert!(configured.models[0].is_configured);
        assert_eq!(configured.models[0].input_tokens, Some(300));
        assert_eq!(configured.models[0].unpriced_message_count, 1);

        connection.execute("DELETE FROM models", []).unwrap();
        let deleted = summarize();
        assert_eq!(deleted.models.len(), 1);
        assert_eq!(deleted.models[0].model_name, "Renamed model");
        assert!(!deleted.models[0].is_configured);

        connection
            .execute(
                "
                INSERT INTO models (
                    id, display_name, supports_image,
                    input_price, output_price, enabled, position, created_at, updated_at
                )
                VALUES ('provider/model-a', 'Restored model', 0,
                        '0.05', '0.06', 1, 0, 3_000, 3_000)
                ",
                [],
            )
            .unwrap();
        let restored = summarize();
        assert_eq!(restored.models.len(), 1);
        assert_eq!(restored.models[0].model_name, "Restored model");
        assert!(restored.models[0].is_configured);
        assert_eq!(restored.models[0].input_tokens, Some(300));
        assert_eq!(restored.models[0].estimated_cost, Some(1.0));
    }

    #[test]
    fn rolls_up_deleted_conversation_usage_into_summary_and_clear() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        upsert_test_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: Some("project-1".to_string()),
                model_id: "provider/model-a".to_string(),
                model_name: "Model A".to_string(),
                started_at: Some(DAY_MS + 900),
                completed_at: Some(DAY_MS + 1_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: DAY_MS + 1_000,
                input_tokens: Some(10),
                output_tokens: Some(20),
                output_thinking_tokens: Some(5),
                total_tokens: Some(30),
                cached_input_tokens: Some(3),
                cache_creation_input_tokens: None,
                billable_request_count: 2,
                input_price: Some("0".to_string()),
                output_price: Some("0".to_string()),
                estimated_cost: Some(0.1),
            },
        )
        .unwrap();
        upsert_test_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-2".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-2".to_string(),
                run_id: "run-2".to_string(),
                project_id: Some("project-1".to_string()),
                model_id: "provider/model-a".to_string(),
                model_name: "Model A".to_string(),
                started_at: Some(DAY_MS + 1_900),
                completed_at: Some(DAY_MS + 2_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: DAY_MS + 2_000,
                input_tokens: Some(1),
                output_tokens: Some(2),
                output_thinking_tokens: None,
                total_tokens: Some(3),
                cached_input_tokens: None,
                cache_creation_input_tokens: Some(4),
                billable_request_count: 1,
                input_price: Some("0".to_string()),
                output_price: Some("0".to_string()),
                estimated_cost: Some(0.2),
            },
        )
        .unwrap();

        roll_up_deleted_usage_for_conversation(&connection, "conversation-1", DAY_MS * 2).unwrap();
        connection
            .execute(
                "DELETE FROM conversations WHERE id = ?1",
                params!["conversation-1"],
            )
            .unwrap();

        let remaining_records: i64 = connection
            .query_row("SELECT COUNT(*) FROM agent_usage_records", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(remaining_records, 0);

        let summary = usage_summary(
            &connection,
            &AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            DAY_MS * 3,
        )
        .unwrap();
        assert_eq!(summary.request_count, 3);
        assert_eq!(summary.message_count, 2);
        assert_eq!(summary.input_tokens, Some(11));
        assert_eq!(summary.output_tokens, Some(22));
        assert_eq!(summary.output_thinking_tokens, Some(5));
        assert_eq!(summary.total_tokens, Some(33));
        assert_eq!(summary.cached_input_tokens, Some(3));
        assert_eq!(summary.cache_creation_input_tokens, Some(4));
        assert!((summary.estimated_cost.unwrap() - 0.3).abs() < f64::EPSILON);
        assert_eq!(summary.models.len(), 1);
        let deleted = clear_usage_records(
            &connection,
            &AgentUsageClearInput {
                from: None,
                to: None,
            },
        )
        .unwrap();
        assert_eq!(deleted.deleted_records, 1);

        let summary = usage_summary(
            &connection,
            &AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            DAY_MS * 3,
        )
        .unwrap();
        assert_eq!(summary.message_count, 0);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn adjacent_local_day_ranges_do_not_double_count_deleted_rollups() {
        let connection = in_memory_connection();
        for (usage_day, input_tokens, estimated_cost) in
            [(DAY_MS * 2, 100_i64, 1.0_f64), (DAY_MS * 3, 200, 2.0)]
        {
            connection
                .execute(
                    "
                    INSERT INTO agent_deleted_usage_daily_rollups (
                        usage_day,
                        model_id,
                        model_name,
                        request_count,
                        message_count,
                        input_tokens,
                        output_tokens,
                        output_thinking_tokens,
                        total_tokens,
                        cached_input_tokens,
                        cache_creation_input_tokens,
                        estimated_cost,
                        created_at,
                        updated_at
                    )
                    VALUES (?1, 'model-a', 'Model A', 1, 1, ?2, 0, 0, ?2, NULL, NULL, ?3, 0, 0)
                    ",
                    params![usage_day, input_tokens, estimated_cost],
                )
                .unwrap();
        }

        // UTC+8 local midnight is 16:00 UTC on the preceding calendar day.
        let utc_plus_eight_offset_ms = 8 * 60 * 60 * 1_000;
        let first_from = DAY_MS * 2 - utc_plus_eight_offset_ms;
        let first_to = first_from + DAY_MS - 1;
        let second_from = first_from + DAY_MS;
        let second_to = second_from + DAY_MS - 1;

        let summarize = |from, to| {
            usage_summary(
                &connection,
                &AgentUsageSummaryInput {
                    range: AgentUsageSummaryRange::Custom,
                    from: Some(from),
                    to: Some(to),
                },
                second_to,
            )
            .unwrap()
        };

        let first = summarize(first_from, first_to);
        let second = summarize(second_from, second_to);
        let combined = summarize(first_from, second_to);

        assert_eq!(first.input_tokens, Some(100));
        assert_eq!(second.input_tokens, Some(200));
        assert_eq!(combined.input_tokens, Some(300));
        assert_eq!(first.estimated_cost, Some(1.0));
        assert_eq!(second.estimated_cost, Some(2.0));
        assert_eq!(combined.estimated_cost, Some(3.0));
    }

    #[test]
    fn clears_usage_records_without_touching_messages() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        upsert_test_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: None,
                model_id: "model-a".to_string(),
                model_name: "Model A".to_string(),
                started_at: Some(900),
                completed_at: Some(1_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 1_000,
                input_tokens: Some(1),
                output_tokens: Some(2),
                output_thinking_tokens: None,
                total_tokens: Some(3),
                cached_input_tokens: None,
                cache_creation_input_tokens: None,
                billable_request_count: 1,
                input_price: Some("0".to_string()),
                output_price: Some("0".to_string()),
                estimated_cost: Some(0.0),
            },
        )
        .unwrap();

        let deleted = clear_usage_records(
            &connection,
            &AgentUsageClearInput {
                from: None,
                to: None,
            },
        )
        .unwrap();
        assert_eq!(deleted.deleted_records, 1);

        let summary = usage_summary(
            &connection,
            &AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::All,
                from: None,
                to: None,
            },
            10_000,
        )
        .unwrap();
        assert_eq!(summary.message_count, 0);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn estimates_cost_from_per_1k_prices() {
        assert_eq!(
            estimate_usage_cost(Some(1_500), Some(500), "0.01", "0.02"),
            Some(0.025)
        );
        assert_eq!(estimate_usage_cost(None, None, "0.01", "0.02"), None);
        assert_eq!(estimate_usage_cost(Some(1), None, "bad", "0.02"), None);
        assert!(!is_valid_price_per_1k(""));
        assert!(!is_valid_price_per_1k("-1"));
        assert!(!is_valid_price_per_1k("NaN"));
        assert!(!is_valid_price_per_1k("inf"));
        assert!(is_valid_price_per_1k("0"));
        assert!(is_valid_price_per_1k("0.021"));
    }
}
