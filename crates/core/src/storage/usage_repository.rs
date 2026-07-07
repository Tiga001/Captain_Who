// Rust core storage usage persistence.
use crate::storage::models::AgentUsageRecordInsert;
use crate::{
    AgentUsageClearInput, AgentUsageClearOutput, AgentUsageModelSummary, AgentUsageSummaryInput,
    AgentUsageSummaryOutput, AgentUsageSummaryRange,
};
use rusqlite::{params, Connection};

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

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
            provider_path,
            started_at,
            completed_at,
            status,
            error,
            created_at,
            input_tokens,
            output_tokens,
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
            provider_path = excluded.provider_path,
            started_at = excluded.started_at,
            completed_at = excluded.completed_at,
            status = excluded.status,
            error = excluded.error,
            created_at = excluded.created_at,
            input_tokens = excluded.input_tokens,
            output_tokens = excluded.output_tokens,
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
            &record.provider_path,
            record.started_at,
            record.completed_at,
            &record.status,
            &record.error,
            record.created_at,
            optional_u64_to_i64(record.input_tokens),
            optional_u64_to_i64(record.output_tokens),
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
        input_tokens: totals.input_tokens,
        output_tokens: totals.output_tokens,
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
    let changed = connection.execute(
        "
        DELETE FROM agent_usage_records
        WHERE (?1 IS NULL OR created_at >= ?1)
          AND (?2 IS NULL OR created_at <= ?2)
        ",
        params![input.from, input.to],
    )?;

    Ok(AgentUsageClearOutput {
        deleted_records: changed as u64,
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
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
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
    connection.query_row(
        "
        SELECT
            COALESCE(SUM(billable_request_count), 0),
            COUNT(*),
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(total_tokens),
            SUM(cached_input_tokens),
            SUM(cache_creation_input_tokens),
            SUM(estimated_cost)
        FROM agent_usage_records
        WHERE (?1 IS NULL OR created_at >= ?1)
          AND (?2 IS NULL OR created_at <= ?2)
        ",
        params![from, to],
        |row| {
            Ok(UsageTotals {
                request_count: i64_to_u64(row.get::<_, i64>(0)?),
                message_count: i64_to_u64(row.get::<_, i64>(1)?),
                input_tokens: optional_i64_to_u64(row.get(2)?),
                output_tokens: optional_i64_to_u64(row.get(3)?),
                total_tokens: optional_i64_to_u64(row.get(4)?),
                cached_input_tokens: optional_i64_to_u64(row.get(5)?),
                cache_creation_input_tokens: optional_i64_to_u64(row.get(6)?),
                estimated_cost: row.get(7)?,
            })
        },
    )
}

fn query_usage_models(
    connection: &Connection,
    from: Option<i64>,
    to: Option<i64>,
) -> rusqlite::Result<Vec<AgentUsageModelSummary>> {
    let mut statement = connection.prepare(
        "
        SELECT
            model_id,
            model_name,
            provider_path,
            COALESCE(SUM(billable_request_count), 0),
            COUNT(*),
            SUM(input_tokens),
            SUM(output_tokens),
            SUM(total_tokens),
            SUM(cached_input_tokens),
            SUM(cache_creation_input_tokens),
            SUM(estimated_cost)
        FROM agent_usage_records
        WHERE (?1 IS NULL OR created_at >= ?1)
          AND (?2 IS NULL OR created_at <= ?2)
        GROUP BY model_id, model_name, provider_path
        ORDER BY COALESCE(SUM(total_tokens), 0) DESC, model_name ASC
        ",
    )?;

    let models = statement
        .query_map(params![from, to], |row| {
            Ok(AgentUsageModelSummary {
                model_id: row.get(0)?,
                model_name: row.get(1)?,
                provider_path: row.get(2)?,
                request_count: i64_to_u64(row.get::<_, i64>(3)?),
                message_count: i64_to_u64(row.get::<_, i64>(4)?),
                input_tokens: optional_i64_to_u64(row.get(5)?),
                output_tokens: optional_i64_to_u64(row.get(6)?),
                total_tokens: optional_i64_to_u64(row.get(7)?),
                cached_input_tokens: optional_i64_to_u64(row.get(8)?),
                cache_creation_input_tokens: optional_i64_to_u64(row.get(9)?),
                estimated_cost: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(models)
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
    normalized.parse::<f64>().ok().filter(|price| *price >= 0.0)
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

    #[test]
    fn summarizes_usage_by_range_and_model() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        insert_conversation(&connection, "conversation-2");
        upsert_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: Some("project-1".to_string()),
                model_id: "model-a".to_string(),
                model_name: "Model A".to_string(),
                provider_path: Some("provider/model-a".to_string()),
                started_at: Some(900),
                completed_at: Some(1_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 1_000,
                input_tokens: Some(1_000),
                output_tokens: Some(500),
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
        upsert_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-2".to_string(),
                conversation_id: "conversation-2".to_string(),
                message_id: "message-2".to_string(),
                run_id: "run-2".to_string(),
                project_id: None,
                model_id: "model-b".to_string(),
                model_name: "Model B".to_string(),
                provider_path: None,
                started_at: Some(8_900),
                completed_at: Some(9_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 9_000,
                input_tokens: Some(2_000),
                output_tokens: Some(100),
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
        assert_eq!(summary.cached_input_tokens, Some(100));
        assert_eq!(summary.cache_creation_input_tokens, None);
        assert_eq!(summary.models.len(), 1);
        assert_eq!(summary.models[0].model_id, "model-a");
    }

    #[test]
    fn clears_usage_records_without_touching_messages() {
        let connection = in_memory_connection();
        insert_conversation(&connection, "conversation-1");
        upsert_usage_record(
            &connection,
            &AgentUsageRecordInsert {
                id: "usage-1".to_string(),
                conversation_id: "conversation-1".to_string(),
                message_id: "message-1".to_string(),
                run_id: "run-1".to_string(),
                project_id: None,
                model_id: "model-a".to_string(),
                model_name: "Model A".to_string(),
                provider_path: None,
                started_at: Some(900),
                completed_at: Some(1_000),
                status: Some("completed".to_string()),
                error: None,
                created_at: 1_000,
                input_tokens: Some(1),
                output_tokens: Some(2),
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
    }

    #[test]
    fn estimates_cost_from_per_1k_prices() {
        assert_eq!(
            estimate_usage_cost(Some(1_500), Some(500), "0.01", "0.02"),
            Some(0.025)
        );
        assert_eq!(estimate_usage_cost(None, None, "0.01", "0.02"), None);
        assert_eq!(estimate_usage_cost(Some(1), None, "bad", "0.02"), None);
    }
}
