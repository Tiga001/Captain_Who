use super::*;
use rusqlite::{config::DbConfig, types::Value};

fn initialize_v59(connection: &Connection) {
    connection
        .execute_batch(
            &CANONICAL_SCHEMA
                .replace(workflow_pending_schema(), "")
                .replace(trace_publication_schema(), "")
                .replace(history_timeline_schema(), ""),
        )
        .unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.pragma_update(None, "user_version", 59).unwrap();
    validate_schema_fingerprint(connection, V59_SCHEMA_FINGERPRINT).unwrap();
}

fn seed_history(connection: &Connection) {
    connection.execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','Keep history',1,2);
         INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES
         ('user','chat','user','Preserve exact request','sent',1,0),
         ('assistant','chat','assistant','Preserve exact response','sent',2,1);
         INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at,completed_at)
         VALUES ('assistant','chat','run',6,'completed',0,1,2,2);
         INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
         VALUES ('assistant',0,'backend_state','{\"type\":\"backend_state\",\"sequence\":0,\"status\":\"completed\"}');
         UPDATE messages SET position=9 WHERE id='assistant';"
    ).unwrap();
}

fn rows(connection: &Connection, sql: &str) -> Vec<Vec<Value>> {
    let mut statement = connection.prepare(sql).unwrap();
    let count = statement.column_count();
    statement
        .query_map([], |row| (0..count).map(|index| row.get(index)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn assert_projection_agrees(connection: &Connection) {
    assert_eq!(
        rows(connection, "SELECT rowid,conversation_id,position,within_message_order FROM conversation_history_fts WHERE record_type IN ('message','trace_item') ORDER BY rowid"),
        rows(connection, "SELECT entry_rowid,conversation_id,position,within_message_order FROM conversation_history_timeline ORDER BY entry_rowid")
    );
    ensure_foreign_keys_are_valid(connection).unwrap();
}

#[test]
fn v59_timeline_migration_preserves_journal_and_backfills_recorded_order() {
    let connection = Connection::open_in_memory().unwrap();
    initialize_v59(&connection);
    seed_history(&connection);
    let tables = [
        "messages",
        "conversation_turn_traces",
        "conversation_turn_trace_items",
        "conversation_history_fts",
        "conversation_history_index_entries",
    ];
    let before: Vec<_> = tables
        .iter()
        .map(|table| {
            rows(
                &connection,
                &format!("SELECT * FROM {table} ORDER BY rowid"),
            )
        })
        .collect();
    run_migrations(&connection).unwrap();
    assert_eq!(
        read_schema_version(&connection).unwrap(),
        STORAGE_SCHEMA_VERSION
    );
    for (table, expected) in tables.iter().zip(before) {
        assert_eq!(
            rows(
                &connection,
                &format!("SELECT * FROM {table} ORDER BY rowid")
            ),
            expected,
            "{table}"
        );
    }
    assert_projection_agrees(&connection);
    let position: i64 = connection.query_row(
        "SELECT t.position FROM conversation_history_timeline t JOIN conversation_history_index_entries i ON i.rowid=t.entry_rowid WHERE i.ref_key='trace:assistant:0'", [], |row| row.get(0)).unwrap();
    assert_eq!(
        position, 1,
        "backfill must preserve the old trace order, not current message position 9"
    );
    let changes = connection.total_changes();
    run_migrations(&connection).unwrap();
    assert_eq!(connection.total_changes(), changes);
}

#[test]
fn failed_v59_timeline_backfill_rolls_back_schema_and_preserves_source() {
    let connection = Connection::open_in_memory().unwrap();
    initialize_v59(&connection);
    seed_history(&connection);
    connection
        .execute(
            "UPDATE conversation_history_fts SET position=NULL WHERE ref_key='message:user'",
            [],
        )
        .unwrap();
    let before = rows(
        &connection,
        "SELECT * FROM conversation_history_fts ORDER BY rowid",
    );
    assert!(run_migrations(&connection)
        .unwrap_err()
        .to_string()
        .contains("NOT NULL"));
    assert_eq!(read_schema_version(&connection).unwrap(), 59);
    validate_schema_fingerprint(&connection, V59_SCHEMA_FINGERPRINT).unwrap();
    assert_eq!(
        rows(
            &connection,
            "SELECT * FROM conversation_history_fts ORDER BY rowid"
        ),
        before
    );
}

#[test]
fn timeline_projection_tracks_reindexing_and_deletion_with_disabled_user_triggers() {
    let connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    seed_history(&connection);
    assert_projection_agrees(&connection);
    connection.execute("UPDATE messages SET role='user', content='Changed response', position=10 WHERE id='assistant'", []).unwrap();
    assert_projection_agrees(&connection);
    connection.execute("UPDATE conversation_turn_trace_items SET item_json='{\"type\":\"backend_state\",\"sequence\":0,\"status\":\"failed\"}' WHERE assistant_message_id='assistant' AND sequence=0", []).unwrap();
    assert_projection_agrees(&connection);
    connection
        .execute(
            "DELETE FROM conversation_turn_trace_items WHERE assistant_message_id='assistant'",
            [],
        )
        .unwrap();
    assert_projection_agrees(&connection);
    assert_eq!(
        connection
            .query_row("PRAGMA foreign_keys", [], |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .unwrap();
    connection
        .execute(
            "DELETE FROM conversation_history_fts WHERE conversation_id='chat'",
            [],
        )
        .unwrap();
    connection
        .execute("DELETE FROM conversations WHERE id='chat'", [])
        .unwrap();
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, true)
        .unwrap();
    assert_projection_agrees(&connection);
    assert!(rows(
        &connection,
        "SELECT entry_rowid FROM conversation_history_timeline"
    )
    .is_empty());
}

#[test]
fn v59_and_current_snapshots_restore_with_consistent_timeline_projection() {
    use crate::storage::database_snapshot::create_verified_sqlite_snapshot;
    let directory = tempfile::tempdir().unwrap();
    for version in [59, 60, STORAGE_SCHEMA_VERSION] {
        let source_path = directory.path().join(format!("source-{version}.sqlite"));
        let restored_path = directory.path().join(format!("restored-{version}.sqlite"));
        let source = Connection::open(&source_path).unwrap();
        if version == 59 {
            initialize_v59(&source);
        } else if version == 60 {
            source
                .execute_batch(
                    &CANONICAL_SCHEMA
                        .replace(workflow_pending_schema(), "")
                        .replace(trace_publication_schema(), ""),
                )
                .unwrap();
            source.pragma_update(None, "user_version", 60).unwrap();
        } else {
            run_migrations(&source).unwrap();
        }
        source.pragma_update(None, "journal_mode", "WAL").unwrap();
        seed_history(&source);
        let original = rows(
            &source,
            "SELECT * FROM conversation_history_fts ORDER BY rowid",
        );
        std::fs::File::create(&restored_path).unwrap();
        create_verified_sqlite_snapshot(&source_path, &restored_path).unwrap();
        let restored = Connection::open(&restored_path).unwrap();
        run_migrations(&restored).unwrap();
        assert_projection_agrees(&restored);
        assert_eq!(
            rows(
                &restored,
                "SELECT * FROM conversation_history_fts ORDER BY rowid"
            ),
            original
        );
        assert_eq!(read_schema_version(&source).unwrap(), version);
        restored.execute("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES ('after-restore','chat','user','next','sent',3,10)", []).unwrap();
        assert_projection_agrees(&restored);
    }
}
