use super::*;
use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::params;

fn add_input(c: &Connection, id: &str, instance: &str, chat: &str, delivered: bool) {
    c.execute(
        "INSERT INTO workflow_mail_inputs(input_id,instance_id,execution_version,node_id,
            conversation_id,input_json,status,delivery_id,created_at,updated_at)
         VALUES(?1,?2,'epoch','node',?3,'{}','pending',?4,1,1)",
        params![
            id,
            instance,
            chat,
            delivered.then(|| format!("delivery-{id}"))
        ],
    )
    .unwrap();
}

fn add_event(c: &Connection, instance: &str, input: &str) -> i64 {
    c.execute(
        "INSERT INTO workflow_mail_events(instance_id,input_id,kind,created_at)
         VALUES(?1,?2,'run_completed',1)",
        params![instance, input],
    )
    .unwrap();
    c.last_insert_rowid()
}

fn rows(c: &Connection, sql: &str) -> Vec<(String, String, i64)> {
    c.prepare(sql)
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap()
}

fn assert_projection(c: &Connection) {
    assert_eq!(
        rows(c, "SELECT instance_id,conversation_id,last_sequence FROM workflow_mail_conversation_changes ORDER BY 1,2"),
        rows(c, "SELECT input.instance_id,input.conversation_id,MAX(event.sequence)
          FROM workflow_mail_events event JOIN workflow_mail_inputs input
          ON input.input_id=event.input_id AND input.instance_id=event.instance_id
          WHERE input.conversation_id IS NOT NULL AND input.delivery_id IS NOT NULL
          GROUP BY input.instance_id,input.conversation_id ORDER BY 1,2")
    );
}

#[test]
fn v70_upgrade_backfills_all_history_without_changing_mail() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(&canonical_schema_v69()).unwrap();
    c.pragma_update(None, "user_version", 69).unwrap();
    validate_schema_fingerprint(&c, V69_SCHEMA_FINGERPRINT).unwrap();
    add_input(&c, "old", "org", "retired-chat", true);
    let first = add_event(&c, "org", "old");
    for _ in 0..600 {
        add_event(&c, "other-org", "missing");
    }
    add_input(&c, "pending", "org", "not-delivered", false);
    add_event(&c, "org", "pending");
    add_input(&c, "other", "other-org", "other-chat", true);
    add_event(&c, "other-org", "other");
    // Input-only receipts do not invent a change sequence, nor do foreign-instance events.
    add_input(&c, "no-event", "org", "no-event-chat", true);
    add_event(&c, "foreign-org", "no-event");
    let before: String = c.query_row(
        "SELECT json_group_array(json_array(input_id,input_json,conversation_id,delivery_id)) FROM workflow_mail_inputs",
        [], |row| row.get(0),
    ).unwrap();
    run_migrations(&c).unwrap();
    assert_eq!(read_schema_version(&c).unwrap(), STORAGE_SCHEMA_VERSION);
    assert_projection(&c);
    assert!(rows(
        &c,
        "SELECT instance_id,conversation_id,last_sequence FROM workflow_mail_conversation_changes"
    )
    .contains(&("org".into(), "retired-chat".into(), first)));
    let after: String = c.query_row(
        "SELECT json_group_array(json_array(input_id,input_json,conversation_id,delivery_id)) FROM workflow_mail_inputs",
        [], |row| row.get(0),
    ).unwrap();
    assert_eq!(before, after);
    let changes = c.total_changes();
    run_migrations(&c).unwrap();
    assert_eq!(changes, c.total_changes());
}

#[test]
fn v70_rejects_modified_v69_without_mutation() {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(&canonical_schema_v69()).unwrap();
    c.pragma_update(None, "user_version", 69).unwrap();
    c.execute_batch("DROP INDEX workflow_mail_event_instance")
        .unwrap();
    let fingerprint = schema_fingerprint(&c).unwrap();
    assert!(run_migrations(&c)
        .unwrap_err()
        .to_string()
        .contains("fingerprint mismatch"));
    assert_eq!(read_schema_version(&c).unwrap(), 69);
    assert_eq!(schema_fingerprint(&c).unwrap(), fingerprint);
}

#[test]
fn v70_recovery_facts_follow_binding_late_events_and_rollback_without_bodies() {
    let mut c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    add_input(&c, "early", "org", "chat", false);
    add_event(&c, "org", "early");
    assert_projection(&c);
    c.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Read {
            table_name: "workflow_mail_inputs",
            column_name: "input_json",
            ..
        }
        | AuthAction::Read {
            table_name: "workflow_mail_messages",
            column_name: "message_json",
            ..
        } => Authorization::Deny,
        _ => Authorization::Allow,
    }));
    c.execute(
        "UPDATE workflow_mail_inputs SET delivery_id='delivery-early' WHERE input_id='early'",
        [],
    )
    .unwrap();
    assert_projection(&c);
    for _ in 0..600 {
        add_event(&c, "other-org", "missing");
    }
    let late = add_event(&c, "org", "early");
    assert_projection(&c);
    assert_eq!(rows(&c,"SELECT instance_id,conversation_id,last_sequence FROM workflow_mail_conversation_changes"), vec![("org".into(), "chat".into(), late)]);
    let before = c.total_changes();
    c.execute("UPDATE workflow_mail_inputs SET status='completed',delivery_id=delivery_id WHERE input_id='early'", []).unwrap();
    assert_eq!(
        c.total_changes() - before,
        1,
        "unchanged identities must not rebuild metadata"
    );
    let tx = c.transaction().unwrap();
    add_event(&tx, "org", "early");
    tx.execute(
        "UPDATE workflow_mail_inputs SET conversation_id='moved' WHERE input_id='early'",
        [],
    )
    .unwrap();
    assert_projection(&tx);
    tx.rollback().unwrap();
    assert_projection(&c);
    assert_eq!(rows(&c,"SELECT instance_id,conversation_id,last_sequence FROM workflow_mail_conversation_changes"), vec![("org".into(), "chat".into(), late)]);
}

#[test]
fn v70_recovery_facts_track_input_and_event_rewrites_deletes_and_fork_origins() {
    let c = Connection::open_in_memory().unwrap();
    run_migrations(&c).unwrap();
    add_input(&c, "first", "org", "chat", true);
    add_event(&c, "org", "first");
    add_input(&c, "second", "org", "chat", true);
    let latest = add_event(&c, "org", "second");
    // Receipt origins on a fork must not move the original recipient's recovery cursor.
    c.execute(
        "INSERT INTO workflow_mail_message_origins VALUES('fork-mail','fork-chat','first')",
        [],
    )
    .unwrap();
    c.execute(
        "DELETE FROM workflow_mail_message_origins WHERE conversation_id='fork-chat'",
        [],
    )
    .unwrap();
    assert_projection(&c);
    c.execute(
        "UPDATE workflow_mail_events SET sequence=?1 WHERE sequence=?2",
        params![latest + 10, latest],
    )
    .unwrap();
    assert_projection(&c);
    c.execute(
        "DELETE FROM workflow_mail_events WHERE input_id='second'",
        [],
    )
    .unwrap();
    assert_projection(&c);
    for sql in [
        "UPDATE workflow_mail_inputs SET conversation_id='moved' WHERE input_id='first'",
        "UPDATE workflow_mail_inputs SET delivery_id=NULL WHERE input_id='first'",
        "UPDATE workflow_mail_inputs SET delivery_id='replacement' WHERE input_id='first'",
        "UPDATE workflow_mail_events SET input_id='second' WHERE input_id='first'",
        "UPDATE workflow_mail_events SET instance_id='other' WHERE input_id='second'",
        "UPDATE workflow_mail_inputs SET instance_id='other' WHERE input_id='second'",
        "DELETE FROM workflow_mail_inputs WHERE input_id='second'",
    ] {
        c.execute_batch(sql).unwrap();
        assert_projection(&c);
    }
    // Import/recovery may restore an input after its events; do not require insertion order.
    add_event(&c, "org", "restored");
    add_input(&c, "restored", "org", "restored-chat", true);
    assert_projection(&c);
    c.execute_batch("DELETE FROM workflow_mail_events; DELETE FROM workflow_mail_inputs;")
        .unwrap();
    assert_projection(&c);
    assert!(rows(
        &c,
        "SELECT instance_id,conversation_id,last_sequence FROM workflow_mail_conversation_changes"
    )
    .is_empty());
}
