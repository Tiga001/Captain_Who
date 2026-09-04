use super::*;

/// Frozen request to copy one immutable Skill resource, or one resource-tree
/// prefix, into the selected workspace.
///
/// The source is a logical `skill://` URI. Managed-store paths and resource
/// bytes never cross the runtime action protocol.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillMaterializationRequest {
    pub id: String,
    pub source_uri: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub source_prefix: Option<String>,
    pub destination: String,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillMaterializationResult {
    pub status: AgentSkillMaterializationResultStatus,
    pub source_uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_prefix: Option<String>,
    pub destination: String,
    pub source_revision: String,
    pub file_count: u64,
    pub byte_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Declarative runtime requirements used only for dependency discovery. They
/// never grant permissions and never trigger dependency installation.
#[derive(Debug, Default, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptRequirements {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub python_distributions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub commands: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillDependencyCheck {
    pub kind: AgentSkillDependencyKind,
    pub name: String,
    pub status: AgentSkillDependencyStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptPreflightReport {
    pub status: AgentSkillScriptPreflightStatus,
    pub interpreter: AgentSkillScriptInterpreter,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<AgentSkillDependencyCheck>,
    pub runtime_fingerprint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Host-derived source classification frozen with a Skill script request.
///
/// This is evidence for later policy checks, not a grant by itself. The Host
/// must match it against the active, revision-bound resource session before
/// execution.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillScriptSourceKind {
    Workspace,
    Bundled,
    Installed,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSkillScriptTrust {
    Untrusted,
    UserApproved,
    Application,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptSourceProof {
    pub source_id: String,
    pub source_kind: AgentSkillScriptSourceKind,
    pub trust: AgentSkillScriptTrust,
}

/// Frozen, revision-bound Skill script request. The script URI and digest
/// identify immutable package bytes; arguments remain a structured argv and
/// are never converted to a shell command.
#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillScriptRequest {
    pub id: String,
    pub script_uri: String,
    pub skill_id: String,
    pub skill_revision: String,
    pub resource_path: String,
    pub resource_digest: String,
    pub source: AgentSkillScriptSourceProof,
    pub interpreter: AgentSkillScriptInterpreter,
    pub args: Vec<String>,
    pub requirements: AgentSkillScriptRequirements,
    pub preflight: AgentSkillScriptPreflightReport,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub timeout_ms: Option<u64>,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentSkillScriptResult {
    pub script_uri: String,
    pub skill_id: String,
    pub skill_revision: String,
    pub resource_digest: String,
    pub preflight: AgentSkillScriptPreflightReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(flatten, default)]
    pub output_capture: crate::command::ProcessOutputCaptureMetadata,
    /// Backend-only complete stdout capture consumed by Exact History.
    #[serde(skip, default)]
    pub stdout_spool: crate::command::ProcessOutputSpool,
    /// Backend-only complete stderr capture consumed by Exact History.
    #[serde(skip, default)]
    pub stderr_spool: crate::command::ProcessOutputSpool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl AgentSkillScriptResult {
    pub fn output_spool_substitutions(
        &self,
    ) -> Vec<crate::command::ProcessOutputSpoolSubstitution> {
        crate::command::process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
}

pub const AGENT_SKILL_INSTALLATION_SCHEMA_VERSION: u32 = 1;

/// Presentation-safe preview frozen by the Host before a Skill installation enters approval.
///
/// Every string originating in the third-party package remains untrusted display data. Authority
/// such as preparation IDs, destination paths and warning acknowledgements deliberately stays
/// behind `install_ref` in the Host application service.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationPreview {
    pub name: String,
    pub description: String,
    pub source_summary: serde_json::Value,
    pub resolved_revision: String,
    pub file_count: u64,
    pub total_bytes: u64,
    pub resource_summary: AgentSkillInstallationResourceSummary,
    pub contains_scripts: bool,
    pub warnings: Vec<AgentSkillInstallationWarning>,
    pub compatibility: String,
    pub operation: String,
    pub impact: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationResourceSummary {
    pub total: u64,
    pub references: u64,
    pub assets: u64,
    pub scripts: u64,
    pub bytes: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationWarning {
    pub code: String,
    pub message: String,
    pub requires_acknowledgement: bool,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSkillInstallationRequest {
    pub schema_version: u32,
    pub id: String,
    pub install_ref: String,
    pub preview: AgentSkillInstallationPreview,
    pub approval_status: AgentApprovalStatus,
    pub expires_at: u64,
}

/// One exact task-scoped request to activate a Host-owned built-in capability.
///
/// This projection contains no executable, transport, credential or managed-server details. The
/// Host must compare every frozen identity field before creating its process-memory grant.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBuiltinCapabilityActivationApproval {
    pub action_id: String,
    pub activation_id: String,
    pub run_id: String,
    pub call_id: String,
    pub capability_id: String,
    pub display_name: String,
    pub reason: String,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub created_at: u64,
    pub expires_at: u64,
    pub approval_status: AgentApprovalStatus,
}

pub const BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION: u32 = 1;
pub const BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS: u64 = 15 * 60;

/// Host-classified sensitive effects of a built-in MCP Tool invocation.
///
/// These values are frozen authorization identity and presentation hints. A model-authored name,
/// Tool prefix, or Renderer claim never selects the policy.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinMcpToolRiskKind {
    FileRead,
    FileWrite,
    FileUpload,
    FileDownload,
    CookieRead,
    CookieWrite,
    LocalStorageRead,
    LocalStorageWrite,
    SessionStorageRead,
    SessionStorageWrite,
    StorageStateImport,
    StorageStateExport,
    NetworkSensitiveRead,
    PageScriptExecution,
    UnsafeCodeExecution,
}

/// Plain-text, value-free resource projection for the native approval card.
///
/// Basenames are display data only. Raw paths, opaque file handles, request data, storage values,
/// script source, and page content are deliberately absent.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinMcpToolResourceSummary {
    pub scope: String,
    pub display_name: String,
    pub file_basenames: Vec<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub origin: Option<String>,
}

/// Complete non-secret identity of one exact sensitive built-in MCP Tool invocation.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinMcpToolApprovalIdentity {
    pub action_id: String,
    pub approval_id: String,
    pub run_id: String,
    pub call_id: String,
    pub capability_id: String,
    pub capability_activation_id: String,
    pub managed_mcp_id: String,
    pub package_name: String,
    pub package_version: String,
    pub upstream_catalog_digest: String,
    pub manifest_digest: String,
    pub policy_digest: String,
    pub policy_revision: u64,
    pub tool_id: String,
    pub raw_name: String,
    pub model_name: String,
    pub upstream_schema_digest: String,
    pub host_overlay_digest: String,
    pub host_input_schema_digest: String,
    pub arguments_digest: String,
    pub resource_scope_digest: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub origin: Option<String>,
}

/// Persistable, secret-free proposal for a sensitive built-in MCP Tool.
///
/// The original Tool invocation remains in flight behind the Host dispatch barrier. This DTO is
/// the approval projection of that same call, not a second Tool call.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentBuiltinMcpToolApproval {
    pub schema_version: u32,
    pub identity: BuiltinMcpToolApprovalIdentity,
    pub capability_display_name: String,
    pub tool_display_name: String,
    pub call_reason: String,
    pub operation_category: String,
    pub resource_summary: BuiltinMcpToolResourceSummary,
    pub risk_kinds: Vec<BuiltinMcpToolRiskKind>,
    pub created_at: u64,
    pub expires_at: u64,
    pub approval_status: AgentApprovalStatus,
}
