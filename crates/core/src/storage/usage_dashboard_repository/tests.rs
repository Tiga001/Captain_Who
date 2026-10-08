use super::*;
use crate::storage::{migrations, usage_repository};
use crate::{AgentUsageClearInput, AgentUsageSummaryInput, AgentUsageSummaryRange};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

pub(crate) fn connection() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&c).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    c
}

pub(crate) fn add(
    c: &Connection,
    id: &str,
    model: &str,
    name: &str,
    at: i64,
    tokens: Option<i64>,
    cost: Option<f64>,
) {
    c.execute("INSERT OR IGNORE INTO conversations(id,title,created_at,updated_at) VALUES('chat','Billing',0,0)", []).unwrap();
    c.execute("INSERT OR IGNORE INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES(?1,'chat','assistant','','sent',?2,0)", params![id, at]).unwrap();
    c.execute("INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,model_name,created_at,billable_request_count,input_tokens,total_tokens,estimated_cost) VALUES(?1,'chat',?1,?1,?2,?3,?4,1,?5,?5,?6)", params![id, model, name, at, tokens, cost]).unwrap();
}

pub(crate) fn manual(c: &Connection, id: &str, model: &str, at: i64, cleared: bool) {
    c.execute("INSERT OR IGNORE INTO conversations(id,title,created_at,updated_at) VALUES('chat','Billing',0,0)", []).unwrap();
    c.execute("INSERT INTO manual_context_compaction_operations(operation_id,request_id,conversation_id,status,phase,operation_json,started_at,updated_at) VALUES(?1,?1,'chat','completed','committing','{}',?2,?2)",params![id,at]).unwrap();
    c.execute("INSERT INTO manual_context_compaction_usage_records(operation_id,conversation_id,model_id,model_name,created_at,billable_request_count,input_tokens,total_tokens,estimated_cost,record_json,cleared_at) VALUES(?1,'chat',?2,'Manual name',?3,2,7,7,0.125,'{}',?4)",params![id,model,at,cleared.then_some(at)]).unwrap();
}

fn input(from: i64, width: i64, count: i64) -> AgentUsageDashboardInput {
    AgentUsageDashboardInput {
        windows: (0..count)
            .map(|i| AgentUsageWindow {
                from: from + width * i,
                to: from + width * (i + 1) - 1,
            })
            .collect(),
    }
}

fn old(c: &Connection, from: i64, to: i64) -> AgentUsageSummaryOutput {
    usage_repository::usage_summary(
        c,
        &AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::Custom,
            from: Some(from),
            to: Some(to),
        },
        to,
    )
    .unwrap()
}

fn equal(actual: &AgentUsageSummaryOutput, expected: &AgentUsageSummaryOutput) {
    fn compare(a: &serde_json::Value, b: &serde_json::Value) {
        match (a, b) {
            (serde_json::Value::Object(a), serde_json::Value::Object(b)) => {
                assert_eq!(a.len(), b.len());
                for (k, v) in a {
                    if k == "estimatedCost" && v.is_number() && b[k].is_number() {
                        let x = v.as_f64().unwrap();
                        let y = b[k].as_f64().unwrap();
                        assert!((x - y).abs() <= 1e-12 * (1.0 + y.abs()), "{x} != {y}");
                    } else {
                        compare(v, &b[k]);
                    }
                }
            }
            (serde_json::Value::Array(a), serde_json::Value::Array(b)) => {
                assert_eq!(a.len(), b.len());
                for (x, y) in a.iter().zip(b) {
                    compare(x, y);
                }
            }
            _ => assert_eq!(a, b),
        }
    }
    compare(
        &serde_json::to_value(actual).unwrap(),
        &serde_json::to_value(expected).unwrap(),
    );
}

pub(crate) fn differential(c: &Connection, request: &AgentUsageDashboardInput) {
    let actual = usage_repository::usage_dashboard(c, request).unwrap();
    equal(
        &actual.summary,
        &old(
            c,
            request.windows[0].from,
            request.windows.last().unwrap().to,
        ),
    );
    for (bucket, window) in actual.buckets.iter().zip(&request.windows) {
        equal(bucket, &old(c, window.from, window.to));
    }
}

#[test]
fn dashboard_matches_ledger_for_calendar_offsets_nulls_names_and_deleted_history() {
    let c = connection();
    for i in 0..240 {
        let at = i * HOUR_MS / 2 + if i % 3 == 0 { HOUR_MS / 2 - 1 } else { 0 };
        add(
            &c,
            &format!("row{i}"),
            if i % 2 == 0 { "model-a" } else { "model-b" },
            if i % 3 == 0 {
                "历史中文"
            } else {
                "Previous"
            },
            at,
            if i % 5 == 0 { None } else { Some(i % 11) },
            if i % 7 == 0 {
                None
            } else {
                Some(i as f64 / 1024.0)
            },
        );
    }
    manual(&c, "manual", "model-a", HOUR_MS + 1, false);
    manual(&c, "cleared", "model-a", HOUR_MS + 2, true);
    c.execute("INSERT INTO agent_deleted_usage_daily_rollups(usage_day,model_id,model_name,request_count,message_count,input_tokens,total_tokens,estimated_cost,created_at,updated_at) VALUES(86400000,'deleted','Deleted',3,2,11,11,0.25,86400001,86400001)",[]).unwrap();
    for (from, width, count) in [
        (0, HOUR_MS, 31),
        (0, 86400000, 7),
        (16 * HOUR_MS, 86400000, 4),
        (11 * HOUR_MS + HOUR_MS / 2, 86400000, 4),
        (HOUR_MS - 1, HOUR_MS / 2, 31),
        (1, 86400000, 7),
    ] {
        differential(&c, &input(from, width, count));
    }
    c.execute("INSERT INTO models(id,provider_model_id,display_name,normalized_display_name,supports_image,provider_connection_revision,provider_protocol_revision,provider_profile_config_json,input_price,output_price,enabled,position,created_at,updated_at) VALUES('model-a','model-a','Current label','current label',0,'provider-connection-v1:test','provider-protocol-v1:test','{}','99','99',1,0,0,0)",[]).unwrap();
    differential(&c, &input(0, 86400000, 7));
    c.execute(
        "UPDATE models SET display_name='Renamed label' WHERE id='model-a'",
        [],
    )
    .unwrap();
    differential(&c, &input(0, 86400000, 7));
    c.execute("DELETE FROM models", []).unwrap();
    differential(&c, &input(0, 86400000, 7));
}

#[test]
fn dashboard_cumulative_updates_move_hours_identity_and_nullable_counts() {
    let c = connection();
    add(&c, "first", "m", "A", 10, Some(3), Some(0.2));
    add(&c, "second", "m", "Z", 11, None, None);
    add(&c, "third", "m", "A", 9, Some(0), Some(0.0));
    for sql in [
        "UPDATE agent_usage_records SET input_tokens=0,total_tokens=0,estimated_cost=0 WHERE id='first'",
        "UPDATE agent_usage_records SET input_tokens=NULL,total_tokens=NULL,estimated_cost=NULL WHERE id='first'",
        "UPDATE agent_usage_records SET created_at=86400003,model_id='other',model_name='Changed',billable_request_count=5,input_tokens=100,total_tokens=110,output_tokens=10,cached_input_tokens=7,cache_creation_input_tokens=2,output_thinking_tokens=4,estimated_cost=0.5 WHERE id='first'",
        "UPDATE agent_usage_records SET created_at=1 WHERE id='second'",
        "DELETE FROM agent_usage_records WHERE id='third'",
    ] {
        c.execute(sql,[]).unwrap(); differential(&c,&input(0,86400000,2));
    }
}

#[test]
fn dashboard_noop_and_status_changes_do_not_rewrite_projection() {
    let c = connection();
    add(&c, "one", "m", "A", 1, Some(1), Some(0.1));
    c.execute_batch("CREATE TEMP TRIGGER reject_hourly_update BEFORE UPDATE ON agent_usage_hourly_rollups BEGIN SELECT RAISE(ABORT,'Unexpected projection write'); END;").unwrap();
    c.execute("UPDATE agent_usage_records SET status='completed',error=NULL,input_tokens=input_tokens,estimated_cost=estimated_cost,created_at=created_at",[]).unwrap();
    differential(&c, &input(0, HOUR_MS, 1));
}

#[test]
fn dashboard_cost_is_exact_zero_when_only_known_zero_rows_remain() {
    for (first, second) in [(0.1, 0.2), (0.3, 0.6)] {
        let c = connection();
        add(&c, "a", "m", "A", 1, Some(0), Some(first));
        add(&c, "b", "m", "A", 2, Some(0), Some(second));
        add(&c, "zero", "m", "A", 3, Some(0), Some(0.0));
        add(&c, "unknown", "m", "A", 4, None, None);
        c.execute("DELETE FROM agent_usage_records WHERE id='b'", [])
            .unwrap();
        c.execute("DELETE FROM agent_usage_records WHERE id='a'", [])
            .unwrap();
        let result = usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1)).unwrap();
        assert_eq!(result.summary.estimated_cost, Some(0.0));
        c.execute("DELETE FROM agent_usage_records WHERE id='zero'", [])
            .unwrap();
        let result = usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1)).unwrap();
        assert_eq!(result.summary.estimated_cost, None);
    }
}

#[test]
fn dashboard_manual_clear_is_scoped_and_all_ledger_clearing_rolls_back_atomically() {
    let c = connection();
    add(&c, "normal", "a", "A", 1, Some(3), Some(1.0));
    add(&c, "later", "b", "B", 2 * HOUR_MS + 1, Some(5), None);
    manual(&c, "manual", "a", 2, false);
    manual(&c, "manual-later", "b", 3 * HOUR_MS, false);
    c.execute("INSERT INTO agent_deleted_usage_daily_rollups(usage_day,model_id,model_name,created_at,updated_at) VALUES(0,'old','Old',0,0)",[]).unwrap();
    c.execute_batch("CREATE TEMP TRIGGER reject_deleted_clear BEFORE DELETE ON agent_deleted_usage_daily_rollups BEGIN SELECT RAISE(ABORT,'rollback test'); END;").unwrap();
    let before = old(&c, 0, 5 * HOUR_MS - 1);
    assert!(usage_repository::clear_usage_records(
        &c,
        &AgentUsageClearInput {
            from: None,
            to: None
        }
    )
    .is_err());
    equal(&old(&c, 0, 5 * HOUR_MS - 1), &before);
    differential(&c, &input(0, HOUR_MS, 5));
    c.execute_batch("DROP TRIGGER reject_deleted_clear; BEGIN;")
        .unwrap();
    usage_repository::clear_usage_records(
        &c,
        &AgentUsageClearInput {
            from: Some(0),
            to: Some(HOUR_MS - 1),
        },
    )
    .unwrap();
    assert_eq!(c.query_row("SELECT last_observed_at FROM agent_usage_hourly_rollups WHERE model_id='b' AND model_name='B'",[],|r|r.get::<_,i64>(0)).unwrap(),2*HOUR_MS+1);
    assert_eq!(c.query_row("SELECT cleared_at FROM manual_context_compaction_usage_records WHERE operation_id='manual'",[],|r|r.get::<_,i64>(0)).unwrap(),2);
    c.execute_batch("ROLLBACK").unwrap();
    equal(&old(&c, 0, 5 * HOUR_MS - 1), &before);
    usage_repository::clear_usage_records(
        &c,
        &AgentUsageClearInput {
            from: Some(0),
            to: Some(HOUR_MS - 1),
        },
    )
    .unwrap();
    differential(&c, &input(0, HOUR_MS, 5));
}

#[test]
fn dashboard_conversation_deletion_preserves_usage_and_removes_live_rollups() {
    let c = connection();
    add(&c, "one", "m", "A", 1, Some(3), Some(0.5));
    manual(&c, "manual", "m", 2, false);
    let before = old(&c, 0, HOUR_MS - 1);
    let transaction = c.unchecked_transaction().unwrap();
    usage_repository::roll_up_deleted_usage_for_conversation(&transaction, "chat", 3).unwrap();
    transaction
        .execute("DELETE FROM conversations WHERE id='chat'", [])
        .unwrap();
    transaction.commit().unwrap();
    equal(&old(&c, 0, HOUR_MS - 1), &before);
    differential(&c, &input(0, HOUR_MS, 1));
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM agent_usage_hourly_rollups", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn dashboard_full_hours_do_not_read_either_raw_ledger() {
    let c = connection();
    add(&c, "one", "m", "A", 1, Some(3), Some(0.5));
    manual(&c, "manual", "m", 2, false);
    c.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Read {
            table_name: "agent_usage_records" | "manual_context_compaction_usage_records",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    assert!(usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1)).is_ok());
    assert!(usage_repository::usage_dashboard(&c, &input(1, HOUR_MS, 1)).is_err());
}

#[test]
fn dashboard_overflow_never_rejects_ledger_writes_or_approximates_tokens() {
    let c = connection();
    add(&c, "max", "m", "A", 1, Some(i64::MAX), None);
    add(&c, "extra", "m", "A", 2, Some(1), None);
    assert_eq!(
        c.query_row(
            "SELECT needs_raw FROM agent_usage_hourly_rollups",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    assert!(usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1)).is_err());
    assert!(usage_repository::usage_summary(
        &c,
        &AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::Custom,
            from: Some(0),
            to: Some(HOUR_MS - 1)
        },
        HOUR_MS
    )
    .is_err());
    c.execute("DELETE FROM agent_usage_records WHERE id='extra'", [])
        .unwrap();
    differential(&c, &input(0, HOUR_MS, 1));
    c.execute("DELETE FROM agent_usage_records", []).unwrap();
    assert_eq!(
        c.query_row("SELECT COUNT(*) FROM agent_usage_hourly_rollups", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn dashboard_validation_bounds_before_reading_database() {
    for request in [
        input(0, 1, 0),
        input(0, 1, 32),
        input(-1, 1, 1),
        input(MAX_SAFE_INTEGER, 2, 1),
        input(0, MAX_SPAN_MS + 1, 1),
        AgentUsageDashboardInput {
            windows: vec![
                AgentUsageWindow { from: 0, to: 1 },
                AgentUsageWindow { from: 3, to: 4 },
            ],
        },
        AgentUsageDashboardInput {
            windows: vec![
                AgentUsageWindow { from: 0, to: 1 },
                AgentUsageWindow { from: 1, to: 2 },
            ],
        },
    ] {
        assert!(validate(&request).is_err());
    }
    assert!(validate(&input(MAX_SAFE_INTEGER, 1, 1)).is_ok());
    assert!(validate(&input(0, MAX_SPAN_MS, 1)).is_ok());
}

#[test]
fn dashboard_latest_name_uses_binary_tie_and_reveals_previous_after_removal() {
    let c = connection();
    add(&c, "earlier", "m", "Earlier", 4, Some(1), None);
    add(&c, "latest-a", "m", "Alpha", 5, Some(1), None);
    add(&c, "latest-z", "m", "Zulu", 5, Some(1), None);
    let request = input(0, HOUR_MS, 1);
    let dashboard = usage_repository::usage_dashboard(&c, &request).unwrap();
    assert_eq!(dashboard.summary.models[0].model_name, "Zulu");
    for id in ["latest-z", "latest-a"] {
        c.execute("DELETE FROM agent_usage_records WHERE id=?1", [id])
            .unwrap();
        differential(&c, &request);
    }
    assert_eq!(
        usage_repository::usage_dashboard(&c, &request)
            .unwrap()
            .summary
            .models[0]
            .model_name,
        "Earlier"
    );
}

#[test]
fn dashboard_reuses_callers_snapshot_without_committing_it() {
    let c = connection();
    c.execute_batch("BEGIN").unwrap();
    add(&c, "uncommitted", "m", "A", 1, Some(1), Some(0.0));
    differential(&c, &input(0, HOUR_MS, 1));
    assert!(!c.is_autocommit());
    c.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1))
            .unwrap()
            .summary
            .request_count,
        0
    );
}

#[test]
fn dashboard_severe_cost_cancellation_falls_back_without_changing_original_ledger() {
    for remaining in [1.0, 40_000.0] {
        let c = connection();
        add(&c, "large", "m", "A", 1, Some(0), Some(1e20));
        add(&c, "small", "m", "A", 2, Some(0), Some(remaining));
        c.execute("DELETE FROM agent_usage_records WHERE id='large'", [])
            .unwrap();
        assert_eq!(
            c.query_row(
                "SELECT needs_raw FROM agent_usage_hourly_rollups",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let dashboard = usage_repository::usage_dashboard(&c, &input(0, HOUR_MS, 1)).unwrap();
        assert_eq!(dashboard.summary.estimated_cost, Some(remaining));
        assert_eq!(dashboard.buckets[0].estimated_cost, Some(remaining));
        assert_eq!(
            c.query_row(
                "SELECT estimated_cost FROM agent_usage_records WHERE id='small'",
                [],
                |row| row.get::<_, f64>(0)
            )
            .unwrap(),
            remaining
        );
        differential(&c, &input(0, HOUR_MS, 1));
    }
}
