use super::*;

impl StorageService {
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
        .map_err(storage_error)?;
        Ok(messages)
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
        let transaction = connection.transaction().map_err(storage_error)?;
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
