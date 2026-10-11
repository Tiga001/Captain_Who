use super::*;
use crate::storage::{write_size::estimated_json_bytes, StorageWriteClass};

fn world_state_job_bytes(parts: &[&str]) -> usize {
    parts.iter().fold(512_usize, |bytes, part| {
        bytes.saturating_add(part.len().saturating_mul(2))
    })
}

impl StorageService {
    pub fn commit_conversation_world_state_request(
        &self,
        request: &world_state_repository::ConversationWorldStateCommitRequest<'_>,
    ) -> Result<world_state_repository::ConversationWorldStateCommitOutcome, String> {
        let estimated_bytes =
            world_state_job_bytes(&[request.conversation_id, request.initial_epoch_id])
                .saturating_add(estimated_json_bytes(&(
                    request.boundary,
                    request.expected_head,
                    request.owned_section_ids,
                    request.sections,
                ))?);
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                world_state_repository::commit_request(connection, request)
                    .map_err(|error| error.to_string())
            })?
    }

    pub fn mark_conversation_world_state_request_observed(
        &self,
        conversation_id: &str,
        boundary: &crate::world_state::WorldStateRequestBoundary,
    ) -> Result<(), String> {
        let estimated_bytes = world_state_job_bytes(&[conversation_id])
            .saturating_add(estimated_json_bytes(boundary)?);
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                world_state_repository::mark_request_observed(connection, conversation_id, boundary)
                    .map_err(|error| error.to_string())
            })?
    }

    pub fn append_conversation_world_state_record(
        &self,
        conversation_id: &str,
        epoch_generation: u64,
        base_summary_id: Option<&str>,
        effective_before_message_id: Option<&str>,
        record: &WorldStateRecord,
        created_at: i64,
    ) -> Result<world_state_repository::ConversationWorldStateAppendOutcome, String> {
        let estimated_bytes = world_state_job_bytes(&[
            conversation_id,
            base_summary_id.unwrap_or_default(),
            effective_before_message_id.unwrap_or_default(),
        ])
        .saturating_add(estimated_json_bytes(record)?);
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                world_state_repository::append_record(
                    connection,
                    &world_state_repository::ConversationWorldStateRecordWrite {
                        conversation_id,
                        epoch_generation,
                        base_summary_id,
                        effective_before_message_id,
                        request_boundary: None,
                        model_observed: true,
                        record,
                        created_at,
                    },
                )
                .map_err(|error| error.to_string())
            })?
    }

    pub fn list_active_conversation_world_state_records(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<world_state_repository::ConversationWorldStateJournalEntry>, String> {
        let pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(storage_error)?;
        let mut entries =
            world_state_repository::list_active_journal_entries(&connection, conversation_id)
                .map_err(|error| error.to_string())?;
        let replacements = conversation_turn_rewrite_repository::replacement_message_ids_by_source(
            &connection,
            conversation_id,
        )
        .map_err(|error| error.to_string())?;
        connection.commit().map_err(storage_error)?;
        drop(pooled_connection);
        for entry in &mut entries {
            if let Some(boundary) = &mut entry.request_boundary {
                boundary.assistant_message_id =
                    conversation_turn_rewrite_repository::resolve_active_message_id(
                        &replacements,
                        &boundary.assistant_message_id,
                    )?;
            }
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
        let pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(storage_error)?;
        world_state_repository::get_active_head_entry(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn rewind_conversation_world_state_after(
        &self,
        conversation_id: &str,
        epoch_id: &str,
        sequence: u64,
    ) -> Result<(), String> {
        let estimated_bytes = world_state_job_bytes(&[conversation_id, epoch_id]);
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                world_state_repository::rewind_after(
                    connection,
                    conversation_id,
                    epoch_id,
                    sequence,
                )
                .map_err(|error| error.to_string())
            })?
    }

    pub fn fold_active_conversation_world_state(
        &self,
        conversation_id: &str,
    ) -> Result<Option<crate::WorldStateSnapshot>, String> {
        let pooled_connection = self.state.read_connection()?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(storage_error)?;
        world_state_repository::fold_active_snapshot(&connection, conversation_id)
            .map_err(|error| error.to_string())
    }

    pub fn rebase_active_conversation_world_state(
        &self,
        request: &world_state_repository::ConversationWorldStateRebaseRequest<'_>,
    ) -> Result<world_state_repository::ConversationWorldStateRebaseOutcome, String> {
        let estimated_bytes = world_state_job_bytes(&[
            request.conversation_id,
            request.expected_source_epoch_id,
            request.expected_source_revision,
            request.new_epoch_id,
            request.base_summary_id,
        ])
        .saturating_add(estimated_json_bytes(request.covered_through)?);
        self.state
            .write_sync(estimated_bytes, StorageWriteClass::Normal, |connection| {
                world_state_repository::rebase_active_epoch(connection, request)
                    .map_err(|error| error.to_string())
            })?
    }
}
