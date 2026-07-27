use super::*;

impl StorageService {
    pub fn initialize_agent_turn_diff(
        &self,
        identity: &AgentTurnDiffIdentity,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        turn_diff_repository::initialize_turn(&connection, identity, now_ms())
            .map_err(storage_error)
    }

    pub fn record_agent_turn_file_change(
        &self,
        identity: &AgentTurnDiffIdentity,
        action_id: &str,
        change: &AgentTurnFileChange,
    ) -> Result<bool, String> {
        let mut connection = self.state.connection()?;
        turn_diff_repository::record_file_change(
            &mut connection,
            identity,
            action_id,
            change,
            now_ms(),
        )
        .map_err(storage_error)
    }

    pub fn load_latest_agent_turn_diff(
        &self,
        conversation_id: &str,
        project_id: &str,
    ) -> Result<Option<AgentTurnDiffRecord>, String> {
        let connection = self.state.connection()?;
        turn_diff_repository::load_latest_turn(&connection, conversation_id, project_id)
            .map_err(storage_error)
    }

    pub fn load_agent_turn_diffs_for_messages(
        &self,
        conversation_id: &str,
        project_id: &str,
        assistant_message_ids: &[String],
    ) -> Result<Vec<AgentTurnDiffRecord>, String> {
        let connection = self.state.connection()?;
        turn_diff_repository::load_turns_for_messages(
            &connection,
            conversation_id,
            project_id,
            assistant_message_ids,
        )
        .map_err(storage_error)
    }
}
