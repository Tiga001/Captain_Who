use super::*;

fn v61() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    connection
        .execute_batch(&CANONICAL_SCHEMA.replace(workflow_pending_schema(), ""))
        .unwrap();
    connection.pragma_update(None, "user_version", 61).unwrap();
    validate_schema_fingerprint(&connection, V61_SCHEMA_FINGERPRINT).unwrap();
    connection.execute("INSERT INTO conversations(id,title,created_at,updated_at) VALUES('keep','unchanged',1,1)",[]).unwrap();
    connection
}
#[test]
fn workflow_pending_v61_upgrade_only_adds_index_and_reopens() {
    let connection = v61();
    run_migrations(&connection).unwrap();
    run_migrations(&connection).unwrap();
    assert_eq!(read_schema_version(&connection).unwrap(), 62);
    assert_eq!(
        connection
            .query_row("SELECT title FROM conversations WHERE id='keep'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "unchanged"
    );
    let index: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE name='workflow_execution_input_pending_sequence'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(index.contains("WHERE status = 'pending'"));
    validate_canonical_schema(&connection).unwrap();
}
#[test]
fn workflow_pending_v61_upgrade_failure_rolls_back_catalog_and_version() {
    let connection = v61();
    connection.authorizer(Some(|context: rusqlite::hooks::AuthContext<'_>| {
        if matches!(
            context.action,
            rusqlite::hooks::AuthAction::CreateIndex {
                index_name: "workflow_execution_input_pending_sequence",
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
    assert_eq!(read_schema_version(&connection).unwrap(), 61);
    validate_schema_fingerprint(&connection, V61_SCHEMA_FINGERPRINT).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT title FROM conversations WHERE id='keep'", [], |r| r
                .get::<_, String>(0))
            .unwrap(),
        "unchanged"
    );
    run_migrations(&connection).unwrap();
}
