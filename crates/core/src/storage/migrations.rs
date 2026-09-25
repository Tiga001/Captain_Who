use rusqlite::{ffi, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const STORAGE_SCHEMA_VERSION: i32 = 55;
pub const DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED: &str =
    "development_storage_schema_reset_required";

const CANONICAL_SCHEMA: &str = include_str!("canonical_schema.sql");
const CANONICAL_SCHEMA_FINGERPRINT: &str =
    "sha256:1befc72d0b8ff219c3de6906c87b61dab6a1002dca69e98179ec0dcee9774fb2";

/// Initializes fresh storage or validates the exact current canonical schema.
///
/// The exact v50 catalog upgrades only with an empty collaboration event log. v51/v52 upgrade
/// without rewriting or deleting history; v53 adds request-owned activity facts only for future
/// events. v54 adds authoring-only workflow definitions without changing existing history.
/// v55 adds explicit workflow availability, disabled by default for every existing definition.
/// Earlier development catalogs require an explicit reset.
pub fn run_migrations(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;

    let schema_version = read_schema_version(connection)?;
    let object_count = application_schema_object_count(connection)?;

    if schema_version == 0 && object_count == 0 {
        return create_canonical_schema(connection);
    }

    if read_schema_version(connection)? == 50 {
        upgrade_parent_activity_placement_v50(connection)?;
    }

    if read_schema_version(connection)? == 51 {
        upgrade_mailbox_text_v51(connection)?;
    }

    if read_schema_version(connection)? == 52 {
        upgrade_request_owned_activities_v52(connection)?;
    }

    if read_schema_version(connection)? == 53 {
        upgrade_workflow_definitions_v53(connection)?;
    }

    if read_schema_version(connection)? == 54 {
        upgrade_workflow_availability_v54(connection)?;
    }

    let schema_version = read_schema_version(connection)?;
    if schema_version != STORAGE_SCHEMA_VERSION {
        return Err(reset_required_error(format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found {schema_version}"
        )));
    }

    validate_canonical_schema(connection)
}

#[cfg(test)]
const V47_SCHEMA_FINGERPRINT: &str =
    "sha256:44c989bbfd6fb7212013c2f27ce721aa967157a85202187d88f0dea0e5c65827";
#[cfg(test)]
const V48_SCHEMA_FINGERPRINT: &str =
    "sha256:c0d1cc5df5ad3dfa8674b8297f0e39b4ab5602c63500108edc6a1b78839ca455";
const V52_SCHEMA_FINGERPRINT: &str =
    "sha256:615c17481af79b297a930607a7ca0d8e45a3825527ac19f74bf4a64b747b4cc7";
const V53_SCHEMA_FINGERPRINT: &str =
    "sha256:af8820cb0e6f0ba8b433ea5fa51f345a515c1e9fcc466830179f7b53f06f76d8";
const V54_SCHEMA_FINGERPRINT: &str =
    "sha256:5a5ce084a58f6a68a3ffa0f0ca071827addff12c3e1c1e13d73c2abd301c3200";
const V51_SCHEMA_FINGERPRINT: &str =
    "sha256:532cd03af014f5d16e333c7b0bdff218cb8b996c74c6331ace300b22c6e81cd7";
const V50_SCHEMA_FINGERPRINT: &str =
    "sha256:fe81aefd5a07fe7fe801fe0b31d198cfa0517528c36791b1e2b1b491a932dc3a";
#[cfg(test)]
const HISTORY_SEARCH_INDEX_V48: &str = include_str!("history_search_index_v48.sql");

#[cfg(test)]
fn canonical_schema_v48() -> String {
    let table_marker = "CREATE TABLE agent_run_guidances (";
    let content_marker = "            content TEXT NOT NULL,\n";
    let canonical_v50 = canonical_schema_v50().replace(
        "            folder_references_json TEXT NOT NULL DEFAULT '[]' CHECK (\n                json_valid(folder_references_json) AND json_type(folder_references_json) = 'array'\n            ),\n",
        "",
    );
    let table_start = canonical_v50
        .find(table_marker)
        .expect("canonical guidance table exists");
    let content_offset = canonical_v50[table_start..]
        .find(content_marker)
        .expect("canonical guidance content column exists");
    let content_start = table_start + content_offset;
    let content_end = content_start + content_marker.len();
    let mut schema = canonical_v50;
    schema.replace_range(
        content_start..content_end,
        "            content TEXT NOT NULL CHECK (length(trim(content)) > 0),\n",
    );
    schema
}

/// Upgrades only the empty event catalog; older timeline facts are never guessed or rewritten.
/// Users must explicitly remove old conversation histories before this schema change can run.
fn upgrade_parent_activity_placement_v50(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V50_SCHEMA_FINGERPRINT)?;
    let events: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM agent_collaboration_events",
        [],
        |row| row.get(0),
    )?;
    if events != 0 {
        return Err(reset_required_error(
            "schema v51 requires manually deleting old conversation histories before upgrading; existing collaboration events were left untouched",
        ));
    }
    // Drop dependents first, and recreate them from the exact canonical catalog.
    let v52 = canonical_schema_v52();
    let schema = collaboration_event_schema(&v52);
    for kind in ["TRIGGER", "INDEX", "VIEW", "TABLE"] {
        for line in schema.lines() {
            let prefix = format!("CREATE {kind} ");
            if let Some(rest) = line.strip_prefix(&prefix) {
                let name = rest.split_whitespace().next().unwrap();
                transaction.execute_batch(&format!("DROP {kind} IF EXISTS {name};"))?;
            }
        }
    }
    transaction.execute_batch(schema)?;
    transaction.pragma_update(None, "user_version", 51)?;
    validate_schema_fingerprint(&transaction, V51_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn collaboration_event_schema(schema: &str) -> &str {
    let start = schema
        .find("CREATE TABLE agent_collaboration_events (")
        .unwrap();
    let end = schema[start..]
        .find("CREATE TRIGGER prevent_agent_command_session_model_receipt_update")
        .unwrap()
        + start;
    &schema[start..end]
}

#[cfg(test)]
fn canonical_schema_v50() -> String {
    canonical_schema_v51().replace(
        collaboration_event_schema(&canonical_schema_v52()),
        include_str!("test_fixtures/collaboration_events_v50.sql"),
    )
}

#[cfg(test)]
fn canonical_schema_v51() -> String {
    canonical_schema_v52().replace(
        "length(CAST(content AS BLOB)) >= 1",
        "length(CAST(content AS BLOB)) BETWEEN 1 AND 1048576",
    )
}

/// Rebuild only the exact v51 mailbox table, preserving every message, dependent fact and
/// AUTOINCREMENT high-water mark. Trigger replay would emit duplicate collaboration events,
/// so table-owned triggers are restored only after copying the existing immutable rows.
fn upgrade_mailbox_text_v51(connection: &Connection) -> rusqlite::Result<()> {
    validate_schema_fingerprint(connection, V51_SCHEMA_FINGERPRINT)?;
    connection.pragma_update(None, "foreign_keys", false)?;
    let result = (|| {
        let transaction = connection.unchecked_transaction()?;
        let dependents = transaction
            .prepare(
                "SELECT sql FROM sqlite_schema WHERE tbl_name = 'agent_mailbox_messages'
             AND type IN ('index', 'trigger') AND sql IS NOT NULL ORDER BY type, name",
            )?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let sequence: Option<i64> = transaction
            .query_row(
                "SELECT seq FROM sqlite_sequence WHERE name = 'agent_mailbox_messages'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        transaction.execute_batch(
            "CREATE TEMP TABLE mailbox_v51_backup AS SELECT * FROM agent_mailbox_messages;
             DROP TABLE agent_mailbox_messages;",
        )?;
        let start = CANONICAL_SCHEMA
            .find("CREATE TABLE agent_mailbox_messages (")
            .unwrap();
        let end = CANONICAL_SCHEMA[start..].find(";\n").unwrap() + start + 1;
        transaction.execute_batch(&CANONICAL_SCHEMA[start..end])?;
        transaction.execute_batch(
            "INSERT INTO agent_mailbox_messages SELECT * FROM mailbox_v51_backup;
             DROP TABLE mailbox_v51_backup;",
        )?;
        if let Some(sequence) = sequence {
            transaction.execute(
                "UPDATE sqlite_sequence SET seq = ?1 WHERE name = 'agent_mailbox_messages'",
                [sequence],
            )?;
        }
        for sql in dependents {
            transaction.execute_batch(&sql)?;
        }
        transaction.pragma_update(None, "user_version", 52)?;
        validate_schema_fingerprint(&transaction, V52_SCHEMA_FINGERPRINT)?;
        transaction.commit()
    })();
    connection.pragma_update(None, "foreign_keys", true)?;
    result
}

/// The v52 log and frozen chat JSON remain untouched. Only future events acquire explicit
/// owner/task projections; no structural-parent history is guessed into task subscriptions.
fn upgrade_request_owned_activities_v52(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V52_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(request_owned_activity_schema())?;
    transaction.pragma_update(None, "user_version", 53)?;
    validate_schema_fingerprint(&transaction, V53_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_workflow_definitions_v53(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V53_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_definition_schema())?;
    transaction.pragma_update(None, "user_version", 54)?;
    validate_schema_fingerprint(&transaction, V54_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_workflow_availability_v54(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V54_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_availability_schema())?;
    transaction.pragma_update(None, "user_version", 55)?;
    validate_canonical_schema(&transaction)?;
    transaction.commit()
}

fn workflow_definition_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Workflow authoring definitions, schema v54.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Workflow availability, schema v55.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_availability_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Workflow availability, schema v55.")
        .unwrap();
    &CANONICAL_SCHEMA[start..]
}

fn canonical_schema_v54() -> String {
    CANONICAL_SCHEMA.replace(workflow_availability_schema(), "")
}

fn canonical_schema_v53() -> String {
    canonical_schema_v54().replace(workflow_definition_schema(), "")
}

fn request_owned_activity_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Request-owned presentation facts, introduced in v53.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("CREATE TRIGGER prevent_agent_command_session_model_receipt_update")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn canonical_schema_v52() -> String {
    canonical_schema_v53().replace(request_owned_activity_schema(), "")
}

fn create_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(CANONICAL_SCHEMA)?;
    initialize_local_token_ledger(&transaction)?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;

    validate_canonical_schema(&transaction)?;
    ensure_foreign_keys_are_valid(&transaction)?;

    transaction.commit()
}

fn initialize_local_token_ledger(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute(
        "INSERT INTO local_token_usage_metadata(singleton, started_at) VALUES (1, ?1)",
        [super::now_ms()],
    )?;
    Ok(())
}

fn validate_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    validate_schema_fingerprint(connection, CANONICAL_SCHEMA_FINGERPRINT)?;
    ensure_foreign_keys_are_valid(connection)
}

fn validate_schema_fingerprint(connection: &Connection, expected: &str) -> rusqlite::Result<()> {
    let actual = schema_fingerprint(connection)?;
    if actual != expected {
        return Err(reset_required_error(format!(
            "schema catalog fingerprint mismatch (expected {expected}, found {actual})"
        )));
    }
    Ok(())
}

fn read_schema_version(connection: &Connection) -> rusqlite::Result<i32> {
    connection.query_row("PRAGMA user_version", [], |row| row.get(0))
}

fn application_schema_object_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COUNT(*) FROM sqlite_schema
         WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get(0),
    )
}

fn schema_fingerprint(connection: &Connection) -> rusqlite::Result<String> {
    let mut statement = connection.prepare(
        "SELECT type, name, tbl_name, sql
         FROM sqlite_schema
         WHERE sql IS NOT NULL
           AND name NOT LIKE 'sqlite_%'
         ORDER BY type, name, tbl_name",
    )?;
    let mut rows = statement.query([])?;
    let mut digest = Sha256::new();

    while let Some(row) = rows.next()? {
        for index in 0..4 {
            let value: String = row.get(index)?;
            digest.update((value.len() as u64).to_be_bytes());
            digest.update(value.as_bytes());
        }
    }

    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn ensure_foreign_keys_are_valid(connection: &Connection) -> rusqlite::Result<()> {
    let violation = connection
        .query_row("PRAGMA foreign_key_check", [], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })
        .optional()?;

    if let Some((table, row_id, parent)) = violation {
        return Err(reset_required_error(format!(
            "foreign key violation in table {table}, row {row_id}, parent {}",
            parent.unwrap_or_else(|| "<unknown>".to_string())
        )));
    }

    Ok(())
}

fn reset_required_error(detail: impl std::fmt::Display) -> rusqlite::Error {
    rusqlite::Error::SqliteFailure(
        ffi::Error::new(ffi::SQLITE_SCHEMA),
        Some(format!(
            "{DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED}: {detail}"
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v54_workflow_availability_migration_preserves_definitions_and_history() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("v54-workflows.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(&canonical_schema_v54()).unwrap();
        connection.pragma_update(None, "user_version", 54).unwrap();
        validate_schema_fingerprint(&connection, V54_SCHEMA_FINGERPRINT).unwrap();
        let definition =
            include_str!("../../../../packages/protocol/fixtures/workflow-definition-v1.json");
        let id = serde_json::from_str::<serde_json::Value>(definition).unwrap()["id"]
            .as_str()
            .unwrap()
            .to_owned();
        connection.execute(
            "INSERT INTO workflow_definitions (workflow_id,definition_json,revision,updated_at) VALUES (?1,?2,7,23)",
            rusqlite::params![id, definition],
        ).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations(id,title,model_id,created_at,updated_at)
             VALUES ('existing-chat','Existing history','model',1,1);
             INSERT INTO messages(id,conversation_id,role,content,created_at,position)
             VALUES ('existing-message','existing-chat','user','Keep this conversation',1,0);",
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(read_schema_version(&connection).unwrap(), 55);
        let record: (String, i64, i64, bool) = connection.query_row(
            "SELECT definition_json,revision,updated_at,enabled FROM workflow_definitions WHERE workflow_id=?1",
            [&id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).unwrap();
        assert_eq!(record, (definition.to_owned(), 7, 23, false));
        assert!(connection
            .execute("UPDATE workflow_definitions SET enabled=2", [])
            .is_err());
        drop(connection);
        let connection = Connection::open(&path).unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT content FROM messages WHERE id='existing-message'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Keep this conversation"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT definition_json FROM workflow_definitions WHERE workflow_id=?1",
                    [&id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            definition
        );
    }

    #[test]
    fn v54_workflow_availability_rejects_unknown_catalog_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&canonical_schema_v54()).unwrap();
        connection.pragma_update(None, "user_version", 54).unwrap();
        connection
            .execute_batch("CREATE TABLE unexpected_table (value TEXT);")
            .unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        assert!(run_migrations(&connection).is_err());
        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
        assert_eq!(read_schema_version(&connection).unwrap(), 54);
    }

    #[test]
    fn v53_workflow_migration_preserves_history_and_reopens() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("v53-workflows.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(&canonical_schema_v53()).unwrap();
        connection.pragma_update(None, "user_version", 53).unwrap();
        validate_schema_fingerprint(&connection, V53_SCHEMA_FINGERPRINT).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations(id,title,model_id,created_at,updated_at)
             VALUES ('existing-chat','Existing history','model',1,1);
             INSERT INTO messages(id,conversation_id,role,content,created_at,position)
             VALUES ('existing-message','existing-chat','user','Keep this conversation',1,0);",
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        let content: String = connection
            .query_row(
                "SELECT content FROM messages WHERE id='existing-message'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(content, "Keep this conversation");
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM workflow_definitions", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        drop(connection);
        let connection = Connection::open(&path).unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT title FROM conversations WHERE id='existing-chat'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "Existing history"
        );
    }

    #[test]
    fn v53_workflow_migration_rejects_unrecognized_schema_without_writes() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&canonical_schema_v53()).unwrap();
        connection.pragma_update(None, "user_version", 53).unwrap();
        connection
            .execute_batch("CREATE TABLE unexpected_table (value TEXT);")
            .unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        assert!(run_migrations(&connection).is_err());
        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
        assert_eq!(read_schema_version(&connection).unwrap(), 53);
    }

    #[test]
    fn v52_history_upgrades_without_rewriting_events_or_inventing_task_owners() {
        use crate::storage::{
            agent_collaboration_event_repository as events, agent_graph_repository as graph,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owners.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        connection.execute_batch(&canonical_schema_v52()).unwrap();
        connection.pragma_update(None, "user_version", 52).unwrap();
        connection
            .execute_batch(
                "INSERT INTO conversations(id,title,model_id,created_at,updated_at)
            VALUES ('root-chat','root','model',1,1),('child-chat','child','model',1,1);",
            )
            .unwrap();
        graph::ensure_root_agent(
            &mut connection,
            &crate::EnsureRootAgentInput {
                agent_id: "root".into(),
                conversation_id: "root-chat".into(),
                creation_request_id: "create-root".into(),
                task_name: "Root".into(),
            },
            1,
        )
        .unwrap();
        graph::create_agent_node(
            &mut connection,
            &crate::CreateAgentNodeInput {
                agent_id: "child".into(),
                root_agent_id: "root".into(),
                parent_agent_id: "root".into(),
                conversation_id: "child-chat".into(),
                creation_request_id: "create-child".into(),
                task_name: "Child".into(),
                task_path: "/root/Child".into(),
                template_snapshot: None,
                model_snapshot: crate::AgentModelSelectionSnapshot {
                    model_config_id: "model".into(),
                    display_name: "Model".into(),
                    supports_image: false,
                    effective_context_window_tokens: 64_000,
                    model_settings_configuration_revision: "model-v1".into(),
                    provider_connection_revision: "connection-v1".into(),
                    provider_protocol_revision: "protocol-v1".into(),
                },
            },
            2,
        )
        .unwrap();
        let request = crate::SendAgentMessageRequest {
            sender_agent_id: "root".into(),
            recipient_agent_id: "child".into(),
            request_id: "before-upgrade".into(),
            content: "keep original task text".into(),
        };
        graph::follow_up_agent(&mut connection, &request, 3).unwrap();
        let old_event_ids = connection
            .prepare("SELECT event_id FROM agent_collaboration_events ORDER BY global_sequence")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        let old_cursor = events::latest_root_sequence(&connection, "root").unwrap();
        connection.execute("INSERT INTO messages(id,conversation_id,role,content,agent_run_json,created_at,position)
            VALUES ('old-final','root-chat','assistant','original final',?1,4,0)",
            [r#"{"runId":"old-run","collaborationTimelineActivities":[{"parentAgentId":"root","parentConversationId":"root-chat"}],"toolResults":[{"result":"unchanged"}]}"#]).unwrap();
        let old_json: String = connection
            .query_row(
                "SELECT agent_run_json FROM messages WHERE id='old-final'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            events::latest_root_sequence(&connection, "root").unwrap(),
            old_cursor
        );
        let old_events = events::list_root_events(&connection, "root", 0, 512).unwrap();
        assert_eq!(
            old_events
                .iter()
                .map(|event| event.event_id.clone())
                .collect::<Vec<_>>(),
            old_event_ids
        );
        assert!(old_events
            .iter()
            .all(|event| event.schema_version == 3 && event.activities.is_empty()));
        let retained: String = connection
            .query_row(
                "SELECT agent_run_json FROM messages WHERE id='old-final'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(retained, old_json);
        drop(connection);
        let mut connection = Connection::open(&path).unwrap();
        run_migrations(&connection).unwrap();
        let sent = graph::follow_up_agent(
            &mut connection,
            &crate::SendAgentMessageRequest {
                request_id: "after-upgrade".into(),
                ..request
            },
            5,
        )
        .unwrap();
        let new_events = events::list_root_events(&connection, "root", old_cursor, 512).unwrap();
        let activities = new_events
            .iter()
            .flat_map(|event| &event.activities)
            .collect::<Vec<_>>();
        assert_eq!(activities.len(), 1);
        assert_eq!(activities[0].owner_agent_id, "root");
        assert_eq!(
            activities[0].task_message_id.as_deref(),
            Some(sent.message.message_id.as_str())
        );
        assert_eq!(
            activities[0].anchor_message_id.as_deref(),
            Some("old-final")
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();
    }

    #[test]
    fn exact_v51_mailbox_upgrade_preserves_history_sequences_and_accepts_full_text() {
        use crate::storage::agent_graph_repository::{
            create_agent_node, ensure_root_agent, get_agent_message, send_agent_message,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("mailbox.sqlite");
        let mut connection = Connection::open(&path).unwrap();
        connection.execute_batch(&canonical_schema_v51()).unwrap();
        connection.pragma_update(None, "user_version", 51).unwrap();
        for id in ["root-chat", "child-chat"] {
            connection
                .execute(
                    "INSERT INTO conversations(id,title,model_id,created_at,updated_at) VALUES (?1,?1,'model',1,1)",
                    [id],
                )
                .unwrap();
        }
        ensure_root_agent(
            &mut connection,
            &crate::EnsureRootAgentInput {
                agent_id: "root".into(),
                conversation_id: "root-chat".into(),
                creation_request_id: "root-create".into(),
                task_name: "Root".into(),
            },
            1,
        )
        .unwrap();
        create_agent_node(
            &mut connection,
            &crate::CreateAgentNodeInput {
                agent_id: "child".into(),
                root_agent_id: "root".into(),
                parent_agent_id: "root".into(),
                conversation_id: "child-chat".into(),
                creation_request_id: "child-create".into(),
                task_name: "Child".into(),
                task_path: "/root/Child".into(),
                template_snapshot: None,
                model_snapshot: crate::AgentModelSelectionSnapshot {
                    model_config_id: "model".into(),
                    display_name: "Model".into(),
                    supports_image: false,
                    effective_context_window_tokens: 64_000,
                    model_settings_configuration_revision: "model-v1".into(),
                    provider_connection_revision: "connection-v1".into(),
                    provider_protocol_revision: "protocol-v1".into(),
                },
            },
            2,
        )
        .unwrap();
        let old = send_agent_message(
            &mut connection,
            &crate::SendAgentMessageRequest {
                sender_agent_id: "child".into(),
                recipient_agent_id: "root".into(),
                request_id: "original".into(),
                content: "原始内容🙂".into(),
            },
            3,
        )
        .unwrap();
        connection
            .execute(
                "UPDATE sqlite_sequence SET seq=900 WHERE name='agent_mailbox_messages'",
                [],
            )
            .unwrap();
        let event_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM agent_collaboration_events",
                [],
                |row| row.get(0),
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            get_agent_message(&connection, &old.message.message_id)
                .unwrap()
                .unwrap(),
            old.message
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_collaboration_events",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            event_count
        );
        let text = "完整消息🙂".repeat(100_000);
        let sent = send_agent_message(
            &mut connection,
            &crate::SendAgentMessageRequest {
                sender_agent_id: "child".into(),
                recipient_agent_id: "root".into(),
                request_id: "full-after-upgrade".into(),
                content: text.clone(),
            },
            4,
        )
        .unwrap();
        assert_eq!(sent.message.sequence, 901);
        let id = sent.message.message_id;
        drop(connection);
        let reopened = Connection::open(&path).unwrap();
        run_migrations(&reopened).unwrap();
        assert_eq!(
            get_agent_message(&reopened, &id).unwrap().unwrap().content,
            text
        );
        assert!(reopened
            .execute(
                "UPDATE agent_mailbox_messages SET content='changed' WHERE message_id=?1",
                [&id]
            )
            .is_err());
        ensure_foreign_keys_are_valid(&reopened).unwrap();
    }

    #[test]
    fn empty_v50_event_catalog_upgrades_without_deleting_other_data() {
        let connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&canonical_schema_v50()).unwrap();
        connection.pragma_update(None, "user_version", 50).unwrap();
        connection
            .execute(
                "INSERT INTO projects (id, name, created_at, updated_at)
            VALUES ('kept-project', 'Keep settings and projects', 1, 1)",
                [],
            )
            .unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT name FROM projects WHERE id = 'kept-project'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "Keep settings and projects"
        );
        run_migrations(&connection).unwrap();
    }

    #[test]
    fn populated_v50_event_catalog_requires_manual_history_deletion_without_mutation() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(&canonical_schema_v50()).unwrap();
        connection.pragma_update(None, "user_version", 50).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (id, title, created_at, updated_at)
            VALUES ('old-conversation', 'Keep history', 1, 1)",
                [],
            )
            .unwrap();
        crate::storage::agent_graph_repository::ensure_root_agent(
            &mut connection,
            &crate::EnsureRootAgentInput {
                agent_id: "old-root".to_string(),
                conversation_id: "old-conversation".to_string(),
                creation_request_id: "old-root-create".to_string(),
                task_name: "root".to_string(),
            },
            1,
        )
        .unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        let error = run_migrations(&connection).unwrap_err();
        assert!(error
            .to_string()
            .contains("manually deleting old conversation histories"));
        assert_eq!(read_schema_version(&connection).unwrap(), 50);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_collaboration_events",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        connection
            .execute(
                "UPDATE conversations SET archived_at = 2 WHERE id = 'old-conversation'",
                [],
            )
            .unwrap();
        // Archiving hides the task but must not count as manually deleting its history.
        assert!(run_migrations(&connection).is_err());
        // The normal tree deletion service temporarily disables immutability triggers while
        // removing the explicitly selected root. Exercise the same deletion order on this old schema.
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
                false,
            )
            .unwrap();
        let deletion = connection.transaction().unwrap();
        deletion
            .execute_batch(
                "PRAGMA defer_foreign_keys = ON;
            DELETE FROM agent_collaboration_events WHERE root_agent_id = 'old-root';
            DELETE FROM agent_collaboration_event_sequences WHERE root_agent_id = 'old-root';",
            )
            .unwrap();
        crate::storage::chat_repository::delete_conversation(&deletion, "old-conversation")
            .unwrap();
        deletion
            .execute(
                "DELETE FROM agent_nodes WHERE root_agent_id = 'old-root'",
                [],
            )
            .unwrap();
        deletion.commit().unwrap();
        connection
            .set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
                true,
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM agent_collaboration_events",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        run_migrations(&connection).unwrap();
        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        run_migrations(&connection).unwrap();
    }

    const V46_SCHEMA_FINGERPRINT: &str =
        "sha256:6af5e743b2fd8ab799ff3fc702137b4dd8289b45420d5338a83b81c88b301179";
    const LOCAL_TOKEN_LEDGER_SCHEMA_MARKER: &str = "-- Independent local token ledger, schema v47.";

    fn canonical_schema_before_folder_refs() -> String {
        let mut schema = canonical_schema_v48();
        let columns = [
            "            folder_references_json TEXT NOT NULL DEFAULT '[]' CHECK (\n                json_valid(folder_references_json) AND json_type(folder_references_json) = 'array'\n            ),\n",
        ];
        for column in columns {
            schema = schema.replace(column, "");
        }
        schema
    }

    fn v47_schema() -> String {
        let schema = canonical_schema_before_folder_refs();
        assert!(schema.contains(HISTORY_SEARCH_INDEX_V48));
        schema.replace(
            HISTORY_SEARCH_INDEX_V48,
            include_str!("test_fixtures/history_search_v47.sql"),
        )
    }

    #[test]
    fn exact_v46_requires_reset_without_changing_user_data_or_creating_token_tables() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                v47_schema()
                    .split_once(LOCAL_TOKEN_LEDGER_SCHEMA_MARKER)
                    .unwrap()
                    .0,
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 46).unwrap();
        connection.execute_batch("INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('keep-chat','Keep me',1,1);").unwrap();
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            V46_SCHEMA_FINGERPRINT
        );
        let before_changes = connection.total_changes();
        let error = run_migrations(&connection).unwrap_err();
        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains("found 46"));
        assert_eq!(read_schema_version(&connection).unwrap(), 46);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            V46_SCHEMA_FINGERPRINT
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT title FROM conversations WHERE id='keep-chat'",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "Keep me"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_schema WHERE name LIKE 'local_token_usage_%'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn fresh_token_ledger_is_initialized_once_and_reopening_preserves_its_start_time() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        let start: i64 = connection
            .query_row(
                "SELECT started_at FROM local_token_usage_metadata",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(start > 0);
        run_migrations(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT started_at FROM local_token_usage_metadata",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            start
        );
    }

    #[test]
    fn modified_v46_catalog_requires_reset_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                CANONICAL_SCHEMA
                    .split_once(LOCAL_TOKEN_LEDGER_SCHEMA_MARKER)
                    .unwrap()
                    .0,
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 46).unwrap();
        connection
            .execute_batch("CREATE TABLE unexpected(value TEXT);")
            .unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();
        let error = run_migrations(&connection).unwrap_err();
        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(read_schema_version(&connection).unwrap(), 46);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
    }

    const MULTI_FOLDER_PROJECT_SCHEMA_MARKER: &str = "-- Multi-folder project roots, schema v45.";
    const V44_CANONICAL_SCHEMA_FINGERPRINT: &str =
        "sha256:358ad31ab6d85de12e335afaeb51e41870fa25c1c7b5ea792c55c91b1ab0241a";
    const PROJECTS_TABLE_V45: &str = "CREATE TABLE projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            pinned_at INTEGER,
            updated_at INTEGER NOT NULL
        );";
    const PROJECTS_TABLE_V44: &str = "CREATE TABLE projects (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT,
            created_at INTEGER NOT NULL,
            pinned_at INTEGER,
            updated_at INTEGER NOT NULL
        );";

    /// Rebuilds the exact v44 catalog: single-path projects and no `project_folders` table.
    fn v44_canonical_schema() -> String {
        let old_schema = v47_schema();
        let (before_multi_folder, _) = old_schema
            .split_once(MULTI_FOLDER_PROJECT_SCHEMA_MARKER)
            .unwrap();
        assert!(before_multi_folder.contains(PROJECTS_TABLE_V45));
        before_multi_folder.replacen(PROJECTS_TABLE_V45, PROJECTS_TABLE_V44, 1)
    }

    #[test]
    fn v45_workspace_schema_requires_explicit_reset_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                v47_schema()
                    .split_once("-- Frozen workspace membership, schema v46.")
                    .unwrap()
                    .0,
            )
            .unwrap();
        connection.pragma_update(None, "user_version", 45).unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        assert_eq!(
            before,
            "sha256:f8d106839487dda40070acc471675c9c4299bad1afb6aaf2a56d75807ce3b2f7"
        );
        let error = run_migrations(&connection).unwrap_err().to_string();
        assert!(error.contains("found 45"));
        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
    }

    #[test]
    fn a_v44_database_requires_reset_without_upgrading_single_path_projects() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("v44.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch(&v44_canonical_schema()).unwrap();
        connection.pragma_update(None, "user_version", 44).unwrap();
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            V44_CANONICAL_SCHEMA_FINGERPRINT,
            "the rebuilt v44 fixture must match the last single-path development catalog"
        );
        connection
            .execute_batch(
                "INSERT INTO projects(id,name,path,created_at,updated_at)
                 VALUES ('legacy-project','旧项目','/tmp/legacy',1,1);
                 INSERT INTO conversations(id,project_id,title,created_at,updated_at)
                 VALUES ('legacy-chat','legacy-project','旧对话',1,1);",
            )
            .unwrap();
        drop(connection);
        let before = std::fs::read(&path).unwrap();

        for _ in 0..2 {
            let connection = Connection::open(&path).unwrap();
            let error = run_migrations(&connection).unwrap_err();
            assert!(error
                .to_string()
                .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
            assert!(error.to_string().contains(&format!(
                "expected schema version {STORAGE_SCHEMA_VERSION}, found 44"
            )));
            assert_eq!(read_schema_version(&connection).unwrap(), 44);
            assert_eq!(
                schema_fingerprint(&connection).unwrap(),
                V44_CANONICAL_SCHEMA_FINGERPRINT
            );
            assert!(connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema WHERE name = 'project_folders'",
                    [],
                    |_| Ok(()),
                )
                .optional()
                .unwrap()
                .is_none());
            drop(connection);
        }
        assert_eq!(std::fs::read(&path).unwrap(), before);
    }

    #[test]
    fn fresh_database_stores_project_folders_with_a_single_primary_per_project() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO projects(id,name,created_at,updated_at) VALUES ('project','P',1,1);
                 INSERT INTO project_folders(id,project_id,path,alias,role,sort_order,created_at)
                 VALUES ('folder-a','project','/tmp/a','a','primary',0,1);
                 INSERT INTO project_folders(id,project_id,path,alias,role,sort_order,created_at)
                 VALUES ('folder-b','project','/tmp/b','b','auxiliary',1,1);",
            )
            .unwrap();
        let second_primary = connection.execute(
            "INSERT INTO project_folders(id,project_id,path,alias,role,sort_order,created_at)
             VALUES ('folder-c','project','/tmp/c','c','primary',2,1)",
            [],
        );
        assert!(
            second_primary.is_err(),
            "only one primary folder per project"
        );
        let duplicate_alias = connection.execute(
            "INSERT INTO project_folders(id,project_id,path,alias,role,sort_order,created_at)
             VALUES ('folder-d','project','/tmp/d','a','auxiliary',3,1)",
            [],
        );
        assert!(
            duplicate_alias.is_err(),
            "aliases are unique within a project"
        );
        connection
            .execute("DELETE FROM projects WHERE id = 'project'", [])
            .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM project_folders", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "folders cascade with their project"
        );
    }

    #[test]
    fn previous_development_versions_require_reset_without_any_write() {
        for (version, suffix) in [
            (34, "CREATE TABLE manual_context_compaction_operations ("),
            (35, "-- Human interaction schema v36."),
        ] {
            let fixture = tempfile::tempdir().unwrap();
            let path = fixture.path().join(format!("v{version}.sqlite"));
            let connection = Connection::open(&path).unwrap();
            connection
                .execute_batch(CANONICAL_SCHEMA.split_once(suffix).unwrap().0)
                .unwrap();
            connection
                .pragma_update(None, "user_version", version)
                .unwrap();
            connection.execute_batch(
                "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('old-history','保存历史',10,20);
                 INSERT INTO messages(id,conversation_id,role,content,status,created_at,position)
                 VALUES ('old-user','old-history','user','原始问题 /路径/中文','sent',10,0);
                 INSERT INTO agent_usage_records(id,conversation_id,message_id,run_id,model_id,model_name,created_at,input_tokens,output_tokens,billable_request_count)
                 VALUES ('old-usage','old-history','old-user','old-run','old-model','旧模型',20,100,20,1);"
            ).unwrap();
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            for _ in 0..2 {
                let connection = Connection::open(&path).unwrap();
                let error = run_migrations(&connection).unwrap_err();
                assert!(error
                    .to_string()
                    .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
                assert_eq!(read_schema_version(&connection).unwrap(), version);
                assert_eq!(
                    connection
                        .query_row(
                            "SELECT content FROM messages WHERE id='old-user'",
                            [],
                            |row| row.get::<_, String>(0)
                        )
                        .unwrap(),
                    "原始问题 /路径/中文"
                );
                assert_eq!(
                    connection
                        .query_row(
                            "SELECT input_tokens FROM agent_usage_records WHERE id='old-usage'",
                            [],
                            |row| row.get::<_, i64>(0)
                        )
                        .unwrap(),
                    100
                );
                assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_schema WHERE name='human_interaction_settings'", [], |row| row.get::<_,i64>(0)).unwrap(), 0);
            }
            assert_eq!(std::fs::read(path).unwrap(), before);
        }
    }

    #[test]
    fn human_interaction_schema_enforces_settlement_and_safe_storage_values() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        assert_eq!(connection.query_row(
            "SELECT enabled,revision,updated_at FROM human_interaction_settings WHERE singleton=1",
            [], |row| Ok((row.get::<_,i64>(0)?,row.get::<_,i64>(1)?,row.get::<_,i64>(2)?))
        ).unwrap(), (1,0,0));
        for value in ["0.5", "-1", "9007199254740992"] {
            for column in ["revision", "updated_at"] {
                assert!(connection
                    .execute(
                        &format!("UPDATE human_interaction_settings SET {column}={value}"),
                        []
                    )
                    .is_err());
            }
        }
        // Exercise local constraints without fabricating the separately validated root graph.
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection.execute_batch(
            "INSERT INTO human_interaction_requests(request_id,conversation_id,agent_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at) VALUES ('request','conversation','agent','run','assistant','tool','async','open',0,0,'[]',1,1);
             UPDATE human_interaction_requests SET status='submitted',revision=1,updated_at=2 WHERE request_id='request';"
        ).unwrap();
        assert!(connection
            .execute("UPDATE human_interaction_requests SET status='open'", [])
            .is_err());
        assert!(connection
            .execute(
                "UPDATE human_interaction_requests SET questions_json='[{}]'",
                []
            )
            .is_err());
        assert!(connection
            .execute("UPDATE human_interaction_requests SET revision=0.5", [])
            .is_err());
        connection.execute("INSERT INTO human_interaction_responses(response_id,request_id,submission_id,base_revision,kind,answers_json,created_at) VALUES ('response','request','submit',0,'submitted','[]',2)", []).unwrap();
        assert!(connection
            .execute(
                "UPDATE human_interaction_responses SET answers_json='[{}]'",
                []
            )
            .is_err());
        let oversize_answers = format!("[\"{}\"]", "a".repeat(262144));
        assert!(connection.execute("INSERT INTO human_interaction_responses(response_id,request_id,submission_id,base_revision,kind,answers_json,created_at) VALUES ('large','other','large',0,'submitted',?1,2)", [oversize_answers]).is_err());
        let oversize_checkpoint = format!("{{\"value\":\"{}\"}}", "a".repeat(1048576));
        assert!(connection.execute("INSERT INTO human_interaction_suspensions(request_id,run_id,assistant_message_id,tool_call_id,checkpoint_json,status,revision,created_at,updated_at) VALUES ('request','run','assistant','tool',?1,'waiting',0,1,1)", [oversize_checkpoint]).is_err());
    }

    #[test]
    fn ignored_history_schema_enforces_owner_shape_and_retains_deleted_target_receipts() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        // Seed only the ownership facts exercised here; the root graph is separately validated.
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        connection.execute_batch(
            "INSERT INTO conversations(id,title,created_at,updated_at) VALUES ('chat','Chat',1,1),('other','Other',1,1);
             INSERT INTO messages(id,conversation_id,role,content,created_at,position) VALUES
               ('source','chat','assistant','Source',1,0),
               ('target','chat','assistant','Target',1,1),
               ('deleted-target','chat','assistant','Deleted target',1,2),
               ('user-target','chat','user','User',1,3),
               ('foreign-target','other','assistant','Foreign',1,0);
             INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at,completed_at) VALUES
               ('source','chat','source-run',1,'completed',0,1,1,1),
               ('target','chat','target-run',1,'completed',0,1,1,1),
               ('deleted-target','chat','deleted-run',1,'completed',0,1,1,1),
               ('foreign-target','other','foreign-run',1,'completed',0,1,1,1);"
        ).unwrap();
        for index in 0..5 {
            let status = if index == 4 { "submitted" } else { "ignored" };
            connection.execute(
                "INSERT INTO human_interaction_requests(request_id,conversation_id,agent_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at)
                 VALUES (?1,'chat','unused-root','source-run','source',?1,'async',?2,1,0,'[]',1,1)",
                rusqlite::params![format!("request-{index}"), status],
            ).unwrap();
            connection.execute(
                "INSERT INTO human_interaction_responses(response_id,request_id,submission_id,base_revision,kind,answers_json,created_at) VALUES (?1,?2,?1,0,?3,'[]',1)",
                rusqlite::params![format!("response-{index}"), format!("request-{index}"), status],
            ).unwrap();
        }
        connection
            .pragma_update(None, "foreign_keys", true)
            .unwrap();
        let insert = |response: &str,
                      target: &str,
                      run: &str,
                      status: &str,
                      sequence: Option<i64>,
                      batch: Option<i64>| {
            connection.execute(
                "INSERT INTO human_interaction_ignored_projections(response_id,conversation_id,target_assistant_message_id,target_run_id,placement,status,trace_sequence,model_batch_index)
                 VALUES (?1,'chat',?2,?3,'timeline',?4,?5,?6)",
                rusqlite::params![response,target,run,status,sequence,batch],
            )
        };
        for (target, run) in [
            ("target", "wrong-run"),
            ("foreign-target", "foreign-run"),
            ("user-target", "target-run"),
            ("missing-target", "target-run"),
        ] {
            assert!(insert("response-0", target, run, "pending", None, None).is_err());
        }
        assert!(insert("response-4", "target", "target-run", "pending", None, None).is_err());
        for (status, sequence, batch) in [
            ("pending", Some(0), None),
            ("pending", None, Some(1)),
            ("materialized", None, None),
            ("materialized", Some(-1), None),
            ("materialized", Some(0), Some(0)),
            ("unknown", None, None),
        ] {
            assert!(insert(
                "response-0",
                "target",
                "target-run",
                status,
                sequence,
                batch
            )
            .is_err());
        }
        insert("response-0", "target", "target-run", "pending", None, None).unwrap();
        insert(
            "response-1",
            "deleted-target",
            "deleted-run",
            "materialized",
            Some(0),
            None,
        )
        .unwrap();
        insert(
            "response-2",
            "deleted-target",
            "deleted-run",
            "pending",
            None,
            None,
        )
        .unwrap();
        assert!(insert(
            "response-3",
            "deleted-target",
            "deleted-run",
            "materialized",
            Some(0),
            None
        )
        .is_err());
        assert!(connection.execute("UPDATE human_interaction_ignored_projections SET target_run_id='wrong-run' WHERE response_id='response-0'", []).is_err());
        connection.execute("UPDATE human_interaction_ignored_projections SET status='materialized',trace_sequence=0,model_batch_index=1 WHERE response_id='response-0'", []).unwrap();
        assert!(connection.execute("UPDATE human_interaction_ignored_projections SET status='pending',trace_sequence=NULL,model_batch_index=NULL WHERE response_id='response-0'", []).is_err());

        connection
            .execute("DELETE FROM messages WHERE id='deleted-target'", [])
            .unwrap();
        for (response, expected_status, expected_sequence) in [
            ("response-1", "materialized", Some(0)),
            ("response-2", "cancelled", None),
        ] {
            let receipt = connection.query_row(
                "SELECT status,target_assistant_message_id,trace_sequence FROM human_interaction_ignored_projections WHERE response_id=?1",
                [response],
                |row| Ok((row.get::<_,String>(0)?, row.get::<_,Option<String>>(1)?, row.get::<_,Option<i64>>(2)?)),
            ).unwrap();
            assert_eq!(
                receipt,
                (expected_status.to_string(), None, expected_sequence)
            );
            assert!(connection.execute("UPDATE human_interaction_ignored_projections SET target_assistant_message_id='target',target_run_id='target-run' WHERE response_id=?1", [response]).is_err());
        }
        connection.execute(
            "INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES ('target',0,'backend_state','{\"type\":\"backend_state\",\"sequence\":0}')", [],
        ).unwrap();
    }

    #[test]
    fn human_interaction_request_sequence_orders_equal_timestamps_and_never_reuses_ids() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .pragma_update(None, "foreign_keys", false)
            .unwrap();
        let insert = |sequence: Option<i64>, id: &str| {
            connection.execute(
            "INSERT INTO human_interaction_requests(sequence,request_id,conversation_id,agent_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at)
             VALUES (?1,?2,'conversation','agent','run','assistant',?2,'async','open',0,0,'[]',1,1)",
            rusqlite::params![sequence,id],
        )
        };
        insert(None, "z-first").unwrap();
        insert(None, "a-second").unwrap();
        assert_eq!(connection.query_row(
            "SELECT request_id FROM human_interaction_requests ORDER BY sequence DESC LIMIT 1",
            [], |row| row.get::<_,String>(0),
        ).unwrap(), "a-second");
        assert!(connection
            .execute(
                "UPDATE human_interaction_requests SET sequence=99 WHERE request_id='z-first'",
                []
            )
            .is_err());
        connection
            .execute(
                "DELETE FROM human_interaction_requests WHERE request_id='a-second'",
                [],
            )
            .unwrap();
        insert(None, "m-third").unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT sequence FROM human_interaction_requests WHERE request_id='m-third'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            3
        );
        assert!(insert(Some(0), "invalid-zero").is_err());
        assert!(insert(Some(9_007_199_254_740_992), "invalid-overflow").is_err());
    }

    fn replace_once(haystack: String, old: &str, new: &str) -> String {
        assert_eq!(
            haystack.matches(old).count(),
            1,
            "schema fixture replacement must have exactly one match"
        );
        haystack.replacen(old, new, 1)
    }

    fn with_legacy_model_credentials_for_test(schema: String) -> String {
        let schema = replace_once(
            schema,
            r#"            api_token_ref TEXT CHECK (
                api_token_ref IS NULL OR (
                    typeof(api_token_ref) = 'text'
                    AND length(CAST(api_token_ref AS BLOB)) BETWEEN 1 AND 1024
                )
            ),"#,
            "            api_token TEXT NOT NULL,",
        );
        let schema = replace_once(
            schema,
            r#"            tavily_api_key_ref TEXT CHECK (
                tavily_api_key_ref IS NULL OR (
                    typeof(tavily_api_key_ref) = 'text'
                    AND length(CAST(tavily_api_key_ref AS BLOB)) BETWEEN 1 AND 1024
                )
            ),"#,
            "            tavily_api_key TEXT NOT NULL,",
        );
        let schema = replace_once(
            schema,
            r#"CREATE TABLE model_provider_credential_staging (
            credential_ref TEXT PRIMARY KEY CHECK (
                typeof(credential_ref) = 'text'
                AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );
CREATE TABLE model_provider_credential_cleanup (
            credential_ref TEXT PRIMARY KEY CHECK (
                typeof(credential_ref) = 'text'
                AND length(CAST(credential_ref AS BLOB)) BETWEEN 1 AND 1024
            ),
            created_at INTEGER NOT NULL CHECK (created_at >= 0)
        );
"#,
            "",
        );
        replace_once(
            schema,
            r#"            api_token_override_ref TEXT CHECK (
                api_token_override_ref IS NULL OR (
                    typeof(api_token_override_ref) = 'text'
                    AND length(CAST(api_token_override_ref AS BLOB)) BETWEEN 1 AND 1024
                )
            ),"#,
            "            api_token_override TEXT,",
        )
    }

    fn canonical_schema_v29_for_test() -> String {
        let schema = replace_once(
            canonical_schema_before_folder_refs(),
            r#"            provider_model_id TEXT NOT NULL,
            display_name TEXT NOT NULL CHECK (
                typeof(display_name) = 'text'
                AND length(CAST(display_name AS BLOB)) BETWEEN 1 AND 512
                AND display_name = trim(display_name)
            ),
            normalized_display_name TEXT NOT NULL,
"#,
            r#"            display_name TEXT NOT NULL,
"#,
        );
        let schema = replace_once(
            schema,
            r#"CREATE UNIQUE INDEX idx_models_normalized_display_name
            ON models(normalized_display_name);
"#,
            "",
        );
        with_legacy_model_credentials_for_test(schema)
    }

    fn canonical_schema_v30_for_test() -> String {
        let schema = replace_once(
            CANONICAL_SCHEMA.to_string(),
            r#"            provider_model_id TEXT NOT NULL,
            display_name TEXT NOT NULL CHECK (
                typeof(display_name) = 'text'
                AND length(CAST(display_name AS BLOB)) BETWEEN 1 AND 512
                AND display_name = trim(display_name)
            ),
            normalized_display_name TEXT NOT NULL,
"#,
            r#"            display_id TEXT NOT NULL,
            normalized_display_id TEXT NOT NULL,
            provider_model_id TEXT NOT NULL,
            display_name TEXT NOT NULL,
"#,
        );
        let schema = replace_once(
            schema,
            r#"CREATE UNIQUE INDEX idx_models_normalized_display_name
            ON models(normalized_display_name);
"#,
            r#"CREATE UNIQUE INDEX idx_models_normalized_display_id
            ON models(normalized_display_id);
"#,
        );
        with_legacy_model_credentials_for_test(schema)
    }

    fn canonical_schema_v28_for_test() -> String {
        let schema = replace_once(
            canonical_schema_v29_for_test(),
            r#"            agent_run_json TEXT CHECK (
                agent_run_json IS NULL OR json_valid(agent_run_json)
            ),
            created_at INTEGER NOT NULL,"#,
            r#"            agent_run_json TEXT CHECK (
                agent_run_json IS NULL OR json_valid(agent_run_json)
            ),
            ui_state_json TEXT CHECK (
                ui_state_json IS NULL OR json_valid(ui_state_json)
            ),
            created_at INTEGER NOT NULL,"#,
        );
        let schema = replace_once(
            schema,
            r#"CREATE TABLE chat_message_ui_states (
            message_id TEXT PRIMARY KEY NOT NULL,
            ui_state_json TEXT NOT NULL CHECK (json_valid(ui_state_json)),
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
"#,
            "",
        );
        let schema = replace_once(
            schema,
            r#"          OR NEW.agent_run_json IS NOT OLD.agent_run_json
          OR NEW.created_at IS NOT OLD.created_at"#,
            r#"          OR NEW.agent_run_json IS NOT OLD.agent_run_json
          OR NEW.ui_state_json IS NOT OLD.ui_state_json
          OR NEW.created_at IS NOT OLD.created_at"#,
        );
        replace_once(
            schema,
            r#"CREATE TRIGGER prevent_agent_message_projection_run_state_rewrite
        BEFORE UPDATE OF agent_run_json ON messages
        WHEN OLD.input_origin_kind = 'agent'
          AND NEW.agent_run_json IS NOT OLD.agent_run_json
        BEGIN
            SELECT RAISE(ABORT, 'Agent input projection cannot carry mutable run state');
        END;"#,
            r#"CREATE TRIGGER prevent_agent_message_projection_ui_rewrite
        BEFORE UPDATE OF agent_run_json, ui_state_json ON messages
        WHEN OLD.input_origin_kind = 'agent' AND (
            NEW.agent_run_json IS NOT OLD.agent_run_json
            OR NEW.ui_state_json IS NOT OLD.ui_state_json
        )
        BEGIN
            SELECT RAISE(ABORT, 'Agent input projection cannot carry mutable run UI state');
        END;"#,
        )
    }

    fn create_v28_fixture(connection: &Connection) {
        connection
            .execute_batch(&canonical_schema_v28_for_test())
            .unwrap();
        connection.pragma_update(None, "user_version", 28).unwrap();
    }

    fn create_v29_fixture(connection: &Connection) {
        connection
            .execute_batch(&canonical_schema_v29_for_test())
            .unwrap();
        connection.pragma_update(None, "user_version", 29).unwrap();
    }

    fn create_v30_fixture(connection: &Connection) {
        connection
            .execute_batch(&canonical_schema_v30_for_test())
            .unwrap();
        connection.pragma_update(None, "user_version", 30).unwrap();
    }

    fn seed_v29_model_identity_fixture(connection: &Connection) {
        connection
            .execute_batch(
                r#"
                INSERT INTO models (
                    id, display_name, api_url_override, api_token_override,
                    supports_image, context_window_tokens, provider_profile_config_json,
                    provider_connection_revision, provider_protocol_revision,
                    input_price, cached_input_price, output_price, enabled, position,
                    created_at, updated_at
                ) VALUES
                (
                    'Kimi-K3', 'First Kimi', NULL, NULL, 0, 128000,
                    '{"schemaVersion":1,"profile":{"id":"generic_openai_chat","version":1},"reasoning":{"mode":"provider_default","effort":"provider_default"}}',
                    'provider-connection-v1:first', 'provider-protocol-v1:first',
                    '0', '', '0', 1, 0, 10, 10
                ),
                (
                    'Ｋｉｍｉ－Ｋ３', 'Second Kimi', NULL, NULL, 0, 128000,
                    '{"schemaVersion":1,"profile":{"id":"generic_openai_chat","version":1},"reasoning":{"mode":"provider_default","effort":"provider_default"}}',
                    'provider-connection-v1:second', 'provider-protocol-v1:second',
                    '0', '', '0', 1, 1, 11, 11
                );
                INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                ) VALUES (
                    'conversation-v29', NULL, 'Ｋｉｍｉ－Ｋ３', 'legacy model ref',
                    12, 12, NULL, NULL, NULL
                );
                INSERT INTO automations (
                    id, schema_version, create_request_id, title, prompt, status,
                    health_state, destination_kind, project_binding_kind, model_id,
                    permission_mode, permission_mode_version, permissions_json, reasoning_json,
                    schedule_kind, schedule_json, rrule, timezone, anchor_at, next_run_at,
                    notification_policy, revision, created_at, updated_at
                ) VALUES (
                    'automation-v29', 1, 'request-v29', 'v29 task', 'run it', 'active',
                    'ok', 'new_chat', 'none', 'Ｋｉｍｉ－Ｋ３',
                    'default', 1, '{}', '{}',
                    'interval', '{}', 'FREQ=DAILY', 'UTC', 12, 13,
                    'all_runs', 1, 12, 12
                );
                "#,
            )
            .unwrap();
        ensure_foreign_keys_are_valid(connection).unwrap();
    }

    fn seed_v28_message_with_ui_state(connection: &Connection) {
        connection
            .execute_batch(
                "INSERT INTO conversations (
                     id, project_id, model_id, title, created_at, updated_at,
                     pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-v28', NULL, NULL, 'v28', 1, 1, NULL, NULL, NULL);
                 INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     ui_state_json, created_at, position
                 ) VALUES (
                     'message-v28', 'conversation-v28', 'assistant', 'preserved', 'sent',
                     NULL, '{\"isBookmarked\":true}', 2, 0
                 );
                 INSERT INTO model_provider_settings (
                     id, api_url, api_token, search_mode, tavily_api_key,
                     configuration_revision, search_connection_revision, updated_at
                 ) VALUES (
                     'default', 'https://provider.example/v1', 'token-sentinel',
                     'tavily', 'search-token-sentinel',
                     'model-settings-v1:fixture', 'search-connection-v1:fixture', 3
                 );
                 INSERT INTO models (
                     id, display_name, api_url_override, api_token_override,
                     supports_image, context_window_tokens, provider_profile_config_json,
                     provider_connection_revision, provider_protocol_revision,
                     input_price, cached_input_price, output_price, enabled, position,
                     created_at, updated_at
                 ) VALUES (
                     'language-model-v28', 'Language model v28',
                     'https://model.example/v1', 'model-token-sentinel', 1, 131072,
                     '{\"schemaVersion\":1,\"profile\":{\"id\":\"generic_openai_chat\",\"version\":1},\"reasoning\":{\"mode\":\"provider_default\",\"effort\":\"provider_default\"}}',
                     'provider-connection-v1:fixture', 'provider-protocol-v1:fixture',
                     '1.25', '0.125', '5.5', 1, 7, 3, 4
                 );
                 INSERT INTO image_generation_profiles (
                     id, schema_version, adapter_id, endpoint_url, model_id,
                     credential_ref, enabled, text_to_image, image_to_image,
                     default_size_preset, default_watermark, generation,
                     created_at, updated_at
                 ) VALUES (
                     'image-profile-v28', 1, 'smartmlSeedream',
                     'https://image.example/generate', 'image-model-v28',
                     'image-credential-v28', 1, 1, 1, '2K', 0, 4, 3, 4
                 );",
            )
            .unwrap();
    }

    #[test]
    fn legacy_v28_schema_requires_explicit_development_reset_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        create_v28_fixture(&connection);
        seed_v28_message_with_ui_state(&connection);
        let fingerprint_before = schema_fingerprint(&connection).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert_eq!(read_schema_version(&connection).unwrap(), 28);
        assert_eq!(schema_fingerprint(&connection).unwrap(), fingerprint_before);
        assert_eq!(
            connection
                .query_row(
                    "SELECT ui_state_json FROM messages WHERE id = 'message-v28'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "{\"isBookmarked\":true}"
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();
    }

    #[test]
    fn legacy_v29_schema_requires_explicit_development_reset_without_backfill() {
        let connection = Connection::open_in_memory().unwrap();
        create_v29_fixture(&connection);
        seed_v29_model_identity_fixture(&connection);
        let fingerprint_before = schema_fingerprint(&connection).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert_eq!(read_schema_version(&connection).unwrap(), 29);
        assert_eq!(schema_fingerprint(&connection).unwrap(), fingerprint_before);
        let columns = connection
            .prepare("PRAGMA table_info(models)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!columns.iter().any(|column| column == "display_id"));
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM conversations WHERE id = 'conversation-v29'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Ｋｉｍｉ－Ｋ３"
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT model_id FROM automations WHERE id = 'automation-v29'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "Ｋｉｍｉ－Ｋ３"
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();
    }

    #[test]
    fn legacy_v30_schema_requires_explicit_development_reset_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        create_v30_fixture(&connection);
        let fingerprint_before = schema_fingerprint(&connection).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 30"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 30);
        assert_eq!(schema_fingerprint(&connection).unwrap(), fingerprint_before);
        let columns = connection
            .prepare("PRAGMA table_info(models)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.iter().any(|column| column == "display_id"));
        assert!(!columns
            .iter()
            .any(|column| column == "normalized_display_name"));
        ensure_foreign_keys_are_valid(&connection).unwrap();
    }

    #[test]
    fn fresh_database_creates_the_canonical_schema() {
        let connection = Connection::open_in_memory().unwrap();

        run_migrations(&connection).unwrap();

        assert_eq!(
            read_schema_version(&connection).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            CANONICAL_SCHEMA_FINGERPRINT
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();

        let model_columns = connection
            .prepare("PRAGMA table_info(models)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(!model_columns.iter().any(|column| column == "display_id"));
        assert!(!model_columns
            .iter()
            .any(|column| column == "normalized_display_id"));
        assert!(model_columns
            .iter()
            .any(|column| column == "normalized_display_name"));

        for required_object in [
            "conversation_world_state_request_commits",
            "validate_conversation_world_state_request_anchor_insert",
            "validate_conversation_world_state_request_anchor_update",
            "idx_conversation_world_state_request_anchor",
            "models",
            "agent_templates",
            "project_agent_template_bindings",
            "project_agent_template_bindings_template",
            "validate_project_agent_template_binding_limit",
            "prevent_agent_template_identity_update",
            "validate_agent_template_revision_update",
            "agent_nodes",
            "agent_nodes_root_conversation_identity",
            "agent_nodes_parent",
            "agent_nodes_conversation",
            "agent_effective_permission_snapshots",
            "agent_effective_permission_snapshots_root",
            "validate_agent_effective_permission_snapshot_source_insert",
            "validate_agent_effective_permission_snapshot_source_update",
            "prevent_agent_effective_permission_snapshot_identity_update",
            "validate_agent_effective_permission_snapshot_revision",
            "validate_agent_node_project_insert",
            "validate_child_agent_conversation_fresh_insert",
            "validate_child_agent_conversation_model_insert",
            "validate_agent_node_parent_path_insert",
            "validate_agent_node_template_snapshot_insert",
            "prevent_agent_node_identity_update",
            "validate_agent_node_lifecycle_update",
            "prevent_agent_lifecycle_deactivation_with_pending_wake",
            "prevent_agent_lifecycle_deactivation_with_active_turn",
            "prevent_agent_lifecycle_deactivation_with_pending_mailbox",
            "prevent_agent_lifecycle_deactivation_with_active_children",
            "validate_agent_lifecycle_activation_parent",
            "prevent_agent_bound_conversation_project_update",
            "prevent_child_agent_conversation_model_update",
            "agent_mailbox_messages",
            "validate_agent_mailbox_active_participants_insert",
            "validate_agent_mailbox_unbound_quota_insert",
            "validate_agent_mailbox_kind_authority_insert",
            "agent_mailbox_recipient_pending",
            "agent_mailbox_claim_token_identity",
            "agent_mailbox_one_claimed_per_recipient",
            "validate_agent_mailbox_projection_identity_insert",
            "prevent_agent_mailbox_identity_update",
            "prevent_agent_mailbox_delete",
            "validate_agent_mailbox_delivery_transition",
            "prevent_agent_mailbox_claim_reassignment",
            "validate_agent_mailbox_lease_update",
            "prevent_agent_mailbox_claim_time_rewrite",
            "prevent_agent_mailbox_acknowledgement_rewrite",
            "agent_wake_requests",
            "validate_agent_wake_active_target_insert",
            "validate_agent_wake_source_authority_insert",
            "validate_agent_wake_claim_source_delivered",
            "agent_wake_one_active_turn",
            "agent_wake_dispatch_queue",
            "agent_wake_claim_token_identity",
            "prevent_agent_wake_identity_update",
            "prevent_agent_wake_delete",
            "validate_agent_wake_transition",
            "validate_agent_wake_status_revision",
            "prevent_agent_wake_claim_reassignment",
            "validate_agent_wake_lease_update",
            "validate_agent_wake_result_kind",
            "prevent_agent_wake_execution_identity_rewrite",
            "validate_agent_wake_execution_identity_bind",
            "agent_interrupt_requests",
            "prevent_agent_interrupt_request_rewrite",
            "agent_model_batch_receipts",
            "agent_model_batch_receipts_conversation_run",
            "validate_agent_model_batch_receipt_identity_insert",
            "prevent_agent_model_batch_receipt_identity_update",
            "prevent_agent_model_batch_receipt_reopen",
            "prevent_agent_model_batch_receipt_delete",
            "agent_model_batch_receipt_replays",
            "validate_agent_model_batch_receipt_replay_insert",
            "prevent_agent_model_batch_receipt_replay_update",
            "prevent_agent_model_batch_receipt_replay_delete",
            "agent_model_batch_receipt_items",
            "agent_model_batch_receipt_items_sequence",
            "validate_agent_model_batch_receipt_item_insert",
            "prevent_agent_model_batch_receipt_item_update",
            "prevent_agent_model_batch_receipt_item_delete",
            "agent_model_batch_receipt_targets",
            "validate_agent_model_batch_receipt_target_insert",
            "prevent_agent_model_batch_receipt_target_update",
            "prevent_agent_model_batch_receipt_target_delete",
            "validate_agent_wake_satisfied_receipt",
            "agent_collaboration_cursors",
            "agent_collaboration_cursors_target",
            "validate_agent_collaboration_cursor_authority_insert",
            "validate_agent_collaboration_cursor_update",
            "prevent_agent_wake_claim_time_rewrite",
            "prevent_agent_wake_start_time_rewrite",
            "prevent_agent_wake_terminal_rewrite",
            "agent_tree_run_stops",
            "agent_tree_run_stops_root",
            "agent_tree_run_stop_members",
            "agent_tree_run_stop_members_run",
            "validate_agent_tree_run_stop_member_insert",
            "prevent_agent_tree_run_stop_member_update",
            "validate_agent_tree_run_stop_insert",
            "prevent_agent_tree_run_stop_update",
            "prevent_stopped_agent_run_pending_action_insert",
            "child_context_snapshots",
            "child_context_snapshots_source_idx",
            "validate_child_context_snapshot_insert",
            "prevent_child_context_snapshot_update",
            "prevent_child_context_snapshot_delete",
            "messages_snapshot_source_idx",
            "validate_child_context_snapshot_message_insert",
            "validate_context_snapshot_message_update",
            "prevent_child_context_snapshot_message_rewrite",
            "prevent_child_context_snapshot_message_delete",
            "validate_agent_message_projection_insert",
            "prevent_human_input_to_child_agent",
            "prevent_human_input_update_to_child_agent",
            "validate_agent_message_projection_update",
            "prevent_agent_message_projection_rewrite",
            "prevent_agent_message_projection_delete",
            "prevent_agent_message_projection_run_state_rewrite",
            "chat_message_ui_states",
            "validate_agent_mailbox_acknowledgement",
            "prevent_agent_bound_conversation_fork_insert",
            "prevent_conversation_fork_update",
            "agent_member_conversation_forks",
            "agent_member_conversation_forks_source_root",
            "agent_member_conversation_forks_target_root",
            "validate_agent_member_conversation_fork_insert",
            "prevent_agent_member_conversation_fork_update",
            "conversations_revision_after_business_update",
            "conversations_revision_after_message_insert",
            "conversations_revision_after_message_update",
            "conversations_revision_after_message_delete",
            "conversation_turn_traces",
            "conversation_turn_traces_one_active_turn",
            "conversation_turn_rewrites",
            "conversation_turn_rewrites_conversation",
            "agent_file_changes",
            "agent_file_change_chunks",
            "agent_file_change_operations",
            "idx_agent_file_changes_run_id",
            "agent_file_changes_source_call_identity",
            "agent_file_change_run_grants",
            "agent_file_change_run_grants_one_active_per_run",
            "validate_conversation_turn_rewrite_insert",
            "prevent_conversation_turn_rewrite_update",
            "prevent_conversation_turn_rewrite_delete",
            "provider_continuations",
            "provider_continuations_projection_identity",
            "prevent_provider_continuation_projection_rewrite",
            "validate_provider_continuation_tool_call_projection",
            "context_compaction_summaries",
            "conversation_forks",
            "agent_command_sessions",
            "mcp_registry_metadata",
            "mcp_registry_servers",
            "mcp_registry_model_namespaces",
            "mcp_registry_servers_revision",
            "mcp_builtin_capability_metadata",
            "mcp_builtin_capability_policies",
            "browser_download_settings",
            "browser_preferences",
            "browser_history",
            "browser_history_visited_idx",
            "browser_history_hostname_idx",
            "browser_downloads",
            "browser_downloads_created_idx",
            "browser_downloads_conversation_idx",
            "browser_downloads_project_idx",
            "conversation_history_fts",
            "conversation_history_fts_message_insert",
            "validate_agent_command_session_model_receipt_payload_insert",
            "agent_collaboration_event_sequences",
            "agent_collaboration_events",
            "agent_collaboration_events_root_sequence",
            "agent_collaboration_events_global_sequence",
            "validate_agent_collaboration_event_identity_insert",
            "validate_agent_collaboration_event_sequence_insert",
            "validate_agent_collaboration_event_sequence_update",
            "prevent_agent_collaboration_event_sequence_delete",
            "prevent_agent_collaboration_event_update",
            "prevent_agent_collaboration_event_delete",
            "emit_agent_created_collaboration_event",
            "emit_agent_updated_collaboration_event",
            "emit_root_conversation_model_updated_collaboration_event",
            "emit_agent_mailbox_enqueued_collaboration_event",
            "emit_agent_mailbox_updated_collaboration_event",
            "emit_agent_wake_created_collaboration_event",
            "emit_agent_wake_updated_collaboration_event",
            "emit_agent_turn_started_collaboration_event",
            "emit_agent_turn_updated_collaboration_event",
            "emit_agent_approval_projected_collaboration_event",
            "emit_agent_approval_updated_collaboration_event",
            "automations",
            "automations_list_idx",
            "automations_due_idx",
            "automations_attention_idx",
            "automation_runs",
            "automation_runs_scheduled_occurrence",
            "automation_runs_manual_request",
            "automation_runs_one_nonterminal_per_task",
            "automation_runs_history_idx",
            "automation_runs_recovery_idx",
            "automation_runs_cancellation_idx",
            "automation_runs_attention_idx",
            "automation_events",
            "automation_events_task_sequence_idx",
            "automation_events_run_sequence_idx",
            "automation_events_lookup_idx",
            "notification_settings",
            "notification_batches",
            "notification_batches_delivery_idx",
            "notification_batches_replace_idx",
            "notification_events",
            "notification_events_center_idx",
            "notification_events_unread_idx",
            "notification_events_supersession_idx",
            "notification_events_approval_idx",
            "notification_events_conversation_idx",
            "notification_batch_items",
            "notification_batch_items_batch_idx",
            "notification_change_events",
            "notification_change_events_sequence_idx",
            "resolve_superseded_notification_before_insert",
            "aggregate_notification_event_after_insert",
            "block_automations_before_conversation_delete",
            "block_automations_after_conversation_archive",
            "block_automations_before_project_delete",
            "block_automations_before_model_delete",
            "block_automations_after_model_disable",
            "emit_automation_attention_event_after_block",
            "emit_automation_blocked_notification_after_block",
            "terminate_unadmitted_automation_run_after_block",
        ] {
            let exists = connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema
                     WHERE name = ?1 AND sql IS NOT NULL",
                    [required_object],
                    |_| Ok(()),
                )
                .optional()
                .unwrap()
                .is_some();
            assert!(exists, "missing canonical schema object {required_object}");
        }

        for retired_goal_object in [
            "conversation_goals",
            "conversation_goal_revisions",
            "validate_conversation_goal_source_insert",
            "validate_conversation_goal_source_update",
            "conversation_goal_revisions_conversation",
            "prevent_conversation_goal_revision_update",
            "prevent_conversation_goal_revision_delete",
        ] {
            let exists = connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema
                     WHERE name = ?1 AND sql IS NOT NULL",
                    [retired_goal_object],
                    |_| Ok(()),
                )
                .optional()
                .unwrap()
                .is_some();
            assert!(
                !exists,
                "retired Goal schema object {retired_goal_object} must stay absent"
            );
        }

        for retired_file_draft_object in [
            "agent_file_drafts",
            "agent_file_draft_chunks",
            "agent_file_draft_operations",
            "idx_agent_file_drafts_conversation_status",
            "idx_agent_file_drafts_project_id",
            "idx_agent_file_drafts_expires_at",
        ] {
            let exists = connection
                .query_row(
                    "SELECT 1 FROM sqlite_schema WHERE name = ?1 AND sql IS NOT NULL",
                    [retired_file_draft_object],
                    |_| Ok(()),
                )
                .optional()
                .unwrap()
                .is_some();
            assert!(
                !exists,
                "retired file-draft object {retired_file_draft_object} must stay absent"
            );
        }

        let maintenance_table_exists = connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND name = 'maintenance_tasks'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some();
        assert!(!maintenance_table_exists);
    }

    #[test]
    fn canonical_model_display_name_enforces_the_512_byte_trimmed_boundary() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        let insert = |id: &str, display_name: &str| {
            connection.execute(
                "INSERT INTO models (
                    id, provider_model_id, display_name, normalized_display_name,
                    supports_image, provider_profile_config_json,
                    provider_connection_revision, provider_protocol_revision,
                    input_price, cached_input_price, output_price, enabled,
                    position, created_at, updated_at
                 ) VALUES (
                    ?1, 'provider-model', ?2, ?2, 0, '{}',
                    'provider-connection-v1:test', 'provider-protocol-v1:test',
                    '0', '', '0', 1, 0, 1, 1
                 )",
                rusqlite::params![id, display_name],
            )
        };

        assert_eq!(insert("boundary", &"x".repeat(512)).unwrap(), 1);
        assert!(insert("oversized", &"x".repeat(513)).is_err());
        assert!(insert("untrimmed", " Model ").is_err());
    }

    #[test]
    fn canonical_agent_member_fork_receipts_enforce_tree_authority_and_snapshot_inserts() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();

        connection
            .execute_batch(
                "INSERT INTO projects (id, name, created_at, pinned_at, updated_at)
                 VALUES
                    ('project-a', 'Project A', 1, NULL, 1),
                    ('project-b', 'Project B', 1, NULL, 1);",
            )
            .unwrap();

        let insert_conversation = |id: &str, project_id: &str, model_id: Option<&str>| {
            connection.execute(
                "INSERT INTO conversations (
                         id, project_id, model_id, title, created_at, updated_at,
                         pinned_at, archived_at, unread_at
                     ) VALUES (?1, ?2, ?3, 'Fork fixture', 1, 1, NULL, NULL, NULL)",
                rusqlite::params![id, project_id, model_id],
            )
        };
        for (id, project_id, model_id) in [
            ("source-root-conversation", "project-a", None),
            ("target-root-conversation", "project-a", None),
            ("source-child-conversation", "project-a", Some("model-a")),
            ("target-child-conversation", "project-a", Some("model-a")),
            ("source-grand-conversation", "project-a", Some("model-a")),
            ("target-grand-conversation", "project-a", Some("model-a")),
            ("target-wrong-conversation", "project-a", Some("model-a")),
            ("other-root-conversation", "project-b", None),
            ("other-child-conversation", "project-b", Some("model-a")),
        ] {
            insert_conversation(id, project_id, model_id).unwrap();
        }

        let insert_root =
            |agent_id: &str, conversation_id: &str, project_id: &str, request_id: &str| {
                connection.execute(
                    "INSERT INTO agent_nodes (
                         agent_id, schema_version, root_agent_id, root_conversation_id,
                         parent_agent_id, conversation_id, project_id, creation_request_id,
                         task_name, task_path, lifecycle, revision, created_at, updated_at
                     ) VALUES (
                         ?1, 1, ?1, ?2, NULL, ?2, ?3, ?4,
                         'Root', '/root', 'active', 1, 1, 1
                     )",
                    rusqlite::params![agent_id, conversation_id, project_id, request_id],
                )
            };
        insert_root(
            "source-root-agent",
            "source-root-conversation",
            "project-a",
            "create-source-root",
        )
        .unwrap();
        insert_root(
            "target-root-agent",
            "target-root-conversation",
            "project-a",
            "create-target-root",
        )
        .unwrap();
        insert_root(
            "other-root-agent",
            "other-root-conversation",
            "project-b",
            "create-other-root",
        )
        .unwrap();

        let insert_member = |agent_id: &str,
                             root_agent_id: &str,
                             root_conversation_id: &str,
                             parent_agent_id: &str,
                             conversation_id: &str,
                             project_id: &str,
                             request_id: &str,
                             task_name: &str,
                             task_path: &str| {
            connection.execute(
                "INSERT INTO agent_nodes (
                         agent_id, schema_version, root_agent_id, root_conversation_id,
                         parent_agent_id, conversation_id, project_id, creation_request_id,
                         task_name, task_path,
                         model_config_id_snapshot, model_display_name_snapshot,
                         model_supports_image_snapshot, model_context_window_tokens_snapshot,
                         model_settings_revision_snapshot, provider_connection_revision_snapshot,
                         provider_protocol_revision_snapshot, model_selection_source_snapshot,
                         lifecycle, revision, created_at, updated_at
                     ) VALUES (
                         ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                         'model-a', 'Model A', 0, 4096,
                         'settings-v1', 'connection-v1', 'protocol-v1', 'explicit',
                         'active', 1, 2, 2
                     )",
                rusqlite::params![
                    agent_id,
                    root_agent_id,
                    root_conversation_id,
                    parent_agent_id,
                    conversation_id,
                    project_id,
                    request_id,
                    task_name,
                    task_path,
                ],
            )
        };
        for fixture in [
            (
                "source-child-agent",
                "source-root-agent",
                "source-root-conversation",
                "source-root-agent",
                "source-child-conversation",
                "project-a",
                "create-source-child",
                "child",
                "/root/child",
            ),
            (
                "target-child-agent",
                "target-root-agent",
                "target-root-conversation",
                "target-root-agent",
                "target-child-conversation",
                "project-a",
                "create-target-child",
                "child",
                "/root/child",
            ),
            (
                "source-grand-agent",
                "source-root-agent",
                "source-root-conversation",
                "source-child-agent",
                "source-grand-conversation",
                "project-a",
                "create-source-grand",
                "grand",
                "/root/child/grand",
            ),
            (
                "target-grand-agent",
                "target-root-agent",
                "target-root-conversation",
                "target-child-agent",
                "target-grand-conversation",
                "project-a",
                "create-target-grand",
                "grand",
                "/root/child/grand",
            ),
            (
                "target-wrong-agent",
                "target-root-agent",
                "target-root-conversation",
                "target-root-agent",
                "target-wrong-conversation",
                "project-a",
                "create-target-wrong",
                "wrong",
                "/root/wrong",
            ),
            (
                "other-child-agent",
                "other-root-agent",
                "other-root-conversation",
                "other-root-agent",
                "other-child-conversation",
                "project-b",
                "create-other-child",
                "child",
                "/root/child",
            ),
        ] {
            insert_member(
                fixture.0, fixture.1, fixture.2, fixture.3, fixture.4, fixture.5, fixture.6,
                fixture.7, fixture.8,
            )
            .unwrap();
        }

        connection
            .execute_batch(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, created_at, position
                 ) VALUES
                    ('source-root-boundary', 'source-root-conversation',
                     'assistant', 'root boundary', 'complete', 3, 0),
                    ('target-root-boundary', 'target-root-conversation',
                     'assistant', 'root boundary', 'complete', 3, 0),
                    ('source-child-history', 'source-child-conversation',
                     'assistant', 'member history', 'complete', 4, 0);
                 INSERT INTO conversation_forks (
                     request_id, target_conversation_id, source_conversation_id,
                     source_message_id, target_message_id, fork_authority,
                     source_root_agent_id, target_root_agent_id,
                     source_fork_point_json, created_at
                 ) VALUES (
                     'root-fork-request', 'target-root-conversation',
                     'source-root-conversation', 'source-root-boundary',
                     'target-root-boundary', 'collaboration_root',
                     'source-root-agent', 'target-root-agent', '{}', 10
                 );",
            )
            .unwrap();

        const INSERT_MEMBER_RECEIPT: &str = "INSERT INTO agent_member_conversation_forks (
                 root_fork_request_id, source_conversation_id, target_conversation_id,
                 source_root_agent_id, target_root_agent_id,
                 source_member_agent_id, target_member_agent_id, created_at
             ) VALUES (
                 'root-fork-request', ?1, ?2, ?3, ?4, ?5, ?6, 10
             )";
        let insert_receipt = |source_conversation_id: &str,
                              target_conversation_id: &str,
                              source_root_agent_id: &str,
                              target_root_agent_id: &str,
                              source_member_agent_id: &str,
                              target_member_agent_id: &str| {
            connection.execute(
                INSERT_MEMBER_RECEIPT,
                rusqlite::params![
                    source_conversation_id,
                    target_conversation_id,
                    source_root_agent_id,
                    target_root_agent_id,
                    source_member_agent_id,
                    target_member_agent_id,
                ],
            )
        };

        for (label, invalid_mapping) in [
            (
                "root node cannot masquerade as a member",
                (
                    "source-root-conversation",
                    "target-child-conversation",
                    "source-root-agent",
                    "target-root-agent",
                    "source-root-agent",
                    "target-child-agent",
                ),
            ),
            (
                "member must belong to the corresponding root and project",
                (
                    "source-child-conversation",
                    "other-child-conversation",
                    "source-root-agent",
                    "other-root-agent",
                    "source-child-agent",
                    "other-child-agent",
                ),
            ),
            (
                "member task path and name must match",
                (
                    "source-child-conversation",
                    "target-wrong-conversation",
                    "source-root-agent",
                    "target-root-agent",
                    "source-child-agent",
                    "target-wrong-agent",
                ),
            ),
            (
                "grandchild receipt requires its parent mapping first",
                (
                    "source-grand-conversation",
                    "target-grand-conversation",
                    "source-root-agent",
                    "target-root-agent",
                    "source-grand-agent",
                    "target-grand-agent",
                ),
            ),
        ] {
            let error = insert_receipt(
                invalid_mapping.0,
                invalid_mapping.1,
                invalid_mapping.2,
                invalid_mapping.3,
                invalid_mapping.4,
                invalid_mapping.5,
            )
            .unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("invalid Agent member Conversation fork authority"),
                "{label}: {error}"
            );
        }

        let unauthorized_snapshot = connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, input_origin_kind,
                     snapshot_source_conversation_id, snapshot_source_message_id,
                     created_at, position
                 ) VALUES (
                     'unauthorized-snapshot', 'target-wrong-conversation',
                     'assistant', 'member history', 'complete', 'snapshot',
                     'source-child-conversation', 'source-child-history', 4, 0
                 )",
                [],
            )
            .unwrap_err();
        assert!(unauthorized_snapshot
            .to_string()
            .contains("invalid child context snapshot message"));

        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM messages
                     WHERE conversation_id = 'target-child-conversation'",
                    [],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap(),
            0
        );
        insert_receipt(
            "source-child-conversation",
            "target-child-conversation",
            "source-root-agent",
            "target-root-agent",
            "source-child-agent",
            "target-child-agent",
        )
        .unwrap();
        insert_receipt(
            "source-grand-conversation",
            "target-grand-conversation",
            "source-root-agent",
            "target-root-agent",
            "source-grand-agent",
            "target-grand-agent",
        )
        .unwrap();

        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status, input_origin_kind,
                     snapshot_source_conversation_id, snapshot_source_message_id,
                     created_at, position
                 ) VALUES (
                     'target-child-snapshot', 'target-child-conversation',
                     'assistant', 'member history', 'complete', 'snapshot',
                     'source-child-conversation', 'source-child-history', 4, 0
                 )",
                [],
            )
            .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT input_origin_kind, snapshot_source_conversation_id,
                            snapshot_source_message_id
                     FROM messages WHERE id = 'target-child-snapshot'",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .unwrap(),
            (
                "snapshot".to_string(),
                "source-child-conversation".to_string(),
                "source-child-history".to_string(),
            )
        );

        let update_error = connection
            .execute(
                "UPDATE agent_member_conversation_forks
                 SET created_at = 11
                 WHERE root_fork_request_id = 'root-fork-request'
                   AND source_member_agent_id = 'source-child-agent'",
                [],
            )
            .unwrap_err();
        assert!(update_error
            .to_string()
            .contains("Agent member Conversation fork receipt is immutable"));

        assert_eq!(
            connection
                .execute(
                    "DELETE FROM agent_member_conversation_forks
                     WHERE root_fork_request_id = 'root-fork-request'
                       AND source_member_agent_id = 'source-grand-agent'",
                    [],
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .execute(
                    "DELETE FROM agent_member_conversation_forks
                     WHERE root_fork_request_id = 'root-fork-request'
                       AND source_member_agent_id = 'source-child-agent'",
                    [],
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                    row.get::<_, u64>(0)
                })
                .unwrap(),
            0
        );
        ensure_foreign_keys_are_valid(&connection).unwrap();
    }

    #[test]
    fn canonical_schema_allows_only_one_in_progress_trace_per_conversation() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection.execute_batch(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES ('conversation-active', NULL, NULL, 'Active', 1, 1, NULL, NULL, NULL);
             INSERT INTO messages (
                id, conversation_id, role, content, status, agent_run_json,
                created_at, position
             ) VALUES
                ('assistant-active-1', 'conversation-active', 'assistant', '', 'pending', NULL, 1, 0),
                ('assistant-active-2', 'conversation-active', 'assistant', '', 'pending', NULL, 2, 1);
             INSERT INTO conversation_turn_traces (
                assistant_message_id, conversation_id, run_id, schema_version,
                terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
             ) VALUES (
                'assistant-active-1', 'conversation-active', 'run-active-1', 3,
                'in_progress', NULL, 0, 1, 1, NULL
             );",
        ).unwrap();
        assert!(connection
            .execute(
                "INSERT INTO conversation_turn_traces (
                assistant_message_id, conversation_id, run_id, schema_version,
                terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
             ) VALUES (
                'assistant-active-2', 'conversation-active', 'run-active-2', 3,
                'in_progress', NULL, 0, 2, 2, NULL
             )",
                [],
            )
            .is_err());
    }

    #[test]
    fn canonical_composer_schema_requires_complete_array_payloads() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();

        let columns = [
            "permission_mode_version",
            "attachments_json",
            "skills_json",
            "queued_messages_json",
        ];
        for column in columns {
            let (not_null, default_value): (i64, Option<String>) = connection
                .query_row(
                    "SELECT [notnull], dflt_value FROM pragma_table_info('composer_drafts')
                     WHERE name = ?1",
                    [column],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(not_null, 1, "{column} must be required");
            assert_eq!(default_value, None, "{column} must not have a default");
        }

        let non_array = connection.execute(
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES ('draft-invalid', '', 'default', 1, NULL, NULL, '{}', '[]', '[]', 1)",
            [],
        );
        assert!(non_array.is_err());

        for statement in [
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, model_id, project_id,
                attachments_json, skills_json, queued_messages_json, updated_at
             ) VALUES ('draft-missing-version', '', 'default', NULL, NULL, '[]', '[]', '[]', 1)",
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, queued_messages_json, updated_at
             ) VALUES ('draft-missing-skills', '', 'default', 1, NULL, NULL, '[]', '[]', 1)",
            "INSERT INTO composer_drafts (
                scope_id, message, permission_mode, permission_mode_version, model_id, project_id,
                attachments_json, skills_json, updated_at
             ) VALUES ('draft-missing-queue', '', 'default', 1, NULL, NULL, '[]', '[]', 1)",
        ] {
            assert!(connection.execute(statement, []).is_err());
        }
    }

    #[test]
    fn reopening_the_current_schema_does_not_mutate_the_catalog() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        let before = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        run_migrations(&connection).unwrap();

        assert_eq!(schema_fingerprint(&connection).unwrap(), before);
        assert_eq!(connection.total_changes(), before_changes);
    }

    #[test]
    fn an_unversioned_non_empty_database_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE historical_development_table (id TEXT PRIMARY KEY);")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'historical_development_table'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_some());
    }

    #[test]
    fn an_unknown_schema_version_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection.pragma_update(None, "user_version", 999).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert_eq!(read_schema_version(&connection).unwrap(), 999);
    }

    #[test]
    fn the_previous_v17_baseline_requires_reset_without_mutation() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE preserved_v17_data (
                    id TEXT PRIMARY KEY,
                    payload TEXT NOT NULL
                 );
                 INSERT INTO preserved_v17_data (id, payload)
                 VALUES ('sentinel', 'preserve-on-reset-required');
                 PRAGMA user_version = 17;",
            )
            .unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 17"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 17);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM preserved_v17_data WHERE id = 'sentinel'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "preserve-on-reset-required"
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'automations'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v3_database_requires_reset_without_rewriting_the_fixture() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v3.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE legacy_v3_sentinel (
                         id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO legacy_v3_sentinel (id, payload)
                     VALUES ('sentinel', 'must remain byte-for-byte visible');
                     PRAGMA user_version = 3;",
                )
                .unwrap();
        }

        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 3"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 3);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        let payload: String = connection
            .query_row(
                "SELECT payload FROM legacy_v3_sentinel WHERE id = 'sentinel'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(payload, "must remain byte-for-byte visible");
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'agent_nodes'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v4_database_requires_reset_without_rewriting_the_fixture() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v4.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE legacy_v4_sentinel (
                         id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO legacy_v4_sentinel (id, payload)
                     VALUES ('sentinel', 'do not rewrite');
                     PRAGMA user_version = 4;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 4"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 4);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM legacy_v4_sentinel WHERE id = 'sentinel'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "do not rewrite"
        );
    }

    #[test]
    fn a_v5_database_requires_reset_without_rewriting_the_fixture() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v5.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE legacy_v5_sentinel (
                         id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO legacy_v5_sentinel (id, payload)
                     VALUES ('sentinel', 'round-2 baseline remains untouched');
                     PRAGMA user_version = 5;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 5"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 5);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM legacy_v5_sentinel WHERE id = 'sentinel'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "round-2 baseline remains untouched"
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'agent_model_batch_receipts'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v6_database_requires_reset_without_rewriting_the_fixture() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v6.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE legacy_v6_sentinel (
                         id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO legacy_v6_sentinel (id, payload)
                     VALUES ('sentinel', 'round-3 baseline remains untouched');
                     PRAGMA user_version = 6;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 6"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 6);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM legacy_v6_sentinel WHERE id = 'sentinel'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "round-3 baseline remains untouched"
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'agent_collaboration_events'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v7_database_requires_reset_without_rewriting_round4_collaboration_facts() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v7.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE agent_collaboration_events (
                         event_id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO agent_collaboration_events (event_id, payload)
                     VALUES ('event-round4', 'must remain untouched');
                     PRAGMA user_version = 7;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 7"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 7);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM agent_collaboration_events
                     WHERE event_id = 'event-round4'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "must remain untouched"
        );
    }

    #[test]
    fn a_v8_database_requires_reset_without_rewriting_round5_collaboration_facts() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v8.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE agent_collaboration_events (
                         event_id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO agent_collaboration_events (event_id, payload)
                     VALUES ('event-round5', 'must remain untouched');
                     PRAGMA user_version = 8;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 8"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 8);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM agent_collaboration_events
                     WHERE event_id = 'event-round5'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "must remain untouched"
        );
    }

    #[test]
    fn a_v9_database_requires_reset_without_rewriting_unanchored_activity_facts() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v9.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE agent_collaboration_events (
                         event_id TEXT PRIMARY KEY,
                         activity_schema_version INTEGER NOT NULL,
                         root_anchor_message_id TEXT
                     );
                     INSERT INTO agent_collaboration_events (
                         event_id, activity_schema_version, root_anchor_message_id
                     ) VALUES ('activity-v9', 1, NULL);
                     PRAGMA user_version = 9;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 9"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 9);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT activity_schema_version, root_anchor_message_id
                     FROM agent_collaboration_events WHERE event_id = 'activity-v9'",
                    [],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .unwrap(),
            (1, None)
        );
    }

    #[test]
    fn a_v10_database_requires_reset_without_rewriting_existing_conversation_facts() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v10.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE conversations (
                         id TEXT PRIMARY KEY,
                         title TEXT NOT NULL
                     );
                     INSERT INTO conversations (id, title)
                     VALUES ('conversation-v10', 'must remain untouched');
                     PRAGMA user_version = 10;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 10"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 10);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT title FROM conversations WHERE id = 'conversation-v10'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "must remain untouched"
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema WHERE name = 'conversation_turn_rewrites'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v11_database_requires_reset_without_rewriting_existing_fork_facts() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v11.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE conversation_forks (
                         request_id TEXT PRIMARY KEY,
                         payload TEXT NOT NULL
                     );
                     INSERT INTO conversation_forks (request_id, payload)
                     VALUES ('fork-v11', 'must remain untouched');
                     PRAGMA user_version = 11;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 11"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 11);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT payload FROM conversation_forks WHERE request_id = 'fork-v11'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "must remain untouched"
        );
        assert!(connection
            .query_row(
                "SELECT 1 FROM sqlite_schema
                 WHERE name = 'agent_member_conversation_forks'",
                [],
                |_| Ok(()),
            )
            .optional()
            .unwrap()
            .is_none());
    }

    #[test]
    fn a_v12_database_requires_reset_without_rewriting_retired_goal_state() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v12.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE conversation_goals (
                         conversation_id TEXT PRIMARY KEY,
                         objective TEXT NOT NULL
                     );
                     INSERT INTO conversation_goals (conversation_id, objective)
                     VALUES ('conversation-v12', 'retired Goal state');
                     PRAGMA user_version = 12;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 12"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 12);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT objective FROM conversation_goals
                     WHERE conversation_id = 'conversation-v12'",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "retired Goal state"
        );
    }

    #[test]
    fn a_v13_database_requires_reset_after_goal_storage_removal() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("legacy-v13.sqlite");
        {
            let connection = Connection::open(&database_path).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE conversation_goal_revisions (
                         goal_id TEXT NOT NULL,
                         sequence INTEGER NOT NULL,
                         event_json TEXT NOT NULL,
                         PRIMARY KEY (goal_id, sequence)
                     );
                     INSERT INTO conversation_goal_revisions (goal_id, sequence, event_json)
                     VALUES ('goal-v13', 1, '{\"type\":\"initial\"}');
                     PRAGMA user_version = 13;",
                )
                .unwrap();
        }
        let connection = Connection::open(&database_path).unwrap();
        let before_fingerprint = schema_fingerprint(&connection).unwrap();
        let before_changes = connection.total_changes();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
        assert!(error.to_string().contains(&format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found 13"
        )));
        assert_eq!(read_schema_version(&connection).unwrap(), 13);
        assert_eq!(schema_fingerprint(&connection).unwrap(), before_fingerprint);
        assert_eq!(connection.total_changes(), before_changes);
        assert_eq!(
            connection
                .query_row(
                    "SELECT event_json FROM conversation_goal_revisions
                     WHERE goal_id = 'goal-v13' AND sequence = 1",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            r#"{"type":"initial"}"#
        );
    }

    #[test]
    fn legacy_human_messages_may_omit_the_structured_origin_columns() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO conversations (
                     id, project_id, model_id, title, created_at, updated_at,
                     pinned_at, archived_at, unread_at
                 ) VALUES ('legacy-conversation', NULL, NULL, 'Legacy', 1, 1, NULL, NULL, NULL)",
                [],
            )
            .unwrap();

        connection
            .execute(
                "INSERT INTO messages (
                     id, conversation_id, role, content, status,
                     agent_run_json, created_at, position
                 ) VALUES (
                     'legacy-user', 'legacy-conversation', 'user', 'hello', 'sent',
                     NULL, 1, 0
                 )",
                [],
            )
            .unwrap();

        let stored: (String, Option<String>, Option<String>, Option<String>) = connection
            .query_row(
                "SELECT role, input_origin_kind, input_origin_agent_id, source_agent_message_id
                 FROM messages WHERE id = 'legacy-user'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(stored, ("user".to_string(), None, None, None));
    }

    #[test]
    fn a_tampered_current_schema_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE incomplete (id TEXT PRIMARY KEY);")
            .unwrap();
        connection
            .pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }

    #[test]
    fn a_current_schema_missing_a_trigger_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch("DROP TRIGGER conversation_history_fts_message_insert;")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }

    #[test]
    fn a_current_schema_missing_an_fts_shadow_table_requires_a_development_reset() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        connection
            .execute_batch("DROP TABLE conversation_history_fts_data;")
            .unwrap();

        let error = run_migrations(&connection).unwrap_err();

        assert!(error
            .to_string()
            .contains(DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED));
    }
}

#[cfg(test)]
#[path = "migrations_history_search_tests.rs"]
mod history_search_tests;
