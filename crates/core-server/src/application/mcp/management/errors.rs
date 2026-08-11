//! Stable management failure construction and backend error mapping.

use super::*;

#[derive(Debug)]
pub(crate) struct McpManagementFailure {
    data: McpManagementErrorData,
}

impl McpManagementFailure {
    pub(crate) fn into_data(self) -> McpManagementErrorData {
        self.data
    }
}

impl std::fmt::Display for McpManagementFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.data.message)
    }
}

impl std::error::Error for McpManagementFailure {}

impl McpManagementService {
    pub(super) fn persisted(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> Result<McpPersistedRegistryRecord, McpManagementFailure> {
        self.registry
            .get_persisted(server_id)
            .map_err(|error| self.registry_failure(operation, Some(server_id), error))?
            .ok_or_else(|| {
                self.registry_failure(
                    operation,
                    Some(server_id),
                    McpRegistryPersistenceError::NotFound,
                )
            })
    }

    pub(super) fn registry_failure(
        &self,
        operation: McpManagementOperationDto,
        server_id: Option<McpServerId>,
        error: McpRegistryPersistenceError,
    ) -> McpManagementFailure {
        let (code, recovery, message) = match error {
            McpRegistryPersistenceError::InvalidConfig => (
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP server configuration is invalid.",
            ),
            McpRegistryPersistenceError::NotFound => (
                McpManagementErrorCodeDto::NotFound,
                McpManagementRecoveryDto::Refresh,
                "The MCP server no longer exists.",
            ),
            McpRegistryPersistenceError::Conflict => (
                McpManagementErrorCodeDto::Conflict,
                McpManagementRecoveryDto::Refresh,
                "The MCP server configuration changed. Refresh before retrying.",
            ),
            McpRegistryPersistenceError::CapacityExceeded => (
                McpManagementErrorCodeDto::PolicyDenied,
                McpManagementRecoveryDto::DoNotRetry,
                "The MCP server capacity has been reached.",
            ),
            McpRegistryPersistenceError::AuthorizationRequired => (
                McpManagementErrorCodeDto::AuthorizationRequired,
                McpManagementRecoveryDto::RequestLaunchAuthorization,
                "The exact MCP launch configuration must be authorized.",
            ),
            McpRegistryPersistenceError::CorruptRecord => (
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::DoNotRetry,
                "The persisted MCP server record is invalid and will not be started.",
            ),
            McpRegistryPersistenceError::DevelopmentStorageSchemaResetRequired => (
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::DoNotRetry,
                "The development storage schema must be reset.",
            ),
            McpRegistryPersistenceError::StorageUnavailable
            | McpRegistryPersistenceError::RevisionExhausted => (
                McpManagementErrorCodeDto::InternalSafeError,
                McpManagementRecoveryDto::Retry,
                "MCP management storage is unavailable.",
            ),
        };
        self.failure(operation, code, recovery, message, server_id)
    }

    pub(super) fn manager_failure(
        &self,
        operation: McpManagementOperationDto,
        server_id: Option<McpServerId>,
        error: McpError,
    ) -> McpManagementFailure {
        let (code, recovery, message) = match error.kind {
            McpErrorKind::Config => (
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::Refresh,
                "The MCP server is not in a valid state for this operation.",
            ),
            McpErrorKind::Timeout => (
                McpManagementErrorCodeDto::Timeout,
                McpManagementRecoveryDto::Retry,
                "The MCP server operation timed out.",
            ),
            McpErrorKind::Capacity => (
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::Retry,
                "MCP Host capacity is temporarily full.",
            ),
            McpErrorKind::Shutdown => (
                McpManagementErrorCodeDto::CleanupIncomplete,
                McpManagementRecoveryDto::StopAndRetry,
                "The MCP server did not shut down cleanly.",
            ),
            McpErrorKind::Spawn => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP server process could not be started.",
            ),
            McpErrorKind::Negotiation => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP protocol negotiation failed.",
            ),
            McpErrorKind::Protocol => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP server returned an invalid protocol response.",
            ),
            McpErrorKind::ServerExited => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP server process exited during the operation.",
            ),
            _ => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP server operation failed.",
            ),
        };
        self.failure(operation, code, recovery, message, server_id)
    }

    pub(super) fn failure(
        &self,
        operation: McpManagementOperationDto,
        code: McpManagementErrorCodeDto,
        recovery: McpManagementRecoveryDto,
        message: &str,
        server_id: Option<McpServerId>,
    ) -> McpManagementFailure {
        McpManagementFailure {
            data: McpManagementErrorData {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                error_type: McpManagementErrorTypeDto::McpManagement,
                operation,
                code,
                recovery,
                message: bounded_text(message, MAX_SAFE_ERROR_BYTES),
                server_id: server_id.map(|id| id.to_string()),
                current_registry_revision: self.registry.current_revision().ok(),
            },
        }
    }
}

pub(super) fn standalone_failure(
    operation: McpManagementOperationDto,
    code: McpManagementErrorCodeDto,
    recovery: McpManagementRecoveryDto,
    message: &str,
    server_id: Option<McpServerId>,
) -> McpManagementFailure {
    McpManagementFailure {
        data: McpManagementErrorData {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            error_type: McpManagementErrorTypeDto::McpManagement,
            operation,
            code,
            recovery,
            message: bounded_text(message, MAX_SAFE_ERROR_BYTES),
            server_id: server_id.map(|id| id.to_string()),
            current_registry_revision: None,
        },
    }
}
