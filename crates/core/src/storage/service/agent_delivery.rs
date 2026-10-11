use super::*;
use crate::storage::StorageWriteClass;
use crate::{
    AgentGraphError, AgentModelBatchDeliveryRecord, AgentWaitReadySnapshot,
    BindAgentSafeBoundaryInput, BindAgentTurnStartInput, PollAgentWaitInput,
};

fn unavailable(error: String) -> AgentGraphError {
    AgentGraphError::StorageUnavailable(error)
}

fn delivery_job_bytes(parts: &[&str], identifiers: &[String]) -> usize {
    parts
        .iter()
        .copied()
        .chain(identifiers.iter().map(String::as_str))
        .fold(512_usize, |bytes, part| {
            bytes.saturating_add(part.len().saturating_mul(2))
        })
}

impl StorageService {
    pub fn list_preloaded_agent_message_ids(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Result<Vec<String>, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
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
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
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
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
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
        self.state
            .write_sync(
                delivery_job_bytes(
                    &[
                        &input.conversation_id,
                        &input.run_id,
                        &input.assistant_message_id,
                    ],
                    preloaded_message_ids,
                ),
                StorageWriteClass::Normal,
                |connection| {
                    agent_delivery_repository::bind_turn_start_messages(
                        connection,
                        input,
                        preloaded_message_ids,
                        now_ms(),
                    )
                },
            )
            .map_err(unavailable)?
    }

    pub fn bind_agent_safe_boundary(
        &self,
        input: &BindAgentSafeBoundaryInput,
    ) -> Result<Option<AgentModelBatchDeliveryRecord>, AgentGraphError> {
        self.state
            .write_sync(
                delivery_job_bytes(
                    &[
                        &input.conversation_id,
                        &input.run_id,
                        &input.assistant_message_id,
                    ],
                    &[],
                ),
                StorageWriteClass::Normal,
                |connection| {
                    agent_delivery_repository::bind_safe_boundary(connection, input, now_ms())
                },
            )
            .map_err(unavailable)?
    }

    pub fn poll_agent_wait_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<Option<AgentWaitReadySnapshot>, AgentGraphError> {
        self.state
            .write_sync(
                delivery_job_bytes(
                    &[
                        &input.caller_agent_id,
                        &input.conversation_id,
                        &input.run_id,
                        &input.assistant_message_id,
                    ],
                    &input.target_agent_ids,
                ),
                StorageWriteClass::Normal,
                |connection| {
                    agent_delivery_repository::poll_wait_ready(connection, input, now_ms())
                },
            )
            .map_err(unavailable)?
    }

    pub fn probe_agent_wait_ready(
        &self,
        input: &PollAgentWaitInput,
    ) -> Result<bool, AgentGraphError> {
        let pooled_connection = self.state.read_connection().map_err(unavailable)?;
        let connection = pooled_connection
            .unchecked_transaction()
            .map_err(|error| unavailable(storage_error(error)))?;
        agent_delivery_repository::probe_wait_ready(&connection, input, now_ms())
    }
}
