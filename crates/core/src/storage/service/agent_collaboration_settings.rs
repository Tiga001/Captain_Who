use super::StorageService;
use crate::storage::{agent_collaboration_settings_repository as repository, now_ms};
use crate::{AgentCollaborationSettings, AgentCollaborationSettingsUpdate};

impl StorageService {
    pub fn load_agent_collaboration_settings(&self) -> Result<AgentCollaborationSettings, String> {
        let connection = self.state.connection()?;
        repository::load_settings(&connection)
    }

    pub fn update_agent_collaboration_settings(
        &self,
        input: &AgentCollaborationSettingsUpdate,
    ) -> Result<AgentCollaborationSettings, String> {
        let mut connection = self.state.connection()?;
        repository::update_settings(&mut connection, input, now_ms())
    }
}
