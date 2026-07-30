//! Two-phase launch authorization preparation and commit.

use super::*;

impl McpManagementService {
    pub(crate) fn prepare_launch_authorization(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpLaunchAuthorizationPreview, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::PrepareLaunchAuthorization)?;
        ensure_schema_version(
            input.schema_version,
            McpManagementOperationDto::PrepareLaunchAuthorization,
        )?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::PrepareLaunchAuthorization,
        )?;
        let record = self.persisted(
            precondition.server_id,
            McpManagementOperationDto::PrepareLaunchAuthorization,
        )?;
        ensure_registry_precondition(&record.entry, &precondition).map_err(|_| {
            self.registry_failure(
                McpManagementOperationDto::PrepareLaunchAuthorization,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        if record.entry.config.enabled {
            return Err(self.failure(
                McpManagementOperationDto::PrepareLaunchAuthorization,
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::StopAndRetry,
                "Disable the MCP server before authorizing its launch configuration.",
                Some(precondition.server_id),
            ));
        }
        let authorization_id = Uuid::new_v4();
        let (file_identity_digest, launch_config) =
            prepare_launch_file_identity(&record.entry.config, &record.launch_spec_digest)
                .map_err(|error| {
                    self.registry_failure(
                        McpManagementOperationDto::PrepareLaunchAuthorization,
                        Some(precondition.server_id),
                        error,
                    )
                })?;
        let stdio = match &launch_config.transport {
            McpTransportConfig::Stdio(stdio) => stdio,
            _ => {
                return Err(self.failure(
                    McpManagementOperationDto::PrepareLaunchAuthorization,
                    McpManagementErrorCodeDto::InvalidState,
                    McpManagementRecoveryDto::DoNotRetry,
                    "The persisted MCP transport is unsupported.",
                    Some(precondition.server_id),
                ))
            }
        };
        let expires_at_ms = now_ms()
            .checked_add(LAUNCH_AUTHORIZATION_PREVIEW_TTL_MS)
            .ok_or_else(|| {
                self.failure(
                    McpManagementOperationDto::PrepareLaunchAuthorization,
                    McpManagementErrorCodeDto::InternalSafeError,
                    McpManagementRecoveryDto::Retry,
                    "The launch authorization preview could not be created.",
                    Some(precondition.server_id),
                )
            })?;
        {
            let mut previews = self.authorization_previews.lock().map_err(|_| {
                self.failure(
                    McpManagementOperationDto::PrepareLaunchAuthorization,
                    McpManagementErrorCodeDto::InternalSafeError,
                    McpManagementRecoveryDto::Retry,
                    "The launch authorization service is unavailable.",
                    Some(precondition.server_id),
                )
            })?;
            let current_time_ms = now_ms();
            previews.retain(|_, preview| preview.expires_at_ms > current_time_ms);
            // Only the newest frozen preview for a Server remains valid. Native-dialog
            // cancellation therefore cannot accumulate orphan previews until the
            // per-process admission limit is exhausted.
            previews.retain(|_, preview| preview.server_id != precondition.server_id);
            if previews.len() >= MAX_LAUNCH_AUTHORIZATION_PREVIEWS {
                return Err(self.failure(
                    McpManagementOperationDto::PrepareLaunchAuthorization,
                    McpManagementErrorCodeDto::PolicyDenied,
                    McpManagementRecoveryDto::Retry,
                    "The launch authorization preview limit has been reached.",
                    Some(precondition.server_id),
                ));
            }
            // `begin_shutdown` may have raced the initial check while this request was reading
            // Registry state. Re-check while holding the preview lock so shutdown cannot clear
            // the map and then have this request repopulate it.
            self.ensure_mutations_open(McpManagementOperationDto::PrepareLaunchAuthorization)?;
            previews.insert(
                authorization_id,
                FrozenLaunchAuthorization {
                    server_id: precondition.server_id,
                    precondition: precondition.clone(),
                    launch_spec_digest: record.launch_spec_digest.clone(),
                    file_identity_digest,
                    expires_at_ms,
                },
            );
        }
        Ok(McpLaunchAuthorizationPreview {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            authorization_id: authorization_id.to_string(),
            expires_at_ms,
            server_id: precondition.server_id.to_string(),
            display_name: record.entry.config.display_name.clone(),
            executable: path_text(
                &stdio.program,
                McpManagementOperationDto::PrepareLaunchAuthorization,
            )?,
            arguments: stdio.arguments.clone(),
            cwd: path_text(
                &stdio.cwd,
                McpManagementOperationDto::PrepareLaunchAuthorization,
            )?,
            launch_spec_digest: record.launch_spec_digest.to_string(),
            precondition: project_precondition(&record.entry),
        })
    }

    pub(crate) fn commit_launch_authorization(
        &self,
        input: McpLaunchAuthorizationCommitInput,
    ) -> Result<McpLaunchAuthorizationResult, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::CommitLaunchAuthorization)?;
        ensure_schema_version(
            input.schema_version,
            McpManagementOperationDto::CommitLaunchAuthorization,
        )?;
        let authorization_id = parse_uuid_v4(
            &input.authorization_id,
            McpManagementOperationDto::CommitLaunchAuthorization,
        )?;
        let preview_lookup_failure = || {
            self.failure(
                McpManagementOperationDto::CommitLaunchAuthorization,
                McpManagementErrorCodeDto::InternalSafeError,
                McpManagementRecoveryDto::Retry,
                "The launch authorization service is unavailable.",
                None,
            )
        };
        let preview_missing_failure = || {
            self.failure(
                McpManagementOperationDto::CommitLaunchAuthorization,
                McpManagementErrorCodeDto::AuthorizationStale,
                McpManagementRecoveryDto::Refresh,
                "The launch authorization preview is missing or has already been consumed.",
                None,
            )
        };
        let frozen_server_id = self
            .authorization_previews
            .lock()
            .map_err(|_| preview_lookup_failure())?
            .get(&authorization_id)
            .map(|preview| preview.server_id)
            .ok_or_else(preview_missing_failure)?;
        let _active_mutation = self.begin_server_mutation(
            frozen_server_id,
            McpManagementOperationDto::CommitLaunchAuthorization,
        )?;
        let frozen = self
            .authorization_previews
            .lock()
            .map_err(|_| preview_lookup_failure())?
            .remove(&authorization_id)
            .ok_or_else(preview_missing_failure)?;
        if frozen.expires_at_ms <= now_ms() {
            return Err(self.failure(
                McpManagementOperationDto::CommitLaunchAuthorization,
                McpManagementErrorCodeDto::AuthorizationStale,
                McpManagementRecoveryDto::Refresh,
                "The launch authorization preview expired.",
                Some(frozen.server_id),
            ));
        }
        let submitted = parse_precondition(
            &frozen.server_id.to_string(),
            &input.precondition,
            McpManagementOperationDto::CommitLaunchAuthorization,
        )?;
        if submitted != frozen.precondition {
            return Err(self.failure(
                McpManagementOperationDto::CommitLaunchAuthorization,
                McpManagementErrorCodeDto::Conflict,
                McpManagementRecoveryDto::Refresh,
                "The MCP server configuration changed before authorization.",
                Some(frozen.server_id),
            ));
        }
        self.registry
            .authorize_launch(
                &frozen.precondition,
                &frozen.launch_spec_digest,
                &frozen.file_identity_digest,
                MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
                i64::try_from(now_ms()).unwrap_or(i64::MAX),
            )
            .map_err(|error| {
                self.registry_failure(
                    McpManagementOperationDto::CommitLaunchAuthorization,
                    Some(frozen.server_id),
                    error,
                )
            })?;
        let server = self
            .details_for(
                frozen.server_id,
                McpManagementOperationDto::CommitLaunchAuthorization,
            )?
            .server;
        Ok(McpLaunchAuthorizationResult {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            authorized: true,
            server,
        })
    }

    pub(super) fn ensure_launch_authorized(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> Result<(), McpManagementFailure> {
        let record = self.persisted(server_id, operation)?;
        if !record.entry.config.enabled {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::InvalidState,
                McpManagementRecoveryDto::FixInput,
                "Enable the MCP server before starting it.",
                Some(server_id),
            ));
        }
        let valid = launch_authorization_is_valid(&record);
        if valid {
            Ok(())
        } else {
            Err(self.failure(
                operation,
                McpManagementErrorCodeDto::AuthorizationRequired,
                McpManagementRecoveryDto::RequestLaunchAuthorization,
                "The exact MCP launch configuration must be authorized before it can start.",
                Some(server_id),
            ))
        }
    }
}
