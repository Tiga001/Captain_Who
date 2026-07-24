use super::*;

pub use crate::storage::guidance_repository::{
    AgentRunGuidanceStoreOutcome, AgentRunGuidanceTransitionOutcome,
};

impl StorageService {
    pub fn store_agent_run_guidance(
        &self,
        record: AgentRunGuidanceRecord,
    ) -> Result<AgentRunGuidanceStoreOutcome, String> {
        let mut connection = self.state.connection()?;
        guidance_repository::store_guidance(&mut connection, &record).map_err(storage_error)
    }

    pub fn load_agent_run_guidance(
        &self,
        guidance_id: &str,
    ) -> Result<Option<AgentRunGuidanceRecord>, String> {
        let connection = self.state.connection()?;
        guidance_repository::load_guidance(&connection, guidance_id).map_err(storage_error)
    }

    pub fn load_agent_run_guidance_by_client_message(
        &self,
        run_id: &str,
        client_message_id: &str,
    ) -> Result<Option<AgentRunGuidanceRecord>, String> {
        let connection = self.state.connection()?;
        guidance_repository::load_guidance_by_client_message(&connection, run_id, client_message_id)
            .map_err(storage_error)
    }

    pub fn list_queued_agent_run_guidances(&self) -> Result<Vec<AgentRunGuidanceRecord>, String> {
        let connection = self.state.connection()?;
        guidance_repository::list_queued_guidances(&connection).map_err(storage_error)
    }

    pub fn mark_agent_run_guidance_applied(
        &self,
        guidance_id: &str,
        trace_sequence: u64,
        updated_at: i64,
    ) -> Result<AgentRunGuidanceTransitionOutcome, String> {
        let connection = self.state.connection()?;
        guidance_repository::mark_guidance_applied(
            &connection,
            guidance_id,
            trace_sequence,
            updated_at,
        )
        .map_err(storage_error)
    }

    pub fn mark_agent_run_guidance_terminal(
        &self,
        guidance_id: &str,
        status: crate::AgentGuidanceStatus,
        reason: &str,
        updated_at: i64,
    ) -> Result<AgentRunGuidanceTransitionOutcome, String> {
        let connection = self.state.connection()?;
        guidance_repository::mark_guidance_terminal(
            &connection,
            guidance_id,
            status,
            reason,
            updated_at,
        )
        .map_err(storage_error)
    }

    pub fn abandon_queued_agent_run_guidances(
        &self,
        run_id: &str,
        reason: &str,
        updated_at: i64,
    ) -> Result<usize, String> {
        let connection = self.state.connection()?;
        guidance_repository::abandon_queued_guidances_for_run(
            &connection,
            run_id,
            reason,
            updated_at,
        )
        .map_err(storage_error)
    }
}
