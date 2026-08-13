use super::*;
use crate::{AgentCollaborationEventError, AgentCollaborationEventRecord};

fn unavailable(_: String) -> AgentCollaborationEventError {
    AgentCollaborationEventError::StorageUnavailable
}

impl StorageService {
    pub fn list_agent_collaboration_events(
        &self,
        root_agent_id: &str,
        after_root_sequence: u64,
        maximum: usize,
    ) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_collaboration_event_repository::list_root_events(
            &connection,
            root_agent_id,
            after_root_sequence,
            maximum,
        )
    }

    pub fn list_global_agent_collaboration_events(
        &self,
        after_global_sequence: u64,
        maximum: usize,
    ) -> Result<Vec<AgentCollaborationEventRecord>, AgentCollaborationEventError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_collaboration_event_repository::list_global_events(
            &connection,
            after_global_sequence,
            maximum,
        )
    }

    pub fn latest_global_agent_collaboration_event_sequence(
        &self,
    ) -> Result<u64, AgentCollaborationEventError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_collaboration_event_repository::latest_global_sequence(&connection)
    }

    pub fn latest_agent_collaboration_event_sequence(
        &self,
        root_agent_id: &str,
    ) -> Result<u64, AgentCollaborationEventError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_collaboration_event_repository::latest_root_sequence(&connection, root_agent_id)
    }

    pub fn latest_agent_collaboration_activity_at(
        &self,
        root_agent_id: &str,
        agent_id: &str,
    ) -> Result<Option<i64>, AgentCollaborationEventError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_collaboration_event_repository::latest_agent_activity_at(
            &connection,
            root_agent_id,
            agent_id,
        )
    }
}
