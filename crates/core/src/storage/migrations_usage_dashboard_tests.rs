use super::*;
use crate::storage::usage_repository;
use crate::{AgentUsageDashboardInput, AgentUsageWindow};
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::{params, types::Value};

const HOUR: i64 = 3_600_000;

fn v70() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(&canonical_schema_v70()).unwrap();
    c.pragma_update(None, "user_version", 70).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    validate_schema_fingerprint(&c, V70_SCHEMA_FINGERPRINT).unwrap();
    c.execute("INSERT INTO conversations(id,title,created_at,updated_at) VALUES('billing-chat','Keep history',0,0)", []).unwrap();
    for (id, at, tokens) in [
        ("max", 1, i64::MAX),
        ("extra", 2, 1),
        ("ordinary", HOUR + 5, 3),
    ] {
        c.execute("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES(?1,'billing-chat','assistant','Keep message','sent',?2,0)", params![id, at]).unwrap();
        c.execute("INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,model_name,created_at,billable_request_count,input_tokens,total_tokens,estimated_cost) VALUES(?1,'billing-chat',?1,?1,'model','Historical name',?2,1,?3,?3,0.25)", params![id, at, tokens]).unwrap();
    }
    for (id, at, cleared) in [
        ("manual-active", HOUR + 6, None),
        ("manual-cleared", HOUR + 7, Some(HOUR + 8)),
    ] {
        c.execute("INSERT INTO manual_context_compaction_operations(operation_id,request_id,conversation_id,status,phase,operation_json,started_at,updated_at) VALUES(?1,?1,'billing-chat','completed','committing','{}',?2,?2)", params![id, at]).unwrap();
        c.execute("INSERT INTO manual_context_compaction_usage_records(operation_id,conversation_id,model_id,model_name,created_at,billable_request_count,input_tokens,total_tokens,estimated_cost,record_json,cleared_at) VALUES(?1,'billing-chat','model','Manual name',?2,2,7,7,0.125,'{}',?3)", params![id, at, cleared]).unwrap();
    }
    c.execute("INSERT INTO agent_deleted_usage_daily_rollups(usage_day,model_id,model_name,request_count,message_count,input_tokens,total_tokens,estimated_cost,created_at,updated_at) VALUES(86400000,'deleted','Deleted history',3,2,11,11,0.25,86400000,86400001)", []).unwrap();
    c
}

fn rows(c: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut query = c.prepare(sql).unwrap();
    let columns = query.column_count();
    let result = query
        .query_map([], |row| {
            (0..columns)
                .map(|index| row.get(index))
                .collect::<rusqlite::Result<Vec<Value>>>()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    result
}

fn ledgers(c: &Connection) -> Vec<Vec<Vec<Value>>> {
    [
        "SELECT * FROM conversations ORDER BY id",
        "SELECT * FROM messages ORDER BY id",
        "SELECT * FROM agent_usage_records ORDER BY id",
        "SELECT * FROM manual_context_compaction_operations ORDER BY operation_id",
        "SELECT * FROM manual_context_compaction_usage_records ORDER BY operation_id",
        "SELECT * FROM agent_deleted_usage_daily_rollups ORDER BY usage_day,model_id,model_name",
    ]
    .into_iter()
    .map(|sql| rows(c, sql))
    .collect()
}

fn dashboard(c: &Connection, from: i64, to: i64) -> crate::AgentUsageDashboardOutput {
    usage_repository::usage_dashboard(
        c,
        &AgentUsageDashboardInput {
            windows: vec![AgentUsageWindow { from, to }],
        },
    )
    .unwrap()
}

#[test]
fn usage_dashboard_v71_backfills_populated_v70_without_changing_ledgers() {
    let c = v70();
    let before = ledgers(&c);
    run_migrations(&c).unwrap();
    assert_eq!(read_schema_version(&c).unwrap(), STORAGE_SCHEMA_VERSION);
    validate_canonical_schema(&c).unwrap();
    assert_eq!(ledgers(&c), before);

    // Migration must not fail or round a legacy hour whose integer sum overflows.
    assert_eq!(
        c.query_row(
            "SELECT needs_raw FROM agent_usage_hourly_rollups WHERE usage_hour=0",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let exact = dashboard(&c, 1, 1);
    assert_eq!(exact.summary.input_tokens, Some(i64::MAX as u64));
    assert_eq!(exact.summary.request_count, 1);
    assert!(
        usage_repository::usage_dashboard(
            &c,
            &AgentUsageDashboardInput {
                windows: vec![AgentUsageWindow {
                    from: 0,
                    to: HOUR - 1
                }],
            }
        )
        .is_err(),
        "an overflowing sum must remain an error, never a rounded total"
    );

    let normal = dashboard(&c, HOUR, 2 * HOUR - 1);
    assert_eq!(normal.summary.request_count, 3);
    assert_eq!(normal.summary.message_count, 1);
    assert_eq!(normal.summary.input_tokens, Some(10));
    assert_eq!(normal.summary.estimated_cost, Some(0.375));
    let deleted = dashboard(&c, 86_400_000, 86_400_000 + HOUR - 1);
    assert_eq!(deleted.summary.request_count, 3);
    assert_eq!(deleted.summary.input_tokens, Some(11));

    let projection = rows(
        &c,
        "SELECT * FROM agent_usage_hourly_rollups ORDER BY usage_hour,model_id,model_name",
    );
    let changes = c.total_changes();
    run_migrations(&c).unwrap();
    assert_eq!(c.total_changes(), changes);
    assert_eq!(ledgers(&c), before);
    assert_eq!(
        rows(
            &c,
            "SELECT * FROM agent_usage_hourly_rollups ORDER BY usage_hour,model_id,model_name"
        ),
        projection
    );
}

#[test]
fn usage_dashboard_v71_rejects_tampered_v70_without_any_mutation() {
    let c = v70();
    c.execute_batch("CREATE TABLE unexpected_billing_schema(id TEXT PRIMARY KEY);")
        .unwrap();
    let before = ledgers(&c);
    let fingerprint = schema_fingerprint(&c).unwrap();
    let changes = c.total_changes();
    let error = run_migrations(&c).unwrap_err();
    assert!(error.to_string().contains("fingerprint mismatch"));
    assert_eq!(read_schema_version(&c).unwrap(), 70);
    assert_eq!(schema_fingerprint(&c).unwrap(), fingerprint);
    assert_eq!(c.total_changes(), changes);
    assert_eq!(ledgers(&c), before);
    assert!(c.is_autocommit());
}

#[test]
fn usage_dashboard_v71_mid_migration_failure_rolls_back_backfill_and_schema() {
    let c = v70();
    let before = ledgers(&c);
    let changes = c.total_changes();
    // This trigger is created after the new table, indexes, backfill and ordinary triggers.
    c.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::CreateTrigger {
            trigger_name: "manual_usage_hourly_insert",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    assert!(run_migrations(&c).is_err());
    c.authorizer(None::<fn(AuthContext<'_>) -> Authorization>);
    // SQLite counts rolled-back changes, proving the failure occurred after backfill writes.
    assert!(c.total_changes() > changes);
    assert_eq!(read_schema_version(&c).unwrap(), 70);
    validate_schema_fingerprint(&c, V70_SCHEMA_FINGERPRINT).unwrap();
    assert_eq!(ledgers(&c), before);
    assert!(c.is_autocommit());
    assert_eq!(c.query_row("SELECT COUNT(*) FROM sqlite_schema WHERE name='agent_usage_hourly_rollups' OR name='idx_agent_usage_hour_identity_time' OR name='agent_usage_hourly_insert'", [], |r| r.get::<_, i64>(0)).unwrap(), 0);
    run_migrations(&c).unwrap();
    validate_canonical_schema(&c).unwrap();
    assert_eq!(ledgers(&c), before);
    assert_eq!(dashboard(&c, HOUR, 2 * HOUR - 1).summary.request_count, 3);
}

#[test]
fn usage_dashboard_v71_fresh_schema_and_repeated_open_are_idempotent() {
    let c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    validate_canonical_schema(&c).unwrap();
    let before = schema_fingerprint(&c).unwrap();
    let changes = c.total_changes();
    run_migrations(&c).unwrap();
    assert_eq!(read_schema_version(&c).unwrap(), STORAGE_SCHEMA_VERSION);
    assert_eq!(schema_fingerprint(&c).unwrap(), before);
    assert_eq!(c.total_changes(), changes);
    let empty = dashboard(&c, 0, HOUR - 1);
    assert_eq!(empty.summary.request_count, 0);
    assert_eq!(empty.summary.message_count, 0);
    assert_eq!(empty.summary.input_tokens, None);
    assert_eq!(empty.summary.estimated_cost, None);
    assert_eq!(empty.buckets.len(), 1);
}
