use super::*;
use rusqlite::{params, StatementStatus};

fn v47_connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    let old_schema = canonical_schema_v48().replace(
        HISTORY_SEARCH_INDEX_V48,
        include_str!("test_fixtures/history_search_v47.sql"),
    );
    connection.execute_batch(&old_schema).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.pragma_update(None, "user_version", 47).unwrap();
    assert_eq!(
        schema_fingerprint(&connection).unwrap(),
        V47_SCHEMA_FINGERPRINT
    );
    connection
}

fn seed_history(connection: &Connection) {
    connection.execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','保持历史',1,2);
         INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
         VALUES ('user','chat','user','原始问题 /路径/中文','sent',1,0),
                ('assistant','chat','assistant','原始回答','sent',2,1);
         INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at,completed_at)
         VALUES ('assistant','chat','run',6,'completed',0,1,2,2);
         INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json)
         VALUES ('assistant',0,'backend_state','{\"type\":\"backend_state\",\"sequence\":0,\"status\":\"completed\"}');"
    ).unwrap();
}

fn fts_snapshot(connection: &Connection) -> Vec<(i64, String, String)> {
    connection
        .prepare("SELECT rowid,ref_key,content FROM conversation_history_fts ORDER BY rowid")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn assert_index_agrees(connection: &Connection) {
    let mismatches: i64 = connection
        .query_row(
            "SELECT (SELECT count(*) FROM conversation_history_fts AS f
                 LEFT JOIN conversation_history_index_entries AS i ON i.rowid=f.rowid
                 WHERE i.rowid IS NULL OR i.ref_key != f.ref_key
                    OR i.archive_ref IS NOT f.archive_ref
                    OR i.owner_message_id != COALESCE(f.message_id,f.assistant_message_id))
              + (SELECT count(*) FROM conversation_history_index_entries AS i
                 LEFT JOIN conversation_history_fts AS f ON f.rowid=i.rowid
                 WHERE f.rowid IS NULL)",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(mismatches, 0);
    ensure_foreign_keys_are_valid(connection).unwrap();
}

fn v48_connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection.execute_batch(&canonical_schema_v48()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", true)
        .unwrap();
    connection.pragma_update(None, "user_version", 48).unwrap();
    assert_eq!(
        schema_fingerprint(&connection).unwrap(),
        V48_SCHEMA_FINGERPRINT
    );
    connection
}

fn seed_guidance(connection: &Connection) {
    connection
        .execute_batch(
            "INSERT INTO conversations(id,title,created_at,updated_at)
             VALUES ('guidance-chat','附件引导',1,2);
             INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
             VALUES ('guidance-assistant','guidance-chat','assistant','等待输入','pending',2,0);
             INSERT INTO attachments(
                 id,conversation_id,message_id,project_id,kind,original_name,mime_type,
                 size_bytes,storage_rel_path,created_at
             ) VALUES (
                 'guidance-attachment','guidance-chat','guidance-assistant',NULL,'file',
                 'notes.txt','text/plain',5,'attachments/guidance-attachment',2
             );
             INSERT INTO agent_run_guidances(
                 guidance_id,client_message_id,run_id,conversation_id,assistant_message_id,
                 content,status,applied_trace_sequence,terminal_reason,created_at,updated_at
             ) VALUES (
                 'guidance-legacy','client-legacy','run-guidance','guidance-chat',
                 'guidance-assistant','查看这个文件','queued',NULL,NULL,3,3
             );
             INSERT INTO agent_run_guidance_attachments(guidance_id,attachment_id,position)
             VALUES ('guidance-legacy','guidance-attachment',0);",
        )
        .unwrap();
}

#[test]
fn exact_v48_requires_reset_without_losing_owned_attachments_or_constraints() {
    let connection = v48_connection();
    seed_guidance(&connection);

    let before_fingerprint = schema_fingerprint(&connection).unwrap();
    let before_changes = connection.total_changes();
    assert!(run_migrations(&connection)
        .unwrap_err()
        .to_string()
        .contains("found 48"));
    assert_eq!(read_schema_version(&connection).unwrap(), 48);
    assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
    assert_eq!(connection.total_changes(), before_changes);
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM agent_run_guidances WHERE guidance_id='guidance-legacy'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "查看这个文件"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT attachment_id FROM agent_run_guidance_attachments
                 WHERE guidance_id='guidance-legacy'",
                [],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
        "guidance-attachment"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('agent_run_guidance_attachments')
                 WHERE \"table\"='agent_run_guidances'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('human_interaction_async_bindings')
                 WHERE \"table\"='agent_run_guidances'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .pragma_query_value(None, "foreign_keys", |row| row.get::<_, i64>(0))
            .unwrap(),
        1
    );
    for object in [
        "idx_agent_run_guidances_run_status",
        "idx_agent_run_guidances_conversation",
        "human_interaction_async_guidance_proof",
        "human_interaction_async_guidance_applied",
        "human_interaction_async_guidance_unconsumed",
    ] {
        assert!(
            connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema WHERE name=?1",
                    [object],
                    |_| Ok(()),
                )
                .is_ok(),
            "missing migrated schema object {object}"
        );
    }
    ensure_foreign_keys_are_valid(&connection).unwrap();
}

#[test]
fn exact_v47_requires_reset_without_rewriting_history_or_search_content() {
    let connection = v47_connection();
    seed_history(&connection);
    let before = fts_snapshot(&connection);
    let changes = connection.total_changes();
    assert!(run_migrations(&connection)
        .unwrap_err()
        .to_string()
        .contains("found 47"));
    assert_eq!(read_schema_version(&connection).unwrap(), 47);
    assert_eq!(connection.total_changes(), changes);
    assert_eq!(fts_snapshot(&connection), before);
    assert_eq!(
        connection
            .query_row("SELECT content FROM messages WHERE id='user'", [], |row| {
                row.get::<_, String>(0)
            })
            .unwrap(),
        "原始问题 /路径/中文"
    );
    let changes = connection.total_changes();
    assert!(run_migrations(&connection).is_err());
    assert_eq!(connection.total_changes(), changes);
    assert_eq!(fts_snapshot(&connection), before);
}

#[test]
fn malformed_v47_history_is_rejected_without_catalog_or_content_changes() {
    let connection = v47_connection();
    seed_history(&connection);
    // A malformed derived row is allowed by the old FTS schema. Its duplicate identity must not
    // silently replace either row; rejecting the obsolete schema must preserve both.
    connection.execute("INSERT INTO conversation_history_fts(ref_key,message_id,content) VALUES ('message:user','user','duplicate')",[]).unwrap();
    let before = fts_snapshot(&connection);
    assert!(run_migrations(&connection).is_err());
    assert_eq!(read_schema_version(&connection).unwrap(), 47);
    assert_eq!(
        schema_fingerprint(&connection).unwrap(),
        V47_SCHEMA_FINGERPRINT
    );
    assert_eq!(fts_snapshot(&connection), before);
    connection
        .execute(
            "UPDATE messages SET content='old triggers remain usable' WHERE id='user'",
            [],
        )
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM conversation_history_fts WHERE ref_key='message:user'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "old triggers remain usable"
    );
}

#[test]
fn tampered_v47_is_rejected_without_mutation() {
    let connection = v47_connection();
    seed_history(&connection);
    connection
        .execute_batch("DROP TRIGGER conversation_history_fts_message_update;")
        .unwrap();
    let fingerprint = schema_fingerprint(&connection).unwrap();
    let changes = connection.total_changes();
    assert!(run_migrations(&connection)
        .unwrap_err()
        .to_string()
        .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    assert_eq!(schema_fingerprint(&connection).unwrap(), fingerprint);
    assert_eq!(connection.total_changes(), changes);
    assert_eq!(read_schema_version(&connection).unwrap(), 47);
}

#[test]
fn search_identity_tracks_message_trace_update_delete_and_cascade() {
    let connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    seed_history(&connection);
    assert_index_agrees(&connection);
    connection
        .execute(
            "UPDATE messages SET content='changed content', status='error' WHERE id='user'",
            [],
        )
        .unwrap();
    connection.execute("UPDATE conversation_turn_trace_items SET item_json='{\"type\":\"backend_state\",\"status\":\"failed\"}' WHERE assistant_message_id='assistant' AND sequence=0",[]).unwrap();
    assert_index_agrees(&connection);
    assert_eq!(connection.query_row("SELECT count(*) FROM conversation_history_fts WHERE conversation_history_fts MATCH 'changed'",[],|row|row.get::<_,i64>(0)).unwrap(),1);
    connection
        .execute(
            "DELETE FROM conversation_turn_trace_items WHERE assistant_message_id='assistant'",
            [],
        )
        .unwrap();
    assert_index_agrees(&connection);
    connection
        .execute("DELETE FROM conversations WHERE id='chat'", [])
        .unwrap();
    assert_index_agrees(&connection);
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM conversation_history_index_entries",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT count(*) FROM conversation_history_fts", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn trace_insert_work_does_not_scale_with_unrelated_full_text_rows() {
    let connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    seed_history(&connection);
    fn insert_steps(connection: &Connection, sequence: i64, archive: Option<&str>) -> i32 {
        let mut stmt=connection.prepare("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES ('assistant',?1,'backend_state',?2)").unwrap();
        stmt.execute(params![
            sequence,
            serde_json::json!({"type":"backend_state","sequence":sequence,"archiveRef":archive})
                .to_string()
        ])
        .unwrap();
        stmt.get_status(StatementStatus::VmStep)
    }
    let small_null = insert_steps(&connection, 1, None);
    let small_missing = insert_steps(&connection, 2, Some("missing-archive"));
    // This fixture represents unrelated existing history. No actual application DB is opened.
    connection
        .execute_batch(
            "BEGIN;
      WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000)
      INSERT INTO conversation_history_index_entries(ref_key,archive_ref,owner_message_id)
      SELECT 'fixture:'||x,'unrelated:'||x,'assistant' FROM n;
      INSERT INTO conversation_history_fts(rowid,ref_key,archive_ref,assistant_message_id,content)
      SELECT rowid,ref_key,archive_ref,owner_message_id,'unrelated search text'
      FROM conversation_history_index_entries WHERE ref_key LIKE 'fixture:%';
      COMMIT;",
        )
        .unwrap();
    let large_null = insert_steps(&connection, 3, None);
    let large_missing = insert_steps(&connection, 4, Some("missing-archive"));
    eprintln!("trace insert VM steps, before/after 20,000 unrelated rows: null {small_null}/{large_null}, missing {small_missing}/{large_missing}");
    assert!(
        large_null < small_null + 100,
        "NULL archive must not scan FTS: {small_null} -> {large_null}"
    );
    assert!(
        large_missing < small_missing + 100,
        "missing archive must use B-tree: {small_missing} -> {large_missing}"
    );
    let old = v47_connection();
    seed_history(&old);
    old.execute_batch(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<20000)
        INSERT INTO conversation_history_fts(ref_key,archive_ref,assistant_message_id,content)
        SELECT 'fixture:'||x,'unrelated:'||x,'assistant','unrelated search text' FROM n;",
    )
    .unwrap();
    let old_large_null = insert_steps(&old, 3, None);
    let old_large_missing = insert_steps(&old, 4, Some("missing-archive"));
    eprintln!("v47 trace insert VM steps with 20,000 unrelated rows: null {old_large_null}, missing {old_large_missing}");
    assert!(old_large_null > large_null * 10);
    assert!(old_large_missing > large_missing * 10);
    assert_index_agrees(&connection);
}

#[test]
fn explicit_cleanup_with_disabled_triggers_keeps_identity_lifecycle_consistent() {
    use rusqlite::config::DbConfig;
    let connection = Connection::open_in_memory().unwrap();
    run_migrations(&connection).unwrap();
    seed_history(&connection);
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
    assert_index_agrees(&connection);
    assert_eq!(
        connection
            .query_row(
                "SELECT count(*) FROM conversation_history_index_entries",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}
