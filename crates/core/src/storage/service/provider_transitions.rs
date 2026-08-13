use super::*;
use crate::storage::{ProviderTransitionCompatibleCommitOutcome, ProviderTransitionTerminalRecord};

impl StorageService {
    #[allow(clippy::too_many_arguments)]
    pub fn commit_compatible_provider_transition_terminal(
        &self,
        operation_id: &str,
        conversation_id: &str,
        target_model_id: &str,
        source_model_display_name: Option<&str>,
        target_model_display_name: Option<&str>,
        started_at: i64,
        completed_at: i64,
        conversation_updated_at: i64,
        expected_current_model_id: Option<&str>,
        expected_conversation_updated_at: i64,
        expected_conversation_revision: i64,
        expected_target_provider_protocol_revision: &str,
    ) -> Result<ProviderTransitionCompatibleCommitOutcome, String> {
        let record = ProviderTransitionTerminalRecord::new(
            operation_id,
            conversation_id,
            target_model_id,
            source_model_display_name.map(str::to_string),
            target_model_display_name.map(str::to_string),
            started_at,
            completed_at,
            conversation_updated_at,
        )
        .map_err(|error| error.to_string())?;
        let mut connection = self.state.connection()?;
        provider_transition_repository::commit_compatible_transition(
            &mut connection,
            &record,
            expected_current_model_id,
            expected_conversation_updated_at,
            expected_conversation_revision,
            expected_target_provider_protocol_revision,
        )
        .map_err(|error| error.to_string())
    }

    pub fn get_provider_transition_terminal_record(
        &self,
        operation_id: &str,
    ) -> Result<Option<ProviderTransitionTerminalRecord>, String> {
        let connection = self.state.connection()?;
        provider_transition_repository::get_terminal_record(&connection, operation_id)
            .map_err(|error| error.to_string())
    }

    pub fn list_provider_transition_terminal_records(
        &self,
        conversation_id: &str,
        limit: usize,
    ) -> Result<Vec<ProviderTransitionTerminalRecord>, String> {
        let connection = self.state.connection()?;
        provider_transition_repository::list_terminal_records(&connection, conversation_id, limit)
            .map_err(|error| error.to_string())
    }
}
