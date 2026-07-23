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
            message_status,
            run_status,
            completed_at,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            trace_created_at,
            completed_at,
        )
        .map_err(storage_error)?;
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
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
        conversation_trace_repository::list_traces_for_conversation(&connection, conversation_id)
            .map_err(storage_error)
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
        message_status: Option<&str>,
        run_status: &str,
        completed_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        chat_repository::update_message_run_terminal_state(
            &connection,
            conversation_id,
            message_id,
            message_status,
            run_status,
            completed_at,
        )
        .map_err(storage_error)
    }
}
