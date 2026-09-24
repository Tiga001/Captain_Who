use super::*;

#[derive(Debug, Clone)]
struct AgentTreeDeletionScope {
    root_agent_id: String,
    conversation_ids: Vec<String>,
}

fn resolve_agent_tree_deletion_scope(
    connection: &rusqlite::Connection,
    conversation_id: &str,
) -> Result<Option<AgentTreeDeletionScope>, String> {
    let Some(node) =
        agent_graph_repository::get_agent_node_by_conversation(connection, conversation_id)
            .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    if node.parent_agent_id.is_some() {
        return Err(
            "child Agent conversations are owned by their root task and cannot be removed independently"
                .to_string(),
        );
    }
    let tree = agent_graph_repository::list_agent_tree(connection, &node.root_agent_id)
        .map_err(|error| error.to_string())?;
    Ok(Some(AgentTreeDeletionScope {
        root_agent_id: node.root_agent_id,
        conversation_ids: tree
            .into_iter()
            .map(|agent| agent.conversation_id)
            .collect(),
    }))
}

fn project_agent_tree_deletion_scopes(
    connection: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<AgentTreeDeletionScope>, String> {
    let mut statement = connection
        .prepare(
            "SELECT agent_id
             FROM agent_nodes
             WHERE project_id = ?1 AND parent_agent_id IS NULL
             ORDER BY created_at, agent_id",
        )
        .map_err(storage_error)?;
    let root_agent_ids = statement
        .query_map([project_id], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage_error)?;
    root_agent_ids
        .into_iter()
        .map(|root_agent_id| {
            let tree = agent_graph_repository::list_agent_tree(connection, &root_agent_id)
                .map_err(|error| error.to_string())?;
            Ok(AgentTreeDeletionScope {
                root_agent_id,
                conversation_ids: tree
                    .into_iter()
                    .map(|agent| agent.conversation_id)
                    .collect(),
            })
        })
        .collect()
}

fn project_conversation_ids(
    connection: &rusqlite::Connection,
    project_id: &str,
) -> Result<Vec<String>, String> {
    let mut statement = connection
        .prepare("SELECT id FROM conversations WHERE project_id = ?1 ORDER BY created_at, id")
        .map_err(storage_error)?;
    let conversation_ids = statement
        .query_map([project_id], |row| row.get::<_, String>(0))
        .map_err(storage_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(storage_error)?;
    Ok(conversation_ids)
}

fn ensure_deletion_scope_has_no_active_execution(
    connection: &rusqlite::Connection,
    conversation_ids: &[String],
) -> Result<(), String> {
    if conversation_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", conversation_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT EXISTS(
             SELECT 1 FROM conversation_turn_traces AS trace
             WHERE trace.conversation_id IN ({placeholders})
               AND trace.terminal_status = 'in_progress'
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs AS run
                   WHERE run.agent_run_id = trace.run_id
                     AND run.status IN ('completed', 'failed', 'cancelled')
               )
             UNION ALL
             SELECT 1 FROM agent_command_sessions
             WHERE conversation_id IN ({placeholders}) AND status IN ('starting', 'running')
             UNION ALL
             SELECT 1 FROM agent_pending_actions AS pending
             WHERE pending.conversation_id IN ({placeholders})
               AND pending.status IN ('pending', 'approved', 'executing')
               AND NOT EXISTS (
                   SELECT 1 FROM automation_runs AS run
                   WHERE run.agent_run_id = pending.run_id
                     AND run.status IN ('completed', 'failed', 'cancelled')
               )
         )"
    );
    let mut values = Vec::with_capacity(conversation_ids.len() * 3);
    for _ in 0..3 {
        values.extend(conversation_ids.iter().cloned());
    }
    let active = connection
        .query_row(&sql, rusqlite::params_from_iter(values), |row| {
            row.get::<_, bool>(0)
        })
        .map_err(storage_error)?;
    if active {
        Err(
            "the Agent task still has an active execution; stop it before removing the task"
                .to_string(),
        )
    } else {
        Ok(())
    }
}

fn cleanup_conversation_owned_records(
    connection: &rusqlite::Connection,
    conversation_ids: &[String],
) -> Result<(), String> {
    let deleted_at = now_ms();
    for conversation_id in conversation_ids {
        notification_repository::resolve_notification_events_by_conversation_id_in_transaction(
            connection,
            conversation_id,
            deleted_at,
        )
        .map_err(storage_error)?;
        usage_repository::roll_up_deleted_usage_for_conversation(
            connection,
            conversation_id,
            deleted_at,
        )
        .map_err(storage_error)?;
        pending_action_repository::delete_pending_actions_for_conversation(
            connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        agent_action_audit_repository::delete_action_audit_for_conversation(
            connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        provider_continuation_repository::delete_for_conversation(connection, conversation_id)
            .map_err(storage_error)?;
        composer_draft_repository::delete_composer_draft(connection, conversation_id)
            .map_err(storage_error)?;
    }
    Ok(())
}

fn message_deletion_has_protected_model_batch_receipts(
    connection: &rusqlite::Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> Result<bool, String> {
    if message_ids.is_empty() {
        return Ok(false);
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut values = Vec::with_capacity(1 + message_ids.len());
    values.push(conversation_id.to_string());
    values.extend(message_ids.iter().cloned());
    connection
        .query_row(
            &format!(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_model_batch_receipts
                     WHERE conversation_id = ?
                       AND assistant_message_id IN ({placeholders})
                 )"
            ),
            rusqlite::params_from_iter(values),
            |row| row.get::<_, bool>(0),
        )
        .map_err(storage_error)
}

fn delete_protected_model_batch_receipts_for_messages(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    message_ids: &[String],
) -> Result<(), String> {
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let receipt_filter = format!(
        "SELECT receipt_id FROM agent_model_batch_receipts
         WHERE conversation_id = ? AND assistant_message_id IN ({placeholders})"
    );
    let mut values = Vec::with_capacity(1 + message_ids.len());
    values.push(conversation_id.to_string());
    values.extend(message_ids.iter().cloned());
    let mut replay_values = values.clone();
    replay_values.extend(values.iter().cloned());
    transaction
        .execute(
            &format!(
                "DELETE FROM agent_model_batch_receipt_replays
                 WHERE receipt_id IN ({receipt_filter})
                    OR source_receipt_id IN ({receipt_filter})"
            ),
            rusqlite::params_from_iter(replay_values),
        )
        .map_err(storage_error)?;
    for table in [
        "agent_model_batch_receipt_items",
        "agent_model_batch_receipt_targets",
    ] {
        transaction
            .execute(
                &format!("DELETE FROM {table} WHERE receipt_id IN ({receipt_filter})"),
                rusqlite::params_from_iter(values.iter()),
            )
            .map_err(storage_error)?;
    }
    transaction
        .execute(
            &format!(
                "DELETE FROM agent_model_batch_receipts WHERE receipt_id IN ({receipt_filter})"
            ),
            rusqlite::params_from_iter(values.iter()),
        )
        .map_err(storage_error)?;

    // The deletion transaction disables immutable coordination triggers. Reproduce the two FTS
    // delete projections which would otherwise have been emitted by messages and trace items.
    let fts_placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut fts_values = Vec::with_capacity(1 + message_ids.len() * 2);
    fts_values.push(conversation_id.to_string());
    fts_values.extend(message_ids.iter().cloned());
    fts_values.extend(message_ids.iter().cloned());
    transaction
        .execute(
            &format!(
                "DELETE FROM conversation_history_fts
                 WHERE conversation_id = ?
                   AND (message_id IN ({fts_placeholders})
                        OR assistant_message_id IN ({fts_placeholders}))"
            ),
            rusqlite::params_from_iter(fts_values),
        )
        .map_err(storage_error)?;
    Ok(())
}

fn delete_chat_message_records_in_transaction(
    transaction: &rusqlite::Transaction<'_>,
    conversation_id: &str,
    message_ids: &[String],
    delete_protected_receipts: bool,
) -> Result<Vec<AttachmentRecord>, String> {
    notification_repository::resolve_notification_events_by_message_ids_in_transaction(
        transaction,
        message_ids,
        now_ms(),
    )
    .map_err(storage_error)?;
    automation_repository::terminalize_automation_runs_before_message_delete(
        transaction,
        conversation_id,
        message_ids,
        now_ms(),
    )
    .map_err(storage_error)?;
    usage_repository::roll_up_deleted_usage_for_messages(
        transaction,
        conversation_id,
        message_ids,
        now_ms(),
    )
    .map_err(storage_error)?;
    let attachments =
        attachment_repository::list_message_attachments(transaction, conversation_id, message_ids)
            .map_err(storage_error)?;
    attachment_repository::delete_message_attachments(transaction, conversation_id, message_ids)
        .map_err(storage_error)?;
    pending_action_repository::delete_pending_actions_for_messages(
        transaction,
        conversation_id,
        message_ids,
    )
    .map_err(storage_error)?;
    agent_action_audit_repository::delete_action_audit_for_messages(
        transaction,
        conversation_id,
        message_ids,
    )
    .map_err(storage_error)?;
    if delete_protected_receipts {
        delete_protected_model_batch_receipts_for_messages(
            transaction,
            conversation_id,
            message_ids,
        )?;
    }
    chat_repository::delete_messages_in_transaction(transaction, conversation_id, message_ids)
        .map_err(storage_error)?;
    Ok(attachments)
}

fn delete_agent_tree_records(
    connection: &rusqlite::Connection,
    scope: &AgentTreeDeletionScope,
) -> Result<(), String> {
    let root_agent_id = &scope.root_agent_id;
    // Fork receipts express lineage, not ownership. Removing either endpoint retires only the
    // receipt; the other independently-owned root tree must remain intact.
    connection
        .execute(
            "DELETE FROM agent_member_conversation_forks
             WHERE source_root_agent_id = ?1 OR target_root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM conversation_forks
             WHERE source_root_agent_id = ?1 OR target_root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_model_batch_receipt_replays
             WHERE receipt_id IN (
                 SELECT receipt_id FROM agent_model_batch_receipts
                 WHERE agent_id IN (SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1)
             ) OR source_receipt_id IN (
                 SELECT receipt_id FROM agent_model_batch_receipts
                 WHERE agent_id IN (SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1)
             )",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    for table in [
        "agent_model_batch_receipt_items",
        "agent_model_batch_receipt_targets",
    ] {
        connection
            .execute(
                &format!(
                    "DELETE FROM {table}
                     WHERE receipt_id IN (
                         SELECT receipt_id FROM agent_model_batch_receipts
                         WHERE agent_id IN (
                             SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1
                         )
                     )"
                ),
                [root_agent_id],
            )
            .map_err(storage_error)?;
    }
    connection
        .execute(
            "DELETE FROM agent_model_batch_receipts
             WHERE agent_id IN (SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1)",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_interrupt_requests WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_tree_run_stops WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_wake_requests WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_collaboration_cursors
             WHERE caller_agent_id IN (
                 SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1
             ) OR target_agent_id IN (
                 SELECT agent_id FROM agent_nodes WHERE root_agent_id = ?1
             )",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_collaboration_event_activities WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_collaboration_events WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_collaboration_event_sequences WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    connection
        .execute(
            "DELETE FROM agent_mailbox_messages WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;

    if !scope.conversation_ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", scope.conversation_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        connection
            .execute(
                &format!(
                    "DELETE FROM conversation_history_fts
                     WHERE conversation_id IN ({placeholders})"
                ),
                rusqlite::params_from_iter(scope.conversation_ids.iter()),
            )
            .map_err(storage_error)?;
        connection
            .execute(
                &format!("DELETE FROM conversations WHERE id IN ({placeholders})"),
                rusqlite::params_from_iter(scope.conversation_ids.iter()),
            )
            .map_err(storage_error)?;
    }
    connection
        .execute(
            "DELETE FROM agent_nodes WHERE root_agent_id = ?1",
            [root_agent_id],
        )
        .map_err(storage_error)?;
    Ok(())
}

fn verify_foreign_keys(connection: &rusqlite::Connection) -> Result<(), String> {
    let violation = connection
        .query_row(
            "SELECT \"table\", rowid, parent, fkid FROM pragma_foreign_key_check LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<i64>>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(storage_error)?;
    if let Some((table, row_id, parent, foreign_key_id)) = violation {
        return Err(format!(
            "Agent tree deletion would violate foreign key {foreign_key_id} from {table} row {row_id:?} to {parent}"
        ));
    }
    Ok(())
}

fn with_agent_deletion_transaction<T>(
    connection: &mut rusqlite::Connection,
    operation: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, String>,
) -> Result<T, String> {
    let triggers_were_enabled = connection
        .db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .map_err(storage_error)?;
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)
        .map_err(storage_error)?;
    let result = (|| {
        let transaction = connection.transaction().map_err(storage_error)?;
        transaction
            .execute_batch("PRAGMA defer_foreign_keys = ON")
            .map_err(storage_error)?;
        let value = operation(&transaction)?;
        verify_foreign_keys(&transaction)?;
        transaction.commit().map_err(storage_error)?;
        Ok(value)
    })();
    let restore_result = connection
        .set_db_config(
            DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            triggers_were_enabled,
        )
        .map_err(storage_error);
    match (result, restore_result) {
        (Ok(value), Ok(_)) => Ok(value),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(operation_error), Err(restore_error)) => Err(format!(
            "{operation_error}; additionally failed to restore SQLite triggers: {restore_error}"
        )),
    }
}

fn conversation_matches_previous_after_removing(
    current: &ChatConversationRecord,
    previous: &ChatConversationRecord,
    removed_message_ids: &[String],
) -> bool {
    if current.id != previous.id
        || current.project_id != previous.project_id
        || current.title != previous.title
        || current.created_at != previous.created_at
        || current.pinned_at != previous.pinned_at
        || current.archived_at != previous.archived_at
        || current.unread_at != previous.unread_at
    {
        return false;
    }
    let retained = current
        .messages
        .iter()
        .filter(|message| !removed_message_ids.contains(&message.id))
        .collect::<Vec<_>>();
    retained.len() == previous.messages.len()
        && retained
            .into_iter()
            .zip(&previous.messages)
            .all(|(current, previous)| {
                current.id == previous.id
                    && current.role == previous.role
                    && current.content == previous.content
                    && current.created_at == previous.created_at
                    && current.status == previous.status
                    && current.agent_run_json == previous.agent_run_json
            })
}

pub(super) fn cleanup_fork_files(staged: &[(PathBuf, PathBuf)], committed: &[PathBuf]) {
    for (staging_path, _) in staged {
        let _ = fs::remove_file(staging_path);
    }
    for target_path in committed {
        let _ = fs::remove_file(target_path);
    }
}

// Each shard extends the same StorageService. Method bodies retain the exact transaction owner and
// ordering from the former monolith.
include!("conversations/projects_and_history.rs");
include!("conversations/turn_writes.rs");
include!("conversations/metadata_and_deletion.rs");
include!("conversations/history_projection.rs");
