//! Persisted and live server state projection into management DTOs.

use super::*;

impl McpManagementService {
    pub(super) fn details_for(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        let record = self.persisted(server_id, operation)?;
        let mut summary = self.project_summary(&record)?;
        // A detail read is explicitly scoped to one Server, so it can afford
        // the bounded live filesystem identity check that would be too costly
        // across a large list. This lets the UI distinguish an authorization
        // whose executable or code entrypoint changed after approval.
        if summary.launch_authorization_state == McpLaunchAuthorizationStateDto::Authorized
            && !launch_authorization_is_valid(&record)
        {
            summary.launch_authorization_state = McpLaunchAuthorizationStateDto::Stale;
        }
        let status = self
            .manager
            .get_status(server_id)
            .map_err(|error| self.manager_failure(operation, Some(server_id), error))?;
        let protocol = status.as_ref().and_then(|status| {
            (status.config_epoch == record.entry.config_epoch)
                .then_some(status.protocol.as_ref())
                .flatten()
        });
        let stdio = match &record.entry.config.transport {
            McpTransportConfig::Stdio(stdio) => stdio,
            _ => {
                return Err(self.failure(
                    operation,
                    McpManagementErrorCodeDto::InvalidState,
                    McpManagementRecoveryDto::DoNotRetry,
                    "The persisted MCP transport is unsupported.",
                    Some(server_id),
                ))
            }
        };
        let registry_revision = summary.registry_revision;
        Ok(McpServerDetailsOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            registry_revision,
            server: McpServerDetailsView {
                summary,
                executable: path_text(&stdio.program, operation)?,
                arguments: stdio.arguments.clone(),
                cwd: path_text(&stdio.cwd, operation)?,
                protocol: protocol.map(|snapshot| McpProtocolView {
                    protocol_version: bounded_text(
                        &snapshot.negotiated_version,
                        MAX_PROTOCOL_VERSION_BYTES,
                    ),
                    lifecycle: match snapshot.lifecycle {
                        McpLifecycleKind::Discover => McpProtocolLifecycleDto::Discover,
                        McpLifecycleKind::InitializeFallback => {
                            McpProtocolLifecycleDto::InitializeFallback
                        }
                        _ => McpProtocolLifecycleDto::Unknown,
                    },
                }),
                capabilities: protocol.map(|snapshot| McpCapabilityView {
                    tools: snapshot.capabilities.tools,
                    resources: snapshot.capabilities.resources,
                    prompts: snapshot.capabilities.prompts,
                    logging: snapshot.capabilities.logging,
                    completion: snapshot.capabilities.completions,
                }),
                created_at_ms: nonnegative_ms(record.created_at_ms),
            },
        })
    }

    pub(super) fn project_summary(
        &self,
        record: &McpPersistedRegistryRecord,
    ) -> Result<McpServerListItem, McpManagementFailure> {
        let server_id = record.entry.config.id;
        let status = self.manager.get_status(server_id).map_err(|error| {
            self.manager_failure(McpManagementOperationDto::List, Some(server_id), error)
        })?;
        let current_status = status
            .as_ref()
            .filter(|status| status.config_epoch == record.entry.config_epoch);
        let catalog = self.manager.catalog(server_id).map_err(|error| {
            self.manager_failure(McpManagementOperationDto::List, Some(server_id), error)
        })?;
        let current_catalog = catalog.as_ref().filter(|catalog| {
            catalog.source_config_epoch == Some(record.entry.config_epoch)
                && catalog.source_config_digest.as_ref() == Some(&record.entry.config_digest)
        });
        // Projection is intentionally structural and non-blocking. Enable/start
        // and the final connector boundary re-hash the live executable/script
        // identity before any process can be spawned.
        let launch_authorization_state = if launch_authorization_identity_is_valid(record) {
            McpLaunchAuthorizationStateDto::Authorized
        } else if record.launch_authorization.is_some() {
            McpLaunchAuthorizationStateDto::Stale
        } else {
            McpLaunchAuthorizationStateDto::Required
        };
        let state = if !record.entry.config.enabled {
            McpConnectionStateDto::Disabled
        } else {
            current_status
                .map(|status| connection_state(status.state))
                .unwrap_or(McpConnectionStateDto::Disabled)
        };
        let completeness = current_catalog
            .map(|catalog| catalog_completeness(&catalog.completeness))
            .unwrap_or(McpCatalogCompletenessDto::Failed);
        let tool_count = current_catalog
            .map(|catalog| catalog.tools.len())
            .unwrap_or_default();
        let last_error = current_status
            .and_then(|status| status.last_error.as_ref())
            .map(|error| McpSafeErrorView {
                code: safe_error_code(&error.code),
                message: bounded_text(&error.message, MAX_SAFE_ERROR_BYTES),
            })
            .or_else(|| {
                record
                    .safe_error_code
                    .as_ref()
                    .map(|code| McpSafeErrorView {
                        code: safe_error_code(code),
                        message: "The persisted MCP server configuration requires attention."
                            .to_string(),
                    })
            });
        Ok(McpServerListItem {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server_id.to_string(),
            display_name: bounded_text(&record.entry.config.display_name, MAX_DISPLAY_NAME_BYTES),
            scope: McpServerScopeDto::User,
            source: McpServerSourceDto::UserManual,
            transport: McpTransportKindDto::Stdio,
            enabled: record.entry.config.enabled,
            trust: match record.entry.config.trust {
                McpTrustLevel::UserApproved => McpTrustLevelDto::UserApproved,
                _ => McpTrustLevelDto::Untrusted,
            },
            approval_mode: match record.entry.config.approval_mode {
                McpApprovalMode::Auto => McpApprovalModeDto::Auto,
                McpApprovalMode::Deny => McpApprovalModeDto::Deny,
                _ => McpApprovalModeDto::Prompt,
            },
            launch_authorization_state,
            state,
            registry_revision: record.entry.revision,
            config_epoch: record.entry.config_epoch.to_string(),
            config_digest: record.entry.config_digest.to_string(),
            catalog_generation: current_catalog
                .map(|catalog| catalog.generation)
                .unwrap_or(0),
            catalog_completeness: completeness,
            tool_count: u32::try_from(tool_count).unwrap_or(u32::MAX),
            active_call_count: current_status
                .map(|status| u32::try_from(status.active_call_count).unwrap_or(u32::MAX))
                .unwrap_or(0),
            last_error,
            updated_at_ms: nonnegative_ms(record.updated_at_ms),
        })
    }
}
