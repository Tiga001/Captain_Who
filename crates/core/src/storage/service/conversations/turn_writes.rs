impl StorageService {
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
            provider_continuation_repository::release_for_messages(
                &transaction,
                &rewrite.conversation_id,
                std::slice::from_ref(&rewrite.source_assistant_message_id),
                rewrite.created_at,
            )
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
}
