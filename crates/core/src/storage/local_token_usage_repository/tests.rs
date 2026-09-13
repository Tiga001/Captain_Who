use super::*;
use crate::storage::{migrations, model_request_observation_repository::insert_observation};
use crate::{
    AgentUsage, ModelRequestActualUsage, ModelRequestObservationStatus, ModelRequestPurpose,
    ModelRequestUsageNormalization, MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
};

fn setup() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
        .execute("UPDATE local_token_usage_metadata SET started_at = 1", [])
        .unwrap();
    connection
}

fn observation(id: &str, completed_at: i64) -> ModelRequestObservation {
    ModelRequestObservation {
        schema_version: MODEL_REQUEST_OBSERVATION_SCHEMA_VERSION,
        id: id.into(),
        run_id: format!("run-{id}"),
        conversation_id: None,
        assistant_message_id: None,
        operation_id: None,
        request_index: 1,
        purpose: ModelRequestPurpose::AgentLoop,
        model: "private-model".into(),
        api_style: AgentApiStyle::OpenAiCompatible,
        status: ModelRequestObservationStatus::Completed,
        tool_set: None,
        estimate: None,
        actual_usage: Some(ModelRequestActualUsage {
            raw: AgentUsage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                output_thinking_tokens: Some(10),
                total_tokens: Some(120),
                cached_input_tokens: Some(80),
                cache_creation_input_tokens: None,
                billable_request_count: Some(1),
            },
            normalized_input_tokens: Some(100),
            normalization: ModelRequestUsageNormalization::OpenAiInputTokens,
        }),
        finish_reason: None,
        error_code: None,
        error_message: None,
        started_at: completed_at - 1,
        completed_at,
    }
}

fn all(connection: &Connection, now: i64) -> LocalTokenUsageSummaryOutput {
    summary(
        connection,
        &LocalTokenUsageSummaryInput {
            from: "1970-01-01".into(),
            to: "1970-01-31".into(),
        },
        now,
    )
    .unwrap()
}

#[test]
fn daily_boundaries_dedup_and_all_history_totals_are_independent_of_display_range() {
    let connection = setup();
    // Beijing midnight is UTC 16:00, independent of the computer's local timezone.
    let before = observation("before", 57_599_999);
    insert_observation(&connection, &before).unwrap();
    insert_observation(&connection, &before).unwrap();
    insert_observation(&connection, &observation("after", 57_600_000)).unwrap();
    let result = summary(
        &connection,
        &LocalTokenUsageSummaryInput {
            from: "1970-01-02".into(),
            to: "1970-01-02".into(),
        },
        57_600_000,
    )
    .unwrap();
    assert_eq!(result.total_tokens, "240");
    assert_eq!(result.today_tokens, "120");
    assert_eq!(result.peak_daily_tokens, "120");
    assert_eq!(
        result.days,
        vec![LocalTokenUsageDay {
            date: "1970-01-02".into(),
            token_count: "120".into()
        }]
    );
}

#[test]
fn chat_and_billing_deletion_do_not_remove_counts_or_reenable_duplicate_ids() {
    let connection = setup();
    connection.execute_batch("INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','Test',1,1);
        INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES ('msg','chat','assistant','','complete',1,0);").unwrap();
    let mut row = observation("owned", 2);
    row.conversation_id = Some("chat".into());
    row.assistant_message_id = Some("msg".into());
    insert_observation(&connection, &row).unwrap();
    crate::storage::usage_repository::clear_usage_records(
        &connection,
        &crate::AgentUsageClearInput::default(),
    )
    .unwrap();
    connection
        .execute("DELETE FROM conversations WHERE id = 'chat'", [])
        .unwrap();
    assert_eq!(all(&connection, 2).total_tokens, "120");
    row.conversation_id = None;
    row.assistant_message_id = None;
    insert_observation(&connection, &row).unwrap();
    assert_eq!(all(&connection, 2).total_tokens, "120");
}

#[test]
fn observations_and_counts_rollback_together_in_outer_transaction_or_on_ledger_error() {
    let mut connection = setup();
    {
        let tx = connection.transaction().unwrap();
        insert_observation(&tx, &observation("rolled-back", 2)).unwrap();
    }
    assert_eq!(all(&connection, 2).total_tokens, "0");
    connection.execute_batch("CREATE TRIGGER reject_local_day BEFORE INSERT ON local_token_usage_days BEGIN SELECT RAISE(ABORT, 'test failure'); END;").unwrap();
    assert!(insert_observation(&connection, &observation("failed-write", 2)).is_err());
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM model_request_observations",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM local_token_usage_requests",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn unreported_usage_is_not_zero_or_estimated_and_failed_actual_usage_counts() {
    let connection = setup();
    let mut missing = observation("missing", 2);
    missing.actual_usage = None;
    insert_observation(&connection, &missing).unwrap();
    insert_observation(&connection, &missing).unwrap();
    let mut failed = observation("failed", 3);
    failed.status = ModelRequestObservationStatus::Failed;
    failed.error_message = Some("connection interrupted after usage".into());
    insert_observation(&connection, &failed).unwrap();
    let result = all(&connection, 3);
    assert_eq!(result.unreported_request_count, 1);
    assert_eq!(result.total_tokens, "120");
}

#[test]
fn no_historical_backfill_or_late_old_observation_counting() {
    let connection = setup();
    connection
        .execute("UPDATE local_token_usage_metadata SET started_at = 100", [])
        .unwrap();
    insert_observation(&connection, &observation("old", 99)).unwrap();
    insert_observation(&connection, &observation("new", 100)).unwrap();
    assert_eq!(all(&connection, 100).total_tokens, "120");
}

#[test]
fn provider_cache_and_thinking_are_not_double_counted_and_large_counts_are_exact() {
    let connection = setup();
    let mut row = observation("anthropic", 2);
    row.api_style = AgentApiStyle::AnthropicCompatible;
    let actual = row.actual_usage.as_mut().unwrap();
    actual.raw.cache_creation_input_tokens = Some(30);
    actual.normalized_input_tokens = Some(210);
    actual.normalization = ModelRequestUsageNormalization::AnthropicInputPlusCache;
    insert_observation(&connection, &row).unwrap();
    assert_eq!(all(&connection, 2).total_tokens, "230");
    let mut large = observation("large", 3);
    let usage = large.actual_usage.as_mut().unwrap();
    usage.raw.input_tokens = Some(9_007_199_254_740_993);
    usage.normalized_input_tokens = usage.raw.input_tokens;
    usage.raw.total_tokens = None;
    insert_observation(&connection, &large).unwrap();
    assert_eq!(all(&connection, 3).total_tokens, "9007199254741243");
}

#[test]
fn invalid_or_excessive_calendar_ranges_are_rejected() {
    for (from, to) in [
        ("2026-02-29", "2026-03-01"),
        ("2026-03-02", "2026-03-01"),
        ("2020-01-01", "2040-01-01"),
        ("2026-1-1", "2026-01-02"),
    ] {
        assert!(validate_range(&LocalTokenUsageSummaryInput {
            from: from.into(),
            to: to.into()
        })
        .is_err());
    }
}

#[test]
fn explicit_openai_total_is_usable_but_incomplete_anthropic_total_is_not_guessed() {
    let connection = setup();
    let mut total_only = observation("total-only", 2);
    let usage = total_only.actual_usage.as_mut().unwrap();
    usage.raw.input_tokens = None;
    usage.raw.output_tokens = None;
    usage.normalized_input_tokens = None;
    usage.normalization = ModelRequestUsageNormalization::Unavailable;
    insert_observation(&connection, &total_only).unwrap();
    assert_eq!(all(&connection, 2).total_tokens, "120");
    let mut partial = observation("partial", 3);
    partial.api_style = AgentApiStyle::AnthropicCompatible;
    let usage = partial.actual_usage.as_mut().unwrap();
    usage.raw.output_tokens = None;
    usage.normalized_input_tokens = Some(180);
    usage.normalization = ModelRequestUsageNormalization::AnthropicInputPlusCache;
    insert_observation(&connection, &partial).unwrap();
    let result = all(&connection, 3);
    assert_eq!(result.total_tokens, "120");
    assert_eq!(result.unreported_request_count, 1);
}
