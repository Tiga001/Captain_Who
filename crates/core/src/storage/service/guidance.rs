use super::*;

pub use crate::storage::guidance_repository::{
    AgentRunGuidanceStoreOutcome, AgentRunGuidanceTransitionOutcome,
};

impl StorageService {
    pub fn store_agent_run_guidance(
        &self,
        record: AgentRunGuidanceRecord,
    ) -> Result<AgentRunGuidanceStoreOutcome, String> {
        self.state.write_sync(
            record.content.len()
                + record.folder_references_json.len()
                + record.attachment_ids.iter().map(String::len).sum::<usize>()
                + record.guidance_id.len()
                + record.client_message_id.len()
                + record.run_id.len()
                + record.conversation_id.len()
                + record.assistant_message_id.len()
                + record.terminal_reason.as_ref().map_or(0, String::len)
                + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                guidance_repository::store_guidance(connection, &record).map_err(storage_error)
            },
        )?
    }

    pub fn load_agent_run_guidance(
        &self,
        guidance_id: &str,
    ) -> Result<Option<AgentRunGuidanceRecord>, String> {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        guidance_repository::load_guidance(&connection, guidance_id).map_err(storage_error)
    }

    pub fn load_agent_run_guidance_by_client_message(
        &self,
        run_id: &str,
        client_message_id: &str,
    ) -> Result<Option<AgentRunGuidanceRecord>, String> {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        guidance_repository::load_guidance_by_client_message(&connection, run_id, client_message_id)
            .map_err(storage_error)
    }

    pub fn list_queued_agent_run_guidances(&self) -> Result<Vec<AgentRunGuidanceRecord>, String> {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        guidance_repository::list_queued_guidances(&connection).map_err(storage_error)
    }

    pub fn agent_run_guidance_attachment_bytes(&self, run_id: &str) -> Result<u64, String> {
        let connection = self.state.read_connection()?;
        guidance_repository::sum_attachment_bytes_for_run(&connection, run_id)
            .map_err(storage_error)
    }

    pub fn mark_agent_run_guidance_applied(
        &self,
        guidance_id: &str,
        trace_sequence: u64,
        updated_at: i64,
    ) -> Result<AgentRunGuidanceTransitionOutcome, String> {
        self.state.write_sync(
            guidance_id.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                guidance_repository::mark_guidance_applied(
                    connection,
                    guidance_id,
                    trace_sequence,
                    updated_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn mark_agent_run_guidance_terminal(
        &self,
        guidance_id: &str,
        status: crate::AgentGuidanceStatus,
        reason: &str,
        updated_at: i64,
    ) -> Result<AgentRunGuidanceTransitionOutcome, String> {
        self.state.write_sync(
            guidance_id.len() + reason.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                guidance_repository::mark_guidance_terminal(
                    connection,
                    guidance_id,
                    status,
                    reason,
                    updated_at,
                )
                .map_err(storage_error)
            },
        )?
    }

    pub fn abandon_queued_agent_run_guidances(
        &self,
        run_id: &str,
        reason: &str,
        updated_at: i64,
    ) -> Result<usize, String> {
        self.state.write_sync(
            run_id.len() + reason.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                guidance_repository::abandon_queued_guidances_for_run(
                    connection, run_id, reason, updated_at,
                )
                .map_err(storage_error)
            },
        )?
    }
}
