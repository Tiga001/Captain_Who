//! Per-server mutation admission and shutdown fencing.

use super::*;

impl McpManagementService {
    pub(super) fn begin_server_mutation(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> Result<ActiveServerMutation<'_>, McpManagementFailure> {
        let mut active = self.active_server_mutations.lock().map_err(|_| {
            self.failure(
                operation,
                McpManagementErrorCodeDto::InternalSafeError,
                McpManagementRecoveryDto::Retry,
                "MCP mutation admission is unavailable.",
                Some(server_id),
            )
        })?;
        if self.shutdown_started.load(Ordering::Acquire) {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::DoNotRetry,
                "MCP management is shutting down.",
                Some(server_id),
            ));
        }
        if !active.insert(server_id) {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::Conflict,
                McpManagementRecoveryDto::Refresh,
                "Another MCP server mutation is already in progress.",
                Some(server_id),
            ));
        }
        drop(active);
        Ok(ActiveServerMutation {
            server_id,
            active_server_mutations: &self.active_server_mutations,
        })
    }

    pub(super) fn validate_mutation_input(
        &self,
        input: &McpServerMutationInput,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerId, McpManagementFailure> {
        ensure_schema_version(input.schema_version, operation)?;
        let precondition = parse_precondition(&input.server_id, &input.precondition, operation)?;
        let record = self.persisted(precondition.server_id, operation)?;
        ensure_registry_precondition(&record.entry, &precondition).map_err(|_| {
            self.registry_failure(
                operation,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        Ok(precondition.server_id)
    }

    pub(super) fn ensure_mutations_open(
        &self,
        operation: McpManagementOperationDto,
    ) -> Result<(), McpManagementFailure> {
        if self.shutdown_started.load(Ordering::Acquire) {
            Err(self.failure(
                operation,
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::DoNotRetry,
                "MCP management is shutting down.",
                None,
            ))
        } else {
            Ok(())
        }
    }
}
