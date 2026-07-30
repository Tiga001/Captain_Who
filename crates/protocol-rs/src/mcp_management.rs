use serde::{de::Error as _, Deserialize, Serialize};

pub const MCP_MANAGEMENT_SCHEMA_VERSION: u32 = 1;
pub const MCP_MANAGEMENT_ERROR_CODE: i64 = -32030;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpTransportKindDto {
    Stdio,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpServerScopeDto {
    User,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpServerSourceDto {
    UserManual,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpTrustLevelDto {
    Untrusted,
    UserApproved,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpApprovalModeDto {
    Prompt,
    Deny,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpLaunchAuthorizationStateDto {
    Required,
    Authorized,
    Stale,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpConnectionStateDto {
    Disabled,
    Starting,
    Discovering,
    Ready,
    Stopping,
    Error,
    Backoff,
    Degraded,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpCatalogCompletenessDto {
    Complete,
    Partial,
    Stale,
    Failed,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerMutationPrecondition {
    pub expected_registry_revision: u64,
    pub expected_config_epoch: String,
    pub expected_config_digest: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerListInput {
    pub schema_version: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerIdInput {
    pub schema_version: u32,
    pub server_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerMutationInput {
    pub schema_version: u32,
    pub server_id: String,
    pub precondition: McpServerMutationPrecondition,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerCreateInput {
    pub schema_version: u32,
    pub display_name: String,
    pub transport: McpTransportKindDto,
    pub executable: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    pub approval_mode: McpApprovalModeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerUpdateInput {
    pub schema_version: u32,
    pub server_id: String,
    pub precondition: McpServerMutationPrecondition,
    pub display_name: String,
    pub transport: McpTransportKindDto,
    pub executable: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    pub approval_mode: McpApprovalModeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpSafeErrorView {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpProtocolLifecycleDto {
    Discover,
    InitializeFallback,
    Unknown,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpProtocolView {
    pub protocol_version: String,
    pub lifecycle: McpProtocolLifecycleDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCapabilityView {
    pub tools: bool,
    pub resources: bool,
    pub prompts: bool,
    pub logging: bool,
    pub completion: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerListItem {
    pub schema_version: u32,
    pub server_id: String,
    pub display_name: String,
    pub scope: McpServerScopeDto,
    pub source: McpServerSourceDto,
    pub transport: McpTransportKindDto,
    pub enabled: bool,
    pub trust: McpTrustLevelDto,
    pub approval_mode: McpApprovalModeDto,
    pub launch_authorization_state: McpLaunchAuthorizationStateDto,
    pub state: McpConnectionStateDto,
    pub registry_revision: u64,
    pub config_epoch: String,
    pub config_digest: String,
    pub catalog_generation: u64,
    pub catalog_completeness: McpCatalogCompletenessDto,
    pub tool_count: u32,
    pub active_call_count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<McpSafeErrorView>,
    pub updated_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerDetailsView {
    #[serde(flatten)]
    pub summary: McpServerListItem,
    pub executable: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<McpProtocolView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<McpCapabilityView>,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerListOutput {
    pub schema_version: u32,
    pub registry_revision: u64,
    pub servers: Vec<McpServerListItem>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerDetailsOutput {
    pub schema_version: u32,
    pub registry_revision: u64,
    pub server: McpServerDetailsView,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpLaunchAuthorizationPreview {
    pub schema_version: u32,
    pub authorization_id: String,
    pub expires_at_ms: u64,
    pub server_id: String,
    pub display_name: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub cwd: String,
    pub launch_spec_digest: String,
    pub precondition: McpServerMutationPrecondition,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpLaunchAuthorizationCommitInput {
    pub schema_version: u32,
    pub authorization_id: String,
    pub precondition: McpServerMutationPrecondition,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpLaunchAuthorizationResult {
    pub schema_version: u32,
    pub authorized: bool,
    pub server: McpServerDetailsView,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogToolsPageInput {
    pub schema_version: u32,
    pub server_id: String,
    #[serde(
        default,
        deserialize_with = "deserialize_present_value",
        skip_serializing_if = "Option::is_none"
    )]
    pub cursor: Option<String>,
    pub limit: u32,
}

fn deserialize_present_value<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpToolSummaryView {
    pub server_id: String,
    pub raw_name: String,
    pub model_name: String,
    pub routable: bool,
    pub disabled: bool,
    pub schema_digest_prefix: String,
    pub description: String,
    pub description_truncated: bool,
    pub diagnostic_codes: Vec<String>,
    pub catalog_generation: u64,
    pub catalog_completeness: McpCatalogCompletenessDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpCatalogToolsPageOutput {
    pub schema_version: u32,
    pub server_id: String,
    pub catalog_generation: u64,
    pub catalog_completeness: McpCatalogCompletenessDto,
    pub tools: Vec<McpToolSummaryView>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpManagementOperationDto {
    List,
    Get,
    Add,
    Update,
    Delete,
    PrepareLaunchAuthorization,
    CommitLaunchAuthorization,
    Enable,
    Disable,
    Start,
    Stop,
    Restart,
    Status,
    ListTools,
    RefreshCatalog,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpManagementErrorCodeDto {
    InvalidInput,
    NotFound,
    Conflict,
    AuthorizationRequired,
    AuthorizationStale,
    PolicyDenied,
    InvalidState,
    ServerError,
    Timeout,
    CleanupIncomplete,
    InternalSafeError,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpManagementRecoveryDto {
    FixInput,
    Refresh,
    RequestLaunchAuthorization,
    Retry,
    StopAndRetry,
    DoNotRetry,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpManagementErrorTypeDto {
    McpManagement,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpManagementErrorData {
    pub schema_version: u32,
    #[serde(rename = "type")]
    pub error_type: McpManagementErrorTypeDto,
    pub operation: McpManagementOperationDto,
    pub code: McpManagementErrorCodeDto,
    pub recovery: McpManagementRecoveryDto,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_registry_revision: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum McpChangedKindDto {
    Added,
    Updated,
    Deleted,
    AuthorizationChanged,
    EnablementChanged,
    StateChanged,
    CatalogChanged,
    ResyncRequired,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpChangedNotification {
    pub schema_version: u32,
    pub source_epoch: String,
    pub sequence: u64,
    pub registry_revision: u64,
    pub kind: McpChangedKindDto,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<McpConnectionStateDto>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct McpChangedNotificationWire {
    schema_version: u32,
    source_epoch: String,
    sequence: u64,
    registry_revision: u64,
    kind: McpChangedKindDto,
    #[serde(default)]
    server_id: Option<String>,
    #[serde(default)]
    state: Option<McpConnectionStateDto>,
}

impl<'de> Deserialize<'de> for McpChangedNotification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = McpChangedNotificationWire::deserialize(deserializer)?;
        let valid = match wire.kind {
            McpChangedKindDto::ResyncRequired => wire.server_id.is_none() && wire.state.is_none(),
            McpChangedKindDto::StateChanged => wire.server_id.is_some() && wire.state.is_some(),
            _ => wire.server_id.is_some() && wire.state.is_none(),
        };
        if !valid {
            return Err(D::Error::custom(
                "MCP changed notification kind has inconsistent Server/state fields",
            ));
        }
        Ok(Self {
            schema_version: wire.schema_version,
            source_epoch: wire.source_epoch,
            sequence: wire.sequence,
            registry_revision: wire.registry_revision,
            kind: wire.kind,
            server_id: wire.server_id,
            state: wire.state,
        })
    }
}
