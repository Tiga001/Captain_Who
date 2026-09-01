use rusqlite::{ffi, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};

pub const STORAGE_SCHEMA_VERSION: i32 = 28;
pub const DEVELOPMENT_STORAGE_SCHEMA_RESET_REQUIRED: &str =
    "development_storage_schema_reset_required";

const CANONICAL_SCHEMA: &str = include_str!("canonical_schema.sql");
const CANONICAL_SCHEMA_FINGERPRINT: &str =
    "sha256:4baf84d4fb76094b9c3c889f0653db3d020203119ae593d6296f21c23684eced";
const STORAGE_SCHEMA_V27_FINGERPRINT: &str =
    "sha256:b2ce395d684d40fa44f0c0274441c57606d4d2aa84f8e196f67cf4a291885942";
const PROVIDER_CONTINUATION_LATE_INDEXES: &str = r#"
CREATE INDEX idx_provider_continuations_replay_scope
            ON provider_continuations(
                conversation_id, assistant_message_id, run_id, request_index
            );
CREATE INDEX idx_provider_continuations_state
            ON provider_continuations(state, updated_at);
CREATE INDEX idx_provider_continuation_tool_calls_runtime
            ON provider_continuation_tool_calls(runtime_call_id, continuation_id);
"#;

/// Opens the single supported development schema.
///
/// A brand-new database is initialized atomically. The one supported development upgrade keeps
/// all non-Conversation state in place while intentionally resetting disposable conversation and
/// runtime history.
pub fn run_migrations(connection: &Connection) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = ON;")?;

    let schema_version = read_schema_version(connection)?;
    let object_count = application_schema_object_count(connection)?;

    if schema_version == 0 && object_count == 0 {
        return create_canonical_schema(connection);
    }

    if schema_version == 27 {
        if schema_fingerprint(connection)? != STORAGE_SCHEMA_V27_FINGERPRINT {
            return Err(reset_required_error(
                "schema 27 catalog fingerprint is not the supported canonical baseline",
            ));
        }
        reset_v27_to_v28_preserving_configuration(connection, false)?;
        return validate_canonical_schema(connection);
    }

    if schema_version != STORAGE_SCHEMA_VERSION {
        return Err(reset_required_error(format!(
            "expected schema version {STORAGE_SCHEMA_VERSION}, found {schema_version}"
        )));
    }

    validate_canonical_schema(connection)
}

/// Complete transitive non-SET-NULL foreign-key child closure of `conversations` in schema 27.
/// The parity test recomputes this closure so future schema work cannot silently leave history
/// behind. Global settings, MCP, Skills, browser history, notification settings, models, projects
/// and automations are intentionally outside this set. Conversation-owned notification facts are
/// cleared separately because their string identities deliberately have no foreign keys.
const CONVERSATION_HISTORY_TABLES: &[&str] = &[
    "agent_collaboration_cursors",
    "agent_collaboration_event_sequences",
    "agent_collaboration_events",
    "agent_command_session_lifecycle_events",
    "agent_command_session_model_read_receipts",
    "agent_command_session_output_chunks",
    "agent_command_session_published_outputs",
    "agent_command_sessions",
    "agent_effective_permission_snapshots",
    "agent_file_change_chunks",
    "agent_file_change_operations",
    "agent_file_change_run_grants",
    "agent_file_changes",
    "agent_interrupt_requests",
    "agent_mailbox_messages",
    "agent_member_conversation_forks",
    "agent_model_batch_receipt_items",
    "agent_model_batch_receipt_replays",
    "agent_model_batch_receipt_targets",
    "agent_model_batch_receipts",
    "agent_nodes",
    "agent_run_guidance_attachments",
    "agent_run_guidances",
    "agent_turn_diff_actions",
    "agent_turn_diff_files",
    "agent_turn_diffs",
    "agent_usage_records",
    "agent_wake_requests",
    "attachments",
    "child_context_snapshots",
    "context_compaction_receipts",
    "context_compaction_summaries",
    "context_compaction_summary_lineage",
    "conversation_context_adaptation_requirements",
    "conversation_context_compaction_heads",
    "conversation_forks",
    "conversation_history_blob_chunks",
    "conversation_history_blobs",
    "conversation_model_context_items",
    "conversation_turn_rewrites",
    "conversation_turn_trace_items",
    "conversation_turn_traces",
    "conversation_world_state_epochs",
    "conversation_world_state_records",
    "managed_artifact_grants",
    "messages",
    "model_request_observations",
    "provider_continuation_tool_calls",
    "provider_continuations",
    "provider_transition_terminal_records",
];

fn reset_v27_to_v28_preserving_configuration(
    connection: &Connection,
    fail_after_rebuild_for_test: bool,
) -> rusqlite::Result<()> {
    connection.execute_batch("PRAGMA foreign_keys = OFF;")?;
    let reset_result = (|| {
        let transaction = Transaction::new_unchecked(connection, TransactionBehavior::Immediate)?;
        let suspended_delete_guards = suspend_history_delete_guards(&transaction)?;

        // These runtime tables intentionally lack foreign keys. Delete only rows with an exact
        // old-Conversation owner; global/automation rows and the `new-conversation` draft survive.
        transaction.execute(
            "DELETE FROM mcp_approval_payload_envelopes
             WHERE action_id IN (
                 SELECT action_id FROM agent_pending_actions
                 WHERE conversation_id IN (SELECT id FROM conversations)
             )",
            [],
        )?;
        transaction.execute(
            "DELETE FROM agent_pending_actions
             WHERE conversation_id IN (SELECT id FROM conversations)",
            [],
        )?;
        transaction.execute(
            "DELETE FROM agent_action_audit
             WHERE conversation_id IN (SELECT id FROM conversations)",
            [],
        )?;
        transaction.execute(
            "DELETE FROM composer_drafts
             WHERE scope_id IN (SELECT id FROM conversations)",
            [],
        )?;

        // Notification facts intentionally use string identities instead of foreign keys. Remove
        // the facts owned by discarded Conversations, their delivery batch, and the associated
        // change-feed rows so the notification center cannot retain a dead Conversation target.
        // Notification preferences remain untouched.
        transaction.execute(
            "DELETE FROM notification_change_events
             WHERE notification_id IN (
                 SELECT id FROM notification_events
                 WHERE conversation_id IN (SELECT id FROM conversations)
             )
                OR batch_id IN (
                    SELECT item.batch_id
                    FROM notification_batch_items AS item
                    INNER JOIN notification_events AS event
                        ON event.id = item.notification_event_id
                    WHERE event.conversation_id IN (SELECT id FROM conversations)
                )",
            [],
        )?;
        transaction.execute(
            "DELETE FROM notification_events
             WHERE conversation_id IN (SELECT id FROM conversations)",
            [],
        )?;
        transaction.execute(
            "DELETE FROM notification_batches
             WHERE id IN (
                 SELECT item.batch_id
                 FROM notification_batch_items AS item
                 LEFT JOIN notification_events AS event
                     ON event.id = item.notification_event_id
                 WHERE event.id IS NULL
             )",
            [],
        )?;
        transaction.execute(
            "DELETE FROM notification_batch_items
             WHERE notification_event_id NOT IN (SELECT id FROM notification_events)
                OR batch_id NOT IN (SELECT id FROM notification_batches)",
            [],
        )?;

        // Keep normal product-side effects for preserved automations. Foreign-key actions are
        // replayed explicitly because enforcement is temporarily disabled.
        transaction.execute("DELETE FROM conversations", [])?;
        transaction.execute(
            "UPDATE automations SET target_conversation_id = NULL
             WHERE target_conversation_id IS NOT NULL",
            [],
        )?;
        transaction.execute(
            "UPDATE automation_runs
             SET conversation_id = NULL, user_message_id = NULL, assistant_message_id = NULL
             WHERE conversation_id IS NOT NULL
                OR user_message_id IS NOT NULL
                OR assistant_message_id IS NOT NULL",
            [],
        )?;
        transaction.execute(
            "UPDATE browser_downloads SET conversation_id = NULL
             WHERE conversation_id IS NOT NULL",
            [],
        )?;
        for table in CONVERSATION_HISTORY_TABLES {
            transaction.execute(&format!("DELETE FROM {}", quote_identifier(table)), [])?;
        }
        restore_suspended_triggers(&transaction, &suspended_delete_guards)?;

        transaction.execute_batch(
            "DROP TABLE provider_continuation_tool_calls;
             DROP TABLE provider_continuations;",
        )?;
        transaction.execute_batch(canonical_provider_continuation_schema()?)?;
        transaction.execute_batch(PROVIDER_CONTINUATION_LATE_INDEXES)?;
        if fail_after_rebuild_for_test {
            return Err(rusqlite::Error::InvalidQuery);
        }
        transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;
        validate_canonical_schema(&transaction)?;
        transaction.commit()
    })();
    let restore_result = connection.execute_batch("PRAGMA foreign_keys = ON;");
    match reset_result {
        Ok(()) => restore_result,
        Err(error) => {
            let _ = restore_result;
            Err(error)
        }
    }
}

fn suspend_history_delete_guards(connection: &Connection) -> rusqlite::Result<Vec<String>> {
    let triggers = {
        let mut statement = connection.prepare(
            "SELECT name, tbl_name, sql FROM sqlite_schema
             WHERE type = 'trigger' AND sql IS NOT NULL ORDER BY name",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    let mut suspended = Vec::new();
    for (name, table, sql) in triggers {
        if table != "conversations"
            && CONVERSATION_HISTORY_TABLES.contains(&table.as_str())
            && sql.to_ascii_uppercase().contains("BEFORE DELETE")
        {
            connection.execute_batch(&format!("DROP TRIGGER {};", quote_identifier(&name)))?;
            suspended.push(sql);
        }
    }
    Ok(suspended)
}

fn restore_suspended_triggers(
    connection: &Connection,
    trigger_sql: &[String],
) -> rusqlite::Result<()> {
    for sql in trigger_sql {
        connection.execute_batch(sql)?;
    }
    Ok(())
}

fn canonical_provider_continuation_schema() -> rusqlite::Result<&'static str> {
    let start = CANONICAL_SCHEMA
        .find("CREATE TABLE provider_continuations (")
        .ok_or(rusqlite::Error::InvalidQuery)?;
    let tail = &CANONICAL_SCHEMA[start..];
    let end = tail
        .find("CREATE TABLE agent_file_changes (")
        .ok_or(rusqlite::Error::InvalidQuery)?;
    Ok(&tail[..end])
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn create_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    let transaction = connection.unchecked_transaction()?;
    transaction.execute_batch(CANONICAL_SCHEMA)?;
    transaction.pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)?;

    validate_canonical_schema(&transaction)?;
    ensure_foreign_keys_are_valid(&transaction)?;

    transaction.commit()
}

fn validate_canonical_schema(connection: &Connection) -> rusqlite::Result<()> {
    let actual_fingerprint = schema_fingerprint(connection)?;
    if actual_fingerprint != CANONICAL_SCHEMA_FINGERPRINT {
        return Err(reset_required_error(format!(
            "schema catalog fingerprint mismatch (expected {CANONICAL_SCHEMA_FINGERPRINT}, found {actual_fingerprint})"
        )));
    }
    ensure_foreign_keys_are_valid(connection)
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

    const PRESERVED_SENTINEL_TABLES: &[&str] = &[
        "model_provider_settings",
        "models",
        "image_generation_profiles",
        "image_generation_credential_staging",
        "image_generation_credential_cleanup",
        "ui_preferences",
        "agent_prompt_preferences",
        "skill_enablement_overrides",
        "notification_settings",
        "browser_download_settings",
        "browser_preferences",
        "browser_history",
        "mcp_registry_metadata",
        "automations",
        "automation_runs",
        "automation_events",
    ];

    const PROVIDER_CONTINUATIONS_V27_SCHEMA: &str = r#"
CREATE TABLE provider_continuations (
            continuation_id TEXT PRIMARY KEY CHECK (
                length(continuation_id) = 61
                AND substr(continuation_id, 1, 25) = 'provider-continuation-v1:'
            ),
            schema_version INTEGER NOT NULL CHECK (schema_version = 1),
            envelope_version INTEGER NOT NULL CHECK (envelope_version = 1),
            conversation_id TEXT NOT NULL,
            assistant_message_id TEXT NOT NULL,
            run_id TEXT NOT NULL CHECK (
                length(CAST(run_id AS BLOB)) BETWEEN 1 AND 2048
            ),
            request_index INTEGER NOT NULL CHECK (request_index >= 0),
            assistant_turn_id TEXT NOT NULL CHECK (
                length(assistant_turn_id) = 68
                AND substr(assistant_turn_id, 1, 4) = 'at1_'
                AND substr(assistant_turn_id, 5) NOT GLOB '*[^0-9a-f]*'
            ),
            assistant_turn_digest TEXT NOT NULL CHECK (
                length(assistant_turn_digest) = 71
                AND substr(assistant_turn_digest, 1, 7) = 'sha256:'
                AND substr(assistant_turn_digest, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            provider_protocol_digest TEXT NOT NULL CHECK (
                length(provider_protocol_digest) = 71
                AND substr(provider_protocol_digest, 1, 7) = 'sha256:'
                AND substr(provider_protocol_digest, 8) NOT GLOB '*[^0-9a-f]*'
            ),
            state TEXT NOT NULL CHECK (state IN ('active', 'superseded', 'released')),
            superseded_by TEXT,
            compression TEXT,
            encryption TEXT,
            payload_digest TEXT,
            nonce BLOB,
            ciphertext BLOB,
            decoded_bytes INTEGER,
            compressed_bytes INTEGER,
            created_at INTEGER NOT NULL CHECK (created_at >= 0),
            updated_at INTEGER NOT NULL CHECK (updated_at >= created_at),
            released_at INTEGER CHECK (released_at IS NULL OR released_at >= created_at),
            activated_at INTEGER CHECK (activated_at IS NULL OR activated_at >= created_at),
            UNIQUE (conversation_id, assistant_message_id, run_id, request_index),
            CHECK (
                (
                    state IN ('active', 'superseded')
                    AND compression = 'zstd_binary_v1'
                    AND encryption = 'chacha20_poly1305_v1'
                    AND payload_digest IS NOT NULL
                    AND length(payload_digest) = 71
                    AND substr(payload_digest, 1, 7) = 'sha256:'
                    AND substr(payload_digest, 8) NOT GLOB '*[^0-9a-f]*'
                    AND nonce IS NOT NULL
                    AND length(nonce) = 12
                    AND ciphertext IS NOT NULL
                    AND length(ciphertext) > 16
                    AND length(ciphertext) <= 2097152
                    AND decoded_bytes BETWEEN 1 AND 8388608
                    AND compressed_bytes BETWEEN 1 AND 2097152
                    AND length(ciphertext) = compressed_bytes + 16
                    AND released_at IS NULL
                ) OR (
                    state = 'released'
                    AND superseded_by IS NULL
                    AND compression = 'zstd_binary_v1'
                    AND encryption = 'chacha20_poly1305_v1'
                    AND payload_digest IS NULL
                    AND nonce IS NULL
                    AND ciphertext IS NULL
                    AND decoded_bytes IS NULL
                    AND compressed_bytes IS NULL
                    AND released_at IS NOT NULL
                )
            ),
            FOREIGN KEY (conversation_id) REFERENCES conversations(id) ON DELETE CASCADE,
            FOREIGN KEY (assistant_message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
"#;

    fn downgrade_fresh_schema_to_v27(connection: &Connection) {
        connection.execute_batch(CANONICAL_SCHEMA).unwrap();
        connection
            .pragma_update(None, "user_version", STORAGE_SCHEMA_VERSION)
            .unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = OFF; PRAGMA legacy_alter_table = ON;")
            .unwrap();
        {
            let transaction = connection.unchecked_transaction().unwrap();
            transaction
                .execute_batch(
                    "DROP TRIGGER validate_provider_continuation_tool_call_projection;
                     DROP INDEX idx_provider_continuation_tool_calls_runtime;",
                )
                .unwrap();
            transaction
                .execute_batch(
                    "ALTER TABLE provider_continuations
                         RENAME TO provider_continuations_v28;",
                )
                .unwrap();
            transaction
                .execute_batch(PROVIDER_CONTINUATIONS_V27_SCHEMA)
                .unwrap();
            transaction
                .execute_batch(
                    "INSERT INTO provider_continuations (
                         continuation_id, schema_version, envelope_version,
                         conversation_id, assistant_message_id, run_id, request_index,
                         assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                         state, superseded_by, compression, encryption,
                         payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
                         created_at, updated_at, released_at, activated_at
                     )
                     SELECT continuation_id, schema_version, envelope_version,
                            conversation_id, assistant_message_id, run_id, request_index,
                            assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                            state, superseded_by, compression, encryption,
                            payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
                            created_at, updated_at, released_at, activated_at
                     FROM provider_continuations_v28;
                     DROP TABLE provider_continuations_v28;",
                )
                .unwrap();
            transaction
                .execute_batch(PROVIDER_CONTINUATION_LATE_INDEXES)
                .unwrap();
            transaction.pragma_update(None, "user_version", 27).unwrap();
            transaction.commit().unwrap();
        }
        connection
            .execute_batch("PRAGMA legacy_alter_table = OFF; PRAGMA foreign_keys = ON;")
            .unwrap();
        assert_eq!(
            schema_fingerprint(connection).unwrap(),
            STORAGE_SCHEMA_V27_FINGERPRINT
        );
    }

    fn seed_v27_history(connection: &Connection) -> (String, String) {
        connection
            .execute_batch(
                "INSERT INTO conversations (
                     id, project_id, model_id, title, created_at, updated_at,
                     pinned_at, archived_at, unread_at
                 ) VALUES ('conversation-v27', NULL, NULL, 'v27', 1, 1, NULL, NULL, NULL);
                 INSERT INTO messages (
                     id, conversation_id, role, content, status, agent_run_json,
                     ui_state_json, created_at, position
                 ) VALUES
                    ('assistant-v27-tool', 'conversation-v27', 'assistant', 'tool', 'sent',
                     NULL, NULL, 1, 0),
                    ('assistant-v27-ordinary', 'conversation-v27', 'assistant', 'ordinary', 'sent',
                     NULL, NULL, 2, 1);
                 INSERT INTO attachments (
                     id, conversation_id, message_id, project_id, kind, original_name,
                     mime_type, size_bytes, storage_rel_path, created_at
                 ) VALUES (
                     'attachment-v27', 'conversation-v27', 'assistant-v27-ordinary', NULL,
                     'image', 'history.png', 'image/png', 7, 'history/attachment-v27', 2
                 );
                 INSERT INTO notification_events (
                     id, schema_version, notification_kind, source_kind, source_id,
                     run_id, automation_id, conversation_id, user_message_id,
                     assistant_message_id, approval_action_id, subject_kind, subject_text,
                     priority, dedupe_key, supersession_key, resource_revision, seen_at,
                     resolved_at, superseded_at, occurred_at, expires_at
                 ) VALUES (
                     'notification-v27', 1, 'task_completed', 'human_root',
                     'conversation-v27', 'run-v27-notification', NULL,
                     'conversation-v27', NULL, 'assistant-v27-ordinary', NULL,
                     'prompt_excerpt', 'historical notification', 'completed',
                     'notification-v27-dedupe', 'notification-v27-supersession',
                     1, NULL, 2, NULL, 2, 1000
                 );
                 INSERT INTO notification_batches (
                     id, schema_version, status, revision, highest_priority,
                     collect_until, replace_until, retry_at, claim_token,
                     claim_expires_at, attempt_count, last_error_code,
                     delivered_revision, delivered_priority, sound_level_played,
                     disposition, created_at, updated_at, displayed_at, sealed_at,
                     suppressed_at
                 ) VALUES (
                     'notification-batch-v27', 1, 'displayed', 1, 'completed',
                     2, 1000, 2, NULL, NULL, 0, NULL, 1, 'completed', 'initial',
                     'delivered', 2, 2, 2, NULL, NULL
                 );
                 INSERT INTO notification_batch_items (
                     batch_id, notification_event_id, added_at
                 ) VALUES ('notification-batch-v27', 'notification-v27', 2);
                 INSERT INTO composer_drafts (
                     scope_id, message, permission_mode, permission_mode_version,
                     model_id, project_id, attachments_json, skills_json,
                     queued_messages_json, updated_at
                 ) VALUES (
                     'conversation-v27', 'historical draft', 'default', 0,
                     NULL, NULL, '[]', '[]', '[]', 2
                 );
                 INSERT INTO composer_drafts (
                     scope_id, message, permission_mode, permission_mode_version,
                     model_id, project_id, attachments_json, skills_json,
                     queued_messages_json, updated_at
                 ) VALUES (
                     'new-conversation', 'global draft sentinel', 'default', 0,
                     NULL, NULL, '[]', '[]', '[]', 3
                 );
                 INSERT INTO agent_pending_actions (
                     action_id, run_id, conversation_id, assistant_message_id,
                     action_type, tool_name, tool_call_id, status, target_status,
                     action_json, agent_input_json, created_at, updated_at
                 ) VALUES
                    (
                     'linked-action-v27', 'linked-run-v27', 'conversation-v27',
                     'assistant-v27-ordinary', 'tool', 'linked_tool', NULL,
                     'pending', NULL, '{}', '{}', 2, 2
                    ),
                    (
                     'global-action-sentinel', 'global-run-sentinel', NULL, NULL,
                     'tool', 'global_tool', NULL, 'pending', NULL, '{}', '{}', 3, 3
                    );
                 INSERT INTO agent_action_audit (
                     action_id, run_id, conversation_id, assistant_message_id,
                     action_type, tool_name, status, action_json, created_at
                 ) VALUES
                    (
                     'linked-audit-v27', 'linked-run-v27', 'conversation-v27',
                     'assistant-v27-ordinary', 'tool', 'linked_tool', 'pending', '{}', 2
                    ),
                    (
                     'global-audit-sentinel', 'global-run-sentinel', NULL, NULL,
                     'tool', 'global_tool', 'pending', '{}', 3
                    );
                 INSERT INTO mcp_approval_payload_envelopes (
                     invocation_id, action_id, envelope_version, envelope_json,
                     aad_digest, created_at, expires_at
                 ) VALUES
                    (
                     'linked-invocation-v27', 'linked-action-v27', 1, '{}',
                     'linked-aad', 2, 20
                    ),
                    (
                     'global-invocation-sentinel', 'global-action-sentinel', 1, '{}',
                     'global-aad', 3, 30
                 );",
            )
            .unwrap();
        let tool_id = format!(
            "provider-continuation-v1:{}",
            uuid::Uuid::new_v4().hyphenated()
        );
        let ordinary_id = format!(
            "provider-continuation-v1:{}",
            uuid::Uuid::new_v4().hyphenated()
        );
        for (continuation_id, assistant_message_id, run_id, request_index) in [
            (&tool_id, "assistant-v27-tool", "run-v27-tool", 0_i64),
            (
                &ordinary_id,
                "assistant-v27-ordinary",
                "run-v27-ordinary",
                0_i64,
            ),
        ] {
            connection
                .execute(
                    "INSERT INTO provider_continuations (
                         continuation_id, schema_version, envelope_version,
                         conversation_id, assistant_message_id, run_id, request_index,
                         assistant_turn_id, assistant_turn_digest, provider_protocol_digest,
                         state, superseded_by, compression, encryption,
                         payload_digest, nonce, ciphertext, decoded_bytes, compressed_bytes,
                         created_at, updated_at, released_at, activated_at
                     ) VALUES (
                         ?1, 1, 1, 'conversation-v27', ?2, ?3, ?4,
                         ?5, ?6, ?7, 'active', NULL, 'zstd_binary_v1',
                         'chacha20_poly1305_v1', ?8, ?9, ?10, 1, 1, 2, 2, NULL, 2
                     )",
                    rusqlite::params![
                        continuation_id,
                        assistant_message_id,
                        run_id,
                        request_index,
                        format!("at1_{}", "a".repeat(64)),
                        format!("sha256:{}", "b".repeat(64)),
                        format!("sha256:{}", "c".repeat(64)),
                        format!("sha256:{}", "d".repeat(64)),
                        vec![1_u8; 12],
                        vec![2_u8; 17],
                    ],
                )
                .unwrap();
        }
        connection
            .execute(
                "INSERT INTO provider_continuation_tool_calls (
                     continuation_id, provider_tool_index, runtime_call_id
                 ) VALUES (?1, 0, 'runtime-call-v27')",
                [&tool_id],
            )
            .unwrap();
        (tool_id, ordinary_id)
    }

    fn seed_v27_configuration(connection: &Connection) {
        connection
            .execute_batch(
                "INSERT INTO model_provider_settings (
                     id, api_url, api_token, search_mode, tavily_api_key,
                     configuration_revision, search_connection_revision, updated_at
                 ) VALUES (
                     'default', 'https://global.example/v1', 'global-token-sentinel',
                     'tavily', 'tavily-token-sentinel',
                     'model-settings-v1:00000000-0000-4000-8000-000000000101',
                     'search-connection-v1:00000000-0000-4000-8000-000000000102', 101
                 );
                 INSERT INTO models (
                     id, display_name, api_url_override, api_token_override,
                     supports_image, context_window_tokens, provider_profile_config_json,
                     provider_connection_revision, provider_protocol_revision,
                     input_price, cached_input_price, output_price, enabled, position,
                     created_at, updated_at
                 ) VALUES
                    (
                     'generic-model', 'Generic sentinel', NULL, NULL, 1, 131072,
                     '{\"schemaVersion\":1,\"profile\":{\"id\":\"generic_openai_chat\",\"version\":1},\"reasoning\":{\"mode\":\"provider_default\",\"effort\":\"provider_default\"}}',
                     'provider-connection-v1:00000000-0000-4000-8000-000000000103',
                     'provider-protocol-v1:00000000-0000-4000-8000-000000000104',
                     '1.25', '0.125', '5.5', 1, 4, 102, 103
                    ),
                    (
                     'deepseek-model', 'DeepSeek sentinel',
                     'https://model.example/v1', 'model-token-sentinel', 0, 65536,
                     '{\"schemaVersion\":1,\"profile\":{\"id\":\"deepseek_v4_chat\",\"version\":1},\"reasoning\":{\"mode\":\"enabled\",\"effort\":\"high\"}}',
                     'provider-connection-v1:00000000-0000-4000-8000-000000000105',
                     'provider-protocol-v1:00000000-0000-4000-8000-000000000106',
                     '2.0', '', '8.0', 0, 9, 104, 105
                    );
                 INSERT INTO image_generation_profiles (
                     id, schema_version, adapter_id, endpoint_url, model_id,
                     credential_ref, enabled, text_to_image, image_to_image,
                     default_size_preset, default_watermark, generation,
                     created_at, updated_at
                 ) VALUES (
                     'image-profile-sentinel', 1, 'smartmlSeedream',
                     'https://image.example/generate', 'seedream-sentinel',
                     'credential-ref-sentinel', 0, 1, 1, '2K', 0, 17, 106, 107
                 );
                 INSERT INTO image_generation_credential_staging (
                     credential_ref, profile_id, expected_generation, created_at
                 ) VALUES (
                     'staged-credential-sentinel', 'image-profile-sentinel', 18, 108
                 );
                 INSERT INTO image_generation_credential_cleanup (
                     credential_ref, created_at
                 ) VALUES ('cleanup-credential-sentinel', 109);",
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO ui_preferences (
                     id, sidebar_conversation_sort, sidebar_project_sort,
                     sidebar_section_order, sidebar_project_order_json,
                     profile_display_name, profile_handle, custom_read_permission,
                     custom_write_permission, custom_command_permission,
                     custom_patch_permission, updated_at
                 ) VALUES (
                     'default', 'updated_desc', 'manual', 'projects_first',
                     '[\"project-sentinel\"]', 'Sentinel User', 'SENTINEL',
                     'workspace_only', 'workspace_only', 'require_approval',
                     'require_approval', 110
                 );
                 INSERT INTO agent_prompt_preferences (
                     id, work_mode, tone, detail_level, custom_instructions, updated_at
                 ) VALUES (
                     'default', 'agent', 'direct', 'detailed',
                     'prompt preference sentinel', 111
                 );
                 INSERT INTO skill_enablement_overrides (
                     skill_id, enabled, generation, updated_at
                 ) VALUES ('skill-sentinel', 0, 7, 112);
                 UPDATE notification_settings
                 SET enabled = 0, sound_enabled = 0, show_task_content = 0,
                     human_completed_enabled = 0, human_failed_enabled = 1,
                     human_approval_enabled = 0, human_cancelled_enabled = 1,
                     revision = 9, updated_at = 113
                 WHERE singleton_id = 1;
                 UPDATE browser_download_settings
                 SET location_mode = 'custom', custom_directory = '/tmp/browser-sentinel',
                     ask_where_to_save = 1, revision = 8, updated_at = 114
                 WHERE id = 'default';
                 UPDATE browser_preferences
                 SET link_open_target = 'builtin', revision = 6, updated_at = 115
                 WHERE id = 'default';
                 INSERT INTO browser_history (
                     history_id, schema_version, url, title, hostname, favicon_url, visited_at
                 ) VALUES (
                     'browser-history:00000000-0000-4000-8000-000000000001', 1,
                     'https://browser.example/sentinel', 'Browser sentinel',
                     'browser.example', 'https://browser.example/favicon.ico', 116
                 );
                 INSERT INTO mcp_registry_metadata (
                     singleton, schema_version, revision_watermark, updated_at
                 ) VALUES (1, 1, 23, 117);
                 INSERT INTO automations (
                     id, schema_version, create_request_id, title, prompt,
                     status, health_state, blocked_code, blocked_message,
                     destination_kind, target_conversation_id, project_binding_kind,
                     project_id, model_id, permission_mode, permission_mode_version,
                     permissions_json, reasoning_json, schedule_kind, schedule_json,
                     rrule, timezone, anchor_at, next_run_at, last_scheduled_at,
                     last_run_at, notification_policy, target_project_snapshot,
                     target_conversation_snapshot, target_model_snapshot,
                     target_project_id_snapshot, target_conversation_id_snapshot,
                     target_model_id_snapshot, attention_required_at, attention_read_at,
                     revision, created_at, updated_at, deleted_at
                 ) VALUES (
                     'automation-sentinel', 1, 'automation-create-sentinel',
                     'Automation sentinel', 'Preserve this automation',
                     'paused', 'ok', NULL, NULL, 'new_chat', NULL, 'none',
                     NULL, 'generic-model', 'default', 1, '{}', '{}',
                     'interval', '{\"intervalMs\":60000}',
                     'FREQ=MINUTELY;INTERVAL=1', 'UTC', 118, NULL, NULL, NULL,
                     'all_runs', NULL, NULL, 'Generic sentinel', NULL, NULL,
                     'generic-model', NULL, NULL, 1, 118, 119, NULL
                 );
                 INSERT INTO automation_runs (
                     id, schema_version, automation_id, config_revision,
                     config_snapshot_json, trigger_kind, scheduled_for,
                     manual_request_id, status, status_revision, retry_at,
                     admission_attempt, admission_token, admission_expires_at,
                     cancellation_requested_at, agent_run_id, conversation_id,
                     user_message_id, assistant_message_id, report_kind,
                     result_preview, error_code, error_message,
                     attention_required_at, attention_read_at,
                     created_at, started_at, completed_at, updated_at
                 ) VALUES (
                     'automation-run-sentinel', 1, 'automation-sentinel', 1, '{}',
                     'scheduled', 120, NULL, 'completed', 1, NULL, 0, NULL,
                     NULL, NULL, NULL, NULL, NULL, NULL, 'completed',
                     'automation result sentinel', NULL, NULL, NULL, NULL,
                     120, 121, 122, 122
                 );",
            )
            .unwrap();
    }

    fn snapshot_tables(
        connection: &Connection,
        tables: &[&str],
    ) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
        tables
            .iter()
            .map(|table| {
                let columns = {
                    let mut statement = connection
                        .prepare(&format!("PRAGMA table_info({})", quote_identifier(table)))
                        .unwrap();
                    statement
                        .query_map([], |row| row.get::<_, String>(1))
                        .unwrap()
                        .collect::<rusqlite::Result<Vec<_>>>()
                        .unwrap()
                };
                let projection = columns
                    .iter()
                    .map(|column| quote_identifier(column))
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut statement = connection
                    .prepare(&format!(
                        "SELECT {projection} FROM {} ORDER BY rowid",
                        quote_identifier(table)
                    ))
                    .unwrap();
                let rows = statement
                    .query_map([], |row| {
                        (0..columns.len())
                            .map(|index| row.get::<_, rusqlite::types::Value>(index))
                            .collect::<rusqlite::Result<Vec<_>>>()
                    })
                    .unwrap()
                    .collect::<rusqlite::Result<Vec<_>>>()
                    .unwrap();
                ((*table).to_string(), rows)
            })
            .collect()
    }

    fn table_row_count(connection: &Connection, table: &str) -> i64 {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap()
    }

    #[test]
    fn conversation_history_table_list_matches_the_canonical_foreign_key_closure() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();
        let actual = {
            let mut statement = connection
                .prepare(
                    "WITH RECURSIVE
                         foreign_keys(child, parent, on_delete) AS (
                             SELECT schema.name, foreign_key.[table], upper(foreign_key.on_delete)
                             FROM sqlite_schema AS schema
                             JOIN pragma_foreign_key_list(schema.name) AS foreign_key
                             WHERE schema.type = 'table'
                         ),
                         history(name) AS (
                             VALUES ('conversations')
                             UNION
                             SELECT foreign_keys.child
                             FROM foreign_keys
                             JOIN history ON foreign_keys.parent = history.name
                             WHERE foreign_keys.on_delete NOT IN ('SET NULL', 'SET DEFAULT')
                         )
                     SELECT name FROM history ORDER BY name",
                )
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap()
        };
        let mut expected = CONVERSATION_HISTORY_TABLES
            .iter()
            .map(|table| (*table).to_string())
            .chain(std::iter::once("conversations".to_string()))
            .collect::<Vec<_>>();
        expected.sort();
        expected.dedup();
        assert_eq!(actual, expected);
    }

    #[test]
    fn v27_to_v28_selective_reset_preserves_configuration_and_clears_history() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture.path().join("provider-continuation-v27.sqlite");
        let connection = Connection::open(&database_path).unwrap();
        downgrade_fresh_schema_to_v27(&connection);
        seed_v27_configuration(&connection);
        seed_v27_history(&connection);
        let configuration_before = snapshot_tables(&connection, PRESERVED_SENTINEL_TABLES);
        assert_eq!(table_row_count(&connection, "conversation_history_fts"), 2);
        assert_eq!(table_row_count(&connection, "notification_events"), 1);
        assert_eq!(table_row_count(&connection, "notification_batches"), 1);
        assert_eq!(table_row_count(&connection, "notification_batch_items"), 1);
        assert_eq!(
            table_row_count(&connection, "notification_change_events"),
            1
        );
        drop(connection);

        let mut reopened = Connection::open(&database_path).unwrap();
        run_migrations(&reopened).unwrap();

        assert_eq!(
            read_schema_version(&reopened).unwrap(),
            STORAGE_SCHEMA_VERSION
        );
        assert_eq!(
            schema_fingerprint(&reopened).unwrap(),
            CANONICAL_SCHEMA_FINGERPRINT
        );
        ensure_foreign_keys_are_valid(&reopened).unwrap();
        assert_eq!(
            snapshot_tables(&reopened, PRESERVED_SENTINEL_TABLES),
            configuration_before
        );
        assert_eq!(table_row_count(&reopened, "model_provider_settings"), 1);
        assert_eq!(table_row_count(&reopened, "models"), 2);
        let hydrated =
            crate::storage::config_repository::load_model_settings_snapshot(&mut reopened)
                .unwrap()
                .unwrap();
        assert_eq!(hydrated.settings.api_url, "https://global.example/v1");
        assert_eq!(hydrated.settings.api_token, "global-token-sentinel");
        assert_eq!(hydrated.settings.search_mode, "tavily");
        assert_eq!(hydrated.settings.tavily_api_key, "tavily-token-sentinel");
        assert_eq!(hydrated.settings.models.len(), 2);
        assert_eq!(
            hydrated.settings.models[0]
                .provider_profile_config
                .profile(),
            crate::ProviderProfileRef::generic_for_dialect(
                crate::ProviderProtocolDialect::OpenAiChatCompletions,
            )
        );
        assert_eq!(
            hydrated.settings.models[1]
                .provider_profile_config
                .profile(),
            crate::ProviderProfileRef::deepseek_v4_chat()
        );
        assert_eq!(
            hydrated.settings.models[1].api_url_override.as_deref(),
            Some("https://model.example/v1")
        );
        assert_eq!(
            hydrated.settings.models[1].api_token_override.as_deref(),
            Some("model-token-sentinel")
        );
        assert_eq!(table_row_count(&reopened, "image_generation_profiles"), 1);
        let image_profile =
            crate::storage::image_generation_repository::load_image_generation_profile(
                &reopened,
                "image-profile-sentinel",
            )
            .unwrap()
            .unwrap();
        assert_eq!(image_profile.endpoint_url, "https://image.example/generate");
        assert_eq!(image_profile.model_id, "seedream-sentinel");
        assert_eq!(
            image_profile.credential_ref.as_deref(),
            Some("credential-ref-sentinel")
        );
        assert_eq!(
            table_row_count(&reopened, "image_generation_credential_staging"),
            1
        );
        assert_eq!(
            table_row_count(&reopened, "image_generation_credential_cleanup"),
            1
        );
        for history_table in [
            "conversations",
            "messages",
            "attachments",
            "provider_continuations",
            "provider_continuation_tool_calls",
            "conversation_history_fts",
            "notification_events",
            "notification_batches",
            "notification_batch_items",
            "notification_change_events",
        ] {
            assert_eq!(
                table_row_count(&reopened, history_table),
                0,
                "{history_table} was not cleared"
            );
        }
        for (table, global_id_column, global_id, linked_id) in [
            (
                "composer_drafts",
                "scope_id",
                "new-conversation",
                "conversation-v27",
            ),
            (
                "agent_pending_actions",
                "action_id",
                "global-action-sentinel",
                "linked-action-v27",
            ),
            (
                "agent_action_audit",
                "action_id",
                "global-audit-sentinel",
                "linked-audit-v27",
            ),
            (
                "mcp_approval_payload_envelopes",
                "invocation_id",
                "global-invocation-sentinel",
                "linked-invocation-v27",
            ),
        ] {
            assert_eq!(
                reopened
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {} WHERE {} = ?1",
                            quote_identifier(table),
                            quote_identifier(global_id_column)
                        ),
                        [global_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                1,
                "global sentinel in {table} was not preserved"
            );
            assert_eq!(
                reopened
                    .query_row(
                        &format!(
                            "SELECT COUNT(*) FROM {} WHERE {} = ?1",
                            quote_identifier(table),
                            quote_identifier(global_id_column)
                        ),
                        [linked_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .unwrap(),
                0,
                "conversation-owned row in {table} was not cleared"
            );
        }
    }

    #[test]
    fn v27_to_v28_selective_reset_failure_rolls_back_configuration_history_and_schema() {
        let fixture = tempfile::tempdir().unwrap();
        let database_path = fixture
            .path()
            .join("provider-continuation-v27-rollback.sqlite");
        let connection = Connection::open(&database_path).unwrap();
        downgrade_fresh_schema_to_v27(&connection);
        seed_v27_configuration(&connection);
        let (tool_id, ordinary_id) = seed_v27_history(&connection);
        let configuration_before = snapshot_tables(&connection, PRESERVED_SENTINEL_TABLES);

        assert!(reset_v27_to_v28_preserving_configuration(&connection, true).is_err());
        assert!(connection
            .pragma_query_value(None, "foreign_keys", |row| row.get::<_, bool>(0))
            .unwrap());
        assert_eq!(read_schema_version(&connection).unwrap(), 27);
        assert_eq!(
            schema_fingerprint(&connection).unwrap(),
            STORAGE_SCHEMA_V27_FINGERPRINT
        );
        assert_eq!(
            snapshot_tables(&connection, PRESERVED_SENTINEL_TABLES),
            configuration_before
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM provider_continuations
                     WHERE continuation_id IN (?1, ?2)",
                    rusqlite::params![tool_id, ordinary_id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT runtime_call_id
                     FROM provider_continuation_tool_calls
                     WHERE continuation_id = ?1",
                    [&tool_id],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "runtime-call-v27"
        );
        assert_eq!(table_row_count(&connection, "conversations"), 1);
        assert_eq!(table_row_count(&connection, "messages"), 2);
        assert_eq!(table_row_count(&connection, "attachments"), 1);
        assert_eq!(table_row_count(&connection, "composer_drafts"), 2);
        assert_eq!(table_row_count(&connection, "agent_pending_actions"), 2);
        assert_eq!(table_row_count(&connection, "agent_action_audit"), 2);
        assert_eq!(
            table_row_count(&connection, "mcp_approval_payload_envelopes"),
            2
        );
        assert_eq!(table_row_count(&connection, "conversation_history_fts"), 2);
        assert_eq!(table_row_count(&connection, "notification_events"), 1);
        assert_eq!(table_row_count(&connection, "notification_batches"), 1);
        assert_eq!(table_row_count(&connection, "notification_batch_items"), 1);
        assert_eq!(
            table_row_count(&connection, "notification_change_events"),
            1
        );
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

        for required_object in [
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
            "prevent_agent_message_projection_ui_rewrite",
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
            "automation_notification_outbox",
            "automation_notification_outbox_run_kind",
            "automation_notification_outbox_task_configuration_kind",
            "automation_notification_outbox_pending_idx",
            "project_automation_notification_to_application_outbox",
            "resolve_projected_automation_notification_after_legacy_suppress",
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
    fn canonical_agent_member_fork_receipts_enforce_tree_authority_and_snapshot_inserts() {
        let connection = Connection::open_in_memory().unwrap();
        run_migrations(&connection).unwrap();

        connection
            .execute_batch(
                "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
                 VALUES
                    ('project-a', 'Project A', NULL, 1, NULL, 1),
                    ('project-b', 'Project B', NULL, 1, NULL, 1);",
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
                ui_state_json, created_at, position
             ) VALUES
                ('assistant-active-1', 'conversation-active', 'assistant', '', 'pending', NULL, NULL, 1, 0),
                ('assistant-active-2', 'conversation-active', 'assistant', '', 'pending', NULL, NULL, 2, 1);
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
                     agent_run_json, ui_state_json, created_at, position
                 ) VALUES (
                     'legacy-user', 'legacy-conversation', 'user', 'hello', 'sent',
                     NULL, NULL, 1, 0
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
