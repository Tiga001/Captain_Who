use super::*;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

fn seeded_v68() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(&canonical_schema_v68()).unwrap();
    c.pragma_update(None, "foreign_keys", true).unwrap();
    c.pragma_update(None, "user_version", 68).unwrap();
    validate_schema_fingerprint(&c, V68_SCHEMA_FINGERPRINT).unwrap();
    c.execute_batch(
        "INSERT INTO conversations(id,title,created_at,updated_at) VALUES('chat','History',1,1),('other','Other',1,1);
         INSERT INTO messages(id,conversation_id,role,content,status,agent_run_json,created_at,position)
         VALUES('message','chat','assistant','original searchable answer','sent','{}',1,0);
         INSERT INTO chat_message_ui_states(message_id,ui_state_json) VALUES('message','{\"favorited\":true}');",
    ).unwrap();
    c
}

fn message_snapshot(c: &Connection) -> Vec<String> {
    [
        "SELECT json_array(id,conversation_id,role,content,status,agent_run_json,created_at,position,presentation_revision) FROM messages",
        "SELECT json_array(conversation_id,epoch,revision,presentation_revision,ui_revision) FROM conversation_message_history_revisions ORDER BY conversation_id",
        "SELECT json_array(message_id,ui_state_json) FROM chat_message_ui_states",
        "SELECT json_array(rowid,ref_key,conversation_id,status,created_at,position,within_message_order,content) FROM conversation_history_fts ORDER BY rowid",
        "SELECT json_array(entry_rowid,conversation_id,position,within_message_order) FROM conversation_history_timeline ORDER BY entry_rowid",
    ].into_iter().flat_map(|sql| {
        c.prepare(sql).unwrap().query_map([], |row| row.get::<_, String>(0)).unwrap()
            .collect::<rusqlite::Result<Vec<_>>>().unwrap()
    }).collect()
}

fn index_write_counter(c: &Connection) -> Arc<AtomicUsize> {
    let changes = Arc::new(AtomicUsize::new(0));
    let observed = changes.clone();
    c.update_hook(Some(
        move |_: rusqlite::hooks::Action, _: &str, table: &str, _: i64| {
            if table == "conversation_history_index_entries" {
                observed.fetch_add(1, Ordering::Relaxed);
            }
        },
    ));
    changes
}

#[test]
fn v69_upgrade_preserves_messages_revisions_and_search_rows() {
    let c = seeded_v68();
    let before = message_snapshot(&c);
    run_migrations(&c).unwrap();
    assert_eq!(read_schema_version(&c).unwrap(), STORAGE_SCHEMA_VERSION);
    assert_eq!(message_snapshot(&c), before);
    let changes = c.total_changes();
    run_migrations(&c).unwrap();
    assert_eq!(c.total_changes(), changes);
    validate_canonical_schema(&c).unwrap();
}

#[test]
fn v69_rejects_modified_v68_catalog_without_mutation() {
    let c = seeded_v68();
    c.execute_batch("DROP TRIGGER conversation_history_fts_message_update;")
        .unwrap();
    let fingerprint = schema_fingerprint(&c).unwrap();
    let before = message_snapshot(&c);
    let changes = c.total_changes();
    assert!(run_migrations(&c)
        .unwrap_err()
        .to_string()
        .contains("fingerprint mismatch"));
    assert_eq!(read_schema_version(&c).unwrap(), 68);
    assert_eq!(schema_fingerprint(&c).unwrap(), fingerprint);
    assert_eq!(message_snapshot(&c), before);
    assert_eq!(c.total_changes(), changes);
}

#[test]
fn v69_fts_skips_unchanged_index_fields_even_when_update_names_them() {
    let c = seeded_v68();
    run_migrations(&c).unwrap();
    let writes = index_write_counter(&c);
    c.execute_batch(
        "UPDATE messages SET conversation_id=conversation_id,role=role,content=content,status=status,created_at=created_at,position=position;
         UPDATE messages SET content=content,status=status,agent_run_json='{\"display\":1}';",
    ).unwrap();
    assert_eq!(writes.load(Ordering::Relaxed), 0);
    let revision: i64 = c.query_row(
        "SELECT presentation_revision FROM conversation_message_history_revisions WHERE conversation_id='chat'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(revision, 1);
}

#[test]
fn v69_fts_tracks_all_indexed_field_changes_and_rolls_back_atomically() {
    let mut c = seeded_v68();
    run_migrations(&c).unwrap();
    let writes = index_write_counter(&c);
    for sql in [
        "UPDATE messages SET content='replacement searchable answer' WHERE id='message'",
        "UPDATE messages SET status=NULL WHERE id='message'",
        "UPDATE messages SET created_at=2 WHERE id='message'",
        "UPDATE messages SET position=5 WHERE id='message'",
        "UPDATE messages SET role='user' WHERE id='message'",
        "UPDATE messages SET conversation_id='other' WHERE id='message'",
    ] {
        let before = writes.load(Ordering::Relaxed);
        c.execute_batch(sql).unwrap();
        assert!(writes.load(Ordering::Relaxed) > before, "{sql}");
        let matches: bool = c.query_row(
            "SELECT f.conversation_id IS m.conversation_id AND f.content IS m.content
             AND f.status IS m.status AND f.created_at IS m.created_at AND f.position IS m.position
             AND f.within_message_order = CASE WHEN m.role='assistant' THEN 9223372036854775807 ELSE 0 END
             FROM conversation_history_fts f JOIN messages m ON m.id=f.message_id WHERE m.id='message'",
            [], |row| row.get(0),
        ).unwrap();
        assert!(matches, "{sql}");
        ensure_foreign_keys_are_valid(&c).unwrap();
    }
    let searchable: i64 = c.query_row(
        "SELECT count(*) FROM conversation_history_fts WHERE conversation_history_fts MATCH 'replacement'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(searchable, 1);
    let before = message_snapshot(&c);
    let tx = c.transaction().unwrap();
    tx.execute_batch(
        "UPDATE messages SET content='rolledback search token',status='sent',position=6;",
    )
    .unwrap();
    tx.rollback().unwrap();
    assert_eq!(message_snapshot(&c), before);
    let rolled_back: i64 = c.query_row(
        "SELECT count(*) FROM conversation_history_fts WHERE conversation_history_fts MATCH 'rolledback'",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(rolled_back, 0);
}
