use super::*;

impl StorageService {
    pub fn append_conversation_world_state_record(
        &self,
        conversation_id: &str,
        epoch_generation: u64,
        base_summary_id: Option<&str>,
        effective_before_message_id: Option<&str>,
        record: &WorldStateRecord,
        created_at: i64,
    ) -> Result<world_state_repository::ConversationWorldStateAppendOutcome, String> {
        let mut connection = self.state.connection()?;
        world_state_repository::append_record(
            &mut connection,
            &world_state_repository::ConversationWorldStateRecordWrite {
                conversation_id,
                epoch_generation,
                base_summary_id,
                effective_before_message_id,
                record,
                created_at,
            },
        )
        .map_err(|error| error.to_string())
    }

    pub fn list_active_conversation_world_state_records(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<world_state_repository::ConversationWorldStateJournalEntry>, String> {
        let connection = self.state.connection()?;
        let mut entries =
            world_state_repository::list_active_journal_entries(&connection, conversation_id)
                .map_err(|error| error.to_string())?;
        let replacements = conversation_turn_rewrite_repository::replacement_message_ids_by_source(
            &connection,
            conversation_id,
        )
        .map_err(|error| error.to_string())?;
        for entry in &mut entries {
            if let Some(anchor) = entry.effective_before_message_id.as_deref() {
                entry.effective_before_message_id = Some(
                    conversation_turn_rewrite_repository::resolve_active_message_id(
                        &replacements,
                        anchor,
                    )?,
                );
            }
        }
        Ok(entries)
    }

    pub fn get_conversation_world_state_head(
        &self,
        conversation_id: &str,
    ) -> Result<Option<world_state_repository::ConversationWorldStateJournalEntry>, String> {
        let connection = self.state.connection()?;
        world_state_repository::get_active_head_entry(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn rewind_conversation_world_state_after(
        &self,
        conversation_id: &str,
        epoch_id: &str,
        sequence: u64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        world_state_repository::rewind_after(&mut connection, conversation_id, epoch_id, sequence)
            .map_err(|error| error.to_string())
    }

    pub fn fold_active_conversation_world_state(
        &self,
        conversation_id: &str,
    ) -> Result<Option<crate::WorldStateSnapshot>, String> {
        let connection = self.state.connection()?;
        world_state_repository::fold_active_snapshot(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn rebase_active_conversation_world_state(
        &self,
        request: &world_state_repository::ConversationWorldStateRebaseRequest<'_>,
    ) -> Result<world_state_repository::ConversationWorldStateRebaseOutcome, String> {
        let mut connection = self.state.connection()?;
        world_state_repository::rebase_active_epoch(&mut connection, request)
            .map_err(|error| error.to_string())
    }
}
