use rusqlite::{ffi, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

pub const STORAGE_SCHEMA_VERSION: i32 = 72;
pub const DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED: &str =
    "development_storage_schema_reset_required";

const CANONICAL_SCHEMA: &str = include_str!("canonical_schema.sql");
const V57_SCHEMA_FINGERPRINT: &str =
    "sha256:ada068a864855b8c42afa6de36f72791212dfd90938fd547f7236e9cf26ed289";
const V58_SCHEMA_FINGERPRINT: &str =
    "sha256:682fa9fd22c87bccdd9d201ac97f200aee3d69718ca216ac46256854ffdbf623";
const V59_SCHEMA_FINGERPRINT: &str =
    "sha256:b3e50620bff925cbc53da8c13146908b8aa3128ac406be9b28c73069b09fc3c4";
const V60_SCHEMA_FINGERPRINT: &str =
    "sha256:41581217d0c9d997bafbe82c1c8ded1691c163b3b70c39601f570250fade8683";
const V61_SCHEMA_FINGERPRINT: &str =
    "sha256:8909ce2f44597641f7cfb60c7cad159d0fb7ff0e165306992dc5d36785943cd7";
const V62_SCHEMA_FINGERPRINT: &str =
    "sha256:704ff3fdec03347fc91caa2adb18468e51180aecb91bf28010a4d88bb0bf4ece";
const V63_SCHEMA_FINGERPRINT: &str =
    "sha256:787e81d73f2a814771846a8cf104152623200986c5b7c8e3c4c72d9818aa1ee0";
const V64_SCHEMA_FINGERPRINT: &str =
    "sha256:5ee4d7a0c9c3e7a8ca588dbf76bb14d0353217143a3afe329d394c7c6d05e56f";
const V65_SCHEMA_FINGERPRINT: &str =
    "sha256:9e91f082fb82ceb7ed3af4ff5178dad656e0026a42b839c9af9cb79114b33568";
const V66_SCHEMA_FINGERPRINT: &str =
    "sha256:d9790cf426f36f5b8288025472f4a66dd0766d74e3cae3c8899691525c8eb352";

const V67_SCHEMA_FINGERPRINT: &str =
    "sha256:d2dab777211771ce0fb8b85ca0d0ddd39a58e5cd4ab0d4bd6e34d1fd0bf8c2ba";

const V68_SCHEMA_FINGERPRINT: &str =
    "sha256:e851d17035a3ed9536f2aa7a960436dfc6c7ec28e4f9ab293b6ed6b485645a0c";

const V69_SCHEMA_FINGERPRINT: &str =
    "sha256:8532d89f095b602369e576f3541172a7b872b7e22619228865cc9cf85eb403d8";
const V70_SCHEMA_FINGERPRINT: &str =
    "sha256:843221527806ceaf0563ede4973442c4040432cc9827639d0d411951ef66714b";
const V71_SCHEMA_FINGERPRINT: &str =
    "sha256:ce54b37ebd2b7243acfdcc875db55cc3a673ed4bb0c4d3f587b72c4496f50054";
const CANONICAL_SCHEMA_FINGERPRINT: &str =
    "sha256:f23d05f9c1ca9b5130e0cb5ae0e5235f01e62ff8af103de080b4fd6fe586fce7";

/// Initializes fresh storage or validates the exact current canonical schema.
///
/// The exact v50 catalog upgrades only with an empty collaboration event log. v51/v52 upgrade
/// without rewriting or deleting history; v53 adds request-owned activity facts only for future
/// events. v54 adds authoring-only workflow definitions without changing existing history.
/// v55 adds explicit workflow availability, disabled by default for every existing definition.
/// v56 adds independent workflow instances and isolated editing drafts; graph JSON is untouched.
/// v57 adds instance activation and prevents archiving conversations of enabled workflows.
/// v58 adds durable workflow execution receipts and delivery provenance.
/// v59 adds a nullable default project for future auto-created workflow conversations.
/// v60 indexes chronological history metadata without rewriting messages, traces or FTS content.
/// v61 adds trusted trace publication revisions; v62 adds a pending-only workflow sequence index.
/// v63 adds optional long-paste source metadata without changing existing attachment contents.
/// v64 installs independent workflow mailboxes; legacy workflow records are not converted.
/// v65 snapshots organization instances and assigns durable member identities.
/// v66 removes the retired graph execution tables; organization mail and chats stay intact.
/// v67 versions model-bearing message changes independently of conversation presentation metadata.
/// v68 refreshes message presentation independently and ignores unchanged trace publications.
/// v69 skips full-text index rewrites for unchanged message values, preserving existing history.
/// v70 keeps organization recovery cursors as transaction-owned metadata, without reading bodies.
/// v71 incrementally projects billing ledgers into exact hourly aggregates, with raw fallback.
/// v72 adds opt-in composer prediction state and independent prediction billing receipts.
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

    if read_schema_version(connection)? == 55 {
        upgrade_workflow_instances_v55(connection)?;
    }

    if read_schema_version(connection)? == 56 {
        upgrade_workflow_activation_v56(connection)?;
    }

    if read_schema_version(connection)? == 57 {
        upgrade_workflow_execution_v57(connection)?;
    }

    if read_schema_version(connection)? == 58 {
        upgrade_workflow_default_project_v58(connection)?;
    }

    if read_schema_version(connection)? == 59 {
        upgrade_history_timeline_v59(connection)?;
    }

    if read_schema_version(connection)? == 60 {
        upgrade_trace_publications_v60(connection)?;
    }

    if read_schema_version(connection)? == 61 {
        upgrade_workflow_pending_v61(connection)?;
    }

    if read_schema_version(connection)? == 62 {
        upgrade_pasted_text_v62(connection)?;
    }

    if read_schema_version(connection)? == 63 {
        install_workflow_mail_v63(connection)?;
    }

    if read_schema_version(connection)? == 64 {
        upgrade_organization_instances_v64(connection)?;
    }

    if read_schema_version(connection)? == 65 {
        remove_retired_workflow_execution_v65(connection)?;
    }

    if read_schema_version(connection)? == 66 {
        install_message_history_revisions_v66(connection)?;
    }

    if read_schema_version(connection)? == 67 {
        install_message_presentation_revisions_v67(connection)?;
    }

    if read_schema_version(connection)? == 68 {
        install_semantic_message_search_updates_v68(connection)?;
    }

    if read_schema_version(connection)? == 69 {
        install_workflow_recovery_metadata_v69(connection)?;
    }

    if read_schema_version(connection)? == 70 {
        install_billing_hourly_aggregates_v70(connection)?;
    }

    if read_schema_version(connection)? == 71 {
        let transaction = connection.unchecked_transaction()?;
        validate_schema_fingerprint(&transaction, V71_SCHEMA_FINGERPRINT)?;
        transaction.execute_batch(prediction_schema())?;
        transaction.pragma_update(None, "user_version", 72)?;
        validate_canonical_schema(&transaction)?;
        transaction.commit()?;
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
const V56_SCHEMA_FINGERPRINT: &str =
    "sha256:9ae5acc2b78d79d3dccd19b9ed8ccd418cd9aa6b1db46251f7e43f0cd43b9afd";
const V55_SCHEMA_FINGERPRINT: &str =
    "sha256:1befc72d0b8ff219c3de6906c87b61dab6a1002dca69e98179ec0dcee9774fb2";
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
    validate_schema_fingerprint(&transaction, V55_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_workflow_instances_v55(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V55_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_instance_schema())?;
    transaction.pragma_update(None, "user_version", 56)?;
    validate_schema_fingerprint(&transaction, V56_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn workflow_instance_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Global workflow instances and isolated editing drafts, schema v56.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Workflow instance activation and protected conversation archiving, schema v57.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_activation_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Workflow instance activation and protected conversation archiving, schema v57.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Durable workflow execution messages and receipts, schema v58.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_execution_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Durable workflow execution messages and receipts, schema v58.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Default project for newly created workflow conversations, schema v59.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_default_project_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Default project for newly created workflow conversations, schema v59.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Indexed chronological history projection, schema v60.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn upgrade_workflow_default_project_v58(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V58_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_default_project_schema())?;
    transaction.pragma_update(None, "user_version", 59)?;
    validate_schema_fingerprint(&transaction, V59_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn history_timeline_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Indexed chronological history projection, schema v60.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Trusted Trace publication revisions and interaction attention, schema v61.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn trace_publication_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Trusted Trace publication revisions and interaction attention, schema v61.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Fair pending workflow scheduling, schema v62.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_pending_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Fair pending workflow scheduling, schema v62.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Long pasted text source metadata, schema v63.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn pasted_text_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Long pasted text source metadata, schema v63.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Independent workflow mailboxes, schema v64.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn workflow_mail_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Independent workflow mailboxes, schema v64.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Independent organization instances and member identities, schema v65.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn message_history_revisions_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Per-conversation model history identity, independent of presentation metadata, schema v67.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find(
            "-- Incremental message presentation refresh and semantic trace revisions, schema v68.",
        )
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn message_presentation_revisions_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find(
            "-- Incremental message presentation refresh and semantic trace revisions, schema v68.",
        )
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Reindex message history only when indexed values change, schema v69.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn semantic_message_search_updates_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Reindex message history only when indexed values change, schema v69.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Durable organization recovery metadata, schema v70.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn workflow_recovery_metadata_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Durable organization recovery metadata, schema v70.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Incremental billing hour aggregates, schema v71.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn prediction_schema() -> &'static str {
    &CANONICAL_SCHEMA[CANONICAL_SCHEMA
        .find("-- Next-message prediction preference and consent generation, schema v72.")
        .unwrap()..]
}
fn billing_hourly_aggregates_schema() -> &'static str {
    &CANONICAL_SCHEMA[CANONICAL_SCHEMA
        .find("-- Incremental billing hour aggregates, schema v71.")
        .unwrap()
        ..CANONICAL_SCHEMA
            .find("-- Next-message prediction preference and consent generation, schema v72.")
            .unwrap()]
}
fn canonical_schema_v70() -> String {
    CANONICAL_SCHEMA
        .replace(prediction_schema(), "")
        .replace(billing_hourly_aggregates_schema(), "")
}
fn canonical_schema_v69() -> String {
    canonical_schema_v70().replace(workflow_recovery_metadata_schema(), "")
}
fn canonical_schema_v68() -> String {
    canonical_schema_v69().replace(semantic_message_search_updates_schema(), "")
}
fn canonical_schema_v67() -> String {
    canonical_schema_v68().replace(message_presentation_revisions_schema(), "")
}
fn canonical_schema_v66() -> String {
    canonical_schema_v67().replace(message_history_revisions_schema(), "")
}
fn install_message_presentation_revisions_v67(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V67_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(message_presentation_revisions_schema())?;
    transaction.pragma_update(None, "user_version", 68)?;
    validate_schema_fingerprint(&transaction, V68_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}
fn install_semantic_message_search_updates_v68(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V68_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(semantic_message_search_updates_schema())?;
    transaction.pragma_update(None, "user_version", 69)?;
    validate_schema_fingerprint(&transaction, V69_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}
fn install_workflow_recovery_metadata_v69(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V69_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_recovery_metadata_schema())?;
    transaction.pragma_update(None, "user_version", 70)?;
    validate_schema_fingerprint(&transaction, V70_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}
fn install_billing_hourly_aggregates_v70(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V70_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(billing_hourly_aggregates_schema())?;
    transaction.pragma_update(None, "user_version", 71)?;
    validate_schema_fingerprint(&transaction, V71_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}
fn retired_workflow_cleanup_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Remove retired graph execution storage, schema v66.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Per-conversation model history identity, independent of presentation metadata, schema v67.").unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn canonical_schema_v65() -> String {
    canonical_schema_v66().replace(retired_workflow_cleanup_schema(), "")
}
fn install_message_history_revisions_v66(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V66_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(message_history_revisions_schema())?;
    transaction.pragma_update(None, "user_version", 67)?;
    validate_schema_fingerprint(&transaction, V67_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}
fn organization_instances_schema() -> &'static str {
    let start = CANONICAL_SCHEMA
        .find("-- Independent organization instances and member identities, schema v65.")
        .unwrap();
    let end = CANONICAL_SCHEMA
        .find("-- Remove retired graph execution storage, schema v66.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}
fn canonical_schema_v64() -> String {
    canonical_schema_v65().replace(organization_instances_schema(), "")
}
fn remove_retired_workflow_execution_v65(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V65_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(retired_workflow_cleanup_schema())?;
    transaction.pragma_update(None, "user_version", 66)?;
    validate_schema_fingerprint(&transaction, V66_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn canonical_schema_v63() -> String {
    canonical_schema_v64().replace(workflow_mail_schema(), "")
}
fn upgrade_organization_instances_v64(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V64_SCHEMA_FINGERPRINT)?;
    // The snapshot copy joins the source template. Reject orphaned instances before rebuilding
    // tables rather than silently dropping them from that join.
    ensure_foreign_keys_are_valid(&transaction)?;
    transaction.execute_batch(organization_instances_schema())?;
    transaction.pragma_update(None, "user_version", 65)?;
    validate_schema_fingerprint(&transaction, V65_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn canonical_schema_v62() -> String {
    canonical_schema_v63().replace(pasted_text_schema(), "")
}

fn upgrade_pasted_text_v62(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V62_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(pasted_text_schema())?;
    transaction.pragma_update(None, "user_version", 63)?;
    validate_schema_fingerprint(&transaction, V63_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn install_workflow_mail_v63(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V63_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_mail_schema())?;
    transaction.pragma_update(None, "user_version", 64)?;
    validate_schema_fingerprint(&transaction, V64_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_workflow_pending_v61(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V61_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_pending_schema())?;
    transaction.pragma_update(None, "user_version", 62)?;
    validate_schema_fingerprint(&transaction, V62_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_trace_publications_v60(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V60_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(trace_publication_schema())?;
    transaction.execute_batch(
        "INSERT INTO conversation_trace_journal_revisions(assistant_message_id)
        SELECT assistant_message_id FROM conversation_turn_traces;",
    )?;
    transaction.pragma_update(None, "user_version", 61)?;
    validate_schema_fingerprint(&transaction, V61_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_history_timeline_v59(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V59_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(history_timeline_schema())?;
    // Preserve the existing journal's order exactly, including trace positions recorded before
    // a later message reordering. Rebuilding from current message positions would change it.
    transaction.execute_batch(
        "INSERT INTO conversation_history_timeline(entry_rowid, conversation_id, position, within_message_order)
         SELECT identity.rowid, history.conversation_id, history.position, history.within_message_order
         FROM conversation_history_fts AS history
         JOIN conversation_history_index_entries AS identity ON identity.rowid = history.rowid
         WHERE history.record_type IN ('message', 'trace_item');",
    )?;
    transaction.pragma_update(None, "user_version", 60)?;
    validate_schema_fingerprint(&transaction, V60_SCHEMA_FINGERPRINT)?;
    transaction.commit()
}

fn upgrade_workflow_execution_v57(connection: &Connection) -> rusqlite::Result<()> {
    validate_schema_fingerprint(connection, V57_SCHEMA_FINGERPRINT)?;
    connection.pragma_update(None, "foreign_keys", false)?;
    let result = (|| {
        let transaction = connection.unchecked_transaction()?;
        // Rebuild the CHECK constraint without replaying history projection triggers.
        let dependents = transaction.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name='conversation_turn_trace_items' AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name")?
            .query_map([], |row| row.get::<_,String>(0))?.collect::<rusqlite::Result<Vec<_>>>()?;
        transaction.execute_batch("CREATE TEMP TABLE workflow_trace_v57_backup AS SELECT * FROM conversation_turn_trace_items; DROP TABLE conversation_turn_trace_items;")?;
        let start = CANONICAL_SCHEMA
            .find("CREATE TABLE conversation_turn_trace_items (")
            .unwrap();
        let end = CANONICAL_SCHEMA[start..].find(";\n").unwrap() + start + 1;
        transaction.execute_batch(&CANONICAL_SCHEMA[start..end])?;
        transaction.execute_batch("INSERT INTO conversation_turn_trace_items SELECT * FROM workflow_trace_v57_backup; DROP TABLE workflow_trace_v57_backup;")?;
        for sql in dependents {
            transaction.execute_batch(&sql)?;
        }
        transaction.execute_batch(workflow_execution_schema())?;
        transaction.pragma_update(None, "user_version", 58)?;
        validate_schema_fingerprint(&transaction, V58_SCHEMA_FINGERPRINT)?;
        ensure_foreign_keys_are_valid(&transaction)?;
        transaction.commit()
    })();
    connection.pragma_update(None, "foreign_keys", true)?;
    result
}

fn canonical_schema_v57() -> String {
    canonical_schema_v62()
        .replace(workflow_pending_schema(), "")
        .replace(trace_publication_schema(), "")
        .replace(history_timeline_schema(), "")
        .replace(workflow_default_project_schema(), "")
        .replace(workflow_execution_schema(), "")
        .replace(
            "'agent_mailbox_delivery', 'workflow_delivery',",
            "'agent_mailbox_delivery',",
        )
}

fn upgrade_workflow_activation_v56(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    validate_schema_fingerprint(&transaction, V56_SCHEMA_FINGERPRINT)?;
    transaction.execute_batch(workflow_activation_schema())?;
    transaction.pragma_update(None, "user_version", 57)?;
    validate_schema_fingerprint(&transaction, V57_SCHEMA_FINGERPRINT)?;
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
    let end = CANONICAL_SCHEMA
        .find("-- Global workflow instances and isolated editing drafts, schema v56.")
        .unwrap();
    &CANONICAL_SCHEMA[start..end]
}

fn canonical_schema_v54() -> String {
    canonical_schema_v57()
        .replace(workflow_activation_schema(), "")
        .replace(workflow_instance_schema(), "")
        .replace(workflow_availability_schema(), "")
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
#[path = "migrations_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "migrations_history_search_tests.rs"]
mod history_search_tests;

#[cfg(test)]
#[path = "migrations_history_timeline_tests.rs"]
mod history_timeline_tests;

#[cfg(test)]
#[path = "migrations_trace_publication_tests.rs"]
mod trace_publication_tests;

#[cfg(test)]
#[path = "migrations_workflow_pending_tests.rs"]
mod workflow_pending_tests;

#[cfg(test)]
#[path = "migrations_message_search_tests.rs"]
mod message_search_tests;

#[cfg(test)]
#[path = "migrations_workflow_recovery_tests.rs"]
mod workflow_recovery_tests;

#[cfg(test)]
#[path = "migrations_usage_dashboard_tests.rs"]
mod usage_dashboard_tests;
