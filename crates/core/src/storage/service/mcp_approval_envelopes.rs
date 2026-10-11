use super::*;

impl StorageService {
    pub fn store_mcp_approval_envelope(
        &self,
        record: McpApprovalEnvelopeRecord,
    ) -> Result<mcp_approval_envelope_repository::McpApprovalEnvelopeStoreOutcome, String> {
        self.state.write_sync(
            record.invocation_id.len()
                + record.action_id.len()
                + record.nonce_base64.len()
                + record.ciphertext_base64.len()
                + record.aad_digest.len()
                + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| match mcp_approval_envelope_repository::store(connection, &record)
                .map_err(storage_error)?
            {
                mcp_approval_envelope_repository::McpApprovalEnvelopeStoreOutcome::Conflict => Err(
                    "MCP approval envelope identity already belongs to another action.".to_string(),
                ),
                outcome => Ok(outcome),
            },
        )?
    }

    pub fn load_mcp_approval_envelope(
        &self,
        invocation_id: &str,
    ) -> Result<Option<McpApprovalEnvelopeRecord>, String> {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        mcp_approval_envelope_repository::load(&connection, invocation_id).map_err(storage_error)
    }

    pub fn take_mcp_approval_envelope(
        &self,
        invocation_id: &str,
    ) -> Result<Option<McpApprovalEnvelopeRecord>, String> {
        self.state.write_sync(
            invocation_id.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                mcp_approval_envelope_repository::take(connection, invocation_id)
                    .map_err(storage_error)
            },
        )?
    }

    pub fn delete_mcp_approval_envelope(&self, invocation_id: &str) -> Result<bool, String> {
        self.state.write_sync(
            invocation_id.len() + 1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                mcp_approval_envelope_repository::delete(connection, invocation_id)
                    .map_err(storage_error)
            },
        )?
    }

    pub fn list_expired_mcp_approval_envelopes(
        &self,
        now_ms: i64,
        limit: usize,
    ) -> Result<Vec<McpApprovalEnvelopeRecord>, String> {
        let connection = self.state.read_connection()?;
        let connection = connection.unchecked_transaction().map_err(storage_error)?;
        mcp_approval_envelope_repository::list_expired(&connection, now_ms, limit)
            .map_err(storage_error)
    }

    pub fn reconcile_mcp_approval_envelopes(&self, now_ms: i64) -> Result<usize, String> {
        self.state.write_sync(
            1024,
            crate::storage::StorageWriteClass::Control,
            |connection| {
                let expired = mcp_approval_envelope_repository::delete_expired(connection, now_ms)
                    .map_err(storage_error)?;
                let orphaned = mcp_approval_envelope_repository::delete_orphans(connection)
                    .map_err(storage_error)?;
                Ok(expired.saturating_add(orphaned))
            },
        )?
    }
}
