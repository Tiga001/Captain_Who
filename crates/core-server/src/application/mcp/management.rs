use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use mycopilot_mcp_client::{
    McpApprovalMode, McpCatalogCompleteness, McpCatalogDiagnosticKind, McpCatalogSnapshot,
    McpConfigDigest, McpConfigEpoch, McpConnectionManager, McpError, McpErrorKind, McpEvent,
    McpLifecycleKind, McpRegistryChangeKind, McpRegistryEntry, McpRegistryMutation,
    McpServerConfig, McpServerId, McpServerScope, McpServerState, McpStdioConfig,
    McpTransportConfig, McpTrustLevel,
};
use mycopilot_protocol_rs::{
    McpApprovalModeDto, McpCapabilityView, McpCatalogCompletenessDto, McpCatalogToolsPageInput,
    McpCatalogToolsPageOutput, McpChangedKindDto, McpChangedNotification, McpConnectionStateDto,
    McpLaunchAuthorizationCommitInput, McpLaunchAuthorizationPreview, McpLaunchAuthorizationResult,
    McpLaunchAuthorizationStateDto, McpManagementErrorCodeDto, McpManagementErrorData,
    McpManagementErrorTypeDto, McpManagementOperationDto, McpManagementRecoveryDto,
    McpProtocolLifecycleDto, McpProtocolView, McpSafeErrorView, McpServerCreateInput,
    McpServerDetailsOutput, McpServerDetailsView, McpServerIdInput, McpServerListInput,
    McpServerListItem, McpServerListOutput, McpServerMutationInput, McpServerMutationPrecondition,
    McpServerScopeDto, McpServerSourceDto, McpServerUpdateInput, McpToolSummaryView,
    McpTransportKindDto, McpTrustLevelDto, MCP_MANAGEMENT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::sqlite_registry::{
    compute_launch_spec_digest, launch_authorization_identity_is_valid,
    launch_authorization_is_valid, prepare_launch_file_identity, McpLaunchSpecDigest,
    McpPersistedRegistryRecord, McpRegistryMutationPrecondition, McpRegistryPersistenceError,
    SqliteMcpRegistry, MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
};

const MAX_DISPLAY_NAME_BYTES: usize = 256;
const MAX_PATH_BYTES: usize = 4096;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 16 * 1024;
const MAX_ARGUMENTS_TOTAL_BYTES: usize = 64 * 1024;
const MAX_TOOL_PAGE_SIZE: usize = 100;
const MAX_HOST_CURSOR_BYTES: usize = 4096;
const MAX_SAFE_DESCRIPTION_BYTES: usize = 1024;
const MAX_SAFE_ERROR_BYTES: usize = 4096;
const MAX_PROTOCOL_VERSION_BYTES: usize = 64;
const MAX_LAUNCH_AUTHORIZATION_PREVIEWS: usize = 128;
const LAUNCH_AUTHORIZATION_PREVIEW_TTL_MS: u64 = 5 * 60 * 1_000;
const CURSOR_SCHEMA_VERSION: u32 = 1;

#[derive(Clone)]
struct FrozenLaunchAuthorization {
    server_id: McpServerId,
    precondition: McpRegistryMutationPrecondition,
    launch_spec_digest: McpLaunchSpecDigest,
    file_identity_digest: McpLaunchSpecDigest,
    expires_at_ms: u64,
}

struct ActiveServerMutation<'a> {
    server_id: McpServerId,
    active_server_mutations: &'a Mutex<BTreeSet<McpServerId>>,
}

impl Drop for ActiveServerMutation<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.active_server_mutations.lock() {
            active.remove(&self.server_id);
        }
    }
}

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

pub(crate) struct McpManagementService {
    registry: Arc<SqliteMcpRegistry>,
    manager: Arc<McpConnectionManager>,
    source_epoch: String,
    authorization_previews: Mutex<BTreeMap<Uuid, FrozenLaunchAuthorization>>,
    active_server_mutations: Mutex<BTreeSet<McpServerId>>,
    shutdown_started: AtomicBool,
}

impl std::fmt::Debug for McpManagementService {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpManagementService")
            .field(
                "shutdown_started",
                &self.shutdown_started.load(Ordering::Acquire),
            )
            .finish_non_exhaustive()
    }
}

impl McpManagementService {
    pub(crate) fn new(
        registry: Arc<SqliteMcpRegistry>,
        manager: Arc<McpConnectionManager>,
    ) -> Self {
        Self {
            registry,
            manager,
            source_epoch: Uuid::new_v4().to_string(),
            authorization_previews: Mutex::new(BTreeMap::new()),
            active_server_mutations: Mutex::new(BTreeSet::new()),
            shutdown_started: AtomicBool::new(false),
        }
    }

    pub(crate) fn begin_shutdown(&self) {
        self.shutdown_started.store(true, Ordering::Release);
        if let Ok(mut previews) = self.authorization_previews.lock() {
            previews.clear();
        }
    }

    pub(crate) fn project_changed_notification(
        &self,
        sequence: u64,
        event: &McpEvent,
    ) -> Option<McpChangedNotification> {
        let (kind, server_id, state) = match event {
            McpEvent::RegistryChanged {
                kind, server_id, ..
            } => (
                match kind {
                    McpRegistryChangeKind::Added => McpChangedKindDto::Added,
                    McpRegistryChangeKind::Updated => McpChangedKindDto::Updated,
                    McpRegistryChangeKind::Removed => McpChangedKindDto::Deleted,
                    _ => return None,
                },
                *server_id,
                None,
            ),
            McpEvent::ServerStateChanged {
                server_id, current, ..
            } => (
                McpChangedKindDto::StateChanged,
                *server_id,
                Some(connection_state(*current)),
            ),
            McpEvent::CatalogChanged { server_id, .. } => {
                (McpChangedKindDto::CatalogChanged, *server_id, None)
            }
            McpEvent::ServerError { server_id, .. } | McpEvent::ServerExited { server_id, .. } => (
                McpChangedKindDto::StateChanged,
                *server_id,
                Some(McpConnectionStateDto::Error),
            ),
            McpEvent::RegistryReconciliationRequired { .. } => {
                return Some(self.project_resync_required_notification(sequence));
            }
            _ => return None,
        };
        let registry_revision = match self.registry.current_revision() {
            Ok(revision) => revision,
            Err(_) => return Some(self.project_resync_required_notification(sequence)),
        };
        Some(McpChangedNotification {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            source_epoch: self.source_epoch.clone(),
            sequence,
            registry_revision,
            kind,
            server_id: Some(server_id.to_string()),
            state,
        })
    }

    pub(crate) fn project_resync_required_notification(
        &self,
        sequence: u64,
    ) -> McpChangedNotification {
        McpChangedNotification {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            source_epoch: self.source_epoch.clone(),
            sequence,
            registry_revision: self.registry.current_revision().unwrap_or_default(),
            kind: McpChangedKindDto::ResyncRequired,
            server_id: None,
            state: None,
        }
    }

    pub(crate) fn list_servers(
        &self,
        input: McpServerListInput,
    ) -> Result<McpServerListOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, McpManagementOperationDto::List)?;
        let (registry_revision, records) = self
            .registry
            .snapshot()
            .map_err(|error| self.registry_failure(McpManagementOperationDto::List, None, error))?;
        let mut servers = records
            .iter()
            .map(|record| self.project_summary(record))
            .collect::<Result<Vec<_>, _>>()?;
        servers.sort_by(|left, right| left.server_id.cmp(&right.server_id));
        Ok(McpServerListOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            registry_revision,
            servers,
        })
    }

    pub(crate) fn get_server(
        &self,
        input: McpServerIdInput,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, operation)?;
        let server_id = parse_server_id(&input.server_id, operation)?;
        self.details_for(server_id, operation)
    }

    pub(crate) fn add_server(
        &self,
        input: McpServerCreateInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Add)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Add)?;
        validate_editable_fields(
            &input.display_name,
            &input.executable,
            &input.arguments,
            &input.cwd,
            McpManagementOperationDto::Add,
        )?;
        let config = McpServerConfig {
            id: McpServerId::new(),
            display_name: input.display_name,
            scope: McpServerScope::User,
            trust: McpTrustLevel::Untrusted,
            approval_mode: approval_mode(input.approval_mode),
            enabled: false,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: input.executable.into(),
                arguments: input.arguments,
                cwd: input.cwd.into(),
                environment: Vec::new(),
            }),
            connect_timeout_ms: McpServerConfig::default_connect_timeout_ms(),
            request_timeout_ms: McpServerConfig::default_request_timeout_ms(),
            shutdown_timeout_ms: McpServerConfig::default_shutdown_timeout_ms(),
        };
        let entry = self
            .registry
            .add_persisted(config)
            .map_err(|error| self.registry_failure(McpManagementOperationDto::Add, None, error))?;
        self.details_for(entry.config.id, McpManagementOperationDto::Add)
    }

    pub(crate) async fn update_server(
        &self,
        input: McpServerUpdateInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Update)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Update)?;
        validate_editable_fields(
            &input.display_name,
            &input.executable,
            &input.arguments,
            &input.cwd,
            McpManagementOperationDto::Update,
        )?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Update,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Update)?;
        let existing = self.persisted(precondition.server_id, McpManagementOperationDto::Update)?;
        ensure_registry_precondition(&existing.entry, &precondition).map_err(|_| {
            self.registry_failure(
                McpManagementOperationDto::Update,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        let config = McpServerConfig {
            id: precondition.server_id,
            display_name: input.display_name,
            scope: McpServerScope::User,
            trust: existing.entry.config.trust,
            approval_mode: approval_mode(input.approval_mode),
            enabled: existing.entry.config.enabled,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: input.executable.into(),
                arguments: input.arguments,
                cwd: input.cwd.into(),
                environment: Vec::new(),
            }),
            connect_timeout_ms: existing.entry.config.connect_timeout_ms,
            request_timeout_ms: existing.entry.config.request_timeout_ms,
            shutdown_timeout_ms: existing.entry.config.shutdown_timeout_ms,
        };
        let proposed_launch_digest = compute_launch_spec_digest(&config).map_err(|error| {
            self.registry_failure(
                McpManagementOperationDto::Update,
                Some(precondition.server_id),
                error,
            )
        })?;
        let launch_changed = proposed_launch_digest != existing.launch_spec_digest;
        let mutation = self
            .registry
            .update_with_precondition(&precondition, config)
            .map_err(|error| {
                self.registry_failure(
                    McpManagementOperationDto::Update,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        if launch_changed && !matches!(mutation, McpRegistryMutation::Unchanged(_)) {
            self.manager
                .stop(precondition.server_id)
                .await
                .map_err(|error| {
                    self.manager_failure(
                        McpManagementOperationDto::Update,
                        Some(precondition.server_id),
                        error,
                    )
                })?;
        }
        self.details_for(precondition.server_id, McpManagementOperationDto::Update)
    }

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

    pub(crate) fn enable_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Enable)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Enable)?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Enable,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Enable)?;
        self.set_enabled_with_precondition(&precondition, true, McpManagementOperationDto::Enable)
    }

    pub(crate) async fn disable_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Disable)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Disable)?;
        let precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Disable,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Disable)?;
        let output = self.set_enabled_with_precondition(
            &precondition,
            false,
            McpManagementOperationDto::Disable,
        )?;
        let server_id = parse_server_id(
            &output.server.summary.server_id,
            McpManagementOperationDto::Disable,
        )?;
        self.manager.stop(server_id).await.map_err(|error| {
            self.manager_failure(McpManagementOperationDto::Disable, Some(server_id), error)
        })?;
        self.details_for(server_id, McpManagementOperationDto::Disable)
    }

    pub(crate) async fn delete_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Delete)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Delete)?;
        let mut precondition = parse_precondition(
            &input.server_id,
            &input.precondition,
            McpManagementOperationDto::Delete,
        )?;
        let _active_mutation =
            self.begin_server_mutation(precondition.server_id, McpManagementOperationDto::Delete)?;
        let existing = self.persisted(precondition.server_id, McpManagementOperationDto::Delete)?;
        ensure_registry_precondition(&existing.entry, &precondition).map_err(|_| {
            self.registry_failure(
                McpManagementOperationDto::Delete,
                Some(precondition.server_id),
                McpRegistryPersistenceError::Conflict,
            )
        })?;
        if existing.entry.config.enabled {
            let disabled = self
                .registry
                .set_enabled(&precondition, false)
                .map_err(|error| {
                    self.registry_failure(
                        McpManagementOperationDto::Delete,
                        Some(precondition.server_id),
                        error,
                    )
                })?;
            precondition = McpRegistryMutationPrecondition::from_entry(&disabled);
        }
        self.manager
            .stop(precondition.server_id)
            .await
            .map_err(|error| {
                self.manager_failure(
                    McpManagementOperationDto::Delete,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        let before_delete =
            self.details_for(precondition.server_id, McpManagementOperationDto::Delete)?;
        let removed = self
            .registry
            .remove_with_precondition(&precondition)
            .map_err(|error| {
                self.registry_failure(
                    McpManagementOperationDto::Delete,
                    Some(precondition.server_id),
                    error,
                )
            })?;
        let revision = removed.revision;
        let mut server = before_delete.server;
        server.summary.registry_revision = revision;
        server.summary.enabled = false;
        server.summary.state = McpConnectionStateDto::Disabled;
        server.summary.updated_at_ms = now_ms();
        Ok(McpServerDetailsOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            registry_revision: revision,
            server,
        })
    }

    pub(crate) async fn start_server(
        &self,
        input: McpServerMutationInput,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(operation)?;
        ensure_schema_version(input.schema_version, operation)?;
        let mutation_server_id = parse_server_id(&input.server_id, operation)?;
        let _active_mutation = self.begin_server_mutation(mutation_server_id, operation)?;
        let server_id = self.validate_mutation_input(&input, operation)?;
        self.ensure_launch_authorized(server_id, operation)?;
        match operation {
            McpManagementOperationDto::Start => self.manager.start(server_id).await,
            McpManagementOperationDto::Restart => self.manager.restart(server_id).await,
            _ => {
                return Err(self.failure(
                    operation,
                    McpManagementErrorCodeDto::InvalidInput,
                    McpManagementRecoveryDto::FixInput,
                    "The MCP management operation is invalid.",
                    Some(server_id),
                ))
            }
        }
        .map_err(|error| self.manager_failure(operation, Some(server_id), error))?;
        self.details_for(server_id, operation)
    }

    pub(crate) async fn stop_server(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::Stop)?;
        ensure_schema_version(input.schema_version, McpManagementOperationDto::Stop)?;
        let mutation_server_id =
            parse_server_id(&input.server_id, McpManagementOperationDto::Stop)?;
        let _active_mutation =
            self.begin_server_mutation(mutation_server_id, McpManagementOperationDto::Stop)?;
        let server_id = self.validate_mutation_input(&input, McpManagementOperationDto::Stop)?;
        self.manager.stop(server_id).await.map_err(|error| {
            self.manager_failure(McpManagementOperationDto::Stop, Some(server_id), error)
        })?;
        self.details_for(server_id, McpManagementOperationDto::Stop)
    }

    pub(crate) fn list_tools(
        &self,
        input: McpCatalogToolsPageInput,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        ensure_schema_version(input.schema_version, McpManagementOperationDto::ListTools)?;
        let server_id = parse_server_id(&input.server_id, McpManagementOperationDto::ListTools)?;
        self.project_catalog_page(
            server_id,
            input.cursor.as_deref(),
            usize::try_from(input.limit).unwrap_or(usize::MAX),
            McpManagementOperationDto::ListTools,
        )
    }

    pub(crate) async fn refresh_catalog(
        &self,
        input: McpServerMutationInput,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        self.ensure_mutations_open(McpManagementOperationDto::RefreshCatalog)?;
        ensure_schema_version(
            input.schema_version,
            McpManagementOperationDto::RefreshCatalog,
        )?;
        let mutation_server_id =
            parse_server_id(&input.server_id, McpManagementOperationDto::RefreshCatalog)?;
        let _active_mutation = self.begin_server_mutation(
            mutation_server_id,
            McpManagementOperationDto::RefreshCatalog,
        )?;
        let server_id =
            self.validate_mutation_input(&input, McpManagementOperationDto::RefreshCatalog)?;
        self.manager.refresh(server_id).await.map_err(|error| {
            self.manager_failure(
                McpManagementOperationDto::RefreshCatalog,
                Some(server_id),
                error,
            )
        })?;
        self.project_catalog_page(
            server_id,
            None,
            MAX_TOOL_PAGE_SIZE,
            McpManagementOperationDto::RefreshCatalog,
        )
    }

    fn set_enabled_with_precondition(
        &self,
        precondition: &McpRegistryMutationPrecondition,
        enabled: bool,
        operation: McpManagementOperationDto,
    ) -> Result<McpServerDetailsOutput, McpManagementFailure> {
        self.registry
            .set_enabled(precondition, enabled)
            .map_err(|error| {
                self.registry_failure(operation, Some(precondition.server_id), error)
            })?;
        self.details_for(precondition.server_id, operation)
    }

    fn begin_server_mutation(
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

    fn validate_mutation_input(
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

    fn ensure_launch_authorized(
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

    fn details_for(
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

    fn project_summary(
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

    fn project_catalog_page(
        &self,
        server_id: McpServerId,
        cursor: Option<&str>,
        limit: usize,
        operation: McpManagementOperationDto,
    ) -> Result<McpCatalogToolsPageOutput, McpManagementFailure> {
        if limit == 0 || limit > MAX_TOOL_PAGE_SIZE {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP tool page size is invalid.",
                Some(server_id),
            ));
        }
        let before = self.persisted(server_id, operation)?;
        let catalog = self
            .manager
            .catalog(server_id)
            .map_err(|error| self.manager_failure(operation, Some(server_id), error))?;
        let after = self.persisted(server_id, operation)?;
        if !same_registry_identity(&before.entry, &after.entry) {
            return Err(self.stale_catalog_failure(server_id, operation));
        }
        let catalog = match catalog {
            Some(catalog) if catalog_matches_registry(&catalog, &after.entry) => catalog,
            Some(_) => return Err(self.stale_catalog_failure(server_id, operation)),
            None => {
                let mut empty = McpCatalogSnapshot::empty(server_id);
                empty.source_config_epoch = Some(after.entry.config_epoch);
                empty.source_registry_revision = Some(after.entry.revision);
                empty.source_config_digest = Some(after.entry.config_digest.clone());
                empty
            }
        };
        let offset = match cursor {
            Some(cursor) => decode_catalog_cursor(cursor, &catalog, operation)?,
            None => 0,
        };
        if offset > catalog.tools.len() {
            return Err(self.failure(
                operation,
                McpManagementErrorCodeDto::Conflict,
                McpManagementRecoveryDto::Refresh,
                "The MCP tool page cursor is stale.",
                Some(server_id),
            ));
        }
        let end = offset.saturating_add(limit).min(catalog.tools.len());
        let completeness = catalog_completeness(&catalog.completeness);
        let mut tools = Vec::with_capacity(end.saturating_sub(offset));
        for tool in &catalog.tools[offset..end] {
            let (description, description_truncated) = truncate_text(
                tool.descriptor.description.as_deref().unwrap_or_default(),
                MAX_SAFE_DESCRIPTION_BYTES,
            );
            let diagnostic_codes = catalog
                .diagnostics
                .iter()
                .filter(|diagnostic| {
                    diagnostic
                        .raw_names
                        .iter()
                        .any(|name| name == &tool.raw_name)
                        || diagnostic
                            .model_name
                            .as_ref()
                            .is_some_and(|name| name == &tool.model_name)
                })
                .map(|diagnostic| diagnostic_code(diagnostic.kind).to_string())
                .collect();
            tools.push(McpToolSummaryView {
                server_id: server_id.to_string(),
                raw_name: bounded_text(&tool.raw_name, 1024),
                model_name: bounded_text(&tool.model_name, 64),
                routable: tool.routable,
                disabled: !tool.routable,
                schema_digest_prefix: tool.schema_digest.as_str()[..12].to_string(),
                description,
                description_truncated,
                diagnostic_codes,
                catalog_generation: catalog.generation,
                catalog_completeness: completeness,
            });
        }
        let next_cursor = (end < catalog.tools.len())
            .then(|| encode_catalog_cursor(&catalog, end))
            .transpose()
            .map_err(|_| {
                self.failure(
                    operation,
                    McpManagementErrorCodeDto::InternalSafeError,
                    McpManagementRecoveryDto::Retry,
                    "The MCP tool page cursor could not be created.",
                    Some(server_id),
                )
            })?;
        Ok(McpCatalogToolsPageOutput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server_id.to_string(),
            catalog_generation: catalog.generation,
            catalog_completeness: completeness,
            tools,
            next_cursor,
        })
    }

    fn stale_catalog_failure(
        &self,
        server_id: McpServerId,
        operation: McpManagementOperationDto,
    ) -> McpManagementFailure {
        self.failure(
            operation,
            McpManagementErrorCodeDto::Conflict,
            McpManagementRecoveryDto::Refresh,
            "The MCP tool catalog is stale. Refresh before retrying.",
            Some(server_id),
        )
    }

    fn persisted(
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

    fn ensure_mutations_open(
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

    fn registry_failure(
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
            McpRegistryPersistenceError::StorageUnavailable
            | McpRegistryPersistenceError::RevisionExhausted => (
                McpManagementErrorCodeDto::InternalSafeError,
                McpManagementRecoveryDto::Retry,
                "MCP management storage is unavailable.",
            ),
        };
        self.failure(operation, code, recovery, message, server_id)
    }

    fn manager_failure(
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
            _ => (
                McpManagementErrorCodeDto::ServerError,
                McpManagementRecoveryDto::Retry,
                "The MCP server operation failed.",
            ),
        };
        self.failure(operation, code, recovery, message, server_id)
    }

    fn failure(
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

fn ensure_schema_version(
    schema_version: u32,
    operation: McpManagementOperationDto,
) -> Result<(), McpManagementFailure> {
    if schema_version == MCP_MANAGEMENT_SCHEMA_VERSION {
        Ok(())
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP management schema version is unsupported.",
            None,
        ))
    }
}

fn parse_server_id(
    value: &str,
    operation: McpManagementOperationDto,
) -> Result<McpServerId, McpManagementFailure> {
    McpServerId::from_str(value).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP server identity is invalid.",
            None,
        )
    })
}

fn parse_uuid_v4(
    value: &str,
    operation: McpManagementOperationDto,
) -> Result<Uuid, McpManagementFailure> {
    let id = Uuid::parse_str(value).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The launch authorization identity is invalid.",
            None,
        )
    })?;
    if id.get_version() == Some(uuid::Version::Random) && id.to_string() == value {
        Ok(id)
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The launch authorization identity is invalid.",
            None,
        ))
    }
}

fn parse_precondition(
    server_id: &str,
    input: &McpServerMutationPrecondition,
    operation: McpManagementOperationDto,
) -> Result<McpRegistryMutationPrecondition, McpManagementFailure> {
    let server_id = parse_server_id(server_id, operation)?;
    if input.expected_registry_revision == 0 {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP Registry revision is invalid.",
            Some(server_id),
        ));
    }
    let expected_config_epoch =
        McpConfigEpoch::from_str(&input.expected_config_epoch).map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP configuration epoch is invalid.",
                Some(server_id),
            )
        })?;
    let expected_config_digest =
        McpConfigDigest::from_str(&input.expected_config_digest).map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP configuration digest is invalid.",
                Some(server_id),
            )
        })?;
    Ok(McpRegistryMutationPrecondition {
        server_id,
        expected_revision: input.expected_registry_revision,
        expected_config_epoch,
        expected_config_digest,
    })
}

fn project_precondition(entry: &McpRegistryEntry) -> McpServerMutationPrecondition {
    McpServerMutationPrecondition {
        expected_registry_revision: entry.revision,
        expected_config_epoch: entry.config_epoch.to_string(),
        expected_config_digest: entry.config_digest.to_string(),
    }
}

fn ensure_registry_precondition(
    entry: &McpRegistryEntry,
    precondition: &McpRegistryMutationPrecondition,
) -> Result<(), ()> {
    if entry.config.id == precondition.server_id
        && entry.revision == precondition.expected_revision
        && entry.config_epoch == precondition.expected_config_epoch
        && entry.config_digest == precondition.expected_config_digest
    {
        Ok(())
    } else {
        Err(())
    }
}

fn same_registry_identity(left: &McpRegistryEntry, right: &McpRegistryEntry) -> bool {
    left.config.id == right.config.id
        && left.revision == right.revision
        && left.config_epoch == right.config_epoch
        && left.config_digest == right.config_digest
}

fn catalog_matches_registry(catalog: &McpCatalogSnapshot, entry: &McpRegistryEntry) -> bool {
    catalog.server_id == entry.config.id
        && catalog.source_config_epoch == Some(entry.config_epoch)
        && catalog.source_registry_revision == Some(entry.revision)
        && catalog.source_config_digest.as_ref() == Some(&entry.config_digest)
}

fn validate_editable_fields(
    display_name: &str,
    executable: &str,
    arguments: &[String],
    cwd: &str,
    operation: McpManagementOperationDto,
) -> Result<(), McpManagementFailure> {
    let arguments_bytes = arguments
        .iter()
        .try_fold(0usize, |total, argument| total.checked_add(argument.len()));
    let valid = !display_name.trim().is_empty()
        && display_name.len() <= MAX_DISPLAY_NAME_BYTES
        && !display_name.contains('\0')
        && !display_name.chars().any(is_unsafe_renderer_text_character)
        && !executable.is_empty()
        && executable.len() <= MAX_PATH_BYTES
        && !executable.contains('\0')
        && !cwd.is_empty()
        && cwd.len() <= MAX_PATH_BYTES
        && !cwd.contains('\0')
        && arguments.len() <= MAX_ARGUMENTS
        && arguments
            .iter()
            .all(|argument| argument.len() <= MAX_ARGUMENT_BYTES && !argument.contains('\0'))
        && arguments_bytes.is_some_and(|total| total <= MAX_ARGUMENTS_TOTAL_BYTES);
    if valid {
        Ok(())
    } else {
        Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP server configuration exceeds the allowed limits.",
            None,
        ))
    }
}

fn approval_mode(value: McpApprovalModeDto) -> McpApprovalMode {
    match value {
        McpApprovalModeDto::Prompt => McpApprovalMode::Prompt,
        McpApprovalModeDto::Deny => McpApprovalMode::Deny,
    }
}

fn connection_state(state: McpServerState) -> McpConnectionStateDto {
    match state {
        McpServerState::Disabled => McpConnectionStateDto::Disabled,
        McpServerState::Starting => McpConnectionStateDto::Starting,
        McpServerState::Discovering => McpConnectionStateDto::Discovering,
        McpServerState::Ready => McpConnectionStateDto::Ready,
        McpServerState::Stopping => McpConnectionStateDto::Stopping,
        McpServerState::Error => McpConnectionStateDto::Error,
        McpServerState::Backoff => McpConnectionStateDto::Backoff,
        McpServerState::Degraded => McpConnectionStateDto::Degraded,
        _ => McpConnectionStateDto::Error,
    }
}

fn catalog_completeness(value: &McpCatalogCompleteness) -> McpCatalogCompletenessDto {
    match value {
        McpCatalogCompleteness::Complete => McpCatalogCompletenessDto::Complete,
        McpCatalogCompleteness::Partial(_) => McpCatalogCompletenessDto::Partial,
        McpCatalogCompleteness::Stale(_) => McpCatalogCompletenessDto::Stale,
        McpCatalogCompleteness::Failed(_) => McpCatalogCompletenessDto::Failed,
        _ => McpCatalogCompletenessDto::Failed,
    }
}

fn diagnostic_code(kind: McpCatalogDiagnosticKind) -> &'static str {
    match kind {
        McpCatalogDiagnosticKind::InvalidRawName => "invalidRawName",
        McpCatalogDiagnosticKind::DescriptionLimitExceeded => "descriptionLimitExceeded",
        McpCatalogDiagnosticKind::InvalidSchema => "invalidSchema",
        McpCatalogDiagnosticKind::DuplicateRawName => "duplicateRawName",
        McpCatalogDiagnosticKind::NormalizationCollision => "normalizationCollision",
        McpCatalogDiagnosticKind::ModelNameCollision => "modelNameCollision",
        McpCatalogDiagnosticKind::ReservedModelName => "reservedModelName",
        _ => "unknown",
    }
}

fn path_text(
    path: &std::path::Path,
    operation: McpManagementOperationDto,
) -> Result<String, McpManagementFailure> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidState,
            McpManagementRecoveryDto::DoNotRetry,
            "The persisted MCP path cannot be represented safely.",
            None,
        )
    })
}

fn bounded_text(value: &str, max_bytes: usize) -> String {
    truncate_text(value, max_bytes).0
}

fn safe_error_code(value: &str) -> String {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
        && value.len() <= 128;
    if valid {
        value.to_string()
    } else {
        "mcp.serverError".to_string()
    }
}

fn truncate_text(value: &str, max_bytes: usize) -> (String, bool) {
    let mut sanitized_changed = false;
    let sanitized = value
        .chars()
        .map(|character| {
            if is_unsafe_renderer_text_character(character) {
                sanitized_changed = true;
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.len() <= max_bytes {
        return (sanitized, sanitized_changed);
    }
    let mut end = max_bytes.min(sanitized.len());
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    (sanitized[..end].to_string(), true)
}

fn is_unsafe_renderer_text_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1345f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

fn nonnegative_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CatalogCursor {
    schema_version: u32,
    server_id: String,
    generation: u64,
    catalog_digest: Option<String>,
    offset: usize,
}

fn encode_catalog_cursor(
    catalog: &McpCatalogSnapshot,
    offset: usize,
) -> Result<String, serde_json::Error> {
    let encoded = serde_json::to_vec(&CatalogCursor {
        schema_version: CURSOR_SCHEMA_VERSION,
        server_id: catalog.server_id.to_string(),
        generation: catalog.generation,
        catalog_digest: catalog.content_digest.as_ref().map(ToString::to_string),
        offset,
    })?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(encoded))
}

fn decode_catalog_cursor(
    cursor: &str,
    catalog: &McpCatalogSnapshot,
    operation: McpManagementOperationDto,
) -> Result<usize, McpManagementFailure> {
    if cursor.len() > MAX_HOST_CURSOR_BYTES {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP tool page cursor is invalid.",
            Some(catalog.server_id),
        ));
    }
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| {
            standalone_failure(
                operation,
                McpManagementErrorCodeDto::InvalidInput,
                McpManagementRecoveryDto::FixInput,
                "The MCP tool page cursor is invalid.",
                Some(catalog.server_id),
            )
        })?;
    let decoded: CatalogCursor = serde_json::from_slice(&bytes).map_err(|_| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidInput,
            McpManagementRecoveryDto::FixInput,
            "The MCP tool page cursor is invalid.",
            Some(catalog.server_id),
        )
    })?;
    if decoded.schema_version != CURSOR_SCHEMA_VERSION
        || decoded.server_id != catalog.server_id.to_string()
        || decoded.generation != catalog.generation
        || decoded.catalog_digest != catalog.content_digest.as_ref().map(ToString::to_string)
    {
        return Err(standalone_failure(
            operation,
            McpManagementErrorCodeDto::Conflict,
            McpManagementRecoveryDto::Refresh,
            "The MCP tool page cursor is stale.",
            Some(catalog.server_id),
        ));
    }
    Ok(decoded.offset)
}

fn standalone_failure(
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Arc;
    use std::time::Duration;

    use mycopilot_mcp_client::{
        BoxMcpFuture, McpCancellationToken, McpCapabilitySnapshot, McpConnectionState,
        McpConnector, McpManagerPolicy, McpPeer, McpProtocolSnapshot, McpRegistry, McpToolCall,
        McpToolDescriptor, McpToolPage, McpToolResult, NoopMcpEventSink,
    };
    use serde_json::{json, Value};
    use tempfile::TempDir;
    use tokio::sync::{oneshot, Notify};

    use super::*;

    struct RejectingConnector;

    impl McpConnector for RejectingConnector {
        fn connect<'a>(&'a self, _: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            Box::pin(async {
                Err(McpError::spawn(
                    "the management test connector never starts a process",
                ))
            })
        }
    }

    struct GatedRejectingConnector {
        entered: Mutex<Option<oneshot::Sender<()>>>,
        release: Arc<Notify>,
    }

    impl McpConnector for GatedRejectingConnector {
        fn connect<'a>(&'a self, _: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            let entered = self
                .entered
                .lock()
                .expect("gated connector entry lock")
                .take();
            let release = Arc::clone(&self.release);
            Box::pin(async move {
                if let Some(entered) = entered {
                    let _ = entered.send(());
                }
                release.notified().await;
                Err(McpError::spawn(
                    "the gated management test connector never starts a process",
                ))
            })
        }
    }

    struct CatalogFixturePeer {
        server_id: McpServerId,
        protocol: McpProtocolSnapshot,
    }

    impl CatalogFixturePeer {
        fn new(server_id: McpServerId) -> Self {
            Self {
                server_id,
                protocol: McpProtocolSnapshot {
                    negotiated_version: "2026-07-28".to_string(),
                    lifecycle: McpLifecycleKind::Discover,
                    server: None,
                    capabilities: McpCapabilitySnapshot {
                        tools: true,
                        ..McpCapabilitySnapshot::default()
                    },
                },
            }
        }
    }

    impl McpPeer for CatalogFixturePeer {
        fn server_id(&self) -> McpServerId {
            self.server_id
        }

        fn connection_state(&self) -> McpConnectionState {
            McpConnectionState::Ready
        }

        fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
            &self.protocol
        }

        fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
            Box::pin(async move {
                if cursor.is_some() {
                    return Err(McpError::protocol(
                        "the management Catalog fixture has one page",
                    ));
                }
                Ok(McpToolPage {
                    tools: vec![McpToolDescriptor {
                        name: "stale_fixture_tool".to_string(),
                        title: None,
                        description: Some("A bounded management Catalog fixture.".to_string()),
                        input_schema: json!({"type": "object"}),
                        output_schema: None,
                        annotations: None,
                    }],
                    next_cursor: None,
                    ttl_ms: None,
                    cache_scope: None,
                })
            })
        }

        fn call_tool<'a>(
            &'a self,
            _call: McpToolCall,
            _cancellation: McpCancellationToken,
        ) -> BoxMcpFuture<'a, McpToolResult> {
            Box::pin(async {
                Err(McpError::protocol(
                    "the management Catalog fixture does not execute tools",
                ))
            })
        }

        fn close(&self) -> BoxMcpFuture<'_, ()> {
            Box::pin(async { Ok(()) })
        }
    }

    struct CatalogFixtureConnector;

    impl McpConnector for CatalogFixtureConnector {
        fn connect<'a>(
            &'a self,
            config: &'a McpServerConfig,
        ) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
            let server_id = config.id;
            Box::pin(
                async move { Ok(Arc::new(CatalogFixturePeer::new(server_id)) as Arc<dyn McpPeer>) },
            )
        }
    }

    struct TestHarness {
        _database_directory: TempDir,
        registry: Arc<SqliteMcpRegistry>,
        manager: Arc<McpConnectionManager>,
        service: Arc<McpManagementService>,
    }

    impl TestHarness {
        fn new() -> Self {
            Self::with_connector(Arc::new(RejectingConnector))
        }

        fn with_connector(connector: Arc<dyn McpConnector>) -> Self {
            let database_directory = tempfile::tempdir().expect("temporary database directory");
            let registry = Arc::new(
                SqliteMcpRegistry::open(database_directory.path().join("mcp-registry.sqlite3"))
                    .expect("open test MCP Registry"),
            );
            let registry_for_manager: Arc<dyn McpRegistry> = registry.clone();
            let manager = Arc::new(
                McpConnectionManager::new(
                    registry_for_manager,
                    connector,
                    Arc::new(NoopMcpEventSink),
                    McpManagerPolicy::default(),
                )
                .expect("construct test MCP connection manager"),
            );
            let service = Arc::new(McpManagementService::new(registry.clone(), manager.clone()));
            Self {
                _database_directory: database_directory,
                registry,
                manager,
                service,
            }
        }

        async fn shutdown(&self) {
            self.service.begin_shutdown();
            let report = self.manager.shutdown(Duration::from_millis(100)).await;
            assert!(
                report.cleanup_complete,
                "test MCP Manager cleanup must complete: {report:?}"
            );
        }
    }

    fn create_input(display_name: &str) -> McpServerCreateInput {
        McpServerCreateInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            display_name: display_name.to_string(),
            transport: McpTransportKindDto::Stdio,
            executable: "/usr/bin/false".to_string(),
            arguments: vec!["--fixture".to_string(), String::new()],
            cwd: "/tmp".to_string(),
            approval_mode: McpApprovalModeDto::Prompt,
        }
    }

    fn mutation_input(server: &McpServerDetailsView) -> McpServerMutationInput {
        McpServerMutationInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server.summary.server_id.clone(),
            precondition: precondition(server),
        }
    }

    fn precondition(server: &McpServerDetailsView) -> McpServerMutationPrecondition {
        McpServerMutationPrecondition {
            expected_registry_revision: server.summary.registry_revision,
            expected_config_epoch: server.summary.config_epoch.clone(),
            expected_config_digest: server.summary.config_digest.clone(),
        }
    }

    fn update_input(
        server: &McpServerDetailsView,
        display_name: &str,
        arguments: Vec<String>,
    ) -> McpServerUpdateInput {
        McpServerUpdateInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server.summary.server_id.clone(),
            precondition: precondition(server),
            display_name: display_name.to_string(),
            transport: McpTransportKindDto::Stdio,
            executable: server.executable.clone(),
            arguments,
            cwd: server.cwd.clone(),
            approval_mode: server.summary.approval_mode,
        }
    }

    fn commit_input(preview: &McpLaunchAuthorizationPreview) -> McpLaunchAuthorizationCommitInput {
        McpLaunchAuthorizationCommitInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            authorization_id: preview.authorization_id.clone(),
            precondition: preview.precondition.clone(),
        }
    }

    fn authorize(
        service: &McpManagementService,
        server: &McpServerDetailsView,
    ) -> McpLaunchAuthorizationResult {
        let preview = service
            .prepare_launch_authorization(mutation_input(server))
            .expect("prepare exact launch authorization");
        service
            .commit_launch_authorization(commit_input(&preview))
            .expect("commit exact launch authorization")
    }

    fn server_id(server: &McpServerDetailsView) -> McpServerId {
        McpServerId::from_str(&server.summary.server_id).expect("valid server ID")
    }

    fn failure_code(error: McpManagementFailure) -> McpManagementErrorCodeDto {
        error.into_data().code
    }

    fn assert_renderer_safe_json(value: &Value) {
        const FORBIDDEN_KEYS: &[&str] = &[
            "environment",
            "env",
            "secretref",
            "token",
            "bearer",
            "header",
            "headers",
            "ciphertext",
            "nonce",
            "stderr",
            "rawarguments",
            "rawresult",
        ];
        match value {
            Value::Object(object) => {
                for (key, child) in object {
                    let normalized = key.to_ascii_lowercase();
                    assert!(
                        !FORBIDDEN_KEYS.contains(&normalized.as_str()),
                        "unsafe Renderer field was serialized: {key}"
                    );
                    assert_renderer_safe_json(child);
                }
            }
            Value::Array(values) => {
                for child in values {
                    assert_renderer_safe_json(child);
                }
            }
            _ => {}
        }
    }

    #[tokio::test]
    async fn add_is_disabled_untrusted_prompt_and_projects_only_safe_fields() {
        let harness = TestHarness::new();
        let output = harness
            .service
            .add_server(create_input("safe fixture"))
            .expect("add server");

        assert!(!output.server.summary.enabled);
        assert_eq!(output.server.summary.trust, McpTrustLevelDto::Untrusted);
        assert_eq!(
            output.server.summary.approval_mode,
            McpApprovalModeDto::Prompt
        );
        assert_eq!(
            output.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Required
        );
        assert_eq!(output.server.summary.state, McpConnectionStateDto::Disabled);

        let persisted = harness
            .registry
            .get_persisted(server_id(&output.server))
            .expect("read persisted server")
            .expect("server exists");
        let McpTransportConfig::Stdio(stdio) = &persisted.entry.config.transport else {
            panic!("Round 5A Registry only accepts stdio");
        };
        assert!(stdio.environment.is_empty());
        assert!(persisted.launch_authorization.is_none());

        let serialized = serde_json::to_value(&output).expect("serialize safe details DTO");
        assert_renderer_safe_json(&serialized);
        let serialized_text = serde_json::to_string(&serialized).expect("serialize safe JSON");
        for forbidden in [
            "MCP_TEST_TOKEN_CANARY",
            "MCP_TEST_HEADER_CANARY",
            "MCP_TEST_CIPHERTEXT_CANARY",
        ] {
            assert!(!serialized_text.contains(forbidden));
        }

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn launch_authorization_preview_is_one_shot_and_commit_is_cas_bound() {
        let harness = TestHarness::new();
        let added = harness
            .service
            .add_server(create_input("authorization fixture"))
            .expect("add server");

        let stale_preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&added.server))
            .expect("prepare stale preview");
        let renamed = harness
            .service
            .update_server(update_input(
                &added.server,
                "renamed authorization fixture",
                added.server.arguments.clone(),
            ))
            .await
            .expect("rename server");

        let stale_error = harness
            .service
            .commit_launch_authorization(commit_input(&stale_preview))
            .expect_err("stale Registry identity must fail authorization");
        assert_eq!(
            failure_code(stale_error),
            McpManagementErrorCodeDto::Conflict
        );
        let consumed_error = harness
            .service
            .commit_launch_authorization(commit_input(&stale_preview))
            .expect_err("failed preview is still consumed exactly once");
        assert_eq!(
            failure_code(consumed_error),
            McpManagementErrorCodeDto::AuthorizationStale
        );

        let preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&renamed.server))
            .expect("prepare current preview");
        let committed = harness
            .service
            .commit_launch_authorization(commit_input(&preview))
            .expect("commit current preview");
        assert!(committed.authorized);
        assert_eq!(
            committed.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Authorized
        );
        let replay_error = harness
            .service
            .commit_launch_authorization(commit_input(&preview))
            .expect_err("authorization preview cannot be replayed");
        assert_eq!(
            failure_code(replay_error),
            McpManagementErrorCodeDto::AuthorizationStale
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn launch_authorization_commit_rejects_file_identity_drift_and_details_show_stale() {
        let owned_launch = tempfile::tempdir().expect("temporary owned launch directory");
        let executable = owned_launch.path().join("owned-mcp-fixture");
        std::fs::write(&executable, b"owned executable version one")
            .expect("write repository-owned launch fixture");
        let mut input = create_input("physical identity fixture");
        input.executable = executable.to_string_lossy().into_owned();
        input.cwd = owned_launch.path().to_string_lossy().into_owned();

        let harness = TestHarness::new();
        let added = harness.service.add_server(input).expect("add server");
        let stale_preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&added.server))
            .expect("prepare launch authorization");
        std::fs::write(
            &executable,
            b"owned executable version two with different length",
        )
        .expect("replace repository-owned launch fixture");
        let conflict = harness
            .service
            .commit_launch_authorization(commit_input(&stale_preview))
            .expect_err("file drift during native confirmation must fail");
        assert_eq!(failure_code(conflict), McpManagementErrorCodeDto::Conflict);

        let current = harness
            .service
            .get_server(
                McpServerIdInput {
                    schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                    server_id: added.server.summary.server_id.clone(),
                },
                McpManagementOperationDto::Get,
            )
            .expect("read current server details");
        let authorized = authorize(&harness.service, &current.server);
        assert_eq!(
            authorized.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Authorized
        );
        std::fs::write(&executable, b"third owned executable replacement")
            .expect("replace authorized repository launch fixture");
        let stale = harness
            .service
            .get_server(
                McpServerIdInput {
                    schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                    server_id: added.server.summary.server_id,
                },
                McpManagementOperationDto::Get,
            )
            .expect("read stale launch authorization details");
        assert_eq!(
            stale.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Stale
        );

        harness.shutdown().await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn launch_authorization_preview_shows_the_canonical_process_plan() {
        use std::os::unix::fs::symlink;

        let owned_launch = tempfile::tempdir().expect("temporary owned launch directory");
        let executable_target = owned_launch.path().join("owned-executable-target");
        let executable_link = owned_launch.path().join("owned-executable-link");
        let script_target = owned_launch.path().join("owned-script-target.js");
        let script_link = owned_launch.path().join("owned-script-link.js");
        std::fs::write(&executable_target, b"owned executable target").unwrap();
        std::fs::write(&script_target, b"owned script target").unwrap();
        symlink(&executable_target, &executable_link).unwrap();
        symlink(&script_target, &script_link).unwrap();
        let mut input = create_input("canonical preview fixture");
        input.executable = executable_link.to_string_lossy().into_owned();
        input.arguments = vec![script_link.to_string_lossy().into_owned()];
        input.cwd = owned_launch.path().to_string_lossy().into_owned();

        let harness = TestHarness::new();
        let added = harness.service.add_server(input).expect("add server");
        let preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&added.server))
            .expect("prepare canonical launch preview");
        assert_eq!(
            preview.executable,
            std::fs::canonicalize(executable_target)
                .unwrap()
                .to_string_lossy()
        );
        assert_eq!(
            preview.arguments,
            vec![std::fs::canonicalize(script_target)
                .unwrap()
                .to_string_lossy()
                .into_owned()]
        );
        assert_eq!(
            preview.cwd,
            std::fs::canonicalize(owned_launch.path())
                .unwrap()
                .to_string_lossy()
        );
        harness.shutdown().await;
    }

    #[tokio::test]
    async fn newest_launch_authorization_preview_atomically_replaces_older_preview() {
        let harness = TestHarness::new();
        let added = harness
            .service
            .add_server(create_input("bounded authorization fixture"))
            .expect("add server");

        let mut previews = Vec::new();
        for _ in 0..8 {
            previews.push(
                harness
                    .service
                    .prepare_launch_authorization(mutation_input(&added.server))
                    .expect("replacement preview"),
            );
        }
        assert_eq!(
            harness
                .service
                .authorization_previews
                .lock()
                .expect("preview lock")
                .len(),
            1
        );
        for stale in &previews[..previews.len() - 1] {
            let failure = harness
                .service
                .commit_launch_authorization(commit_input(stale))
                .expect_err("a superseded preview must fail closed");
            assert_eq!(
                failure_code(failure),
                McpManagementErrorCodeDto::AuthorizationStale
            );
        }
        harness
            .service
            .commit_launch_authorization(commit_input(previews.last().expect("latest preview")))
            .expect("the newest preview remains one-shot usable");

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn add_maps_registry_capacity_to_policy_denied_without_publishing_an_event() {
        let harness = TestHarness::new();
        for index in 0..super::super::sqlite_registry::MCP_REGISTRY_MAX_SERVERS {
            harness
                .service
                .add_server(create_input(&format!("capacity fixture {index}")))
                .expect("add within the Registry capacity");
        }
        let mut changes = harness.registry.subscribe();
        let failure = harness
            .service
            .add_server(create_input("over capacity"))
            .expect_err("the Host capacity policy must reject another Server")
            .into_data();
        assert_eq!(failure.code, McpManagementErrorCodeDto::PolicyDenied);
        assert_eq!(failure.recovery, McpManagementRecoveryDto::DoNotRetry);
        assert_eq!(
            failure.current_registry_revision,
            Some(super::super::sqlite_registry::MCP_REGISTRY_MAX_SERVERS as u64)
        );
        let (revision, records) = harness.registry.snapshot().unwrap();
        assert_eq!(
            revision,
            super::super::sqlite_registry::MCP_REGISTRY_MAX_SERVERS as u64
        );
        assert_eq!(
            records.len(),
            super::super::sqlite_registry::MCP_REGISTRY_MAX_SERVERS
        );
        let no_event = tokio::time::timeout(Duration::from_millis(20), changes.recv()).await;
        assert!(no_event.is_err());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn launch_authorization_previews_have_global_hard_admission() {
        let harness = TestHarness::new();
        for index in 0..MAX_LAUNCH_AUTHORIZATION_PREVIEWS {
            let added = harness
                .service
                .add_server(create_input(&format!("bounded fixture {index}")))
                .expect("add server within global preview admission");
            harness
                .service
                .prepare_launch_authorization(mutation_input(&added.server))
                .expect("prepare preview within global admission");
        }
        let overflow = harness
            .service
            .add_server(create_input("overflow fixture"))
            .expect("add overflow server without authorizing it");
        let denied = harness
            .service
            .prepare_launch_authorization(mutation_input(&overflow.server))
            .expect_err("global authorization preview storm must be rejected");
        let data = denied.into_data();
        assert_eq!(data.code, McpManagementErrorCodeDto::PolicyDenied);
        assert_eq!(data.recovery, McpManagementRecoveryDto::Retry);
        assert!(!data.message.contains("overflow fixture"));
        assert_eq!(
            harness
                .service
                .authorization_previews
                .lock()
                .expect("preview lock")
                .len(),
            MAX_LAUNCH_AUTHORIZATION_PREVIEWS
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn one_server_mutation_is_exclusive_across_await_but_other_servers_continue() {
        let (entered_sender, entered_receiver) = oneshot::channel();
        let release = Arc::new(Notify::new());
        let harness = TestHarness::with_connector(Arc::new(GatedRejectingConnector {
            entered: Mutex::new(Some(entered_sender)),
            release: Arc::clone(&release),
        }));
        let added = harness
            .service
            .add_server(create_input("gated mutation fixture"))
            .expect("add gated server");
        let authorization_preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&added.server))
            .expect("prepare launch authorization");
        let authorized = harness
            .service
            .commit_launch_authorization(commit_input(&authorization_preview))
            .expect("authorize launch");
        let second_preview = harness
            .service
            .prepare_launch_authorization(mutation_input(&authorized.server))
            .expect("prepare a second one-shot preview before enabling");
        let enabled = harness
            .service
            .enable_server(mutation_input(&authorized.server))
            .expect("enable authorized server");

        let start_service = Arc::clone(&harness.service);
        let start_input = mutation_input(&enabled.server);
        let start = tokio::spawn(async move {
            start_service
                .start_server(start_input, McpManagementOperationDto::Start)
                .await
        });
        entered_receiver
            .await
            .expect("start reaches the gated connector");

        let update_error = harness
            .service
            .update_server(update_input(
                &enabled.server,
                "blocked rename",
                enabled.server.arguments.clone(),
            ))
            .await
            .expect_err("update cannot overlap start for the same Server");
        assert_eq!(
            failure_code(update_error),
            McpManagementErrorCodeDto::Conflict
        );
        let delete_error = harness
            .service
            .delete_server(mutation_input(&enabled.server))
            .await
            .expect_err("delete cannot overlap start for the same Server");
        assert_eq!(
            failure_code(delete_error),
            McpManagementErrorCodeDto::Conflict
        );
        let enable_error = harness
            .service
            .enable_server(mutation_input(&enabled.server))
            .expect_err("enable cannot overlap start for the same Server");
        assert_eq!(
            failure_code(enable_error),
            McpManagementErrorCodeDto::Conflict
        );
        let disable_error = harness
            .service
            .disable_server(mutation_input(&enabled.server))
            .await
            .expect_err("disable cannot overlap start for the same Server");
        assert_eq!(
            failure_code(disable_error),
            McpManagementErrorCodeDto::Conflict
        );
        for operation in [
            McpManagementOperationDto::Start,
            McpManagementOperationDto::Restart,
        ] {
            let error = harness
                .service
                .start_server(mutation_input(&enabled.server), operation)
                .await
                .expect_err("start/restart cannot overlap another start");
            assert_eq!(failure_code(error), McpManagementErrorCodeDto::Conflict);
        }
        let stop_error = harness
            .service
            .stop_server(mutation_input(&enabled.server))
            .await
            .expect_err("stop cannot overlap start for the same Server");
        assert_eq!(
            failure_code(stop_error),
            McpManagementErrorCodeDto::Conflict
        );
        let refresh_error = harness
            .service
            .refresh_catalog(mutation_input(&enabled.server))
            .await
            .expect_err("refresh cannot overlap start for the same Server");
        assert_eq!(
            failure_code(refresh_error),
            McpManagementErrorCodeDto::Conflict
        );
        let commit_error = harness
            .service
            .commit_launch_authorization(commit_input(&second_preview))
            .expect_err("authorization commit cannot overlap start for the same Server");
        assert_eq!(
            failure_code(commit_error),
            McpManagementErrorCodeDto::Conflict
        );

        let other = harness
            .service
            .add_server(create_input("independent mutation fixture"))
            .expect("add independent server");
        harness
            .service
            .stop_server(mutation_input(&other.server))
            .await
            .expect("a different Server is not blocked by the gated mutation");

        release.notify_one();
        let start_error = start
            .await
            .expect("join gated start")
            .expect_err("gated connector deliberately rejects without spawning");
        assert_eq!(
            failure_code(start_error),
            McpManagementErrorCodeDto::ServerError
        );
        harness
            .service
            .stop_server(mutation_input(&enabled.server))
            .await
            .expect("RAII admission is released after the awaited start fails");
        assert!(harness
            .service
            .active_server_mutations
            .lock()
            .expect("active mutation lock")
            .is_empty());

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn enable_requires_exact_authorization_and_preserves_authorization_identity() {
        let harness = TestHarness::new();
        let added = harness
            .service
            .add_server(create_input("enable fixture"))
            .expect("add server");

        let denied = harness
            .service
            .enable_server(mutation_input(&added.server))
            .expect_err("untrusted server cannot be enabled");
        assert_eq!(
            failure_code(denied),
            McpManagementErrorCodeDto::AuthorizationRequired
        );

        let authorized = authorize(&harness.service, &added.server);
        let id = server_id(&authorized.server);
        let authorized_record = harness
            .registry
            .get_persisted(id)
            .expect("read authorized server")
            .expect("authorized server exists");
        let authorization = authorized_record
            .launch_authorization
            .clone()
            .expect("launch authorization exists");
        assert_eq!(
            authorization.authored_config_epoch,
            authorized_record.entry.config_epoch
        );
        assert_eq!(
            authorization.authored_config_digest,
            authorized_record.entry.config_digest
        );

        let enabled = harness
            .service
            .enable_server(mutation_input(&authorized.server))
            .expect("enable authorized server");
        assert!(enabled.server.summary.enabled);
        assert_eq!(enabled.server.summary.trust, McpTrustLevelDto::UserApproved);
        assert_eq!(
            enabled.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Authorized
        );

        let enabled_record = harness
            .registry
            .get_persisted(id)
            .expect("read enabled server")
            .expect("enabled server exists");
        let enabled_authorization = enabled_record
            .launch_authorization
            .expect("authorization is retained");
        assert_eq!(
            enabled_record.entry.config_epoch, enabled_authorization.authored_config_epoch,
            "Host mutations rebind exact authorization to the committed config identity"
        );
        assert_eq!(
            enabled_record.entry.config_digest,
            enabled_authorization.authored_config_digest
        );
        assert_ne!(
            enabled_authorization.authored_config_epoch,
            authorization.authored_config_epoch
        );
        assert_ne!(
            enabled_authorization.authored_config_digest,
            authorization.authored_config_digest
        );
        assert_eq!(
            enabled_authorization.launch_spec_digest,
            enabled_record.launch_spec_digest
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn start_and_restart_require_enabled_state_before_launch_authorization() {
        let harness = TestHarness::new();
        let added = harness
            .service
            .add_server(create_input("disabled start fixture"))
            .expect("add server");
        let authorized = authorize(&harness.service, &added.server);
        assert!(!authorized.server.summary.enabled);
        assert_eq!(
            authorized.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Authorized
        );

        for operation in [
            McpManagementOperationDto::Start,
            McpManagementOperationDto::Restart,
        ] {
            let failure = harness
                .service
                .start_server(mutation_input(&authorized.server), operation)
                .await
                .expect_err("an authorized but disabled Server must be enabled first")
                .into_data();
            assert_eq!(failure.code, McpManagementErrorCodeDto::InvalidState);
            assert_eq!(failure.recovery, McpManagementRecoveryDto::FixInput);
            assert!(failure.message.contains("Enable"));
        }

        harness.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn catalog_projection_rejects_snapshot_from_previous_registry_identity() {
        let harness = TestHarness::with_connector(Arc::new(CatalogFixtureConnector));
        let added = harness
            .service
            .add_server(create_input("Catalog identity fixture"))
            .expect("add server");
        let authorized = authorize(&harness.service, &added.server);
        let enabled = harness
            .service
            .enable_server(mutation_input(&authorized.server))
            .expect("enable server");
        let started = harness
            .service
            .start_server(
                mutation_input(&enabled.server),
                McpManagementOperationDto::Start,
            )
            .await
            .expect("start bounded Catalog fixture");
        let server_id = server_id(&started.server);
        let page_input = McpCatalogToolsPageInput {
            schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
            server_id: server_id.to_string(),
            cursor: None,
            limit: 25,
        };
        let current = harness
            .service
            .list_tools(page_input.clone())
            .expect("current Catalog identity is projectable");
        assert_eq!(current.tools.len(), 1);
        assert_eq!(current.tools[0].raw_name, "stale_fixture_tool");

        let persisted = harness
            .registry
            .get_persisted(server_id)
            .expect("read current Registry identity")
            .expect("server exists");
        let mut changed_config = persisted.entry.config.clone();
        changed_config.display_name = "Catalog identity changed".to_string();
        let mutation = harness
            .registry
            .update_with_precondition(
                &McpRegistryMutationPrecondition::from_entry(&persisted.entry),
                changed_config,
            )
            .expect("commit a newer Registry identity");
        let McpRegistryMutation::Updated(changed) = mutation else {
            panic!("display name change must update Registry identity");
        };
        assert_ne!(changed.config_epoch, persisted.entry.config_epoch);
        assert_ne!(changed.revision, persisted.entry.revision);
        assert_ne!(changed.config_digest, persisted.entry.config_digest);

        // This synchronous read intentionally occurs before the Manager watcher can
        // invalidate its old snapshot. The Host boundary must reject that snapshot
        // rather than returning descriptors produced by the prior Registry identity.
        let failure = harness
            .service
            .list_tools(page_input)
            .expect_err("stale Catalog descriptors must fail closed")
            .into_data();
        assert_eq!(failure.code, McpManagementErrorCodeDto::Conflict);
        assert_eq!(failure.recovery, McpManagementRecoveryDto::Refresh);

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn non_launch_update_preserves_authorization_but_launch_update_revokes_it() {
        let harness = TestHarness::new();
        let added = harness
            .service
            .add_server(create_input("update fixture"))
            .expect("add server");
        let authorized = authorize(&harness.service, &added.server);
        let enabled = harness
            .service
            .enable_server(mutation_input(&authorized.server))
            .expect("enable server without creating a Manager connection");
        let id = server_id(&enabled.server);
        let before_rename = harness
            .registry
            .get_persisted(id)
            .expect("read enabled server")
            .expect("enabled server exists");
        let original_authorization = before_rename
            .launch_authorization
            .clone()
            .expect("authorization exists");

        let renamed = harness
            .service
            .update_server(update_input(
                &enabled.server,
                "renamed update fixture",
                enabled.server.arguments.clone(),
            ))
            .await
            .expect("non-launch update");
        let after_rename = harness
            .registry
            .get_persisted(id)
            .expect("read renamed server")
            .expect("renamed server exists");
        assert!(renamed.server.summary.enabled);
        assert_eq!(
            renamed.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Authorized
        );
        assert_ne!(
            after_rename.entry.config_epoch,
            before_rename.entry.config_epoch
        );
        assert_ne!(
            after_rename.entry.config_digest,
            before_rename.entry.config_digest
        );
        assert_eq!(
            after_rename.launch_spec_digest,
            before_rename.launch_spec_digest
        );
        let rebound_authorization = after_rename
            .launch_authorization
            .as_ref()
            .expect("non-launch update retains authorization");
        assert_eq!(
            rebound_authorization.authored_config_epoch,
            after_rename.entry.config_epoch
        );
        assert_eq!(
            rebound_authorization.authored_config_digest,
            after_rename.entry.config_digest
        );
        assert_ne!(
            rebound_authorization.authored_config_epoch,
            original_authorization.authored_config_epoch
        );
        assert_eq!(
            rebound_authorization.launch_spec_digest,
            original_authorization.launch_spec_digest
        );

        let launch_changed = harness
            .service
            .update_server(update_input(
                &renamed.server,
                "renamed update fixture",
                vec![
                    "--different-fixture".to_string(),
                    String::new(),
                    "--tail".to_string(),
                ],
            ))
            .await
            .expect("launch update must stop cleanly even without a prior Manager entry");
        let after_launch_change = harness
            .registry
            .get_persisted(id)
            .expect("read launch-updated server")
            .expect("launch-updated server exists");
        assert!(!launch_changed.server.summary.enabled);
        assert_eq!(
            launch_changed.server.summary.trust,
            McpTrustLevelDto::Untrusted
        );
        assert_eq!(
            launch_changed.server.summary.launch_authorization_state,
            McpLaunchAuthorizationStateDto::Required
        );
        assert!(after_launch_change.launch_authorization.is_none());
        assert_ne!(
            after_launch_change.launch_spec_digest,
            before_rename.launch_spec_digest
        );

        harness.shutdown().await;
    }

    #[tokio::test]
    async fn disable_and_delete_succeed_without_a_preexisting_manager_entry() {
        let harness = TestHarness::new();

        let disable_added = harness
            .service
            .add_server(create_input("disable fixture"))
            .expect("add disable fixture");
        let disable_authorized = authorize(&harness.service, &disable_added.server);
        let disable_enabled = harness
            .service
            .enable_server(mutation_input(&disable_authorized.server))
            .expect("enable without starting");
        let disabled = harness
            .service
            .disable_server(mutation_input(&disable_enabled.server))
            .await
            .expect("disable must construct and stop an inert Manager entry");
        assert!(!disabled.server.summary.enabled);
        assert_eq!(
            disabled.server.summary.state,
            McpConnectionStateDto::Disabled
        );

        let delete_added = harness
            .service
            .add_server(create_input("delete fixture"))
            .expect("add delete fixture");
        let delete_authorized = authorize(&harness.service, &delete_added.server);
        let delete_enabled = harness
            .service
            .enable_server(mutation_input(&delete_authorized.server))
            .expect("enable without starting");
        let delete_id = server_id(&delete_enabled.server);
        let deleted = harness
            .service
            .delete_server(mutation_input(&delete_enabled.server))
            .await
            .expect("delete must stop an inert Manager entry before removing Registry state");
        assert!(!deleted.server.summary.enabled);
        assert_eq!(
            deleted.server.summary.state,
            McpConnectionStateDto::Disabled
        );
        assert!(harness
            .registry
            .get_persisted(delete_id)
            .expect("read deleted identity")
            .is_none());

        harness.shutdown().await;
    }

    #[test]
    fn host_catalog_cursor_rejects_generation_digest_and_server_drift() {
        let server_id = McpServerId::new();
        let mut catalog = McpCatalogSnapshot::empty(server_id);
        catalog.generation = 7;
        let cursor = encode_catalog_cursor(&catalog, 11).expect("encode Host cursor");
        assert_eq!(
            decode_catalog_cursor(&cursor, &catalog, McpManagementOperationDto::ListTools)
                .expect("decode unchanged Host cursor"),
            11
        );

        let mut newer_generation = catalog.clone();
        newer_generation.generation += 1;
        let generation_error = decode_catalog_cursor(
            &cursor,
            &newer_generation,
            McpManagementOperationDto::ListTools,
        )
        .expect_err("Catalog generation drift must stale the Host cursor");
        assert_eq!(
            failure_code(generation_error),
            McpManagementErrorCodeDto::Conflict
        );

        let other_server = McpCatalogSnapshot::empty(McpServerId::new());
        let server_error =
            decode_catalog_cursor(&cursor, &other_server, McpManagementOperationDto::ListTools)
                .expect_err("a Host cursor is bound to one Server ID");
        assert_eq!(
            failure_code(server_error),
            McpManagementErrorCodeDto::Conflict
        );

        let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&cursor)
            .expect("decode cursor envelope");
        let mut envelope: Value = serde_json::from_slice(&raw).expect("parse cursor envelope");
        envelope["catalogDigest"] = Value::String("f".repeat(64));
        let digest_cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&envelope).expect("encode changed cursor envelope"));
        let digest_error = decode_catalog_cursor(
            &digest_cursor,
            &catalog,
            McpManagementOperationDto::ListTools,
        )
        .expect_err("Catalog digest drift must stale the Host cursor");
        assert_eq!(
            failure_code(digest_error),
            McpManagementErrorCodeDto::Conflict
        );
    }

    #[test]
    fn renderer_safe_management_dtos_have_no_secret_bearing_field_names() {
        let mut cursor_catalog = McpCatalogSnapshot::empty(McpServerId::new());
        cursor_catalog.generation = 1;
        let cursor = encode_catalog_cursor(&cursor_catalog, 0).expect("encode Host cursor");
        let examples = [
            serde_json::to_value(McpServerCreateInput {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                display_name: "fixture".to_string(),
                transport: McpTransportKindDto::Stdio,
                executable: "/usr/bin/false".to_string(),
                arguments: Vec::new(),
                cwd: "/tmp".to_string(),
                approval_mode: McpApprovalModeDto::Prompt,
            })
            .expect("serialize create input"),
            serde_json::to_value(McpCatalogToolsPageInput {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                server_id: cursor_catalog.server_id.to_string(),
                cursor: Some(cursor),
                limit: 25,
            })
            .expect("serialize Catalog input"),
            serde_json::to_value(McpManagementErrorData {
                schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
                error_type: McpManagementErrorTypeDto::McpManagement,
                operation: McpManagementOperationDto::Get,
                code: McpManagementErrorCodeDto::InvalidInput,
                recovery: McpManagementRecoveryDto::FixInput,
                message: "safe test error".to_string(),
                server_id: None,
                current_registry_revision: Some(1),
            })
            .expect("serialize management error"),
        ];
        for example in examples {
            assert_renderer_safe_json(&example);
        }

        let forbidden_keys = [
            "environment",
            "env",
            "secretRef",
            "token",
            "header",
            "headers",
            "ciphertext",
        ]
        .into_iter()
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
        for key in forbidden_keys {
            let mut untrusted =
                serde_json::to_value(create_input("fixture")).expect("serialize create input");
            untrusted
                .as_object_mut()
                .expect("create input object")
                .insert(key, Value::String("MCP_TEST_SECRET_CANARY".to_string()));
            assert!(
                serde_json::from_value::<McpServerCreateInput>(untrusted).is_err(),
                "strict DTO parser accepted a forbidden extra field"
            );
        }
    }

    #[test]
    fn renderer_text_projection_replaces_directional_controls_and_marks_the_change() {
        let (projected, changed) = truncate_text("safe\u{202e}spoof", MAX_SAFE_DESCRIPTION_BYTES);
        assert_eq!(projected, "safe\u{fffd}spoof");
        assert!(changed);
        assert!(validate_editable_fields(
            "unsafe\u{2066}name",
            "/usr/bin/false",
            &[],
            "/tmp",
            McpManagementOperationDto::Add,
        )
        .is_err());
    }

    #[test]
    fn renderer_catalog_projection_replaces_controls_before_serialization() {
        let (description, description_truncated) = truncate_text(
            "safe\n\t\u{00ad}\u{061c}\u{2028}\u{202e}\u{2066}description",
            MAX_SAFE_DESCRIPTION_BYTES,
        );
        let tool = McpToolSummaryView {
            server_id: McpServerId::new().to_string(),
            raw_name: bounded_text("raw\u{2066}name", 1024),
            model_name: bounded_text("model\u{0007}name", 64),
            routable: true,
            disabled: false,
            schema_digest_prefix: "0123456789ab".to_string(),
            description,
            description_truncated,
            diagnostic_codes: Vec::new(),
            catalog_generation: 1,
            catalog_completeness: McpCatalogCompletenessDto::Complete,
        };

        assert!(tool.description_truncated);
        for projected in [&tool.raw_name, &tool.model_name, &tool.description] {
            assert!(
                !projected.chars().any(is_unsafe_renderer_text_character),
                "catalog projection retained an unsafe display character"
            );
            assert!(projected.contains('\u{fffd}'));
        }
        assert_eq!(safe_error_code("unsafe code\nvalue"), "mcp.serverError");
        assert_eq!(safe_error_code("mcp.server_error-1"), "mcp.server_error-1");
        assert_renderer_safe_json(
            &serde_json::to_value(tool).expect("serialize safe Catalog projection"),
        );
    }
}
