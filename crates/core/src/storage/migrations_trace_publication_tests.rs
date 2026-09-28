use super::*;

#[test]
fn upgrades_v60_without_rewriting_history_and_tracks_every_journal_mutation() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(&CANONICAL_SCHEMA.replace(trace_publication_schema(), ""))
        .unwrap();
    connection.pragma_update(None, "user_version", 60).unwrap();
    connection.execute_batch("INSERT INTO conversations(id,title,created_at,updated_at) VALUES('chat','Keep',1,1);
        INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES('assistant','chat','assistant','keep','sent',1,0);
        INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES('assistant','chat','run',6,'in_progress',0,1,1);").unwrap();
    let original: String = connection
        .query_row(
            "SELECT content FROM messages WHERE id='assistant'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    run_migrations(&connection).unwrap();
    assert_eq!(read_schema_version(&connection).unwrap(), 61);
    assert_eq!(
        connection
            .query_row(
                "SELECT content FROM messages WHERE id='assistant'",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        original
    );
    let revision = || {
        connection.query_row("SELECT revision FROM conversation_trace_journal_revisions WHERE assistant_message_id='assistant'", [], |row| row.get::<_,i64>(0)).unwrap()
    };
    assert_eq!(revision(), 0);
    connection.execute("UPDATE conversation_turn_traces SET updated_at=2 WHERE assistant_message_id='assistant'", []).unwrap();
    assert_eq!(revision(), 1);
    connection.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES('assistant',0,'assistant_narration','{\"type\":\"assistant_narration\",\"sequence\":0,\"content\":\"text\",\"truncated\":false}')", []).unwrap();
    assert_eq!(revision(), 2);
    connection.execute("UPDATE conversation_turn_trace_items SET item_json=json_set(item_json,'$.content','new') WHERE assistant_message_id='assistant'", []).unwrap();
    assert_eq!(revision(), 3);
    connection
        .execute(
            "DELETE FROM conversation_turn_trace_items WHERE assistant_message_id='assistant'",
            [],
        )
        .unwrap();
    assert_eq!(revision(), 4);
    let epoch: String = connection.query_row("SELECT epoch FROM conversation_trace_journal_revisions WHERE assistant_message_id='assistant'", [], |row| row.get(0)).unwrap();
    connection
        .execute("DELETE FROM messages WHERE id='assistant'", [])
        .unwrap();
    connection.execute_batch("INSERT INTO messages(id,conversation_id,role,content,status,created_at,position) VALUES('assistant','chat','assistant','new identity','pending',1,0);
        INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES('assistant','chat','run',6,'in_progress',0,1,1);").unwrap();
    let recreated: String = connection.query_row("SELECT epoch FROM conversation_trace_journal_revisions WHERE assistant_message_id='assistant'", [], |row| row.get(0)).unwrap();
    assert_ne!(epoch, recreated);
}

#[test]
fn v61_upgrade_failure_keeps_exact_v60_catalog_and_rows() {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(&CANONICAL_SCHEMA.replace(trace_publication_schema(), ""))
        .unwrap();
    connection.pragma_update(None, "user_version", 60).unwrap();
    connection.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
        if matches!(
            context.action,
            rusqlite::hooks::AuthAction::CreateIndex {
                index_name: "idx_human_interaction_requests_attention",
                ..
            }
        ) {
            rusqlite::hooks::Authorization::Deny
        } else {
            rusqlite::hooks::Authorization::Allow
        }
    }));
    assert!(run_migrations(&connection).is_err());
    connection
        .authorizer(None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>);
    assert_eq!(read_schema_version(&connection).unwrap(), 60);
    validate_schema_fingerprint(&connection, V60_SCHEMA_FINGERPRINT).unwrap();
}
