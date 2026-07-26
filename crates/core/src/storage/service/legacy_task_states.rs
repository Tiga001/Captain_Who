use super::*;

impl StorageService {
    /// Internal support-only export for the retired Task Continuation State tables.
    ///
    /// The rows are never injected into model context and cannot be mutated through this API.
    pub fn export_legacy_task_states(
        &self,
        conversation_id: &str,
    ) -> Result<serde_json::Value, String> {
        let connection = self.state.connection()?;
        legacy_task_state_repository::export_for_conversation(&connection, conversation_id)
    }
}
