//! Query-plan regressions and an opt-in, synthetic benchmark using the bundled SQLite.
use super::*;
use crate::storage::{conversation_history_archive_repository, migrations};
use rusqlite::types::Value as SqlValue;
use std::time::Instant;

const LEGACY_REFERENCE_ORDER_SQL: &str = "SELECT position, within_message_order
    FROM conversation_history_fts
    WHERE conversation_id = ?1 AND ref_key = ?2
      AND NOT EXISTS (
          SELECT 1 FROM conversation_turn_rewrites AS rewrite
          WHERE rewrite.conversation_id = conversation_history_fts.conversation_id
            AND (rewrite.source_user_message_id = conversation_history_fts.message_id
              OR rewrite.source_assistant_message_id = conversation_history_fts.message_id
              OR rewrite.source_assistant_message_id = conversation_history_fts.assistant_message_id)
      )";

fn legacy_timeline_query(
    conversation_id: &str,
    start: Option<(i64, i64)>,
    end: Option<(i64, i64)>,
    limit: usize,
    backwards: bool,
) -> (String, Vec<SqlValue>) {
    let mut sql = String::from(
        "SELECT record_type, item_kind, message_id, assistant_message_id, sequence,
                archive_ref, tool, status, run_id, created_at,
                substr(content, 1, 321), length(content)
         FROM conversation_history_fts
         WHERE conversation_id = ? AND record_type != 'archive'
           AND NOT EXISTS (
               SELECT 1 FROM conversation_turn_rewrites AS rewrite
               WHERE rewrite.conversation_id = conversation_history_fts.conversation_id
                 AND (rewrite.source_user_message_id = conversation_history_fts.message_id
                   OR rewrite.source_assistant_message_id = conversation_history_fts.message_id
                   OR rewrite.source_assistant_message_id = conversation_history_fts.assistant_message_id)
           )",
    );
    let mut values = vec![SqlValue::from(conversation_id.to_string())];
    if let Some((position, within)) = start {
        sql.push_str(if backwards {
            " AND (position < ? OR (position = ? AND within_message_order < ?))"
        } else {
            " AND (position > ? OR (position = ? AND within_message_order >= ?))"
        });
        values.extend([position.into(), position.into(), within.into()]);
    }
    if let Some((position, within)) = end {
        sql.push_str(" AND (position < ? OR (position = ? AND within_message_order <= ?))");
        values.extend([position.into(), position.into(), within.into()]);
    }
    sql.push_str(if backwards {
        " ORDER BY position DESC, within_message_order DESC LIMIT ?"
    } else {
        " ORDER BY position ASC, within_message_order ASC LIMIT ?"
    });
    values.push(i64::try_from(limit).unwrap_or(i64::MAX).into());
    (sql, values)
}

fn raw_rows(connection: &Connection, sql: &str, values: &[SqlValue]) -> Vec<Vec<SqlValue>> {
    let mut statement = connection.prepare(sql).unwrap();
    let column_count = statement.column_count();
    statement
        .query_map(params_from_iter(values), |row| {
            (0..column_count).map(|column| row.get(column)).collect()
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn explain(connection: &Connection, sql: &str, values: &[SqlValue]) -> Vec<String> {
    connection
        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
        .unwrap()
        .query_map(params_from_iter(values), |row| row.get(3))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

#[test]
fn identity_and_timeline_use_indexed_candidates_before_reading_fts() {
    let connection = super::tests::setup();
    let values = vec![
        "conversation-1".to_string().into(),
        "message:user-1".to_string().into(),
    ];
    let identity = explain(&connection, REFERENCE_ORDER_SQL, &values).join("\n");
    assert!(identity.contains("ref_key=?"), "{identity}");
    assert!(identity.contains("VIRTUAL TABLE INDEX 0:="), "{identity}");
    let (sql, values) = timeline_query("conversation-1", Some((1, 0)), None, 10, false);
    let timeline = explain(&connection, &sql, &values).join("\n");
    let order_index = timeline
        .find("conversation_history_timeline_order")
        .unwrap();
    let fts = timeline.find("history VIRTUAL TABLE INDEX 0:=").unwrap();
    assert!(order_index < fts, "{timeline}");
    assert!(!timeline.contains("TEMP B-TREE"), "{timeline}");
}

#[test]
fn timeline_matches_legacy_for_boundaries_directions_and_limits() {
    let connection = super::tests::setup();
    // Trace entries and their assistant message share position 1; the assistant sorts last.
    let boundaries = [
        None,
        Some((-1, 0)),
        Some((0, 0)),
        Some((1, 0)),
        Some((1, 1)),
        Some((1, 2)),
        Some((1, i64::MAX)),
        Some((99, 0)),
    ];
    for conversation in ["conversation-1", "conversation-2", "missing"] {
        for start in boundaries {
            for end in boundaries {
                for backwards in [false, true] {
                    for limit in [0, 1, 2, 100] {
                        let (old_sql, values) =
                            legacy_timeline_query(conversation, start, end, limit, backwards);
                        let (new_sql, new_values) =
                            timeline_query(conversation, start, end, limit, backwards);
                        assert_eq!(raw_rows(&connection, &old_sql, &values), raw_rows(&connection, &new_sql, &new_values),
                            "conversation={conversation}, start={start:?}, end={end:?}, backwards={backwards}, limit={limit}");
                    }
                }
            }
        }
    }
    let first = ConversationHistoryRecordRef::Message {
        message_id: "user-1".into(),
    };
    let last = ConversationHistoryRecordRef::Message {
        message_id: "assistant-1".into(),
    };
    assert_eq!(
        records_in_range(&connection, "conversation-1", &first, &last, 100).unwrap(),
        records_in_range(&connection, "conversation-1", &last, &first, 100).unwrap()
    );
    for message_id in ["missing", "other-user"] {
        let reference = ConversationHistoryRecordRef::Message {
            message_id: message_id.into(),
        };
        assert!(reference_order(&connection, "conversation-1", &reference)
            .unwrap()
            .is_none());
        assert!(
            records_around(&connection, "conversation-1", &reference, 2, 2)
                .unwrap()
                .is_none()
        );
    }
}

#[test]
fn equal_position_records_preserve_legacy_tie_order_in_both_directions() {
    let connection = super::tests::setup();
    connection.execute_batch(
        "INSERT INTO messages(id, conversation_id, role, content, status, created_at, position) VALUES
         ('same-position-first', 'conversation-1', 'user', 'first tie', 'sent', 4000, 2),
         ('same-position-second', 'conversation-1', 'user', 'second tie', 'sent', 4001, 2);"
    ).unwrap();
    for backwards in [false, true] {
        for limit in [1, 2, 3, 100] {
            let (old_sql, old_values) =
                legacy_timeline_query("conversation-1", None, None, limit, backwards);
            let (new_sql, new_values) =
                timeline_query("conversation-1", None, None, limit, backwards);
            assert_eq!(
                raw_rows(&connection, &old_sql, &old_values),
                raw_rows(&connection, &new_sql, &new_values)
            );
        }
    }
}

#[test]
fn indexed_candidates_preserve_trace_order_after_message_reordering_and_archiving() {
    let connection = super::tests::setup();
    connection
        .execute(
            "UPDATE messages SET position = 11 WHERE id = 'assistant-1'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "UPDATE conversations SET archived_at = 6000 WHERE id = 'conversation-1'",
            [],
        )
        .unwrap();
    for start in [None, Some((1, 1)), Some((10, 0))] {
        let (old_sql, values) = legacy_timeline_query("conversation-1", start, None, 10, false);
        let (new_sql, new_values) = timeline_query("conversation-1", start, None, 10, false);
        assert_eq!(
            raw_rows(&connection, &old_sql, &values),
            raw_rows(&connection, &new_sql, &new_values)
        );
    }
    assert_eq!(
        reference_order(
            &connection,
            "conversation-1",
            &ConversationHistoryRecordRef::TraceItem {
                assistant_message_id: "assistant-1".into(),
                sequence: 1
            }
        )
        .unwrap(),
        Some((1, 2))
    );
    assert_eq!(
        reference_order(
            &connection,
            "conversation-1",
            &ConversationHistoryRecordRef::Message {
                message_id: "assistant-1".into()
            }
        )
        .unwrap(),
        Some((11, i64::MAX))
    );
}

fn add_archive(connection: &mut Connection) -> ConversationHistoryRecordRef {
    let archive = conversation_history_archive_repository::store_archive(
        connection,
        &conversation_history_archive_repository::ConversationHistoryArchiveInput {
            conversation_id: "conversation-1".into(),
            assistant_message_id: "assistant-1".into(),
            sequence: 1,
            call_id: "call-1".into(),
            tool: "read_file".into(),
            content_type: "text/plain".into(),
            content: "archive-only needle searchable exact content".into(),
            truncated_at_source: false,
            model_projection_truncated: false,
            archive_projection_truncated: false,
            created_at: 2_000,
        },
    )
    .unwrap();
    ConversationHistoryRecordRef::Archive {
        archive_ref: archive.archive_ref,
    }
}

#[test]
fn archives_remain_searchable_without_duplicate_timeline_rows_and_delete_cleanly() {
    let mut connection = super::tests::setup();
    let archive = add_archive(&mut connection);
    assert!(reference_order(&connection, "conversation-1", &archive)
        .unwrap()
        .is_some());
    assert!(reference_order(&connection, "conversation-2", &archive)
        .unwrap()
        .is_none());
    let records = query_timeline(&connection, "conversation-1", None, None, 100, false).unwrap();
    assert_eq!(records.len(), 4);
    assert!(records.iter().all(|record| record.record_type != "archive"));
    let filter = ConversationHistorySearchFilter {
        include_archives: true,
        ..Default::default()
    };
    let hits = search_records(
        &connection,
        "conversation-1",
        "archive-only needle",
        &filter,
        10,
    )
    .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].reference, archive);
    connection
        .execute("DELETE FROM conversations WHERE id = 'conversation-1'", [])
        .unwrap();
    assert!(reference_order(&connection, "conversation-1", &archive)
        .unwrap()
        .is_none());
    assert!(
        query_timeline(&connection, "conversation-1", None, None, 100, false)
            .unwrap()
            .is_empty()
    );
    assert!(search_records(
        &connection,
        "conversation-1",
        "archive-only needle",
        &filter,
        10
    )
    .unwrap()
    .is_empty());
    assert_eq!(
        query_timeline(&connection, "conversation-2", None, None, 100, false)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn rewritten_messages_traces_and_archives_stay_hidden() {
    use crate::storage::{conversation_trace_repository, conversation_turn_rewrite_repository};
    let mut connection = super::tests::setup();
    let archive = add_archive(&mut connection);
    connection.execute_batch(
        "INSERT INTO messages(id, conversation_id, role, content, status, created_at, position) VALUES
         ('replacement-user', 'conversation-1', 'user', 'replacement request', 'sent', 4000, 2),
         ('replacement-assistant', 'conversation-1', 'assistant', '', 'pending', 4001, 3);"
    ).unwrap();
    let trace = crate::ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "replacement-run".into(),
        conversation_id: "conversation-1".into(),
        assistant_message_id: "replacement-assistant".into(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![],
    };
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 4001, 4001)
        .unwrap();
    conversation_turn_rewrite_repository::insert_in_transaction(
        &connection,
        &conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission {
            request_id: "rewrite".into(),
            request_fingerprint: format!("sha256:{}", "0".repeat(64)),
            conversation_id: "conversation-1".into(),
            source_user_message_id: "user-1".into(),
            source_assistant_message_id: "assistant-1".into(),
            replacement_user_message_id: "replacement-user".into(),
            replacement_assistant_message_id: "replacement-assistant".into(),
            run_id: "replacement-run".into(),
            response_json: "{}".into(),
            created_at: 4001,
        },
    )
    .unwrap();
    for reference in [
        ConversationHistoryRecordRef::Message {
            message_id: "user-1".into(),
        },
        ConversationHistoryRecordRef::Message {
            message_id: "assistant-1".into(),
        },
        ConversationHistoryRecordRef::TraceItem {
            assistant_message_id: "assistant-1".into(),
            sequence: 1,
        },
        archive,
    ] {
        assert!(reference_order(&connection, "conversation-1", &reference)
            .unwrap()
            .is_none());
        let values = vec![
            "conversation-1".to_string().into(),
            reference_key(&reference).into(),
        ];
        assert_eq!(
            raw_rows(&connection, LEGACY_REFERENCE_ORDER_SQL, &values),
            raw_rows(&connection, REFERENCE_ORDER_SQL, &values)
        );
    }
    let (old_sql, values) = legacy_timeline_query("conversation-1", None, None, 100, false);
    let (new_sql, new_values) = timeline_query("conversation-1", None, None, 100, false);
    assert_eq!(
        raw_rows(&connection, &old_sql, &values),
        raw_rows(&connection, &new_sql, &new_values)
    );
    let visible = query_timeline(&connection, "conversation-1", None, None, 100, false).unwrap();
    assert_eq!(visible.len(), 2);
    assert_eq!(
        visible[0].reference,
        ConversationHistoryRecordRef::Message {
            message_id: "replacement-user".into()
        }
    );
    let filter = ConversationHistorySearchFilter {
        include_messages: true,
        include_trace_items: true,
        include_archives: true,
        ..Default::default()
    };
    for query in ["first exact request", "v1-exact", "archive-only needle"] {
        assert!(
            search_records(&connection, "conversation-1", query, &filter, 10)
                .unwrap()
                .is_empty()
        );
    }
}

fn seed_benchmark(connection: &mut Connection, total: usize, target: usize) {
    migrations::run_migrations(connection).unwrap();
    let transaction = connection.transaction().unwrap();
    transaction
        .execute_batch(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES
         ('target', 'Target', 1, 1), ('unrelated', 'Unrelated', 1, 1);",
        )
        .unwrap();
    transaction.execute(
        "WITH RECURSIVE items(n) AS (VALUES(0) UNION ALL SELECT n + 1 FROM items WHERE n + 1 < ?1)
         INSERT INTO messages(id, conversation_id, role, content, status, created_at, position)
         SELECT 'benchmark-' || n, CASE WHEN n < ?2 THEN 'target' ELSE 'unrelated' END,
                'user', 'Synthetic searchable content ' || n, 'sent', 1000 + n, n
         FROM items",
        params![total as i64, target as i64],
    ).unwrap();
    transaction.commit().unwrap();
}

fn seed_long_turn_benchmark(connection: &mut Connection, total: usize, trace_items: usize) {
    seed_benchmark(connection, total - trace_items - 1, 0);
    let transaction = connection.transaction().unwrap();
    transaction.execute_batch(
        "INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
         VALUES ('long-assistant','target','assistant','Long completed turn','sent',1000,0);
         INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at,completed_at)
         VALUES ('long-assistant','target','long-run',6,'completed',0,1000,1001,1001);"
    ).unwrap();
    transaction.execute(
        "WITH RECURSIVE items(n) AS (VALUES(0) UNION ALL SELECT n + 1 FROM items WHERE n + 1 < ?1)
         INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
         SELECT 'long-assistant',n,'backend_state',json_object('type','backend_state','sequence',n,'status','completed') FROM items",
        [trace_items as i64]
    ).unwrap();
    transaction.commit().unwrap();
}

#[test]
fn page_query_work_stays_bounded_when_unrelated_history_grows() {
    use rusqlite::StatementStatus;
    let mut measured = Vec::new();
    for total in [100, 10_000] {
        let mut connection = Connection::open_in_memory().unwrap();
        seed_benchmark(&mut connection, total, 100);
        let (sql, values) = timeline_query("target", Some((50, 0)), None, 20, false);
        let mut statement = connection.prepare(&sql).unwrap();
        let mut rows = statement.query(params_from_iter(&values)).unwrap();
        let mut count = 0;
        while rows.next().unwrap().is_some() {
            count += 1;
        }
        drop(rows);
        assert_eq!(count, 20);
        measured.push(statement.get_status(StatementStatus::VmStep));
    }
    assert!(
        measured[1] <= measured[0] + 20,
        "page work grew with unrelated rows: {measured:?}"
    );
}

fn benchmark_case(
    connection: &Connection,
    storage: &str,
    total: usize,
    target: usize,
    reopen_cache: bool,
    long_turn: bool,
) {
    let cursor = if long_turn {
        (0, (target / 2 + 1) as i64)
    } else {
        ((target / 2) as i64, 0)
    };
    let identity_values = vec![
        "target".to_string().into(),
        if long_turn {
            format!("trace:long-assistant:{}", target / 2)
        } else {
            format!("message:benchmark-{}", target / 2)
        }
        .into(),
    ];
    let (old_timeline, timeline_values) =
        legacy_timeline_query("target", Some(cursor), None, 20, false);
    let (new_timeline, new_timeline_values) =
        timeline_query("target", Some(cursor), None, 20, false);
    for (name, old, new, old_values, new_values) in [
        (
            "reference",
            LEGACY_REFERENCE_ORDER_SQL,
            REFERENCE_ORDER_SQL,
            &identity_values,
            &identity_values,
        ),
        (
            "timeline",
            old_timeline.as_str(),
            new_timeline.as_str(),
            &timeline_values,
            &new_timeline_values,
        ),
    ] {
        assert_eq!(
            raw_rows(connection, old, old_values),
            raw_rows(connection, new, new_values)
        );
        if total == 10_000 && !reopen_cache {
            println!(
                "PLAN {name} legacy: {:?}",
                explain(connection, old, old_values)
            );
            println!(
                "PLAN {name} indexed: {:?}",
                explain(connection, new, new_values)
            );
        }
        let mut before = Vec::new();
        let mut after = Vec::new();
        for iteration in 0..31 {
            // Alternate order. shrink_memory evicts SQLite's page cache, not the OS cache.
            for (sql, values, elapsed) in if iteration % 2 == 0 {
                [
                    (old, old_values, &mut before),
                    (new, new_values, &mut after),
                ]
            } else {
                [
                    (new, new_values, &mut after),
                    (old, old_values, &mut before),
                ]
            } {
                if reopen_cache {
                    connection.execute_batch("PRAGMA shrink_memory;").unwrap();
                }
                let started = Instant::now();
                std::hint::black_box(raw_rows(connection, sql, values));
                elapsed.push(started.elapsed().as_secs_f64() * 1_000.0);
            }
        }
        before.sort_by(f64::total_cmp);
        after.sort_by(f64::total_cmp);
        println!("{storage} total={total} target={target} long_turn={long_turn} {name}: legacy median={:.3}ms p95={:.3}ms; indexed median={:.3}ms p95={:.3}ms",
            before[15], before[29], after[15], after[29]);
    }
}

/// Reproduce with:
/// MYCOPILOT_BENCH_HISTORY=1 cargo test -p mycopilot-core benchmark_indexed_history_queries -- --nocapture
/// Uses only synthetic temporary databases; reported timings include prepare and row decoding.
#[test]
fn benchmark_indexed_history_queries() {
    if std::env::var("MYCOPILOT_BENCH_HISTORY").as_deref() != Ok("1") {
        return;
    }
    println!(
        "bundled SQLite {} / {}",
        rusqlite::version(),
        std::env::consts::ARCH
    );
    for (total, target, long_turn) in [
        (10_000, 100, false),
        (100_000, 100, false),
        (300_000, 100, false),
        (100_000, 50_000, false),
        (100_000, 50_000, true),
    ] {
        let mut memory = Connection::open_in_memory().unwrap();
        if long_turn {
            seed_long_turn_benchmark(&mut memory, total, target);
        } else {
            seed_benchmark(&mut memory, total, target);
        }
        benchmark_case(&memory, "memory warm", total, target, false, long_turn);
        drop(memory);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("history.sqlite");
        let mut file = Connection::open(&path).unwrap();
        file.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        if long_turn {
            seed_long_turn_benchmark(&mut file, total, target);
        } else {
            seed_benchmark(&mut file, total, target);
        }
        file.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
            .unwrap();
        drop(file);
        let file = Connection::open(path).unwrap();
        benchmark_case(
            &file,
            "file SQLite-cold/OS-warm",
            total,
            target,
            true,
            long_turn,
        );
        benchmark_case(&file, "file warm", total, target, false, long_turn);
    }
}
