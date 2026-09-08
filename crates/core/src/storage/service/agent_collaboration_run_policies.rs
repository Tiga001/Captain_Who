use super::StorageService;
use crate::storage::agent_collaboration_run_policy_repository;
use crate::AgentCollaborationSettings;
use rusqlite::OptionalExtension;

impl StorageService {
    /// Returns only the identity of the current logical run, without loading its potentially
    /// large trace. Context previews use it to distinguish this run from the next-run overlay.
    pub fn load_active_conversation_turn_identity(
        &self,
        conversation_id: &str,
    ) -> Result<Option<(String, String)>, String> {
        let connection = self.state.connection()?;
        connection
            .query_row(
                "SELECT run_id, assistant_message_id FROM conversation_turn_traces
             WHERE conversation_id=?1 AND terminal_status='in_progress'",
                [conversation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| error.to_string())
    }

    /// Reads the immutable Host capability recorded when this logical Turn was admitted.
    pub fn load_agent_collaboration_run_policy(
        &self,
        run_id: &str,
    ) -> Result<Option<AgentCollaborationSettings>, String> {
        let connection = self.state.connection()?;
        agent_collaboration_run_policy_repository::load_run(&connection, run_id)
            .map_err(|error| error.to_string())
    }

    /// A running conversation previews its frozen execution; an idle conversation previews the
    /// next run's global setting instead. The partial trace index permits only one active turn.
    pub fn load_active_agent_collaboration_run_policy(
        &self,
        conversation_id: &str,
    ) -> Result<Option<AgentCollaborationSettings>, String> {
        let connection = self.state.connection()?;
        connection
            .query_row(
                "SELECT p.enabled, p.revision, p.updated_at
             FROM agent_collaboration_run_policies p
             JOIN conversation_turn_traces t ON t.run_id = p.run_id
             WHERE t.conversation_id = ?1 AND t.terminal_status = 'in_progress'",
                [conversation_id],
                |row| {
                    Ok(AgentCollaborationSettings {
                        enabled: row.get(0)?,
                        revision: row.get(1)?,
                        updated_at: row.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(|error| error.to_string())
    }
}
