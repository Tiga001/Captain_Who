use super::*;
use crate::storage::human_interaction_repository;

/// Outcome of publishing a Runtime's `WaitingForApproval` projection.
///
/// The two non-persisted outcomes are expected lifecycle races. Identity corruption, a missing
/// action generation, and unrecognized durable lifecycle values remain hard errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentWaitingForApprovalPersistenceOutcome {
    Persisted,
    TurnTerminal,
    PendingActionAdvanced,
}

/// Outcome of accounting for the Runtime segment which opened an approval boundary.
///
/// A terminal Turn is an expected race with cancellation or shutdown. Missing or mismatched
/// owner identity, invalid lifecycle values, and malformed Waiting usage remain hard errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentWaitingSegmentUsagePersistenceOutcome {
    Persisted,
    TurnTerminal,
}

impl StorageService {
    /// Persists a Runtime approval-boundary Usage snapshot only while its exact Turn is active.
    ///
    /// The immediate transaction serializes this write with terminal Turn persistence. If the
    /// terminal transaction wins, a late Waiting snapshot cannot regress the durable Usage row
    /// from `completed`, `failed`, or `cancelled` back to `waiting_for_approval`.
    pub fn persist_waiting_segment_usage_if_run_in_progress(
        &self,
        usage: &AgentUsageRecordInsert,
    ) -> Result<AgentWaitingSegmentUsagePersistenceOutcome, String> {
        if usage.conversation_id.trim().is_empty()
            || usage.message_id.trim().is_empty()
            || usage.run_id.trim().is_empty()
        {
            return Err(
                "waiting-segment usage persistence requires non-empty Turn identity".to_string(),
            );
        }
        if usage.status.as_deref() != Some("waiting_for_approval") || usage.completed_at.is_some() {
            return Err(
                "waiting-segment usage persistence requires a nonterminal waiting_for_approval record"
                    .to_string(),
            );
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let trace_identity = transaction
            .query_row(
                "
                SELECT conversation_id, run_id, terminal_status
                FROM conversation_turn_traces
                WHERE assistant_message_id = ?1
                ",
                [&usage.message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        let Some((trace_conversation_id, trace_run_id, terminal_status)) = trace_identity else {
            return Err(format!(
                "waiting-segment Turn trace `{}` does not exist",
                usage.message_id
            ));
        };
        if trace_conversation_id != usage.conversation_id || trace_run_id != usage.run_id {
            return Err(format!(
                "waiting-segment Turn trace `{}` is owned by another conversation or run",
                usage.message_id
            ));
        }
        match terminal_status.as_str() {
            "in_progress" => {}
            "completed" | "failed" | "cancelled" => {
                transaction.commit().map_err(storage_error)?;
                return Ok(AgentWaitingSegmentUsagePersistenceOutcome::TurnTerminal);
            }
            _ => {
                return Err(format!(
                    "waiting-segment Turn trace `{}` has an invalid terminal status",
                    usage.message_id
                ));
            }
        }

        usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(AgentWaitingSegmentUsagePersistenceOutcome::Persisted)
    }

    pub fn upsert_chat_messages(
        &self,
        conversation_id: &str,
        messages: Vec<ChatMessageRecord>,
        position_offset: i64,
    ) -> Result<Vec<ChatMessageRecord>, String> {
        let mut connection = self.state.connection()?;
        ensure_conversation_exists(&connection, conversation_id)?;
        chat_repository::upsert_messages(
            &mut connection,
            conversation_id,
            &messages,
            position_offset,
        )
        .map_err(storage_error)
    }

    pub fn get_assistant_message_created_at(
        &self,
        conversation_id: &str,
        message_id: &str,
    ) -> Result<Option<i64>, String> {
        let connection = self.state.connection()?;
        chat_repository::get_assistant_message_created_at(&connection, conversation_id, message_id)
            .map_err(storage_error)
    }

    pub fn replace_conversation_turn_trace(
        &self,
        trace: &ConversationTurnTrace,
        created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        conversation_trace_repository::replace_trace(
            &mut connection,
            trace,
            created_at,
            completed_at,
        )
        .map_err(storage_error)
    }

    pub fn append_in_progress_conversation_turn_trace(
        &self,
        trace: &ConversationTurnTrace,
        created_at: i64,
        updated_at: i64,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        conversation_trace_repository::append_in_progress_trace(
            &mut connection,
            trace,
            created_at,
            updated_at,
        )
        .map_err(storage_error)
    }

    /// Atomically appends an in-progress trace and advances every included guidance journal row
    /// to `applied`. A crash can therefore never expose guidance in model history while leaving
    /// its durable admission record in `queued`.
    pub fn append_in_progress_conversation_turn_trace_and_apply_guidances(
        &self,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        created_at: i64,
        updated_at: i64,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection.transaction().map_err(storage_error)?;
        let changed = conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            created_at,
            updated_at,
        )
        .map_err(storage_error)?;
        let model_context_changed =
            conversation_model_context_repository::commit_items_in_connection(
                &transaction,
                &trace.conversation_id,
                &trace.assistant_message_id,
                model_context_items,
            )
            .map_err(storage_error)?;
        for item in &trace.items {
            let ConversationTurnTraceItem::UserGuidance {
                guidance_id,
                sequence,
                ..
            } = item
            else {
                continue;
            };
            match guidance_repository::mark_guidance_applied(
                &transaction,
                guidance_id,
                *sequence,
                updated_at,
            )
            .map_err(storage_error)?
            {
                AgentRunGuidanceTransitionOutcome::Updated
                | AgentRunGuidanceTransitionOutcome::Idempotent => {}
                AgentRunGuidanceTransitionOutcome::NotFound => {
                    return Err(format!(
                        "conversation trace references missing guidance journal `{guidance_id}`"
                    ));
                }
                AgentRunGuidanceTransitionOutcome::Conflict { current_status } => {
                    return Err(format!(
                        "conversation trace guidance `{guidance_id}` conflicts with journal status `{}`",
                        current_status.as_str()
                    ));
                }
            }
        }
        provider_continuation_repository::promote_staged_trace_projections_in_connection(
            &transaction,
            &trace.conversation_id,
            &trace.assistant_message_id,
            &trace.run_id,
            updated_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)?;
        Ok(changed || model_context_changed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_trace(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        self.finalize_chat_message_with_conversation_trace_and_usage(
            conversation_id,
            message_id,
            content,
            message_status,
            run_status,
            trace,
            trace_created_at,
            completed_at,
            None,
        )
    }

    /// Atomically commits every durable fact that makes an agent run terminal.
    ///
    /// A terminal assistant message, its trace, and its usage row form one visibility boundary.
    /// Keeping the optional usage write in this transaction prevents a retryable pending action
    /// from being exposed after its assistant message has already become terminal.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_trace_and_usage(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
    ) -> Result<(), String> {
        self.finalize_chat_message_with_conversation_trace_model_context_and_usage(
            conversation_id,
            message_id,
            content,
            message_status,
            run_status,
            trace,
            None,
            trace_created_at,
            completed_at,
            usage,
            None,
        )
    }

    /// Terminal assistant visibility boundary with an optional newly produced exact model log.
    ///
    /// When no projection is supplied, the transaction validates the already committed observer
    /// log. Supplying a projection commits and validates it with the terminal message and trace.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_trace_model_context_and_usage(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        self.finalize_chat_message_with_conversation_trace_model_context_usage_and_notification(
            conversation_id,
            message_id,
            content,
            message_status,
            run_status,
            trace,
            model_context_items,
            trace_created_at,
            completed_at,
            usage,
            None,
            collaboration_cutoff,
        )
    }

    /// Atomically commits a terminal HumanRoot Turn and its structured notification fact.
    ///
    /// The notification contains only routing identities and a pre-sanitized subject. Keeping it
    /// inside the trace transaction closes both crash windows: no banner for a rolled-back Turn,
    /// and no terminal Turn whose notification enqueue was lost before process exit.
    #[allow(clippy::too_many_arguments)]
    pub fn finalize_chat_message_with_conversation_turn_notification(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
        notification: &notification_repository::NewNotificationEventRecord,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        self.finalize_chat_message_with_conversation_trace_model_context_usage_and_notification(
            conversation_id,
            message_id,
            content,
            message_status,
            run_status,
            trace,
            model_context_items,
            trace_created_at,
            completed_at,
            usage,
            Some(notification),
            collaboration_cutoff,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn finalize_chat_message_with_conversation_trace_model_context_usage_and_notification(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        message_status: Option<&str>,
        run_status: &str,
        trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
        trace_created_at: i64,
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
        notification: Option<&notification_repository::NewNotificationEventRecord>,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        // A terminal retry can follow an idle observation; a failed binding reply can also leave
        // the active Runtime unaware of an already committed ignored event. Preserve an exact
        // suffix only, and require the durable receipt for every unacknowledged active event.
        let mut effective_trace = trace.clone();
        let mut effective_model_items = model_context_items.map(<[_]>::to_vec);
        if let Some(stored) =
            conversation_trace_repository::get_trace_for_message(&transaction, message_id)
                .map_err(storage_error)?
        {
            if stored.items.len() > trace.items.len()
                && stored.items[..trace.items.len()] == trace.items
                && stored.items[trace.items.len()..]
                    .iter()
                    .all(|item| matches!(item, ConversationTurnTraceItem::BackendState { .. }))
            {
                let mut authorized = stored.terminal_status.is_terminal();
                if !authorized {
                    authorized = true;
                    for item in &stored.items[trace.items.len()..] {
                        if !human_interaction_repository::is_materialized_ignored_backend_state(
                            &transaction,
                            &stored,
                            item,
                        )
                        .map_err(|error| error.to_string())?
                        {
                            authorized = false;
                            break;
                        }
                    }
                }
                if authorized {
                    effective_trace.items = stored.items;
                    effective_trace.truncated |= stored.truncated;
                    if let Some(incoming) = effective_model_items.as_mut() {
                        let stored_items =
                            conversation_model_context_repository::get_log_for_message(
                                &transaction,
                                message_id,
                            )
                            .map_err(storage_error)?
                            .map(|log| log.items)
                            .unwrap_or_default();
                        if incoming.len() <= stored_items.len()
                            && *incoming == stored_items[..incoming.len()]
                        {
                            *incoming = stored_items;
                        }
                    }
                }
            }
        }
        let trace = &effective_trace;
        let model_context_items = effective_model_items.as_deref();
        chat_repository::update_message_status_and_content(
            &transaction,
            conversation_id,
            message_id,
            content,
            message_status,
            completed_at,
        )
        .map_err(storage_error)?;
        chat_repository::update_message_run_terminal_state(
            &transaction,
            conversation_id,
            message_id,
            &trace.run_id,
            message_status,
            run_status,
            completed_at,
            collaboration_cutoff,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            trace_created_at,
            completed_at,
        )
        .map_err(storage_error)?;
        if let Some(model_context_items) = model_context_items {
            conversation_model_context_repository::commit_items_in_connection(
                &transaction,
                conversation_id,
                message_id,
                model_context_items,
            )
            .map_err(storage_error)?;
        }
        let durable_model_context_items =
            conversation_model_context_repository::get_log_for_message(&transaction, message_id)
                .map_err(storage_error)?
                .map(|log| log.items)
                .unwrap_or_default();
        trace
            .validate_complete_model_context(&durable_model_context_items)
            .map_err(|error| format!("terminal Assistant model context is incomplete: {error}"))?;
        human_interaction_repository::flush_ignored_for_terminal(
            &transaction,
            message_id,
            completed_at,
        )
        .map_err(|error| error.to_string())?;
        provider_continuation_repository::promote_staged_trace_projections_in_connection(
            &transaction,
            conversation_id,
            message_id,
            &trace.run_id,
            completed_at,
        )
        .map_err(storage_error)?;
        provider_continuation_repository::settle_staged_conversation_message_in_connection(
            &transaction,
            conversation_id,
            message_id,
            &trace.run_id,
            trace.terminal_status,
            completed_at,
        )
        .map_err(storage_error)?;
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
        if let Some(notification) = notification {
            notification_repository::enqueue_notification_event_in_transaction(
                &transaction,
                notification,
            )
            .map_err(storage_error)?;
        }
        // A terminal Run must never remain usable as remembered FileChange authority. Keep the
        // revocation in the same visibility transaction as the terminal Assistant/Trace so a
        // crash cannot publish Completed/Failed/Cancelled while leaving a grant active.
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &transaction,
            &trace.run_id,
            completed_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    pub fn get_conversation_turn_trace(
        &self,
        assistant_message_id: &str,
    ) -> Result<Option<ConversationTurnTrace>, String> {
        let connection = self.state.connection()?;
        conversation_trace_repository::get_trace_for_message(&connection, assistant_message_id)
            .map_err(storage_error)
    }

    pub fn list_conversation_turn_traces(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationTurnTrace>, String> {
        let connection = self.state.connection()?;
        let mut traces = conversation_trace_repository::list_traces_for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        let superseded = conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        traces.retain(|trace| !superseded.contains(&trace.assistant_message_id));
        Ok(traces)
    }

    pub fn get_conversation_model_context_log(
        &self,
        assistant_message_id: &str,
    ) -> Result<Option<ConversationModelContextLog>, String> {
        let connection = self.state.connection()?;
        conversation_model_context_repository::get_log_for_message(
            &connection,
            assistant_message_id,
        )
        .map_err(storage_error)
    }

    pub fn list_conversation_model_context_logs(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ConversationModelContextLog>, String> {
        let connection = self.state.connection()?;
        let mut logs = conversation_model_context_repository::list_logs_for_conversation(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        let superseded = conversation_turn_rewrite_repository::superseded_message_ids(
            &connection,
            conversation_id,
        )
        .map_err(storage_error)?;
        logs.retain(|log| !superseded.contains(&log.assistant_message_id));
        Ok(logs)
    }

    /// Lists durable in-progress traces for process-startup side-effect reconciliation.
    pub fn list_in_progress_conversation_turn_traces(
        &self,
    ) -> Result<Vec<ConversationTurnTrace>, String> {
        let connection = self.state.connection()?;
        conversation_trace_repository::list_in_progress_traces(&connection).map_err(storage_error)
    }

    pub fn save_chat_message_state(
        &self,
        conversation_id: &str,
        message: ChatMessageStateRecord,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_state(&connection, conversation_id, &message)
            .map_err(storage_error)
    }

    pub fn save_chat_message_ui_state(
        &self,
        conversation_id: &str,
        message_id: &str,
        ui_state_json: Option<&str>,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_ui_state(
            &connection,
            conversation_id,
            message_id,
            ui_state_json,
        )
        .map_err(storage_error)
    }

    pub fn update_chat_message_status_and_content(
        &self,
        conversation_id: &str,
        message_id: &str,
        content: &str,
        status: Option<&str>,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_status_and_content(
            &connection,
            conversation_id,
            message_id,
            content,
            status,
            updated_at,
        )
        .map_err(storage_error)
    }

    /// Commits a late `WaitingForApproval` projection only while its exact Turn is active.
    ///
    /// The immediate transaction is the serialization boundary with terminal Turn persistence:
    /// once a matching trace becomes terminal, a retiring Runtime worker can no longer overwrite
    /// the assistant message with stale pending content or publish its stale usage snapshot.
    /// Missing or mismatched owner identities are errors rather than benign stale writes.
    #[allow(clippy::too_many_arguments)]
    pub fn persist_waiting_for_approval_if_run_in_progress(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        run_id: &str,
        pending_action_storage_id: &str,
        content: &str,
        message_status: Option<&str>,
        updated_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
    ) -> Result<AgentWaitingForApprovalPersistenceOutcome, String> {
        if conversation_id.trim().is_empty()
            || assistant_message_id.trim().is_empty()
            || run_id.trim().is_empty()
            || pending_action_storage_id.trim().is_empty()
        {
            return Err(
                "waiting-for-approval persistence requires non-empty Turn and pending-action identity"
                    .to_string(),
            );
        }
        if let Some(usage) = usage {
            if usage.conversation_id != conversation_id
                || usage.message_id != assistant_message_id
                || usage.run_id != run_id
            {
                return Err(
                    "waiting-for-approval usage owner does not match the requested Turn"
                        .to_string(),
                );
            }
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let trace_identity = transaction
            .query_row(
                "
                SELECT conversation_id, run_id, terminal_status
                FROM conversation_turn_traces
                WHERE assistant_message_id = ?1
                ",
                [assistant_message_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        let Some((trace_conversation_id, trace_run_id, terminal_status)) = trace_identity else {
            return Err(format!(
                "waiting-for-approval Turn trace `{assistant_message_id}` does not exist"
            ));
        };
        if trace_conversation_id != conversation_id || trace_run_id != run_id {
            return Err(format!(
                "waiting-for-approval Turn trace `{assistant_message_id}` is owned by another conversation or run"
            ));
        }
        match terminal_status.as_str() {
            "in_progress" => {}
            "completed" | "failed" | "cancelled" => {
                transaction.commit().map_err(storage_error)?;
                return Ok(AgentWaitingForApprovalPersistenceOutcome::TurnTerminal);
            }
            _ => {
                return Err(format!(
                    "waiting-for-approval Turn trace `{assistant_message_id}` has an invalid terminal status"
                ));
            }
        }

        let pending_identity = transaction
            .query_row(
                "
                SELECT run_id, conversation_id, assistant_message_id, status, target_status
                FROM agent_pending_actions
                WHERE action_id = ?1
                ",
                [pending_action_storage_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<String>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(storage_error)?;
        let Some((
            pending_run_id,
            pending_conversation_id,
            pending_assistant_message_id,
            pending_status,
            pending_target_status,
        )) = pending_identity
        else {
            return Err(format!(
                "waiting-for-approval pending action `{pending_action_storage_id}` does not exist"
            ));
        };
        if pending_run_id != run_id
            || pending_conversation_id.as_deref() != Some(conversation_id)
            || pending_assistant_message_id.as_deref() != Some(assistant_message_id)
        {
            return Err(format!(
                "waiting-for-approval pending action `{pending_action_storage_id}` is owned by another Turn"
            ));
        }
        if !matches!(
            pending_status.as_str(),
            "pending"
                | "approved"
                | "executing"
                | "rejected"
                | "cancelled"
                | "completed"
                | "failed"
        ) {
            return Err(format!(
                "waiting-for-approval pending action `{pending_action_storage_id}` has an invalid status"
            ));
        }
        if pending_target_status.as_deref().is_some_and(|status| {
            !matches!(status, "rejected" | "cancelled" | "completed" | "failed")
        }) {
            return Err(format!(
                "waiting-for-approval pending action `{pending_action_storage_id}` has an invalid target status"
            ));
        }
        if pending_status != "pending" || pending_target_status.is_some() {
            transaction.commit().map_err(storage_error)?;
            return Ok(AgentWaitingForApprovalPersistenceOutcome::PendingActionAdvanced);
        }

        let message_role = transaction
            .query_row(
                "
                SELECT role
                FROM messages
                WHERE id = ?1 AND conversation_id = ?2
                ",
                rusqlite::params![assistant_message_id, conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(storage_error)?;
        match message_role.as_deref() {
            Some("assistant") => {}
            Some(_) => {
                return Err(format!(
                    "waiting-for-approval message `{assistant_message_id}` is not an assistant message"
                ));
            }
            None => {
                return Err(format!(
                    "waiting-for-approval message `{assistant_message_id}` does not exist in its conversation"
                ));
            }
        }

        let updated_messages = transaction
            .execute(
                "
                UPDATE messages
                SET content = ?1, status = ?2
                WHERE conversation_id = ?3 AND id = ?4 AND role = 'assistant'
                ",
                rusqlite::params![
                    content,
                    message_status,
                    conversation_id,
                    assistant_message_id
                ],
            )
            .map_err(storage_error)?;
        if updated_messages != 1 {
            return Err(format!(
                "waiting-for-approval message `{assistant_message_id}` changed identity during persistence"
            ));
        }
        let updated_conversations = transaction
            .execute(
                "UPDATE conversations SET updated_at = ?1 WHERE id = ?2",
                rusqlite::params![updated_at, conversation_id],
            )
            .map_err(storage_error)?;
        if updated_conversations != 1 {
            return Err(format!(
                "waiting-for-approval conversation `{conversation_id}` does not exist"
            ));
        }
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)?;
        Ok(AgentWaitingForApprovalPersistenceOutcome::Persisted)
    }

    pub fn update_chat_message_run_terminal_state(
        &self,
        conversation_id: &str,
        message_id: &str,
        run_id: &str,
        message_status: Option<&str>,
        run_status: &str,
        completed_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_run_terminal_state(
            &connection,
            conversation_id,
            message_id,
            run_id,
            message_status,
            run_status,
            completed_at,
            None,
        )
        .map_err(storage_error)
    }
}
