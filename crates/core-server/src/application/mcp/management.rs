//! MCP management-service facade and shared process-owned state.
//!
//! RPC behavior is grouped into focused authorization, catalog, configuration, lifecycle,
//! projection, validation, and error modules without changing the service's crate-visible API.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use mycopilot_core::{BuiltinCapabilityId as CoreBuiltinCapabilityId, BuiltinCapabilityRuntime};
use mycopilot_mcp_client::{
    McpApprovalMode, McpCatalogCompleteness, McpCatalogDiagnosticKind, McpCatalogSnapshot,
    McpConfigDigest, McpConfigEpoch, McpConnectionManager, McpError, McpErrorKind, McpEvent,
    McpLifecycleKind, McpRegistryChangeKind, McpRegistryEntry, McpRegistryMutation,
    McpServerConfig, McpServerId, McpServerScope, McpServerState, McpStdioConfig,
    McpTransportConfig, McpTrustLevel,
};
use mycopilot_protocol_rs::{
    McpApprovalModeDto, McpBuiltinCapabilityIdDto, McpBuiltinCapabilityListInput,
    McpBuiltinCapabilityListItem, McpBuiltinCapabilityListOutput,
    McpBuiltinCapabilityMutationOutput, McpBuiltinCapabilitySetAllowedInput, McpCapabilityView,
    McpCatalogCompletenessDto, McpCatalogToolsPageInput, McpCatalogToolsPageOutput,
    McpChangedKindDto, McpChangedNotification, McpConnectionStateDto,
    McpLaunchAuthorizationCommitInput, McpLaunchAuthorizationPreview, McpLaunchAuthorizationResult,
    McpLaunchAuthorizationStateDto, McpManagementEntryKindDto, McpManagementErrorCodeDto,
    McpManagementErrorData, McpManagementErrorTypeDto, McpManagementOperationDto,
    McpManagementRecoveryDto, McpProtocolLifecycleDto, McpProtocolView, McpSafeErrorView,
    McpServerCreateInput, McpServerDetailsOutput, McpServerDetailsView, McpServerIdInput,
    McpServerListInput, McpServerListItem, McpServerListOutput, McpServerMutationInput,
    McpServerMutationPrecondition, McpServerScopeDto, McpServerSourceDto, McpServerUpdateInput,
    McpToolSummaryView, McpTransportKindDto, McpTrustLevelDto, MCP_MANAGEMENT_SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::builtin_capability_policy::{
    BuiltinCapabilityId, BuiltinCapabilityPolicyError, BuiltinCapabilityPolicyRecord,
    SqliteBuiltinCapabilityPolicyStore,
};
use super::sqlite_registry::{
    compute_launch_spec_digest, launch_authorization_identity_is_valid,
    launch_authorization_is_valid, prepare_launch_file_identity, McpLaunchSpecDigest,
    McpPersistedRegistryRecord, McpRegistryMutationPrecondition, McpRegistryPersistenceError,
    SqliteMcpRegistry, MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION,
};

mod authorization;
mod builtin_capabilities;
mod catalog;
mod errors;
mod events;
mod lifecycle;
mod mutations;
mod projection;
mod renderer_projection;
mod server_config;
mod validation;

#[cfg(test)]
use catalog::{decode_catalog_cursor, encode_catalog_cursor};
use errors::standalone_failure;
pub(crate) use errors::McpManagementFailure;
use renderer_projection::*;
use validation::*;

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

pub(crate) struct McpManagementService {
    registry: Arc<SqliteMcpRegistry>,
    manager: Arc<McpConnectionManager>,
    builtin_capability_policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    builtin_capability_runtime: BuiltinCapabilityRuntime,
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
        builtin_capability_policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        builtin_capability_runtime: BuiltinCapabilityRuntime,
    ) -> Self {
        Self {
            registry,
            manager,
            builtin_capability_policies,
            builtin_capability_runtime,
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
}

#[cfg(test)]
mod tests;
