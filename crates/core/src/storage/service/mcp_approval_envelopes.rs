use super::*;

impl StorageService {
    pub fn store_mcp_approval_envelope(
        &self,
        record: McpApprovalEnvelopeRecord,
    ) -> Result<mcp_approval_envelope_repository::McpApprovalEnvelopeStoreOutcome, String> {
        let connection = self.state.connection()?;
        match mcp_approval_envelope_repository::store(&connection, &record)
            .map_err(storage_error)?
        {
            mcp_approval_envelope_repository::McpApprovalEnvelopeStoreOutcome::Conflict => {
                Err("MCP approval envelope identity already belongs to another action.".to_string())
            }
            outcome => Ok(outcome),
        }
    }

    pub fn load_mcp_approval_envelope(
        &self,
        invocation_id: &str,
    ) -> Result<Option<McpApprovalEnvelopeRecord>, String> {
        let connection = self.state.connection()?;
        mcp_approval_envelope_repository::load(&connection, invocation_id).map_err(storage_error)
    }

    pub fn take_mcp_approval_envelope(
        &self,
        invocation_id: &str,
    ) -> Result<Option<McpApprovalEnvelopeRecord>, String> {
        let mut connection = self.state.connection()?;
        mcp_approval_envelope_repository::take(&mut connection, invocation_id)
            .map_err(storage_error)
    }

    pub fn delete_mcp_approval_envelope(&self, invocation_id: &str) -> Result<bool, String> {
        let connection = self.state.connection()?;
        mcp_approval_envelope_repository::delete(&connection, invocation_id).map_err(storage_error)
    }

    pub fn list_expired_mcp_approval_envelopes(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<McpApprovalEnvelopeRecord>, String> {
        let connection = self.state.connection()?;
        mcp_approval_envelope_repository::list_expired(&connection, now_ms, limit)
            .map_err(storage_error)
    }

    pub fn reconcile_mcp_approval_envelopes(&self, now_ms: i64) -> Result<usize, String> {
        let connection = self.state.connection()?;
        let expired = mcp_approval_envelope_repository::delete_expired(&connection, now_ms)
            .map_err(storage_error)?;
        let orphaned =
            mcp_approval_envelope_repository::delete_orphans(&connection).map_err(storage_error)?;
        Ok(expired.saturating_add(orphaned))
    }
}
