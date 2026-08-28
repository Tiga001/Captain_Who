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
                    && current.ui_state_json == previous.ui_state_json
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

impl StorageService {
    pub fn load_projects(&self) -> Result<Vec<ProjectRecord>, String> {
        let connection = self.state.connection()?;
        project_repository::list_projects(&connection).map_err(storage_error)
    }

    pub fn save_project(&self, project: ProjectRecord) -> Result<ProjectRecord, String> {
        let connection = self.state.connection()?;
        project_repository::save_project(&connection, project.clone()).map_err(storage_error)?;
        Ok(project)
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let attachments =
            attachment_repository::list_project_deletion_attachments(&connection, project_id)
                .map_err(storage_error)?;
        let tree_scopes = project_agent_tree_deletion_scopes(&connection, project_id)?;
        let conversation_ids = project_conversation_ids(&connection, project_id)?;
        if tree_scopes.is_empty() {
            let transaction = connection.transaction().map_err(storage_error)?;
            let deleted_at = now_ms();
            for conversation_id in &conversation_ids {
                notification_repository::resolve_notification_events_by_conversation_id_in_transaction(
                    &transaction,
                    conversation_id,
                    deleted_at,
                )
                .map_err(storage_error)?;
            }
            automation_repository::terminalize_automation_runs_before_project_delete(
                &transaction,
                project_id,
                &conversation_ids,
                now_ms(),
            )
            .map_err(storage_error)?;
            usage_repository::roll_up_deleted_usage_for_project(&transaction, project_id, now_ms())
                .map_err(storage_error)?;
            pending_action_repository::delete_pending_actions_for_project(&transaction, project_id)
                .map_err(storage_error)?;
            agent_action_audit_repository::delete_action_audit_for_project(
                &transaction,
                project_id,
            )
            .map_err(storage_error)?;
            composer_draft_repository::delete_project_composer_drafts(&transaction, project_id)
                .map_err(storage_error)?;
            project_repository::delete_project(&transaction, project_id).map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        } else {
            with_agent_deletion_transaction(&mut connection, |transaction| {
                automation_repository::terminalize_automation_runs_before_project_delete(
                    transaction,
                    project_id,
                    &conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                ensure_deletion_scope_has_no_active_execution(transaction, &conversation_ids)?;
                automation_repository::invalidate_automations_before_trigger_disabled_project_delete(
                    transaction,
                    project_id,
                    &conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                usage_repository::roll_up_deleted_usage_for_project(
                    transaction,
                    project_id,
                    now_ms(),
                )
                .map_err(storage_error)?;
                pending_action_repository::delete_pending_actions_for_project(
                    transaction,
                    project_id,
                )
                .map_err(storage_error)?;
                agent_action_audit_repository::delete_action_audit_for_project(
                    transaction,
                    project_id,
                )
                .map_err(storage_error)?;
                composer_draft_repository::delete_project_composer_drafts(transaction, project_id)
                    .map_err(storage_error)?;
                if !conversation_ids.is_empty() {
                    let placeholders = std::iter::repeat_n("?", conversation_ids.len())
                        .collect::<Vec<_>>()
                        .join(", ");
                    transaction
                        .execute(
                            &format!(
                                "DELETE FROM conversation_history_fts
                                 WHERE conversation_id IN ({placeholders})"
                            ),
                            rusqlite::params_from_iter(conversation_ids.iter()),
                        )
                        .map_err(storage_error)?;
                }
                for scope in &tree_scopes {
                    delete_agent_tree_records(transaction, scope)?;
                }
                project_repository::delete_project(transaction, project_id)
                    .map_err(storage_error)?;
                Ok(())
            })?;
        }
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted project attachment files: {error}");
        }
        Ok(())
    }

    pub fn load_conversations(&self) -> Result<Vec<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversations =
            chat_repository::list_active_conversations(&connection).map_err(storage_error)?;
        self.attach_message_attachments(&connection, &mut conversations)?;
        attach_message_guidance_timelines(&connection, &mut conversations)?;
        Ok(conversations)
    }

    /// Loads complete conversations for the renderer and decorates only forked tasks with their
    /// backend-owned continuation boundary. Internal Agent callers continue to use the plain
    /// conversation record and never consume presentation lineage.
    pub fn load_conversation_views(&self) -> Result<Vec<ChatConversationViewRecord>, String> {
        let conversations = self.load_conversations()?;
        let connection = self.state.connection()?;
        conversations
            .into_iter()
            .filter_map(
                |conversation| match agent_graph_repository::get_agent_node_by_conversation(
                    &connection,
                    &conversation.id,
                ) {
                    Ok(Some(node)) if node.parent_agent_id.is_some() => None,
                    Ok(_) => Some(Ok(conversation)),
                    Err(error) => Some(Err(error.to_string())),
                },
            )
            .collect::<Result<Vec<_>, String>>()?
            .into_iter()
            .map(|conversation| {
                let mut conversation = conversation;
                chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
                    .map_err(storage_error)?;
                let continuation_origin = conversation_fork_repository::get_continuation_origin(
                    &connection,
                    &conversation.id,
                )
                .map_err(storage_error)?;
                Ok(ChatConversationViewRecord {
                    conversation,
                    continuation_origin,
                })
            })
            .collect()
    }

    pub fn load_conversation_metas(&self) -> Result<Vec<ChatConversationMetaRecord>, String> {
        let connection = self.state.connection()?;
        chat_repository::list_conversation_metas(&connection)
            .map_err(storage_error)?
            .into_iter()
            .filter_map(
                |conversation| match agent_graph_repository::get_agent_node_by_conversation(
                    &connection,
                    &conversation.id,
                ) {
                    Ok(Some(node)) if node.parent_agent_id.is_some() => None,
                    Ok(_) => Some(Ok(conversation)),
                    Err(error) => Some(Err(error.to_string())),
                },
            )
            .collect()
    }

    pub fn load_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationRecord>, String> {
        let connection = self.state.connection()?;
        let mut conversation =
            chat_repository::get_active_conversation(&connection, conversation_id)
                .map_err(storage_error)?;
        if let Some(conversation) = &mut conversation {
            self.attach_message_attachments(&connection, std::slice::from_mut(conversation))?;
            attach_message_guidance_timelines(&connection, std::slice::from_mut(conversation))?;
        }
        Ok(conversation)
    }

    /// Loads a complete observer Conversation and its input provenance from one SQLite read cut.
    ///
    /// This is intentionally a narrow bulk boundary: actor facts are decoded in one ordered
    /// query and returned as an in-memory map, rather than opening a new connection for every
    /// message. A missing or corrupt origin aborts the whole snapshot instead of silently
    /// presenting an Agent instruction as a human message.
    pub fn load_conversation_observer_snapshot(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ConversationObserverSnapshot>, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let mut conversation =
            chat_repository::get_active_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
        let Some(mut conversation) = conversation.take() else {
            transaction.commit().map_err(storage_error)?;
            return Ok(None);
        };
        self.attach_message_attachments(&transaction, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&transaction, std::slice::from_mut(&mut conversation))?;
        let mut input_origins =
            agent_graph_repository::conversation_message_origins(&transaction, conversation_id)
                .map_err(|error| error.to_string())?
                .into_iter()
                .collect::<BTreeMap<_, _>>();
        let active_user_ids = conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .map(|message| message.id.as_str())
            .collect::<HashSet<_>>();
        input_origins.retain(|message_id, _| active_user_ids.contains(message_id.as_str()));
        let expected_input_count = conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count();
        if input_origins.len() != expected_input_count
            || conversation
                .messages
                .iter()
                .filter(|message| message.role == "user")
                .any(|message| !input_origins.contains_key(&message.id))
        {
            return Err(
                "Conversation observer snapshot has incomplete input provenance.".to_string(),
            );
        }
        transaction.commit().map_err(storage_error)?;
        Ok(Some(ConversationObserverSnapshot {
            conversation,
            input_origins,
        }))
    }

    /// Loads the complete persisted Conversation together with the opaque revision used for Turn
    /// admission. Both facts come from one SQLite read transaction, so a later
    /// `save_conversation_and_begin_turn` can reject a stale full snapshot even when a competing
    /// Host has already completed and released its active trace.
    ///
    /// Unlike renderer/observer reads, this admission snapshot deliberately does not decorate
    /// `agent_run_json` from durable Trace, Guidance, or Command Session facts. Those decorations
    /// are presentation projections and may differ from the raw JSON copied into immutable Agent
    /// and fork-snapshot messages. Feeding them back into a write would make a valid continuation
    /// look like an attempted rewrite of immutable history.
    pub fn load_conversation_for_turn(
        &self,
        conversation_id: &str,
    ) -> Result<(Option<ChatConversationRecord>, Option<i64>), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let revision = transaction
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)?;
        let mut conversation =
            chat_repository::get_active_persisted_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
        if conversation.is_some() != revision.is_some() {
            return Err(
                "Conversation Turn admission snapshot is internally inconsistent.".to_string(),
            );
        }
        if let Some(conversation) = &mut conversation {
            self.attach_message_attachments(&transaction, std::slice::from_mut(conversation))?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok((conversation, revision))
    }

    pub fn conversation_revision(&self, conversation_id: &str) -> Result<Option<i64>, String> {
        let connection = self.state.connection()?;
        connection
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)
    }

    pub fn load_conversation_view(
        &self,
        conversation_id: &str,
    ) -> Result<Option<ChatConversationViewRecord>, String> {
        let Some(mut conversation) = self.load_conversation(conversation_id)? else {
            return Ok(None);
        };
        let connection = self.state.connection()?;
        chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
            .map_err(storage_error)?;
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, conversation_id)
                .map_err(storage_error)?;
        Ok(Some(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        }))
    }

    fn fork_conversation_request_with_domain_error(
        &self,
        input: ForkConversationRequest,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        self.fork_conversation_at_point_with_domain_error(
            input.request_id,
            input.source_conversation_id,
            input.fork_point,
            provider_continuation_vault,
        )
    }

    fn fork_conversation_at_point_with_domain_error(
        &self,
        request_id: String,
        source_conversation_id: String,
        point: ConversationForkPoint,
        provider_continuation_vault: Option<&ProviderContinuationVault>,
    ) -> Result<ChatConversationRecord, conversation_fork_repository::ConversationForkError> {
        conversation_fork_repository::validate_fork_point_input(
            &request_id,
            &source_conversation_id,
            &point,
        )
        .map_err(conversation_fork_repository::ConversationForkError::Other)?;
        let mut connection = self.state.connection()?;
        if let Some(existing) =
            conversation_fork_repository::find_existing_fork(&connection, &request_id)
                .map_err(storage_error)?
        {
            if existing.source_conversation_id != source_conversation_id
                || existing.source_fork_point != point
            {
                return Err("同一个分叉请求 ID 不能用于不同的历史快照。"
                    .to_string()
                    .into());
            }
            conversation_fork_repository::validate_existing_fork_authority(&connection, &existing)?;
            let mut conversation = chat_repository::get_active_conversation(
                &connection,
                &existing.target_conversation_id,
            )
            .map_err(storage_error)?
            .ok_or_else(|| "分叉记录指向的新任务不存在。".to_string())?;
            self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
            attach_message_guidance_timelines(
                &connection,
                std::slice::from_mut(&mut conversation),
            )?;
            return Ok(conversation);
        }

        let mut plan = conversation_fork_repository::build_fork_plan_at_point(
            &connection,
            &request_id,
            &source_conversation_id,
            &point,
            now_ms(),
        )?;
        ensure_project_reference_exists(&connection, plan.target.project_id.as_deref())?;
        let provider_continuations = if plan.provider_continuation_mappings.is_empty() {
            Vec::new()
        } else {
            let vault = provider_continuation_vault.ok_or_else(|| {
                conversation_fork_repository::ConversationForkError::Other(
                    "原任务包含 Provider continuation，但当前 Host 未提供安全克隆能力。"
                        .to_string(),
                )
            })?;
            vault
                .prepare_fork_clones(&plan.provider_continuation_mappings)
                .map_err(|error| {
                    conversation_fork_repository::ConversationForkError::Other(format!(
                        "Provider continuation 安全克隆失败：{}",
                        error.code()
                    ))
                })?
        };

        let mut staged_files = Vec::new();
        let mut committed_files = Vec::new();
        let prepare_files = (|| -> Result<(), String> {
            for attachment in plan.attachments_mut() {
                let source_path = safe_existing_attachment_storage_path(
                    &self.attachment_root,
                    &attachment.source.storage_rel_path,
                )
                .ok_or_else(|| {
                    format!("原任务附件文件不存在：{}", attachment.source.original_name)
                })?;
                let target_rel_path = attachment_storage_rel_path(
                    &attachment.target.conversation_id,
                    &attachment.target.message_id,
                    &attachment.target.id,
                    &attachment.target.original_name,
                );
                attachment.target.storage_rel_path = slash_path(&target_rel_path);
                let target_path = self.attachment_root.join(&target_rel_path);
                let parent = target_path
                    .parent()
                    .ok_or_else(|| "新任务附件路径无效。".to_string())?;
                fs::create_dir_all(parent)
                    .map_err(|error| format!("创建新任务附件目录失败：{error}"))?;
                let staging_path = parent.join(format!(
                    ".{}.forking-{}",
                    safe_path_component(&attachment.target.id, "attachment"),
                    Uuid::new_v4()
                ));
                staged_files.push((staging_path.clone(), target_path));
                let copied = fs::copy(&source_path, &staging_path)
                    .map_err(|error| format!("复制附件失败：{error}"))?;
                if copied != attachment.source.size_bytes {
                    return Err(format!(
                        "复制附件时大小不一致：{}",
                        attachment.source.original_name
                    ));
                }
            }
            for (staging_path, target_path) in &staged_files {
                fs::rename(staging_path, target_path)
                    .map_err(|error| format!("提交新任务附件失败：{error}"))?;
                committed_files.push(target_path.clone());
            }
            Ok(())
        })();
        if let Err(error) = prepare_files {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error.into());
        }

        if let Err(error) =
            conversation_fork_repository::commit_fork_plan_with_provider_continuations(
                &mut connection,
                &plan,
                &provider_continuations,
            )
        {
            cleanup_fork_files(&staged_files, &committed_files);
            return Err(error);
        }
        let mut conversation = chat_repository::get_conversation(&connection, &plan.target.id)
            .map_err(storage_error)?
            .ok_or_else(|| "新任务创建后无法重新读取。".to_string())?;
        self.attach_message_attachments(&connection, std::slice::from_mut(&mut conversation))?;
        attach_message_guidance_timelines(&connection, std::slice::from_mut(&mut conversation))?;
        Ok(conversation)
    }

    pub fn fork_conversation_request_view(
        &self,
        input: ForkConversationRequest,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation = self.fork_conversation_request_with_domain_error(input, None)?;
        self.decorate_fork_conversation_view(conversation)
    }

    pub fn fork_conversation_request_view_with_provider_continuation_vault(
        &self,
        input: ForkConversationRequest,
        provider_continuation_vault: &ProviderContinuationVault,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let conversation = self.fork_conversation_request_with_domain_error(
            input,
            Some(provider_continuation_vault),
        )?;
        self.decorate_fork_conversation_view(conversation)
    }

    fn decorate_fork_conversation_view(
        &self,
        mut conversation: ChatConversationRecord,
    ) -> Result<ChatConversationViewRecord, conversation_fork_repository::ConversationForkError>
    {
        let connection = self.state.connection()?;
        chat_repository::retain_user_facing_root_messages(&connection, &mut conversation)
            .map_err(storage_error)?;
        let continuation_origin =
            conversation_fork_repository::get_continuation_origin(&connection, &conversation.id)
                .map_err(storage_error)?;
        Ok(ChatConversationViewRecord {
            conversation,
            continuation_origin,
        })
    }

    pub fn search_chats(&self, input: &ChatSearchInput) -> Result<Vec<ChatSearchResult>, String> {
        let connection = self.state.connection()?;
        chat_search_repository::search_chats(&connection, input).map_err(storage_error)
    }

    pub fn search_conversation_history(
        &self,
        conversation_id: &str,
        query: &str,
        filter: &conversation_history_repository::ConversationHistorySearchFilter,
        limit: usize,
    ) -> Result<Vec<conversation_history_repository::ConversationHistorySearchHit>, String> {
        let connection = self.state.connection()?;
        conversation_history_repository::search_records(
            &connection,
            conversation_id,
            query,
            filter,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_around(
        &self,
        conversation_id: &str,
        reference: &conversation_history_repository::ConversationHistoryRecordRef,
        before: usize,
        after: usize,
    ) -> Result<
        Option<Vec<conversation_history_repository::ConversationHistoryTimelineRecord>>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_repository::records_around(
            &connection,
            conversation_id,
            reference,
            before,
            after,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_range(
        &self,
        conversation_id: &str,
        start: &conversation_history_repository::ConversationHistoryRecordRef,
        end: &conversation_history_repository::ConversationHistoryRecordRef,
        limit: usize,
    ) -> Result<
        Option<Vec<conversation_history_repository::ConversationHistoryTimelineRecord>>,
        String,
    > {
        let connection = self.state.connection()?;
        conversation_history_repository::records_in_range(
            &connection,
            conversation_id,
            start,
            end,
            limit,
        )
        .map_err(storage_error)
    }

    pub fn conversation_history_tool_exchange(
        &self,
        conversation_id: &str,
        reference: Option<&conversation_history_repository::ConversationHistoryRecordRef>,
        call_id: Option<&str>,
        run_id: Option<&str>,
    ) -> Result<Option<Vec<conversation_history_repository::ConversationHistoryRecord>>, String>
    {
        let connection = self.state.connection()?;
        conversation_history_repository::get_tool_exchange(
            &connection,
            conversation_id,
            reference,
            call_id,
            run_id,
        )
        .map_err(storage_error)
    }

    pub fn read_conversation_history_record(
        &self,
        conversation_id: &str,
        reference: &conversation_history_repository::ConversationHistoryRecordRef,
    ) -> Result<Option<conversation_history_repository::ConversationHistoryRecord>, String> {
        let connection = self.state.connection()?;
        conversation_history_repository::read_record(&connection, conversation_id, reference)
            .map_err(storage_error)
    }

    pub fn archive_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::store_archive(&mut connection, &input)
            .map_err(storage_error)
    }

    pub fn delete_unreferenced_exact_conversation_tool_result(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveInput,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::delete_unreferenced_exact_archive(
            &mut connection,
            &input,
        )
        .map_err(storage_error)
    }

    pub fn archive_conversation_tool_result_file(
        &self,
        input: conversation_history_archive_repository::ConversationHistoryArchiveFileInput,
    ) -> Result<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor, String>
    {
        let mut connection = self.state.connection()?;
        conversation_history_archive_repository::store_archive_file(&mut connection, &input)
            .map_err(storage_error)
    }

    pub fn find_conversation_history_archive_for_trace_item(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        sequence: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor>,
        String,
    > {
        let connection = self.state.connection()?;
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(assistant_message_id)
        {
            return Ok(None);
        }
        conversation_history_archive_repository::find_archive_for_trace_item(
            &connection,
            conversation_id,
            assistant_message_id,
            sequence,
        )
        .map_err(storage_error)
    }

    pub fn find_conversation_history_archive_by_ref(
        &self,
        conversation_id: &str,
        archive_ref: &str,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchiveDescriptor>,
        String,
    > {
        let connection = self.state.connection()?;
        let archive = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?;
        let Some(archive) = archive else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
        Ok(Some(archive))
    }

    pub fn find_conversation_history_archive_match_char_offset(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        query: &str,
    ) -> Result<Option<u64>, String> {
        let connection = self.state.connection()?;
        let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?
        else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
        conversation_history_archive_repository::find_archive_match_char_offset(
            &connection,
            conversation_id,
            archive_ref,
            query,
        )
        .map_err(storage_error)
    }

    pub fn read_conversation_history_archive_page(
        &self,
        conversation_id: &str,
        archive_ref: &str,
        unit: conversation_history_archive_repository::ConversationHistoryArchivePageUnit,
        start: u64,
        maximum: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchivePage>,
        String,
    > {
        let connection = self.state.connection()?;
        let Some(archive) = conversation_history_archive_repository::find_archive_by_ref(
            &connection,
            conversation_id,
            archive_ref,
        )
        .map_err(storage_error)?
        else {
            return Ok(None);
        };
        if conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?
        .contains(&archive.assistant_message_id)
        {
            return Ok(None);
        }
        conversation_history_archive_repository::read_archive_page(
            &connection,
            conversation_id,
            archive_ref,
            unit,
            start,
            maximum,
        )
        .map_err(storage_error)
    }

    /// Follows the same opaque Archive route accepted by `conversation_history` while preserving
    /// the caller's conversation boundary. This narrow storage entry point is useful to Host
    /// integrations that must verify a ToolResult recovery route without exposing archive ids or
    /// the route codec itself.
    pub fn read_conversation_history_archive_page_from_open(
        &self,
        conversation_id: &str,
        open: &str,
        maximum_chars: u64,
    ) -> Result<
        Option<conversation_history_archive_repository::ConversationHistoryArchivePage>,
        String,
    > {
        let crate::storage::conversation_history_open::HistoryOpenRoute::Archive {
            archive_ref,
            start_char,
        } = crate::storage::conversation_history_open::decode_history_open(open)?
        else {
            return Err("history open does not reference an Exact Archive page".to_string());
        };
        self.read_conversation_history_archive_page(
            conversation_id,
            &archive_ref,
            conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Char,
            start_char,
            maximum_chars,
        )
    }

    /// Creates the opaque Archive route accepted by `conversation_history` after verifying that
    /// the immutable Archive belongs to the caller's conversation.
    ///
    /// Host integrations use this narrow method to project a recovery capability without exposing
    /// the raw Archive reference or the route codec across the Core boundary.
    pub fn conversation_history_archive_open(
        &self,
        conversation_id: &str,
        archive_ref: &str,
    ) -> Result<Option<String>, String> {
        if self
            .find_conversation_history_archive_by_ref(conversation_id, archive_ref)?
            .is_none()
        {
            return Ok(None);
        }
        crate::storage::conversation_history_open::encode_archive_history_open(archive_ref, 0)
            .map(Some)
    }

    pub fn save_conversation(
        &self,
        conversation: ChatConversationRecord,
    ) -> Result<ChatConversationRecord, String> {
        let mut connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, conversation.project_id.as_deref())?;
        chat_repository::save_conversation(&mut connection, conversation.clone())
            .map_err(storage_error)?;
        Ok(conversation)
    }

    /// Atomically installs the provisional messages and the durable one-active-Turn trace.
    /// `BEGIN IMMEDIATE` plus the canonical partial unique index serializes independent Hosts;
    /// a losing Host leaves neither messages nor metadata behind.
    #[allow(clippy::too_many_arguments)]
    pub fn save_conversation_and_begin_turn(
        &self,
        conversation: ChatConversationRecord,
        expected_revision: Option<i64>,
        trusted_wake: Option<&crate::TrustedAgentWakeTurnAdmission>,
        permission_source: crate::AgentTurnPermissionSource,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        trace_updated_at: i64,
    ) -> Result<(ChatConversationRecord, crate::AgentPermissions), String> {
        self.save_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            expected_revision,
            trusted_wake,
            permission_source,
            &[],
            trace,
            trace_created_at,
            trace_updated_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn save_conversation_and_begin_turn_with_preloaded_agent_messages(
        &self,
        conversation: ChatConversationRecord,
        expected_revision: Option<i64>,
        trusted_wake: Option<&crate::TrustedAgentWakeTurnAdmission>,
        permission_source: crate::AgentTurnPermissionSource,
        preloaded_agent_message_ids: &[String],
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        trace_updated_at: i64,
    ) -> Result<(ChatConversationRecord, crate::AgentPermissions), String> {
        let (conversation, permissions, outcome, automation_outcome) = self
            .save_conversation_and_begin_turn_internal(
                conversation,
                expected_revision,
                trusted_wake,
                permission_source,
                preloaded_agent_message_ids,
                trace,
                trace_created_at,
                trace_updated_at,
                None,
                None,
                None,
            )?;
        if !matches!(outcome, super::ConversationTurnRewriteBeginOutcome::Started) {
            return Err(
                "ordinary Turn admission unexpectedly replayed an edit request".to_string(),
            );
        }
        if automation_outcome.is_some() {
            return Err(
                "ordinary Turn admission unexpectedly consumed an automation claim".to_string(),
            );
        }
        Ok((conversation, permissions))
    }

    /// Atomically admits a scheduler-owned HumanRoot Turn.
    ///
    /// Conversation metadata, the user/assistant messages, the empty in-progress Trace, delivery
    /// bindings, and the `automation_runs` admitting->running transition share one
    /// `BEGIN IMMEDIATE` transaction. A caller may launch model/MCP work only after this method
    /// returns `Admitted`.
    #[allow(clippy::too_many_arguments)]
    pub fn save_automation_conversation_and_begin_turn_with_preloaded_agent_messages(
        &self,
        conversation: ChatConversationRecord,
        expected_revision: Option<i64>,
        permission_source: crate::AgentTurnPermissionSource,
        preloaded_agent_message_ids: &[String],
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        trace_updated_at: i64,
        automation_admission: &automation_repository::AutomationRunAdmissionInput,
    ) -> Result<
        (
            ChatConversationRecord,
            crate::AgentPermissions,
            automation_repository::AutomationRunAdmissionOutcome,
        ),
        String,
    > {
        let (conversation, permissions, rewrite_outcome, automation_outcome) = self
            .save_conversation_and_begin_turn_internal(
                conversation,
                expected_revision,
                None,
                permission_source,
                preloaded_agent_message_ids,
                trace,
                trace_created_at,
                trace_updated_at,
                None,
                None,
                Some(automation_admission),
            )?;
        if !matches!(
            rewrite_outcome,
            super::ConversationTurnRewriteBeginOutcome::Started
        ) {
            return Err(
                "automation Turn admission unexpectedly replayed an edit request".to_string(),
            );
        }
        let automation_outcome = automation_outcome.ok_or_else(|| {
            "automation Turn admission did not consume its durable claim".to_string()
        })?;
        Ok((conversation, permissions, automation_outcome))
    }

    /// Atomically records an immutable logical replacement and starts its new root Turn.
    ///
    /// The source messages, Trace, Usage, Tool effects, and model-batch receipts are retained.
    /// Only active Conversation projections hide the source pair after this transaction commits.
    #[allow(clippy::too_many_arguments)]
    pub fn rewrite_conversation_turn_and_begin_turn(
        &self,
        conversation: ChatConversationRecord,
        expected_revision: Option<i64>,
        permission_source: crate::AgentTurnPermissionSource,
        preloaded_agent_message_ids: &[String],
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        trace_updated_at: i64,
        rewrite: &conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission,
        prepared_attachments: &super::PreparedConversationTurnRewriteAttachments,
    ) -> Result<
        (
            ChatConversationRecord,
            crate::AgentPermissions,
            super::ConversationTurnRewriteBeginOutcome,
        ),
        String,
    > {
        let (conversation, permissions, outcome, automation_outcome) = self
            .save_conversation_and_begin_turn_internal(
                conversation,
                expected_revision,
                None,
                permission_source,
                preloaded_agent_message_ids,
                trace,
                trace_created_at,
                trace_updated_at,
                Some(rewrite),
                Some(prepared_attachments),
                None,
            )?;
        if automation_outcome.is_some() {
            return Err("rewrite admission unexpectedly consumed an automation claim".to_string());
        }
        Ok((conversation, permissions, outcome))
    }

    #[allow(clippy::too_many_arguments)]
    fn save_conversation_and_begin_turn_internal(
        &self,
        conversation: ChatConversationRecord,
        expected_revision: Option<i64>,
        trusted_wake: Option<&crate::TrustedAgentWakeTurnAdmission>,
        permission_source: crate::AgentTurnPermissionSource,
        preloaded_agent_message_ids: &[String],
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        trace_updated_at: i64,
        rewrite: Option<&conversation_turn_rewrite_repository::ConversationTurnRewriteAdmission>,
        prepared_attachments: Option<&super::PreparedConversationTurnRewriteAttachments>,
        automation_admission: Option<&automation_repository::AutomationRunAdmissionInput>,
    ) -> Result<
        (
            ChatConversationRecord,
            crate::AgentPermissions,
            super::ConversationTurnRewriteBeginOutcome,
            Option<automation_repository::AutomationRunAdmissionOutcome>,
        ),
        String,
    > {
        let mut connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, conversation.project_id.as_deref())?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let bound_agent = transaction
            .query_row(
                "SELECT agent_id, parent_agent_id, lifecycle
                 FROM agent_nodes WHERE conversation_id = ?1",
                [&conversation.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        match (trusted_wake, bound_agent.as_ref()) {
            (Some(trusted), Some((agent_id, _, lifecycle)))
                if agent_id == &trusted.agent_id && lifecycle == "active" => {}
            (Some(_), _) => {
                return Err(
                    "Trusted Agent Turn no longer owns an active bound Conversation.".to_string(),
                );
            }
            (None, Some((_, parent_agent_id, lifecycle)))
                if parent_agent_id.is_none() && lifecycle == "active" => {}
            (None, Some(_)) => {
                return Err(
                    "Human Turn admission requires an active root Agent Conversation.".to_string(),
                );
            }
            (None, None) => {}
        }
        let effective_permissions = match (permission_source, trusted_wake, bound_agent.as_ref()) {
            (
                crate::AgentTurnPermissionSource::HostAuthenticatedRoot(permissions),
                None,
                None | Some((_, None, _)),
            ) => permissions,
            (
                crate::AgentTurnPermissionSource::InheritTrustedAncestors,
                Some(_),
                Some((agent_id, Some(_), _)),
            ) => agent_graph_repository::inherit_agent_permissions_in_transaction(
                &transaction,
                agent_id,
            )
            .map_err(|error| error.to_string())?,
            _ => {
                return Err(
                    "Turn permission authority does not match its trusted root/child admission."
                        .to_string(),
                );
            }
        };
        if let Some(rewrite) = rewrite {
            if let Some(existing) = conversation_turn_rewrite_repository::get_by_request_id(
                &transaction,
                &rewrite.request_id,
            )
            .map_err(storage_error)?
            {
                if existing.request_fingerprint != rewrite.request_fingerprint
                    || existing.conversation_id != rewrite.conversation_id
                    || existing.source_user_message_id != rewrite.source_user_message_id
                    || existing.source_assistant_message_id != rewrite.source_assistant_message_id
                    || existing.replacement_user_message_id != rewrite.replacement_user_message_id
                    || existing.replacement_assistant_message_id
                        != rewrite.replacement_assistant_message_id
                {
                    return Err(
                        "edit_turn_request_conflict: requestId was already used for another rewrite"
                            .to_string(),
                    );
                }
                transaction.commit().map_err(storage_error)?;
                return Ok((
                    conversation,
                    effective_permissions,
                    super::ConversationTurnRewriteBeginOutcome::Replayed(Box::new(existing)),
                    None,
                ));
            }
            if rewrite.conversation_id != conversation.id
                || rewrite.replacement_user_message_id
                    != conversation
                        .messages
                        .iter()
                        .find(|message| message.id == rewrite.replacement_user_message_id)
                        .map(|message| message.id.as_str())
                        .unwrap_or_default()
                || rewrite.replacement_assistant_message_id != trace.assistant_message_id
                || rewrite.run_id != trace.run_id
            {
                return Err(
                    "edit_turn_identity_mismatch: rewrite does not match the new Turn".to_string(),
                );
            }
            conversation_turn_rewrite_repository::validate_source_is_editable_tail(
                &transaction,
                rewrite,
            )?;
        }
        let claimed_wake = if let Some(trusted) = trusted_wake {
            let wake = transaction
                .query_row(
                    "SELECT agent_id, source_agent_message_id, status, claim_token,
                            lease_expires_at, run_id, assistant_message_id
                     FROM agent_wake_requests WHERE wake_id = ?1",
                    [&trusted.wake_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<i64>>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, Option<String>>(6)?,
                        ))
                    },
                )
                .optional()
                .map_err(storage_error)?
                .ok_or_else(|| "Trusted Agent Wake no longer exists.".to_string())?;
            if wake.0 != trusted.agent_id
                || wake.1.as_deref() != Some(trusted.source_agent_message_id.as_str())
                || wake.2 != "claimed"
                || wake.3.as_deref() != Some(trusted.claim_token.as_str())
                || wake.4.is_none_or(|deadline| trace_updated_at >= deadline)
                || wake.5.is_some()
                || wake.6.is_some()
            {
                return Err(
                    "Trusted Agent Wake claim is stale, mismatched, or already dispatched."
                        .to_string(),
                );
            }
            let source_is_projected = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1
                         FROM agent_mailbox_messages AS mailbox
                         JOIN messages AS projection
                           ON projection.source_agent_message_id = mailbox.message_id
                          AND projection.id = mailbox.projection_message_id
                         JOIN agent_nodes AS recipient
                           ON recipient.agent_id = mailbox.recipient_agent_id
                         WHERE mailbox.message_id = ?1
                           AND mailbox.delivery_status = 'acknowledged'
                           AND recipient.agent_id = ?2
                           AND projection.conversation_id = recipient.conversation_id
                           AND projection.role = 'user'
                           AND projection.input_origin_kind = 'agent'
                           AND projection.input_origin_agent_id = mailbox.sender_agent_id
                           AND projection.content = mailbox.content
                     )",
                    rusqlite::params![&trusted.source_agent_message_id, &trusted.agent_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_error)?;
            if !source_is_projected {
                return Err(
                    "Trusted Agent Wake source has not been durably projected and acknowledged."
                        .to_string(),
                );
            }
            Some(trusted)
        } else {
            None
        };
        let current_revision = transaction
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [&conversation.id],
                |row| row.get::<_, i64>(0),
            )
            .optional()
            .map_err(storage_error)?;
        if current_revision != expected_revision {
            return Err(
                "Conversation changed after Turn preparation began; retry from fresh history."
                    .to_string(),
            );
        }
        let active_run = transaction
            .query_row(
                "SELECT run_id FROM conversation_turn_traces
                 WHERE conversation_id = ?1 AND terminal_status = 'in_progress'
                 LIMIT 1",
                [&conversation.id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        if let Some(active_run) = active_run {
            return Err(format!(
                "conversation already has an active durable Turn ({active_run})"
            ));
        }
        if let Some(admission) = automation_admission {
            match automation_repository::revalidate_automation_permission_for_admission_in_transaction(
                &transaction,
                admission,
            )
            .map_err(storage_error)?
            {
                automation_repository::AutomationPermissionAdmissionOutcome::Enabled => {}
                automation_repository::AutomationPermissionAdmissionOutcome::Blocked => {
                    // No Conversation, message, Trace, attachment, or delivery write has happened
                    // yet. Commit only the task/run block and its trigger-owned attention/outbox
                    // effects, then surface the stable Host-only sentinel to the Agent adapter.
                    transaction.commit().map_err(storage_error)?;
                    return Err(
                        automation_repository::AUTOMATION_PERMISSION_DISABLED_AT_ADMISSION
                            .to_string(),
                    );
                }
            }
        }
        chat_repository::save_conversation_in_connection(&transaction, &conversation)
            .map_err(storage_error)?;
        if let Some(prepared) = prepared_attachments {
            let rewrite = rewrite.ok_or_else(|| {
                "prepared rewrite attachments require a rewrite admission".to_string()
            })?;
            for attachment in &prepared.records {
                if attachment.conversation_id != conversation.id
                    || attachment.message_id != rewrite.replacement_user_message_id
                    || attachment.project_id != conversation.project_id
                {
                    return Err(
                        "prepared rewrite attachment ownership does not match the new user message"
                            .to_string(),
                    );
                }
                attachment_repository::insert_attachment(&transaction, attachment)
                    .map_err(storage_error)?;
            }
        }
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            trace_created_at,
            trace_updated_at,
        )
        .map_err(storage_error)?;
        if let Some((agent_id, _, _)) = bound_agent.as_ref() {
            agent_graph_repository::record_agent_effective_permissions_in_transaction(
                &transaction,
                agent_id,
                &conversation.id,
                &trace.run_id,
                &trace.assistant_message_id,
                effective_permissions,
                trace_updated_at,
            )
            .map_err(|error| error.to_string())?;
        }
        agent_delivery_repository::bind_turn_start_messages_in_transaction(
            &transaction,
            &crate::BindAgentTurnStartInput {
                conversation_id: conversation.id.clone(),
                run_id: trace.run_id.clone(),
                assistant_message_id: trace.assistant_message_id.clone(),
                model_batch_index: 1,
            },
            preloaded_agent_message_ids,
            trace_updated_at,
        )
        .map_err(|error| error.to_string())?;
        if let Some(trusted) = claimed_wake {
            let changed = transaction
                .execute(
                    "UPDATE agent_wake_requests
                     SET status = 'running', status_revision = status_revision + 1,
                         run_id = ?1, assistant_message_id = ?2, started_at = ?3
                     WHERE wake_id = ?4 AND agent_id = ?5 AND status = 'claimed'
                       AND claim_token = ?6 AND source_agent_message_id = ?7
                       AND lease_expires_at > ?3 AND run_id IS NULL
                       AND assistant_message_id IS NULL",
                    rusqlite::params![
                        &trace.run_id,
                        &trace.assistant_message_id,
                        trace_updated_at,
                        &trusted.wake_id,
                        &trusted.agent_id,
                        &trusted.claim_token,
                        &trusted.source_agent_message_id,
                    ],
                )
                .map_err(storage_error)?;
            if changed != 1 {
                return Err(
                    "Trusted Agent Wake lost its claim during atomic Turn admission.".to_string(),
                );
            }
        }
        if let Some(rewrite) = rewrite {
            conversation_turn_rewrite_repository::insert_in_transaction(&transaction, rewrite)
                .map_err(storage_error)?;
        }
        let automation_outcome = if let Some(admission) = automation_admission {
            if trusted_wake.is_some() || rewrite.is_some() {
                return Err(
                    "automation admission cannot be combined with a Wake or rewrite".to_string(),
                );
            }
            if admission.agent_run_id != trace.run_id
                || admission.conversation_id != conversation.id
                || admission.assistant_message_id != trace.assistant_message_id
                || !conversation.messages.iter().any(|message| {
                    message.id == admission.user_message_id && message.role == "user"
                })
            {
                return Err(
                    "automation admission identity does not match the prepared HumanRoot Turn"
                        .to_string(),
                );
            }
            let outcome =
                automation_repository::admit_automation_run_in_transaction(&transaction, admission)
                    .map_err(storage_error)?;
            match &outcome {
                automation_repository::AutomationRunAdmissionOutcome::Admitted(_) => {}
                automation_repository::AutomationRunAdmissionOutcome::Replayed(_) => {
                    return Err(
                        "automation Turn was already admitted; recover it from the durable trace"
                            .to_string(),
                    );
                }
                automation_repository::AutomationRunAdmissionOutcome::Stale(_) => {
                    return Err("automation Turn admission claim is stale".to_string());
                }
                automation_repository::AutomationRunAdmissionOutcome::Cancelled(_) => {
                    return Err("automation Turn admission was cancelled".to_string());
                }
            }
            Some(outcome)
        } else {
            None
        };
        transaction.commit().map_err(storage_error)?;
        Ok((
            conversation,
            effective_permissions,
            super::ConversationTurnRewriteBeginOutcome::Started,
            automation_outcome,
        ))
    }

    pub fn get_conversation_turn_rewrite(
        &self,
        request_id: &str,
    ) -> Result<Option<conversation_turn_rewrite_repository::ConversationTurnRewriteRecord>, String>
    {
        let connection = self.state.connection()?;
        conversation_turn_rewrite_repository::get_by_request_id(&connection, request_id)
            .map_err(storage_error)
    }

    pub fn save_conversation_meta(
        &self,
        conversation: ChatConversationMetaRecord,
    ) -> Result<ChatConversationMetaRecord, String> {
        let connection = self.state.connection()?;
        ensure_project_reference_exists(&connection, conversation.project_id.as_deref())?;
        chat_repository::save_conversation_meta(&connection, &conversation)
            .map_err(storage_error)?;
        Ok(conversation)
    }

    /// Removes the exact provisional rows written while preparing a Conversation Turn.
    ///
    /// Runtime admission normally commits an empty in-progress trace immediately after prepare.
    /// If any prepare step or that trace reservation fails, this rollback restores the prior
    /// Conversation metadata and removes the provisional messages, attachments, and their
    /// message-anchored World State. A Conversation created solely for the failed Turn is removed
    /// altogether. Callers must serialize this operation with Turn admission.
    pub fn rollback_conversation_turn_preparation(
        &self,
        conversation_id: &str,
        user_message_id: &str,
        assistant_message_id: &str,
        provisional_run_id: Option<&str>,
        previous: Option<&ChatConversationRecord>,
        previous_world_state_was_empty: bool,
    ) -> Result<(), String> {
        let message_ids = vec![
            user_message_id.to_string(),
            assistant_message_id.to_string(),
        ];
        self.rollback_prepared_turn_messages(
            conversation_id,
            assistant_message_id,
            &message_ids,
            provisional_run_id,
            previous,
            previous_world_state_was_empty,
        )
    }

    /// Rolls back child-Wake preparation without ever deleting its immutable parent-task
    /// projection. The projection predates this Turn and is the Conversation-side view of the
    /// Mailbox source of truth; only the provisional assistant belongs to the failed start.
    pub fn rollback_agent_wake_turn_preparation(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        provisional_run_id: Option<&str>,
        previous: &ChatConversationRecord,
        previous_world_state_was_empty: bool,
    ) -> Result<(), String> {
        self.rollback_prepared_turn_messages(
            conversation_id,
            assistant_message_id,
            &[assistant_message_id.to_string()],
            provisional_run_id,
            Some(previous),
            previous_world_state_was_empty,
        )
    }

    fn rollback_prepared_turn_messages(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message_ids: &[String],
        provisional_run_id: Option<&str>,
        previous: Option<&ChatConversationRecord>,
        previous_world_state_was_empty: bool,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        if let Some(run_id) = provisional_run_id {
            let trace_is_exact_empty = transaction
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM conversation_turn_traces AS trace
                         WHERE trace.assistant_message_id = ?1
                           AND trace.conversation_id = ?2
                           AND trace.run_id = ?3
                           AND trace.terminal_status = 'in_progress'
                           AND NOT EXISTS (
                               SELECT 1 FROM conversation_turn_trace_items AS item
                               WHERE item.assistant_message_id = trace.assistant_message_id
                           )
                     )",
                    rusqlite::params![assistant_message_id, conversation_id, run_id],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(storage_error)?;
            if trace_is_exact_empty {
                transaction
                    .execute(
                        "DELETE FROM conversation_turn_traces
                         WHERE assistant_message_id = ?1 AND conversation_id = ?2 AND run_id = ?3",
                        rusqlite::params![assistant_message_id, conversation_id, run_id],
                    )
                    .map_err(storage_error)?;
            } else {
                let provisional_identity_exists = transaction
                    .query_row(
                        "SELECT EXISTS(
                             SELECT 1 FROM messages
                             WHERE id = ?1 AND conversation_id = ?2
                             UNION ALL
                             SELECT 1 FROM conversation_turn_traces
                             WHERE assistant_message_id = ?1
                         )",
                        rusqlite::params![assistant_message_id, conversation_id],
                        |row| row.get::<_, bool>(0),
                    )
                    .map_err(storage_error)?;
                // Validation/model resolution can fail before the atomic begin transaction. That
                // zero-write case is safe to roll back idempotently; any persisted identity must
                // be exact, empty and owned by this run.
                if provisional_identity_exists {
                    return Err(
                        "provisional Conversation Turn trace is non-empty or owned by another run"
                            .to_string(),
                    );
                }
                return Ok(());
            }
        }
        let attachments = attachment_repository::list_message_attachments(
            &transaction,
            conversation_id,
            message_ids,
        )
        .map_err(storage_error)?;
        let current_before_rollback =
            chat_repository::get_conversation(&transaction, conversation_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    "provisional Conversation disappeared before rollback".to_string()
                })?;
        let current_revision = transaction
            .query_row(
                "SELECT revision FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(storage_error)?;
        let exact_previous_projection = previous.is_some_and(|previous| {
            conversation_matches_previous_after_removing(
                &current_before_rollback,
                previous,
                message_ids,
            )
        });
        let exact_new_projection = previous.is_none()
            && current_revision == i64::try_from(message_ids.len()).unwrap_or(i64::MAX)
            && current_before_rollback
                .messages
                .iter()
                .all(|message| message_ids.contains(&message.id))
            && current_before_rollback.pinned_at.is_none()
            && current_before_rollback.archived_at.is_none()
            && current_before_rollback.unread_at.is_none();

        if let Some(previous) = previous {
            attachment_repository::delete_message_attachments(
                &transaction,
                conversation_id,
                message_ids,
            )
            .map_err(storage_error)?;
            pending_action_repository::delete_pending_actions_for_messages(
                &transaction,
                conversation_id,
                message_ids,
            )
            .map_err(storage_error)?;
            agent_action_audit_repository::delete_action_audit_for_messages(
                &transaction,
                conversation_id,
                message_ids,
            )
            .map_err(storage_error)?;
            chat_repository::delete_messages_in_transaction(
                &transaction,
                conversation_id,
                message_ids,
            )
            .map_err(storage_error)?;
            if previous_world_state_was_empty && exact_previous_projection {
                transaction
                    .execute(
                        "DELETE FROM conversation_world_state_epochs WHERE conversation_id = ?1",
                        [conversation_id],
                    )
                    .map_err(storage_error)?;
            }
            // Restore the old model/timestamp only when no unrelated Conversation/message fact
            // changed after admission. Otherwise targeted deletion above is the complete inverse
            // of this Turn; overwriting title/pin/archive or another Host's model choice would be
            // data loss.
            if exact_previous_projection {
                let restored = transaction
                    .execute(
                        "UPDATE conversations
                         SET project_id = ?1, model_id = ?2, title = ?3, created_at = ?4,
                             updated_at = ?5, pinned_at = ?6, archived_at = ?7, unread_at = ?8
                         WHERE id = ?9",
                        rusqlite::params![
                            &previous.project_id,
                            &previous.model_id,
                            &previous.title,
                            previous.created_at,
                            previous.updated_at,
                            previous.pinned_at,
                            previous.archived_at,
                            previous.unread_at,
                            &previous.id,
                        ],
                    )
                    .map_err(storage_error)?;
                if restored != 1 {
                    return Err(
                        "previous Conversation metadata disappeared during rollback".to_string()
                    );
                }
            }
        } else if exact_new_projection {
            agent_graph_repository::ensure_conversation_unbound(&transaction, conversation_id)
                .map_err(|error| error.to_string())?;
            chat_repository::delete_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove rolled-back Turn attachments: {error}");
        }
        Ok(())
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let tree_scope = resolve_agent_tree_deletion_scope(&connection, conversation_id)?;
        let attachments = if let Some(scope) = &tree_scope {
            attachment_repository::list_conversations_deletion_attachments(
                &connection,
                &scope.conversation_ids,
            )
            .map_err(storage_error)?
        } else {
            attachment_repository::list_conversation_attachments(&connection, conversation_id)
                .map_err(storage_error)?
        };
        if let Some(scope) = tree_scope {
            with_agent_deletion_transaction(&mut connection, |transaction| {
                automation_repository::terminalize_automation_runs_before_conversation_delete(
                    transaction,
                    &scope.conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                ensure_deletion_scope_has_no_active_execution(
                    transaction,
                    &scope.conversation_ids,
                )?;
                automation_repository::invalidate_automations_before_trigger_disabled_conversation_delete(
                    transaction,
                    &scope.conversation_ids,
                    now_ms(),
                )
                .map_err(storage_error)?;
                cleanup_conversation_owned_records(transaction, &scope.conversation_ids)?;
                delete_agent_tree_records(transaction, &scope)
            })?;
        } else {
            let transaction = connection.transaction().map_err(storage_error)?;
            notification_repository::resolve_notification_events_by_conversation_id_in_transaction(
                &transaction,
                conversation_id,
                now_ms(),
            )
            .map_err(storage_error)?;
            automation_repository::terminalize_automation_runs_before_conversation_delete(
                &transaction,
                &[conversation_id.to_string()],
                now_ms(),
            )
            .map_err(storage_error)?;
            usage_repository::roll_up_deleted_usage_for_conversation(
                &transaction,
                conversation_id,
                now_ms(),
            )
            .map_err(storage_error)?;
            pending_action_repository::delete_pending_actions_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
            agent_action_audit_repository::delete_action_audit_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
            provider_continuation_repository::delete_for_conversation(
                &transaction,
                conversation_id,
            )
            .map_err(storage_error)?;
            chat_repository::delete_conversation(&transaction, conversation_id)
                .map_err(storage_error)?;
            composer_draft_repository::delete_composer_draft(&transaction, conversation_id)
                .map_err(storage_error)?;
            transaction.commit().map_err(storage_error)?;
        }
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted conversation attachment files: {error}");
        }
        Ok(())
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        if message_ids.is_empty() {
            return Ok(());
        }

        let mut connection = self.state.connection()?;
        let has_protected_receipts = message_deletion_has_protected_model_batch_receipts(
            &connection,
            conversation_id,
            message_ids,
        )?;
        let attachments = if has_protected_receipts {
            with_agent_deletion_transaction(&mut connection, |transaction| {
                delete_chat_message_records_in_transaction(
                    transaction,
                    conversation_id,
                    message_ids,
                    true,
                )
            })?
        } else {
            let transaction = connection.transaction().map_err(storage_error)?;
            let attachments = delete_chat_message_records_in_transaction(
                &transaction,
                conversation_id,
                message_ids,
                false,
            )?;
            transaction.commit().map_err(storage_error)?;
            attachments
        };
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted message attachment files: {error}");
        }
        Ok(())
    }
}

fn attach_message_guidance_timelines(
    connection: &rusqlite::Connection,
    conversations: &mut [ChatConversationRecord],
) -> Result<(), String> {
    for conversation in conversations {
        let traces = conversation_trace_repository::list_traces_for_conversation(
            connection,
            &conversation.id,
        )
        .map_err(storage_error)?;
        let traces = traces
            .into_iter()
            .map(|trace| (trace.assistant_message_id.clone(), trace))
            .collect::<HashMap<_, _>>();
        let command_sessions = agent_command_session_repository::list_sessions_for_conversation(
            connection,
            &conversation.id,
            agent_command_session_repository::MAX_RETAINED_TERMINAL_COMMAND_SESSIONS_PER_CONVERSATION,
        )
        .map_err(storage_error)?;

        for message in &mut conversation.messages {
            if message.role != "assistant" {
                continue;
            }
            let guidances =
                guidance_repository::list_guidances_for_assistant_message(connection, &message.id)
                    .map_err(storage_error)?;
            let trace = traces.get(&message.id);
            if guidances.is_empty() && trace.is_none() {
                continue;
            }
            message.agent_run_json = Some(project_guidance_timeline(
                connection,
                message.agent_run_json.as_deref(),
                trace,
                &guidances,
                &command_sessions,
                message.created_at,
            )?);
        }
    }
    Ok(())
}

fn project_guidance_timeline(
    connection: &rusqlite::Connection,
    existing_run_json: Option<&str>,
    trace: Option<&ConversationTurnTrace>,
    guidances: &[AgentRunGuidanceRecord],
    command_sessions: &[agent_command_session_repository::AgentCommandSessionRecord],
    fallback_started_at: i64,
) -> Result<String, String> {
    let existing_run = if let Some(raw) = existing_run_json {
        let value = serde_json::from_str::<serde_json::Value>(raw)
            .map_err(|error| format!("current AgentRun projection is invalid JSON: {error}"))?;
        let run = value
            .as_object()
            .cloned()
            .ok_or_else(|| "current AgentRun projection must be an object".to_string())?;
        // Some pre-runtime Renderer fixtures persisted only lifecycle fields. They are not a
        // valid current projection and must never be merged field-by-field, but an authoritative
        // trace can safely rebuild the complete presentation skeleton from scratch.
        validate_current_agent_run_projection(&run)
            .ok()
            .map(|()| run)
    } else {
        None
    };
    let mut run = if let Some(run) = existing_run {
        run
    } else {
        let (run_id, status, completed_at) = trace
            .map(|trace| {
                let status = match trace.terminal_status {
                    crate::ConversationTurnTraceTerminalStatus::InProgress => "running",
                    crate::ConversationTurnTraceTerminalStatus::Completed => "completed",
                    crate::ConversationTurnTraceTerminalStatus::Failed => "failed",
                    crate::ConversationTurnTraceTerminalStatus::Cancelled => "cancelled",
                };
                (
                    trace.run_id.as_str(),
                    status,
                    (trace.terminal_status
                        != crate::ConversationTurnTraceTerminalStatus::InProgress)
                        .then_some(fallback_started_at),
                )
            })
            .or_else(|| {
                guidances
                    .first()
                    .map(|guidance| (guidance.run_id.as_str(), "running", None))
            })
            .ok_or_else(|| "guidance projection has no current run identity".to_string())?;
        let canonical = chat_repository::canonical_agent_run_lifecycle_projection(
            None,
            run_id,
            status,
            fallback_started_at,
            fallback_started_at,
            completed_at,
        )
        .map_err(storage_error)?;
        serde_json::from_str::<serde_json::Value>(&canonical)
            .map_err(|error| format!("decode canonical AgentRun projection: {error}"))?
            .as_object()
            .cloned()
            .expect("canonical AgentRun projection is an object")
    };
    project_durable_mcp_invocations(connection, &mut run, trace)?;
    let mcp_trace_anchors = mcp_trace_anchors(&run)?;
    let existing_timeline = run
        .remove("timeline")
        .and_then(|value| value.as_array().cloned())
        .ok_or_else(|| "current AgentRun timeline must be an array".to_string())?;
    let terminal_trace_error = trace.and_then(|trace| {
        trace
            .terminal_status
            .is_terminal()
            .then_some(trace.terminal_error.as_deref())
            .flatten()
    });
    let trace_is_authoritative = trace.is_some();
    let guidance_is_authoritative = trace_is_authoritative || !guidances.is_empty();
    let mut presentation_suffix = existing_timeline
        .into_iter()
        .filter(|item| {
            !timeline_item_is_rebuilt_from_durable_state(
                item,
                trace_is_authoritative,
                guidance_is_authoritative,
                terminal_trace_error.is_some(),
            )
        })
        .collect::<Vec<_>>();
    // A durable Trace is the ordered authority for the work performed during a Turn. Renderer-only
    // items have no position in that sequence, so they form a presentation suffix (most notably
    // the final answer) instead of being prepended ahead of the reconstructed work. With no Trace,
    // preserve the existing presentation order and append journal-only Guidance as before.
    let mut timeline = if trace_is_authoritative {
        Vec::new()
    } else {
        std::mem::take(&mut presentation_suffix)
    };
    let mut emitted_mcp_invocations = HashSet::new();
    let mut tool_calls = run
        .remove("toolCalls")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut tool_results = run
        .remove("toolResults")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut projected_command_sessions = run
        .remove("commandSessions")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let mut activated_skills = run
        .remove("activatedSkills")
        .and_then(|value| value.as_array().cloned())
        .unwrap_or_default();
    let mut skill_activation_revision = run.remove("skillActivationRevision");
    let mut projected_tool_call_ids = tool_calls
        .iter()
        .filter_map(|call| call.get("id").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();
    let mut projected_tool_result_ids = tool_results
        .iter()
        .filter_map(|result| result.get("callId").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .collect::<HashSet<_>>();

    if let Some(trace) = trace {
        let mut emitted_terminal_error = false;
        run.insert("runId".to_string(), trace.run_id.clone().into());
        match trace.terminal_status {
            crate::ConversationTurnTraceTerminalStatus::InProgress => {}
            crate::ConversationTurnTraceTerminalStatus::Completed => {
                run.insert("status".to_string(), "completed".into());
                run.entry("completedAt".to_string())
                    .or_insert_with(|| fallback_started_at.into());
            }
            crate::ConversationTurnTraceTerminalStatus::Failed => {
                run.insert("status".to_string(), "failed".into());
                run.entry("completedAt".to_string())
                    .or_insert_with(|| fallback_started_at.into());
            }
            crate::ConversationTurnTraceTerminalStatus::Cancelled => {
                run.insert("status".to_string(), "cancelled".into());
                run.entry("completedAt".to_string())
                    .or_insert_with(|| fallback_started_at.into());
            }
        }
        for item in &trace.items {
            match item {
                ConversationTurnTraceItem::AssistantNarration {
                    sequence, content, ..
                } => timeline.push(serde_json::json!({
                    "id": format!("trace-message-{sequence}"),
                    "type": "message",
                    "content": content,
                    "traceSequence": sequence,
                })),
                ConversationTurnTraceItem::UserGuidance {
                    sequence,
                    guidance_id,
                    client_message_id,
                    content,
                    attachments,
                    created_at,
                    ..
                } => timeline.push(serde_json::json!({
                    "id": format!("user-guidance-{client_message_id}"),
                    "type": "user_guidance",
                    "guidanceId": guidance_id,
                    "clientMessageId": client_message_id,
                    "content": content,
                    "attachments": attachments,
                    "status": "applied",
                    "createdAt": created_at,
                    "sequence": sequence,
                    "traceSequence": sequence,
                })),
                ConversationTurnTraceItem::ToolCall {
                    sequence,
                    call_id,
                    tool,
                    provenance,
                    operation,
                    approval_status,
                    ..
                } => {
                    if let Some(invocation_id) = mcp_trace_anchors.get(call_id) {
                        if emitted_mcp_invocations.insert(invocation_id.clone()) {
                            timeline.push(serde_json::json!({
                                "id": format!("mcp-invocation-{invocation_id}"),
                                "type": "mcp_tool_call",
                                "invocationId": invocation_id,
                                "traceSequence": sequence,
                            }));
                        }
                    } else {
                        if projected_tool_call_ids.insert(call_id.clone()) {
                            tool_calls.push(serde_json::json!({
                                "id": call_id,
                                "tool": tool,
                                "args": operation,
                                "approvalStatus": approval_status,
                                "reason": serde_json::Value::Null,
                            }));
                        }
                        timeline.push(serde_json::json!({
                            "id": format!("tool-call-{call_id}"),
                            "type": "tool_call",
                            "callId": call_id,
                            "identity": provenance,
                            "traceSequence": sequence,
                        }));
                    }
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    success,
                    observation,
                    error,
                    ..
                } => {
                    if tool == "skills_activate" && *success {
                        if let Some((skill, activation_revision)) =
                            project_activated_skill_from_trace_result(observation)
                        {
                            let skill_id = skill
                                .get("id")
                                .and_then(serde_json::Value::as_str)
                                .expect("projected activated Skill has an id");
                            if let Some(existing) = activated_skills.iter_mut().find(|existing| {
                                existing.get("id").and_then(serde_json::Value::as_str)
                                    == Some(skill_id)
                            }) {
                                *existing = skill;
                            } else {
                                activated_skills.push(skill);
                            }
                            skill_activation_revision = Some(activation_revision.into());
                        }
                    }
                    if !mcp_trace_anchors.contains_key(call_id)
                        && projected_tool_result_ids.insert(call_id.clone())
                    {
                        let mut projected = serde_json::json!({
                            "callId": call_id,
                            "tool": tool,
                            "ok": success,
                            "result": observation,
                        });
                        if let Some(error) = error {
                            projected
                                .as_object_mut()
                                .expect("projected ToolResult is an object")
                                .insert("error".to_string(), error.clone().into());
                        }
                        tool_results.push(projected);
                    }
                }
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    sequence,
                    phase,
                    operation_id,
                    outcome,
                } => match phase {
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Started => {
                        timeline.push(serde_json::json!({
                            "id": format!("context-compaction-{operation_id}"),
                            "type": "context_compaction",
                            "operationId": operation_id,
                            "status": "running",
                            "traceSequence": sequence,
                        }));
                    }
                    crate::conversation_trace::ConversationContextCompactionLifecyclePhase::Finished => {
                        let status = match outcome.expect("validated compaction finish has outcome") {
                            crate::protocol::AgentContextCompactionEventOutcome::Applied => "applied",
                            crate::protocol::AgentContextCompactionEventOutcome::Skipped => "skipped",
                            crate::protocol::AgentContextCompactionEventOutcome::Failed => "failed",
                            crate::protocol::AgentContextCompactionEventOutcome::Cancelled => "cancelled",
                        };
                        if let Some(existing) = timeline.iter_mut().find(|item| {
                            item.get("type").and_then(serde_json::Value::as_str)
                                == Some("context_compaction")
                                && item.get("operationId").and_then(serde_json::Value::as_str)
                                    == Some(operation_id.as_str())
                        }) {
                            existing["status"] = status.into();
                        }
                    }
                },
                ConversationTurnTraceItem::RuntimeError {
                    sequence, message, ..
                } => {
                    emitted_terminal_error |= trace.terminal_error.as_deref() == Some(message);
                    timeline.push(serde_json::json!({
                        "id": format!("trace-error-{sequence}"),
                        "type": "error",
                        "message": message,
                        "traceSequence": sequence,
                    }));
                }
                ConversationTurnTraceItem::AgentMailboxDelivery { .. }
                | ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {}
            }
        }
        if let Some(terminal_error) = trace
            .terminal_status
            .is_terminal()
            .then_some(trace.terminal_error.as_deref())
            .flatten()
            .filter(|_| !emitted_terminal_error)
        {
            timeline.push(serde_json::json!({
                "id": "terminal-error",
                "type": "error",
                "message": terminal_error,
            }));
        }
    }

    for record in command_sessions.iter().filter(|record| {
        trace.is_some_and(|trace| {
            record.snapshot.assistant_message_id == trace.assistant_message_id
                && record.snapshot.origin_run_id == trace.run_id
                && record.snapshot.status.is_terminal()
        })
    }) {
        let snapshot = &record.snapshot;
        let mut projection = serde_json::json!({
            "callId": snapshot.call_id,
            "status": snapshot.status,
            "startedAt": snapshot.started_at,
            "latestSequence": snapshot.latest_sequence,
            "outputTruncated": snapshot.output_truncated,
            "outputs": snapshot.outputs,
        });
        let object = projection
            .as_object_mut()
            .expect("command Session projection is an object");
        if let Some(ended_at) = snapshot.ended_at {
            object.insert("endedAt".to_string(), ended_at.into());
        }
        if let Some(exit_code) = snapshot.exit_code {
            object.insert("exitCode".to_string(), exit_code.into());
        }
        projected_command_sessions.insert(snapshot.call_id.clone(), projection);
    }

    for guidance in guidances {
        if !matches!(
            guidance.status,
            crate::AgentGuidanceStatus::Queued | crate::AgentGuidanceStatus::Abandoned
        ) {
            continue;
        }
        let mut attachments = Vec::with_capacity(guidance.attachment_ids.len());
        for attachment_id in &guidance.attachment_ids {
            let attachment = attachment_repository::get_attachment(connection, attachment_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    format!(
                        "guidance `{}` references missing attachment `{attachment_id}`",
                        guidance.guidance_id
                    )
                })?;
            attachments.push(serde_json::json!({
                "id": attachment.id,
                "kind": attachment.kind,
                "name": attachment.original_name,
                "mimeType": attachment.mime_type,
                "sizeBytes": attachment.size_bytes,
            }));
        }
        run.insert("runId".to_string(), guidance.run_id.clone().into());
        if guidance.status == crate::AgentGuidanceStatus::Abandoned {
            timeline.push(serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "rejected",
                "rejectionCode": "run_interrupted",
                "error": guidance.terminal_reason,
                "recoverable": true,
                "createdAt": guidance.created_at,
            }));
        } else {
            timeline.push(serde_json::json!({
                "id": format!("user-guidance-{}", guidance.client_message_id),
                "type": "user_guidance",
                "guidanceId": guidance.guidance_id,
                "clientMessageId": guidance.client_message_id,
                "content": guidance.content,
                "attachments": attachments,
                "status": "queued",
                "createdAt": guidance.created_at,
            }));
        }
    }

    if trace_is_authoritative {
        timeline.extend(presentation_suffix);
    }

    run.insert("timeline".to_string(), timeline.into());
    run.insert("toolCalls".to_string(), tool_calls.into());
    run.insert("toolResults".to_string(), tool_results.into());
    if !projected_command_sessions.is_empty() {
        run.insert(
            "commandSessions".to_string(),
            projected_command_sessions.into(),
        );
    }
    if !activated_skills.is_empty() {
        run.insert("activatedSkills".to_string(), activated_skills.into());
    }
    if let Some(skill_activation_revision) = skill_activation_revision {
        run.insert(
            "skillActivationRevision".to_string(),
            skill_activation_revision,
        );
    }
    run.entry("startedAt".to_string())
        .or_insert_with(|| fallback_started_at.into());
    for field in [
        "toolDefinitions",
        "approvals",
        "diffs",
        "fileDrafts",
        "webSearchActivities",
        "readActivities",
    ] {
        run.entry(field.to_string())
            .or_insert_with(|| serde_json::json!([]));
    }
    run.entry("messageStreamCheckpoints".to_string())
        .or_insert_with(|| serde_json::json!({}));
    if !run.contains_key("status") {
        let status = trace
            .map(|trace| match trace.terminal_status {
                crate::ConversationTurnTraceTerminalStatus::InProgress => "running",
                crate::ConversationTurnTraceTerminalStatus::Completed => "completed",
                crate::ConversationTurnTraceTerminalStatus::Failed => "failed",
                crate::ConversationTurnTraceTerminalStatus::Cancelled => "cancelled",
            })
            .unwrap_or("running");
        run.insert("status".to_string(), status.into());
    }

    serde_json::to_string(&serde_json::Value::Object(run))
        .map_err(|error| format!("serialize guidance timeline: {error}"))
}

fn timeline_item_is_rebuilt_from_durable_state(
    item: &serde_json::Value,
    trace_is_authoritative: bool,
    guidance_is_authoritative: bool,
    has_terminal_trace_error: bool,
) -> bool {
    // Timeline ids are Renderer presentation identities, not persistence identities. A committed
    // stream keeps ids such as `message-stream-*`, so id-prefix checks duplicate it on reload.
    // `traceSequence` is the durable ordering anchor and must be projected exactly once.
    if trace_is_authoritative
        && item
            .get("traceSequence")
            .and_then(serde_json::Value::as_u64)
            .is_some()
    {
        return true;
    }

    match item.get("type").and_then(serde_json::Value::as_str) {
        // These are views over durable Trace/MCP state. Older live projections may predate a
        // traceSequence, so their type is also authoritative once the Trace exists.
        Some("tool_call" | "mcp_tool_call" | "context_compaction") => trace_is_authoritative,
        // Guidance can be queued in its journal before it receives a Trace sequence. Rebuild all
        // Guidance from that journal/Trace pair so an in-run user insertion cannot appear twice.
        Some("user_guidance") => guidance_is_authoritative,
        // A terminal Trace error has one canonical projection. Host-only errors remain untouched
        // when the Trace has no terminal error of its own.
        Some("error") => trace_is_authoritative && has_terminal_trace_error,
        _ => false,
    }
}

fn project_activated_skill_from_trace_result(
    observation: &serde_json::Value,
) -> Option<(serde_json::Value, &str)> {
    let status = observation.get("status")?.as_str()?;
    if !matches!(status, "activated" | "alreadyActivated") {
        return None;
    }
    let revision = observation.get("activationRevision")?.as_str()?;
    let skill = observation.get("skill")?;
    let id = skill.get("id")?.as_str()?;
    let name = skill.get("name")?.as_str()?;
    let skill_revision = skill.get("revision")?.as_str()?;
    let source = skill.get("source")?.as_str()?;
    let (source_kind, source_id) = source.split_once(':')?;
    if !matches!(source_kind, "workspace" | "bundled" | "installed") || source_id.trim().is_empty()
    {
        return None;
    }
    Some((
        serde_json::json!({
            "id": id,
            "name": name,
            "revision": skill_revision,
            "source": { "kind": source_kind, "id": source_id },
        }),
        revision,
    ))
}

fn validate_current_agent_run_projection(
    run: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), String> {
    if !run.get("runId").is_some_and(serde_json::Value::is_string)
        || !run.get("status").is_some_and(serde_json::Value::is_string)
        || !run.get("startedAt").is_some_and(serde_json::Value::is_i64)
    {
        return Err("current AgentRun lifecycle identity is incomplete".to_string());
    }
    for field in [
        "toolDefinitions",
        "toolCalls",
        "toolResults",
        "approvals",
        "diffs",
        "fileDrafts",
        "webSearchActivities",
        "readActivities",
        "mcpInvocations",
        "timeline",
    ] {
        if !run.get(field).is_some_and(serde_json::Value::is_array) {
            return Err(format!("current AgentRun field `{field}` must be an array"));
        }
    }
    if !run
        .get("messageStreamCheckpoints")
        .is_some_and(serde_json::Value::is_object)
    {
        return Err("current AgentRun messageStreamCheckpoints must be an object".to_string());
    }
    Ok(())
}

fn project_durable_mcp_invocations(
    connection: &rusqlite::Connection,
    run: &mut serde_json::Map<String, serde_json::Value>,
    trace: Option<&ConversationTurnTrace>,
) -> Result<(), String> {
    let Some(trace) = trace else {
        return Ok(());
    };
    let existing_call_ids = mcp_trace_anchors(run)?;
    let rows = pending_action_repository::list_mcp_actions_for_assistant_run(
        connection,
        &trace.conversation_id,
        &trace.assistant_message_id,
        &trace.run_id,
    )
    .map_err(storage_error)?;
    let mut projected = run
        .get("mcpInvocations")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .ok_or_else(|| "current AgentRun MCP invocations must be an array".to_string())?;

    for item in &trace.items {
        let ConversationTurnTraceItem::ToolCall {
            call_id,
            tool,
            provenance: crate::AgentToolIdentity::Mcp { provenance },
            ..
        } = item
        else {
            continue;
        };
        if existing_call_ids.contains_key(call_id) {
            continue;
        }
        let matching = rows
            .iter()
            .filter(|row| row.tool_call_id.as_deref() == Some(call_id.as_str()))
            .collect::<Vec<_>>();
        let [row] = matching.as_slice() else {
            continue;
        };
        // Terminal automatic journals deliberately scrub this public action. If no prior safe
        // projection exists, trace provenance alone cannot recover the one-time invocation UUID
        // or historical display name, so leave the item generic instead of inventing facts.
        let Ok(crate::AgentProposedAction::McpToolCall { approval }) =
            serde_json::from_str::<crate::AgentProposedAction>(&row.action_json)
        else {
            continue;
        };
        let identity = &approval.identity;
        let expected_storage_id = format!(
            "v2:{}:{}:{}",
            identity.run_id.len(),
            identity.run_id,
            identity.action_id
        );
        if row.action_id != expected_storage_id
            || row.run_id != trace.run_id
            || identity.call_id != *call_id
            || identity.provenance != *provenance
            || approval.call.id != *call_id
            || approval.call.tool != *tool
            || approval.summary.server_id != provenance.server_id
            || approval.summary.scope != provenance.scope
            || approval.summary.raw_tool_name != provenance.raw_tool_name
            || approval.summary.model_tool_name != provenance.model_tool_name
            || !approval.summary.external
        {
            continue;
        }
        let terminal_result = trace.items.iter().find_map(|candidate| {
            let ConversationTurnTraceItem::ToolResult {
                call_id: result_call_id,
                observation,
                success,
                ..
            } = candidate
            else {
                return None;
            };
            (result_call_id == call_id).then_some((observation, *success))
        });
        let duration_ms = u64::try_from(row.updated_at.saturating_sub(row.created_at)).unwrap_or(0);
        let lifecycle = if let Some((observation, success)) = terminal_result {
            project_terminal_mcp_trace_lifecycle(observation, success, duration_ms)
        } else {
            match row.status.as_str() {
                "pending" => Some((
                    "pending_approval",
                    "definitely_not_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                "approved" => Some((
                    "approved",
                    "definitely_not_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                "executing" => Some((
                    "dispatching",
                    "possibly_dispatched",
                    None,
                    None,
                    None,
                    None,
                    false,
                )),
                _ => None,
            }
        };
        let Some((state, dispatch, outcome, is_error, error_code, duration, truncated)) = lifecycle
        else {
            continue;
        };
        let mut invocation = serde_json::json!({
            "actionId": identity.action_id,
            "invocationId": identity.invocation_id,
            "callId": identity.call_id,
            "serverId": provenance.server_id,
            "serverDisplayName": approval.summary.server_display_name,
            "scope": provenance.scope,
            "rawToolName": provenance.raw_tool_name,
            "modelToolName": provenance.model_tool_name,
            "external": true,
            "state": state,
            "dispatchCertainty": dispatch,
            "outputTruncated": truncated,
        });
        let object = invocation
            .as_object_mut()
            .expect("MCP invocation projection is an object");
        if let Some(display_reason) = &approval.summary.display_reason {
            object.insert("displayReason".to_string(), display_reason.clone().into());
        }
        if let Some(outcome) = outcome {
            object.insert("outcome".to_string(), outcome.into());
        }
        if let Some(is_error) = is_error {
            object.insert("isError".to_string(), is_error.into());
        }
        if let Some(error_code) = error_code {
            object.insert("errorCode".to_string(), error_code.into());
        }
        if let Some(duration) = duration {
            object.insert("durationMs".to_string(), duration.into());
        }
        projected.push(invocation);
    }
    run.insert("mcpInvocations".to_string(), projected.into());
    Ok(())
}

type ProjectedMcpLifecycle<'a> = (
    &'a str,
    &'a str,
    Option<&'a str>,
    Option<bool>,
    Option<&'a str>,
    Option<u64>,
    bool,
);

fn project_terminal_mcp_trace_lifecycle(
    observation: &serde_json::Value,
    success: bool,
    duration_ms: u64,
) -> Option<ProjectedMcpLifecycle<'_>> {
    let value = observation.as_object()?;
    if value.get("type").and_then(serde_json::Value::as_str) != Some("mcp_tool")
        || value.get("external").and_then(serde_json::Value::as_bool) != Some(true)
    {
        return None;
    }
    let status = value.get("status")?.as_str()?;
    let outcome = value.get("outcome")?.as_str()?;
    let dispatch = value.get("dispatchCertainty")?.as_str()?;
    let truncated = value
        .get("truncatedAtSource")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let code = value
        .get("code")
        .and_then(serde_json::Value::as_str)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        });
    match (status, outcome, dispatch, success) {
        ("completed", "succeeded", "response_received", true) => Some((
            "completed",
            dispatch,
            Some(outcome),
            Some(false),
            None,
            Some(duration_ms),
            truncated,
        )),
        ("completed", "tool_error", "response_received", false) => Some((
            "completed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_error")),
            Some(duration_ms),
            truncated,
        )),
        ("rejected", "rejected", "definitely_not_dispatched", false) => Some((
            "rejected",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_rejected")),
            None,
            false,
        )),
        ("cancelled", "cancelled", "definitely_not_dispatched", false) => Some((
            "cancelled",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.tool_cancelled")),
            None,
            false,
        )),
        ("expired", "expired", "definitely_not_dispatched", false) => Some((
            "expired",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_payload_expired")),
            None,
            false,
        )),
        ("payload_unavailable", "payload_unavailable", "definitely_not_dispatched", false) => {
            Some((
                "payload_unavailable",
                dispatch,
                Some(outcome),
                Some(true),
                Some(code.unwrap_or("mcp.approval_payload_unavailable")),
                None,
                false,
            ))
        }
        ("policy_denied", "policy_denied", "definitely_not_dispatched", false) => Some((
            "policy_denied",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.approval_policy_denied")),
            None,
            false,
        )),
        ("outcome_unknown", "outcome_unknown", "possibly_dispatched", false) => Some((
            "outcome_unknown",
            dispatch,
            Some(outcome),
            None,
            Some(code.unwrap_or("mcp.tool_outcome_unknown")),
            None,
            false,
        )),
        ("failed", "output_too_large", "response_received", false) => Some((
            "failed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_output_too_large")),
            Some(duration_ms),
            true,
        )),
        ("failed", "timed_out", "definitely_not_dispatched", false) => Some((
            "failed",
            dispatch,
            Some(outcome),
            Some(true),
            Some(code.unwrap_or("mcp.tool_timeout")),
            Some(duration_ms),
            false,
        )),
        ("failed", "transport_error", "definitely_not_dispatched" | "response_received", false) => {
            Some((
                "failed",
                dispatch,
                Some(outcome),
                Some(true),
                Some(code.unwrap_or("mcp.tool_failed")),
                Some(duration_ms),
                truncated && dispatch == "response_received",
            ))
        }
        _ => None,
    }
}

fn mcp_trace_anchors(
    run: &serde_json::Map<String, serde_json::Value>,
) -> Result<HashMap<String, String>, String> {
    let invocations = run
        .get("mcpInvocations")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "current AgentRun MCP invocations must be an array".to_string())?;

    let mut invocation_ids_by_call_id = HashMap::<String, String>::new();
    let mut call_ids_by_invocation_id = HashMap::<String, String>::new();
    for invocation in invocations {
        let call_id = invocation
            .get("callId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "current MCP invocation is missing callId".to_string())?;
        let invocation_id = invocation
            .get("invocationId")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "current MCP invocation is missing invocationId".to_string())?;
        if invocation_ids_by_call_id
            .insert(call_id.to_string(), invocation_id.to_string())
            .is_some()
        {
            return Err("current MCP invocation callId is duplicated".to_string());
        }
        if call_ids_by_invocation_id
            .insert(invocation_id.to_string(), call_id.to_string())
            .is_some()
        {
            return Err("current MCP invocation identity is duplicated".to_string());
        }
    }
    Ok(invocation_ids_by_call_id)
}
