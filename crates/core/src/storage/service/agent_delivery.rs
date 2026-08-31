use super::*;
use crate::{
    AgentGraphError, AgentModelBatchDeliveryRecord, AgentWaitReadySnapshot,
    BindAgentSafeBoundaryInput, BindAgentTurnStartInput, PollAgentWaitInput,
};

fn unavailable(error: String) -> AgentGraphError {
    AgentGraphError::StorageUnavailable(error)
}

impl StorageService {
    pub fn list_preloaded_agent_message_ids(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Result<Vec<String>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::list_preloaded_agent_message_ids(
            &connection,
            conversation_id,
            assistant_message_id,
        )
    }

    pub fn filter_preloaded_agent_message_ids(
        &self,
        conversation_id: &str,
        projected_message_ids: &[String],
    ) -> Result<Vec<String>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::filter_preloaded_agent_message_ids(
            &connection,
            conversation_id,
            projected_message_ids,
        )
    }

    pub fn list_trace_bound_agent_projection_message_ids(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<String>, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::list_trace_bound_projection_message_ids(
            &connection,
            conversation_id,
        )
    }

    pub fn bind_agent_turn_start_messages(
        &self,
        input: &BindAgentTurnStartInput,
        preloaded_message_ids: &[String],
    ) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::bind_turn_start_messages(
            &mut connection,
            input,
            preloaded_message_ids,
            now_ms(),
        )
    }

    pub fn bind_agent_safe_boundary(
        &self,
        input: &BindAgentSafeBoundaryInput,
    ) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::bind_safe_boundary(&mut connection, input, now_ms())
    }

    pub fn poll_agent_wait_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError> {
        let mut connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::poll_wait_ready(&mut connection, input, now_ms())
    }

    pub fn probe_agent_wait_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<bool, AgentGraphError> {
        let connection = self.state.connection().map_err(unavailable)?;
        agent_delivery_repository::probe_wait_ready(&connection, input, now_ms())
    }
}
