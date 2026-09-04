impl StorageService {
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
        drop(connection);
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
        drop(connection);
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
        drop(connection);
        if let Err(error) = self.cleanup_attachment_files(attachments) {
            eprintln!("failed to remove deleted message attachment files: {error}");
        }
        Ok(())
    }
}
