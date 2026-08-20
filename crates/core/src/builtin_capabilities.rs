//! Host-owned, task-scoped built-in capability contracts.
//!
//! A built-in capability is not an external MCP configuration. The Host owns its manifest,
//! policy and process-memory grants; Core only exposes reviewed Tool contracts and routes typed
//! invocations. No executable, transport, credential or raw MCP type crosses this boundary.

use crate::protocol::{
    AgentApprovalStatus, AgentBrowserRiskApproval, AgentBuiltinCapabilityActivationApproval,
    AgentBuiltinMcpToolApproval, AgentError, AgentResult, AgentToolDefinition, AgentToolResult,
    BrowserDestinationIdentity, BrowserRiskKind, BrowserRiskTrigger,
    BuiltinMcpToolApprovalIdentity, BuiltinMcpToolResourceSummary, BuiltinMcpToolRiskKind,
    BROWSER_RISK_APPROVAL_SCHEMA_VERSION, BROWSER_RISK_APPROVAL_TTL_SECONDS,
    BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION, BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS,
};
use crate::AgentCancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub const BUILTIN_CAPABILITY_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS: u64 = 15 * 60;
pub(crate) const BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID: &str = "builtin.capabilities";
pub(crate) const ACTIVATE_CAPABILITY_TOOL_NAME: &str = "activate_capability";
/// Process-memory grants are run-bound and deliberately outlive the approval prompt.
///
/// They are never persisted, are unusable from another run, and are revalidated against the live
/// policy/manifest at every dispatch. A finite ceiling bounds abandoned-run memory without making
/// a long-running task ask again merely because its approval dialog was created 15 minutes ago.
pub const BUILTIN_CAPABILITY_GRANT_TTL_SECONDS: u64 = 24 * 60 * 60;
const ID_MAX_BYTES: usize = 128;
const DISPLAY_NAME_MAX_BYTES: usize = 256;
const DESCRIPTION_MAX_BYTES: usize = 4 * 1024;
const REASON_MAX_BYTES: usize = 4 * 1024;
const MANIFEST_VERSION_MAX_BYTES: usize = 128;
const MANIFEST_TOOL_MAX_COUNT: usize = 256;
const TOOL_SCHEMA_MAX_BYTES: usize = 256 * 1024;
const MANIFEST_SCHEMA_TOTAL_MAX_BYTES: usize = 4 * 1024 * 1024;
const BROWSER_DESTINATION_MAX_BYTES: usize = 8 * 1024;
const BROWSER_RISK_MAX_COUNT: usize = 32;
const BUILTIN_MCP_RISK_MAX_COUNT: usize = 32;
const BUILTIN_MCP_FILE_BASENAME_MAX_COUNT: usize = 32;
const BUILTIN_MCP_SAFE_DISPLAY_MAX_BYTES: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BuiltinMcpToolApprovalMode {
    #[default]
    Never,
    Always,
    Dynamic,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuiltinCapabilityId(String);

impl BuiltinCapabilityId {
    pub fn parse(value: impl Into<String>) -> AgentResult<Self> {
        let value = value.into();
        let valid = !value.is_empty()
            && value.len() <= ID_MAX_BYTES
            && value.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
            })
            && value
                .bytes()
                .next()
                .is_some_and(|byte| byte.is_ascii_lowercase())
            && !value.ends_with(['.', '_', '-'])
            && !value.contains("..");
        if !valid {
            return Err(AgentError::new(
                "内置能力 id 无效；必须是小写、稳定且带命名空间的标识。",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CapabilityActivationId(String);

impl CapabilityActivationId {
    pub fn generate() -> Self {
        Self(Uuid::new_v4().to_string())
    }

    pub fn parse(value: impl Into<String>) -> AgentResult<Self> {
        let value = value.into();
        let id = Uuid::parse_str(&value)
            .map_err(|_| AgentError::new("内置能力 activation id 无效。"))?;
        if id.is_nil() || id.get_version() != Some(uuid::Version::Random) || id.to_string() != value
        {
            return Err(AgentError::new(
                "内置能力 activation id 必须是规范 UUID v4。",
            ));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityDescriptor {
    pub id: BuiltinCapabilityId,
    pub display_name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityToolDescriptor {
    /// Host-owned identity used to address the reviewed managed-server tool.
    pub tool_id: String,
    /// Exact upstream raw name. This remains distinct from the Host routing id and model name.
    pub raw_name: String,
    /// Stable Provider-facing name. It is presentation only and is never parsed for routing.
    pub model_name: String,
    pub description: String,
    pub input_schema: Value,
    pub schema_digest: String,
    /// Exact fixed Provider schema before Host overlays are applied.
    pub upstream_schema_digest: String,
    /// Digest of the reviewed Host overlay applied to the upstream schema.
    pub host_overlay_digest: String,
    pub safety: crate::protocol::AgentToolSafety,
    pub requires_workspace: bool,
    /// Internal approval policy for this exact reviewed Tool identity. `Never` keeps the ordinary
    /// automatic built-in path; `Always`/`Dynamic` enter the same durable approval lifecycle as
    /// run_command before any managed Host dispatch.
    #[serde(default)]
    pub builtin_approval_mode: BuiltinMcpToolApprovalMode,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_risk_kinds: Vec<BuiltinMcpToolRiskKind>,
}

impl BuiltinCapabilityToolDescriptor {
    pub fn new(
        tool_id: impl Into<String>,
        model_name: impl Into<String>,
        description: impl Into<String>,
        input_schema: Value,
        safety: crate::protocol::AgentToolSafety,
        requires_workspace: bool,
    ) -> AgentResult<Self> {
        let input_schema_digest = schema_digest(&input_schema)?;
        let empty_overlay_digest = schema_digest(&json!({}))?;
        let tool_id = tool_id.into();
        let descriptor = Self {
            raw_name: tool_id.clone(),
            tool_id,
            model_name: model_name.into(),
            description: description.into(),
            input_schema,
            upstream_schema_digest: input_schema_digest.clone(),
            host_overlay_digest: empty_overlay_digest,
            schema_digest: input_schema_digest,
            safety,
            requires_workspace,
            builtin_approval_mode: BuiltinMcpToolApprovalMode::Never,
            builtin_risk_kinds: Vec::new(),
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn with_upstream_contract(
        mut self,
        raw_name: impl Into<String>,
        upstream_schema_digest: impl Into<String>,
        host_overlay_digest: impl Into<String>,
    ) -> AgentResult<Self> {
        self.raw_name = raw_name.into();
        self.upstream_schema_digest = upstream_schema_digest.into();
        self.host_overlay_digest = host_overlay_digest.into();
        self.validate()?;
        Ok(self)
    }

    pub fn with_builtin_approval_policy(
        mut self,
        mode: BuiltinMcpToolApprovalMode,
        mut risk_kinds: Vec<BuiltinMcpToolRiskKind>,
    ) -> AgentResult<Self> {
        risk_kinds.sort_unstable();
        risk_kinds.dedup();
        self.builtin_approval_mode = mode;
        self.builtin_risk_kinds = risk_kinds;
        self.validate()?;
        Ok(self)
    }

    pub fn validate(&self) -> AgentResult<()> {
        BuiltinCapabilityId::parse(self.tool_id.clone())?;
        BuiltinCapabilityId::parse(self.raw_name.clone())?;
        if self.model_name.is_empty()
            || self.model_name.len() > 64
            || !self
                .model_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            || self.description.trim().is_empty()
            || self.description.len() > DESCRIPTION_MAX_BYTES
            || !valid_sha256_digest(&self.upstream_schema_digest)
            || !valid_sha256_digest(&self.host_overlay_digest)
        {
            return Err(AgentError::new("内置能力 Tool identity/description 无效。"));
        }
        if self.builtin_risk_kinds.len() > BUILTIN_MCP_RISK_MAX_COUNT
            || (self.builtin_approval_mode == BuiltinMcpToolApprovalMode::Never
                && !self.builtin_risk_kinds.is_empty())
            || (self.builtin_approval_mode != BuiltinMcpToolApprovalMode::Never
                && self.builtin_risk_kinds.is_empty())
        {
            return Err(AgentError::new("内置能力 Tool 敏感审批策略无效。"));
        }
        if serde_json::to_vec(&self.input_schema)
            .map_err(|error| AgentError::new(format!("无法检查内置能力 Tool schema：{error}")))?
            .len()
            > TOOL_SCHEMA_MAX_BYTES
        {
            return Err(AgentError::new("内置能力 Tool schema 超过 256 KiB 上限。"));
        }
        crate::tools::schema::validate_portable_tool_input_schema(
            &self.model_name,
            &self.input_schema,
        )?;
        if self.schema_digest != schema_digest(&self.input_schema)? {
            return Err(AgentError::new("内置能力 Tool schema digest 不匹配。"));
        }
        Ok(())
    }

    pub fn definition(&self) -> AgentToolDefinition {
        AgentToolDefinition {
            name: self.model_name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            safety: self.safety,
            requires_workspace: self.requires_workspace,
            requires_approval: self.builtin_approval_mode != BuiltinMcpToolApprovalMode::Never,
            approval_mode: match self.builtin_approval_mode {
                BuiltinMcpToolApprovalMode::Never => crate::protocol::AgentToolApprovalMode::Never,
                BuiltinMcpToolApprovalMode::Always => {
                    crate::protocol::AgentToolApprovalMode::Always
                }
                BuiltinMcpToolApprovalMode::Dynamic => {
                    crate::protocol::AgentToolApprovalMode::Dynamic
                }
            },
        }
    }
}

pub fn builtin_tool_requires_approval(
    descriptor: &BuiltinCapabilityToolDescriptor,
    arguments: &Value,
) -> bool {
    match descriptor.builtin_approval_mode {
        BuiltinMcpToolApprovalMode::Never => false,
        BuiltinMcpToolApprovalMode::Always => true,
        BuiltinMcpToolApprovalMode::Dynamic => match descriptor.tool_id.as_str() {
            "browser_file_upload" | "browser_drop" => arguments
                .get("paths")
                .and_then(Value::as_array)
                .is_some_and(|paths| !paths.is_empty()),
            // Unknown dynamic policies fail closed; exact Tool identity is intentional.
            _ => true,
        },
    }
}

pub fn validate_builtin_sensitive_target_scope(
    _invocation: &BuiltinCapabilityInvocation,
) -> AgentResult<()> {
    // Sensitive page targets are resources within the Host-owned active managed Surface, not
    // independent authorities. Main separately freezes surface/generation/navigation epoch and
    // origin, while the approval binds the exact official arguments (including target/index).
    // The official connection can enumerate only that Surface, so iframe refs, the pending file
    // chooser and the connection-local request ledger cannot escape into another guest.
    Ok(())
}

fn builtin_mcp_tool_binding_scope(
    invocation: &BuiltinCapabilityInvocation,
) -> BuiltinMcpToolBindingScope {
    match invocation.tool_id.as_str() {
        "browser_cookie_clear"
        | "browser_cookie_delete"
        | "browser_cookie_get"
        | "browser_cookie_list"
        | "browser_set_storage_state"
        | "browser_storage_state" => BuiltinMcpToolBindingScope::ManagedBrowserProfile,
        // Fixed 0.0.79 derives an omitted cookie domain from currentTabOrDie(). The broader
        // profile approval therefore still freezes one exact Surface for that parameter shape.
        "browser_cookie_set"
            if invocation
                .arguments
                .get("domain")
                .and_then(Value::as_str)
                .is_some_and(|domain| !domain.trim().is_empty()) =>
        {
            BuiltinMcpToolBindingScope::ManagedBrowserProfile
        }
        _ => BuiltinMcpToolBindingScope::ManagedSurface,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityProviderContract {
    pub package_name: String,
    pub package_version: String,
    pub upstream_catalog_digest: String,
    pub policy_digest: String,
}

impl BuiltinCapabilityProviderContract {
    pub fn new(
        package_name: impl Into<String>,
        package_version: impl Into<String>,
        upstream_catalog_digest: impl Into<String>,
        policy_digest: impl Into<String>,
    ) -> AgentResult<Self> {
        let contract = Self {
            package_name: package_name.into(),
            package_version: package_version.into(),
            upstream_catalog_digest: upstream_catalog_digest.into(),
            policy_digest: policy_digest.into(),
        };
        contract.validate()?;
        Ok(contract)
    }

    fn host_native(managed_mcp_id: &str, version: &str) -> AgentResult<Self> {
        Self::new(
            managed_mcp_id,
            version,
            schema_digest(&json!({
                "kind": "host_native_catalog",
                "managedMcpId": managed_mcp_id,
                "version": version,
            }))?,
            schema_digest(&json!({
                "kind": "host_native_policy",
                "managedMcpId": managed_mcp_id,
                "version": version,
            }))?,
        )
    }

    pub fn validate(&self) -> AgentResult<()> {
        if self.package_name.trim().is_empty()
            || self.package_name.trim() != self.package_name
            || self.package_name.len() > 256
            || self.package_name.chars().any(char::is_control)
            || self.package_version.trim().is_empty()
            || self.package_version.trim() != self.package_version
            || self.package_version.len() > 128
            || self.package_version.chars().any(char::is_control)
            || !valid_sha256_digest(&self.upstream_catalog_digest)
            || !valid_sha256_digest(&self.policy_digest)
        {
            return Err(AgentError::new(
                "内置能力 Provider package/catalog/policy identity 无效。",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityManifest {
    pub schema_version: u32,
    pub descriptor: BuiltinCapabilityDescriptor,
    /// Stable non-secret identity of the Host-managed MCP implementation.
    pub managed_mcp_id: String,
    pub provider_contract: BuiltinCapabilityProviderContract,
    pub version: String,
    pub tools: Vec<BuiltinCapabilityToolDescriptor>,
    pub manifest_digest: String,
}

impl BuiltinCapabilityManifest {
    pub fn new(
        descriptor: BuiltinCapabilityDescriptor,
        managed_mcp_id: impl Into<String>,
        version: impl Into<String>,
        tools: Vec<BuiltinCapabilityToolDescriptor>,
    ) -> AgentResult<Self> {
        let managed_mcp_id = managed_mcp_id.into();
        let version = version.into();
        let provider_contract =
            BuiltinCapabilityProviderContract::host_native(&managed_mcp_id, &version)?;
        Self::new_with_provider_contract(
            descriptor,
            managed_mcp_id,
            version,
            provider_contract,
            tools,
        )
    }

    pub fn new_with_provider_contract(
        descriptor: BuiltinCapabilityDescriptor,
        managed_mcp_id: impl Into<String>,
        version: impl Into<String>,
        provider_contract: BuiltinCapabilityProviderContract,
        mut tools: Vec<BuiltinCapabilityToolDescriptor>,
    ) -> AgentResult<Self> {
        tools.sort_by(|left, right| {
            left.tool_id
                .cmp(&right.tool_id)
                .then_with(|| left.model_name.cmp(&right.model_name))
        });
        let mut manifest = Self {
            schema_version: BUILTIN_CAPABILITY_MANIFEST_SCHEMA_VERSION,
            descriptor,
            managed_mcp_id: managed_mcp_id.into(),
            provider_contract,
            version: version.into(),
            tools,
            manifest_digest: String::new(),
        };
        manifest.validate_shape()?;
        manifest.manifest_digest = manifest.compute_digest()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> AgentResult<()> {
        self.validate_shape()?;
        let expected = self.compute_digest()?;
        if self.manifest_digest != expected {
            return Err(AgentError::new("内置能力 manifest digest 不匹配。"));
        }
        Ok(())
    }

    fn validate_shape(&self) -> AgentResult<()> {
        if self.schema_version != BUILTIN_CAPABILITY_MANIFEST_SCHEMA_VERSION {
            return Err(AgentError::new("不支持的内置能力 manifest 版本。"));
        }
        BuiltinCapabilityId::parse(self.descriptor.id.as_str().to_string())?;
        self.provider_contract.validate()?;
        if self.descriptor.display_name.trim().is_empty()
            || self.descriptor.display_name.len() > DISPLAY_NAME_MAX_BYTES
            || self.descriptor.description.trim().is_empty()
            || self.descriptor.description.len() > DESCRIPTION_MAX_BYTES
            || BuiltinCapabilityId::parse(self.managed_mcp_id.clone()).is_err()
            || self.version.trim().is_empty()
            || self.version.len() > MANIFEST_VERSION_MAX_BYTES
            || self.tools.len() > MANIFEST_TOOL_MAX_COUNT
        {
            return Err(AgentError::new("内置能力 manifest 字段无效。"));
        }
        let mut names = std::collections::BTreeSet::new();
        let mut tool_ids = std::collections::BTreeSet::new();
        let mut total_schema_bytes = 0_usize;
        for tool in &self.tools {
            if !names.insert(tool.model_name.as_str()) || !tool_ids.insert(tool.tool_id.as_str()) {
                return Err(AgentError::new("内置能力 manifest 包含重复工具。"));
            }
            tool.validate()?;
            total_schema_bytes = total_schema_bytes
                .checked_add(
                    serde_json::to_vec(&tool.input_schema)
                        .map_err(|error| {
                            AgentError::new(format!("无法检查内置能力 manifest schema：{error}"))
                        })?
                        .len(),
                )
                .ok_or_else(|| AgentError::new("内置能力 manifest schema 总量溢出。"))?;
            if total_schema_bytes > MANIFEST_SCHEMA_TOTAL_MAX_BYTES {
                return Err(AgentError::new(
                    "内置能力 manifest schema 总量超过 4 MiB 上限。",
                ));
            }
        }
        Ok(())
    }

    fn compute_digest(&self) -> AgentResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestMaterial<'a> {
            schema_version: u32,
            descriptor: &'a BuiltinCapabilityDescriptor,
            managed_mcp_id: &'a str,
            provider_contract: &'a BuiltinCapabilityProviderContract,
            version: &'a str,
            tools: Vec<&'a BuiltinCapabilityToolDescriptor>,
        }
        let mut tools = self.tools.iter().collect::<Vec<_>>();
        tools.sort_by(|left, right| {
            left.tool_id
                .cmp(&right.tool_id)
                .then_with(|| left.model_name.cmp(&right.model_name))
        });
        let bytes = serde_json::to_vec(&DigestMaterial {
            schema_version: self.schema_version,
            descriptor: &self.descriptor,
            managed_mcp_id: &self.managed_mcp_id,
            provider_contract: &self.provider_contract,
            version: &self.version,
            tools,
        })
        .map_err(|error| AgentError::new(format!("无法计算内置能力 manifest digest：{error}")))?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityPolicy {
    pub user_allowed: bool,
    pub revision: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityActivationState {
    WaitingApproval,
    Active,
    Rejected,
    DisabledByUser,
    Expired,
    Revoked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGrant {
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub activation_id: CapabilityActivationId,
    pub manifest_digest: String,
    pub upstream_catalog_digest: String,
    pub provider_policy_digest: String,
    pub policy_revision: u64,
    pub created_at: u64,
    pub expires_at: u64,
}

impl CapabilityGrant {
    pub fn is_live_for(
        &self,
        run_id: &str,
        manifest: &BuiltinCapabilityManifest,
        policy: &BuiltinCapabilityPolicy,
        now: u64,
    ) -> bool {
        policy.user_allowed
            && self.run_id == run_id
            && self.capability_id == manifest.descriptor.id
            && self.manifest_digest == manifest.manifest_digest
            && self.upstream_catalog_digest == manifest.provider_contract.upstream_catalog_digest
            && self.provider_policy_digest == manifest.provider_contract.policy_digest
            && self.policy_revision == policy.revision
            && self.expires_at > now
    }

    fn is_live_for_identity(
        &self,
        run_id: &str,
        capability_id: &BuiltinCapabilityId,
        activation_id: &CapabilityActivationId,
        manifest_digest: &str,
        policy_digest: &str,
        policy_revision: u64,
    ) -> bool {
        self.run_id == run_id
            && &self.capability_id == capability_id
            && &self.activation_id == activation_id
            && self.manifest_digest == manifest_digest
            && self.provider_policy_digest == policy_digest
            && self.policy_revision == policy_revision
    }
}

/// Full invocation prepared by Core before a sensitive built-in Tool enters durable approval.
/// Raw arguments remain process-local behind the Provider; only their digest and a safe resource
/// projection are copied into `AgentBuiltinMcpToolApproval`.
#[derive(Debug, Clone)]
pub struct BuiltinMcpToolApprovalRequest {
    pub invocation: BuiltinCapabilityInvocation,
    pub capability_grant: CapabilityGrant,
    pub capability_display_name: String,
    pub tool_display_name: String,
    pub call_reason: String,
    pub operation_category: String,
    pub resource_summary: BuiltinMcpToolResourceSummary,
    pub resource_scope_digest: String,
    pub origin: Option<String>,
    pub risk_kinds: Vec<BuiltinMcpToolRiskKind>,
    /// Process-only Main authority. It is deliberately absent from the durable approval DTO.
    pub target_binding: Option<PreparedBuiltinMcpToolTargetBinding>,
}

#[derive(Debug, Clone)]
pub struct BuiltinMcpToolTargetBindingRequest {
    pub binding_request_id: String,
    pub binding_scope: BuiltinMcpToolBindingScope,
    pub invocation: BuiltinCapabilityInvocation,
    pub capability_grant: CapabilityGrant,
    pub arguments_digest: String,
    pub created_at: u64,
    pub expires_at: u64,
    pub cancellation: AgentCancellationToken,
    /// Process-only file authority. Resolved paths were already constrained by the Tool context;
    /// native picker requests contain no path at all.
    pub file_preparation: Option<BuiltinMcpToolFilePreparation>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinMcpToolBindingScope {
    ManagedSurface,
    ManagedBrowserProfile,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuiltinMcpToolFilePreparation {
    ResolvedPaths(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedBuiltinMcpToolTargetBinding {
    pub binding_id: String,
    pub target_binding_digest: String,
    pub origin: Option<String>,
    pub created_at: u64,
    pub expires_at: u64,
    pub file_basenames: Vec<String>,
    pub file_revision_digest: Option<String>,
}

impl BuiltinMcpToolApprovalRequest {
    pub fn validate(&self) -> AgentResult<()> {
        self.invocation.validate_identity()?;
        if !self.capability_grant.is_live_for_identity(
            &self.invocation.run_id,
            &self.invocation.capability_id,
            &self.invocation.activation_id,
            &self.invocation.manifest_digest,
            &self.invocation.policy_digest,
            self.invocation.policy_revision,
        ) || self.capability_display_name.trim().is_empty()
            || self.capability_display_name.len() > DISPLAY_NAME_MAX_BYTES
            || self.tool_display_name.trim().is_empty()
            || self.tool_display_name.len() > DISPLAY_NAME_MAX_BYTES
            || self.call_reason.trim().is_empty()
            || self.call_reason.len() > REASON_MAX_BYTES
            || !valid_policy_token(&self.operation_category)
            || !valid_sha256_digest(&self.resource_scope_digest)
            || self.risk_kinds.is_empty()
            || self.risk_kinds.len() > BUILTIN_MCP_RISK_MAX_COUNT
        {
            return Err(AgentError::new(
                "内置 MCP 敏感 Tool 审批请求 identity 无效。",
            ));
        }
        let mut risks = self.risk_kinds.clone();
        risks.sort_unstable();
        if risks.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(AgentError::new("内置 MCP 敏感 Tool 风险分类重复。"));
        }
        validate_builtin_resource_summary(&self.resource_summary)?;
        validate_optional_origin(self.origin.as_deref())?;
        if self.resource_summary.origin != self.origin {
            return Err(AgentError::new("内置 MCP 敏感 Tool origin 投影不一致。"));
        }
        if let Some(binding) = &self.target_binding {
            let binding_id = Uuid::parse_str(&binding.binding_id)
                .map_err(|_| AgentError::new("内置 MCP 敏感 Tool 页面绑定 identity 无效。"))?;
            if binding_id.get_version() != Some(uuid::Version::Random)
                || !valid_sha256_digest(&binding.target_binding_digest)
                || binding.origin != self.origin
                || binding.created_at >= binding.expires_at
                || binding.expires_at - binding.created_at > BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS
                || binding.expires_at > self.capability_grant.expires_at
                || binding.file_basenames != self.resource_summary.file_basenames
                || (binding.file_basenames.is_empty() != binding.file_revision_digest.is_none())
                || binding
                    .file_revision_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256_digest(digest))
            {
                return Err(AgentError::new(
                    "内置 MCP 敏感 Tool 页面绑定 identity 无效。",
                ));
            }
        }
        Ok(())
    }
}

/// Process-only single-use authority for one exact sensitive invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinMcpToolGrant {
    pub grant_id: String,
    pub approval_id: String,
    pub run_id: String,
    pub call_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub capability_activation_id: CapabilityActivationId,
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
    pub target_binding_id: Option<String>,
    pub target_binding_digest: Option<String>,
    pub origin: Option<String>,
    pub risk_kinds: Vec<BuiltinMcpToolRiskKind>,
    pub created_at: u64,
    pub expires_at: u64,
}

impl BuiltinMcpToolGrant {
    pub fn is_live_for(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
        capability_grant: &CapabilityGrant,
        now: u64,
    ) -> bool {
        let identity = &approval.identity;
        self.approval_id == identity.approval_id
            && self.run_id == identity.run_id
            && self.call_id == identity.call_id
            && self.capability_id.as_str() == identity.capability_id
            && self.capability_activation_id.as_str() == identity.capability_activation_id
            && self.managed_mcp_id == identity.managed_mcp_id
            && self.package_name == identity.package_name
            && self.package_version == identity.package_version
            && self.upstream_catalog_digest == identity.upstream_catalog_digest
            && self.manifest_digest == identity.manifest_digest
            && self.policy_digest == identity.policy_digest
            && self.policy_revision == identity.policy_revision
            && self.tool_id == identity.tool_id
            && self.raw_name == identity.raw_name
            && self.model_name == identity.model_name
            && self.upstream_schema_digest == identity.upstream_schema_digest
            && self.host_overlay_digest == identity.host_overlay_digest
            && self.host_input_schema_digest == identity.host_input_schema_digest
            && self.arguments_digest == identity.arguments_digest
            && self.resource_scope_digest == identity.resource_scope_digest
            && self.origin == identity.origin
            && self.risk_kinds == approval.risk_kinds
            && self.expires_at > now
            && capability_grant.run_id == self.run_id
            && capability_grant.capability_id == self.capability_id
            && capability_grant.activation_id == self.capability_activation_id
            && capability_grant.manifest_digest == self.manifest_digest
            && capability_grant.upstream_catalog_digest == self.upstream_catalog_digest
            && capability_grant.provider_policy_digest == self.policy_digest
            && capability_grant.policy_revision == self.policy_revision
            && capability_grant.expires_at >= self.expires_at
    }
}

/// Process-only task authorization for one exact normalized browser origin and risk identity.
/// This grant is intentionally not serializable and must never enter a checkpoint or Renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserRiskGrant {
    pub grant_id: String,
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub capability_activation_id: CapabilityActivationId,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub destination: BrowserDestinationIdentity,
    pub resolution_fingerprint: String,
    pub target_fingerprint: String,
    pub trigger: BrowserRiskTrigger,
    pub trigger_tool_name: String,
    pub risk_kinds: Vec<BrowserRiskKind>,
    pub created_at: u64,
    pub expires_at: u64,
}

impl BrowserRiskGrant {
    pub fn is_live_for_request(
        &self,
        capability_grant: &CapabilityGrant,
        request: &BrowserRiskAuthorizationRequest,
        now: u64,
    ) -> bool {
        self.is_live_for_candidate(
            capability_grant,
            BrowserRiskGrantCandidate {
                run_id: &request.run_id,
                destination: &request.destination,
                resolution_fingerprint: &request.resolution_fingerprint,
                target_fingerprint: &request.target_fingerprint,
                trigger: request.trigger,
                trigger_tool_name: &request.trigger_tool_name,
                risk_kinds: &request.risk_kinds,
            },
            now,
        )
    }

    pub fn is_live_for_approval(
        &self,
        capability_grant: &CapabilityGrant,
        approval: &AgentBrowserRiskApproval,
        now: u64,
    ) -> bool {
        self.is_live_for_candidate(
            capability_grant,
            BrowserRiskGrantCandidate {
                run_id: &approval.run_id,
                destination: &approval.destination,
                resolution_fingerprint: &self.resolution_fingerprint,
                target_fingerprint: &self.target_fingerprint,
                trigger: approval.trigger,
                trigger_tool_name: &approval.trigger_tool_name,
                risk_kinds: &approval.risk_kinds,
            },
            now,
        )
    }

    fn is_live_for_candidate(
        &self,
        capability_grant: &CapabilityGrant,
        candidate: BrowserRiskGrantCandidate<'_>,
        now: u64,
    ) -> bool {
        self.run_id == candidate.run_id
            && self.capability_id == capability_grant.capability_id
            && self.capability_activation_id == capability_grant.activation_id
            && self.manifest_digest == capability_grant.manifest_digest
            && self.policy_revision == capability_grant.policy_revision
            && self.destination.origin == candidate.destination.origin
            && self.destination.scheme == candidate.destination.scheme
            && self.destination.ascii_host == candidate.destination.ascii_host
            && self.destination.effective_port == candidate.destination.effective_port
            && self.destination.address_class == candidate.destination.address_class
            && self.resolution_fingerprint == candidate.resolution_fingerprint
            && (!requires_exact_browser_risk_scope(candidate.risk_kinds)
                || (self.target_fingerprint == candidate.target_fingerprint
                    && self.trigger == candidate.trigger
                    && self.trigger_tool_name == candidate.trigger_tool_name))
            && self.risk_kinds == candidate.risk_kinds
            && self.expires_at > now
    }
}

struct BrowserRiskGrantCandidate<'a> {
    run_id: &'a str,
    destination: &'a BrowserDestinationIdentity,
    resolution_fingerprint: &'a str,
    target_fingerprint: &'a str,
    trigger: BrowserRiskTrigger,
    trigger_tool_name: &'a str,
    risk_kinds: &'a [BrowserRiskKind],
}

/// Credential-free, Host-classified request at a browser network boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserRiskAuthorizationRequest {
    pub run_id: String,
    pub call_id: String,
    pub trigger_tool_name: String,
    pub capability_id: BuiltinCapabilityId,
    pub capability_activation_id: CapabilityActivationId,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub display_name: String,
    pub reason: String,
    pub destination: BrowserDestinationIdentity,
    /// Host-only HMAC/digest of the resolver answer set; never serialized to Agent/Renderer.
    pub resolution_fingerprint: String,
    /// Host-only HMAC of the exact destination/action identity. Never serialized to Agent/UI.
    pub target_fingerprint: String,
    pub trigger: BrowserRiskTrigger,
    pub risk_kinds: Vec<BrowserRiskKind>,
}

impl BrowserRiskAuthorizationRequest {
    pub fn validate(&self) -> AgentResult<()> {
        if self.run_id.trim().is_empty()
            || self.call_id.trim().is_empty()
            || !valid_browser_tool_name(&self.trigger_tool_name)
            || self.display_name.trim().is_empty()
            || self.reason.trim().is_empty()
            || self.reason.len() > REASON_MAX_BYTES
            || !valid_sha256_digest(&self.manifest_digest)
            || !valid_resolution_fingerprint(&self.resolution_fingerprint)
            || !valid_resolution_fingerprint(&self.target_fingerprint)
        {
            return Err(AgentError::new("浏览器风险请求 identity 无效。"));
        }
        validate_browser_destination(&self.destination)?;
        validate_browser_risk_kinds(&self.risk_kinds)
    }
}

pub type BuiltinCapabilityFuture<'a, T> = Pin<Box<dyn Future<Output = AgentResult<T>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct BuiltinCapabilityInvocation {
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub managed_mcp_id: String,
    pub package_name: String,
    pub package_version: String,
    pub upstream_catalog_digest: String,
    pub policy_digest: String,
    pub activation_id: CapabilityActivationId,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub tool_id: String,
    pub raw_name: String,
    pub model_name: String,
    pub upstream_schema_digest: String,
    pub host_overlay_digest: String,
    pub host_input_schema_digest: String,
    pub call_id: String,
    pub arguments: Value,
    pub builtin_tool_grant: Option<BuiltinMcpToolGrant>,
}

impl BuiltinCapabilityInvocation {
    fn validate_identity(&self) -> AgentResult<()> {
        if self.run_id.trim().is_empty()
            || self.call_id.trim().is_empty()
            || BuiltinCapabilityId::parse(self.managed_mcp_id.clone()).is_err()
            || self.package_name.trim().is_empty()
            || self.package_version.trim().is_empty()
            || !valid_sha256_digest(&self.upstream_catalog_digest)
            || !valid_sha256_digest(&self.policy_digest)
            || !valid_sha256_digest(&self.manifest_digest)
            || BuiltinCapabilityId::parse(self.tool_id.clone()).is_err()
            || BuiltinCapabilityId::parse(self.raw_name.clone()).is_err()
            || self.model_name.trim().is_empty()
            || !valid_sha256_digest(&self.upstream_schema_digest)
            || !valid_sha256_digest(&self.host_overlay_digest)
            || !valid_sha256_digest(&self.host_input_schema_digest)
        {
            return Err(AgentError::new("内置能力 Tool invocation identity 无效。"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BuiltinCapabilityDispatchRequest {
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub managed_mcp_id: String,
    pub package_name: String,
    pub package_version: String,
    pub upstream_catalog_digest: String,
    pub policy_digest: String,
    pub manifest_digest: String,
    pub tool_id: String,
    pub raw_name: String,
    pub model_name: String,
    pub upstream_schema_digest: String,
    pub host_overlay_digest: String,
    pub host_input_schema_digest: String,
    pub call_id: String,
    pub arguments: Value,
    pub cancellation: AgentCancellationToken,
}

/// Host implementation for manifests, policy, process-memory grants and typed invocation.
///
/// `invoke_authorized` is the final TOCTOU boundary: implementations must atomically revalidate
/// the supplied grant against live policy/manifest state before dispatching the managed server.
pub trait BuiltinCapabilityProvider: Send + Sync {
    fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>>;
    fn policy(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy>;
    fn grant(
        &self,
        run_id: &str,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<Option<CapabilityGrant>>;
    fn approve_activation(
        &self,
        approval: &AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<CapabilityGrant>;
    /// Rolls back one exact process-memory activation if durable approval settlement fails.
    fn revoke_activation(
        &self,
        activation_id: &CapabilityActivationId,
        action_id: &str,
    ) -> AgentResult<()>;
    /// Erases process-memory grants for a capability after durable user policy is disabled.
    ///
    /// Dispatch must still revalidate live policy; this hook is defense-in-depth cleanup and
    /// prevents revoked task authority from occupying process memory until natural expiry.
    fn revoke_grants(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<()>;
    /// Retires every process-only authority owned by one terminal Agent run.
    ///
    /// Grants are deliberately not durable, and terminal run cleanup must not wait for their TTL.
    /// The default keeps test/alternate Providers source-compatible while the Host implementation
    /// performs authoritative cleanup.
    fn revoke_run_grants(&self, _run_id: &str) -> AgentResult<()> {
        Ok(())
    }
    /// Freezes one exact sensitive invocation and stores its raw arguments behind a process-only
    /// approval binding before Core publishes a durable pending action.
    fn prepare_builtin_mcp_tool_approval(
        &self,
        _request: BuiltinMcpToolApprovalRequest,
    ) -> AgentResult<AgentBuiltinMcpToolApproval> {
        Err(AgentError::new(
            "当前 Host 未提供内置 MCP 敏感 Tool 审批准备能力。",
        ))
    }
    /// Freezes the exact Host-owned browser Surface before a sensitive approval is published.
    fn prepare_builtin_mcp_tool_target_binding<'a>(
        &'a self,
        _request: BuiltinMcpToolTargetBindingRequest,
    ) -> BuiltinCapabilityFuture<'a, PreparedBuiltinMcpToolTargetBinding> {
        Box::pin(async {
            Err(AgentError::new(
                "当前 Host 未提供内置 MCP 敏感 Tool 页面绑定能力。",
            ))
        })
    }
    /// Idempotent process-only cleanup used by proposal failure/reject/cancel/revoke paths.
    fn release_builtin_mcp_tool_target_binding(
        &self,
        _binding: &PreparedBuiltinMcpToolTargetBinding,
        _run_id: &str,
        _activation_id: &CapabilityActivationId,
        _call_id: &str,
        _reason: BuiltinMcpToolTargetBindingReleaseReason,
    ) -> AgentResult<()> {
        Ok(())
    }
    /// Consumes one exact pending approval and creates a process-only single-use grant.
    fn approve_builtin_mcp_tool(
        &self,
        _approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<BuiltinMcpToolGrant> {
        Err(AgentError::new(
            "当前 Host 未提供内置 MCP 敏感 Tool 批准能力。",
        ))
    }
    /// Removes a still-undispatched prepared payload after reject/cancel/expiry.
    fn dismiss_builtin_mcp_tool_approval(
        &self,
        _approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        Ok(())
    }
    fn reject_builtin_mcp_tool_approval(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        self.dismiss_builtin_mcp_tool_approval(approval)
    }
    fn revoke_builtin_mcp_tool_grant(
        &self,
        _grant_id: &str,
        _approval_id: &str,
    ) -> AgentResult<()> {
        Ok(())
    }
    /// Dispatches the prepared invocation exactly once after durable approval settlement.
    fn invoke_approved_builtin_mcp_tool<'a>(
        &'a self,
        _approval: AgentBuiltinMcpToolApproval,
        _grant: BuiltinMcpToolGrant,
        _cancellation: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async {
            Err(AgentError::new(
                "当前 Host 未提供内置 MCP 敏感 Tool 执行能力。",
            ))
        })
    }
    fn browser_risk_grant(
        &self,
        _request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<Option<BrowserRiskGrant>> {
        Ok(None)
    }
    fn prepare_browser_risk_approval(
        &self,
        request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<AgentBrowserRiskApproval> {
        build_browser_risk_approval(request, unix_timestamp())
    }
    fn approve_browser_risk(
        &self,
        _approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<BrowserRiskGrant> {
        Err(AgentError::new("当前 Host 未提供浏览器风险批准能力。"))
    }
    fn dismiss_browser_risk_approval(
        &self,
        _approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<()> {
        Err(AgentError::new(
            "当前 Host 未提供浏览器风险批准终态清理能力。",
        ))
    }
    fn revoke_browser_risk_grants(
        &self,
        _run_id: Option<&str>,
        _capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<()> {
        Ok(())
    }
    fn invoke_authorized<'a>(
        &'a self,
        invocation: BuiltinCapabilityInvocation,
        expected_grant: CapabilityGrant,
        cancellation: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinMcpToolTargetBindingReleaseReason {
    ProposalFailed,
    Rejected,
    Cancelled,
    Expired,
    RunRevoked,
    CapabilityRevoked,
    GrantRevoked,
    Shutdown,
}

pub trait BuiltinCapabilityPolicyStore: Send + Sync {
    fn policy(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy>;
    fn set_user_allowed(
        &self,
        capability_id: &BuiltinCapabilityId,
        expected_revision: u64,
        allowed: bool,
    ) -> AgentResult<BuiltinCapabilityPolicy>;
}

#[derive(Clone)]
pub struct BuiltinCapabilityRuntime {
    provider: Arc<dyn BuiltinCapabilityProvider>,
    manifests: Arc<Vec<BuiltinCapabilityManifest>>,
}

impl std::fmt::Debug for BuiltinCapabilityRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BuiltinCapabilityRuntime")
            .field("manifest_count", &self.manifests.len())
            .finish_non_exhaustive()
    }
}

impl BuiltinCapabilityRuntime {
    pub fn new(provider: Arc<dyn BuiltinCapabilityProvider>) -> AgentResult<Self> {
        let mut manifests = provider.manifests()?;
        manifests.sort_by(|left, right| left.descriptor.id.cmp(&right.descriptor.id));
        let mut capability_ids = std::collections::BTreeSet::new();
        let mut model_names = std::collections::BTreeSet::new();
        let mut managed_tool_ids = std::collections::BTreeSet::new();
        for manifest in &manifests {
            manifest.validate()?;
            if !capability_ids.insert(manifest.descriptor.id.clone()) {
                return Err(AgentError::new("Host 注册了重复的内置能力 id。"));
            }
            for tool in &manifest.tools {
                if !model_names.insert(tool.model_name.clone())
                    || !managed_tool_ids
                        .insert((manifest.managed_mcp_id.clone(), tool.tool_id.clone()))
                {
                    return Err(AgentError::new(
                        "内置能力工具 identity 冲突；manifest 工具不能互相覆盖。",
                    ));
                }
            }
        }
        Ok(Self {
            provider,
            manifests: Arc::new(manifests),
        })
    }

    pub fn manifests(&self) -> &[BuiltinCapabilityManifest] {
        self.manifests.as_slice()
    }

    pub fn manifest(
        &self,
        capability_id: &BuiltinCapabilityId,
    ) -> Option<&BuiltinCapabilityManifest> {
        self.manifests
            .iter()
            .find(|manifest| manifest.descriptor.id == *capability_id)
    }

    pub fn policy(
        &self,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<BuiltinCapabilityPolicy> {
        self.provider.policy(capability_id)
    }

    pub fn live_grant(
        &self,
        run_id: &str,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<Option<CapabilityGrant>> {
        let Some(manifest) = self.manifest(capability_id) else {
            return Ok(None);
        };
        let policy = self.policy(capability_id)?;
        let grant = self.provider.grant(run_id, capability_id)?;
        Ok(grant.filter(|grant| grant.is_live_for(run_id, manifest, &policy, unix_timestamp())))
    }

    pub fn approve_activation(
        &self,
        approval: &AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<CapabilityGrant> {
        let capability_id = BuiltinCapabilityId::parse(approval.capability_id.clone())?;
        let activation_id = CapabilityActivationId::parse(approval.activation_id.clone())?;
        let action_id = Uuid::parse_str(&approval.action_id)
            .map_err(|_| AgentError::new("内置能力 action id 无效。"))?;
        let manifest = self
            .manifest(&capability_id)
            .ok_or_else(|| AgentError::new("批准的内置能力已不存在。"))?;
        let policy = self.policy(&capability_id)?;
        let now = unix_timestamp();
        if approval.approval_status != AgentApprovalStatus::Approved
            || approval.run_id.trim().is_empty()
            || approval.call_id.trim().is_empty()
            || action_id.is_nil()
            || action_id.get_version() != Some(uuid::Version::Random)
            || action_id.to_string() != approval.action_id
            || approval.expires_at <= now
            || approval.created_at > now
            || approval.manifest_digest != manifest.manifest_digest
            || approval.policy_revision != policy.revision
        {
            return Err(AgentError::new(
                "内置能力在批准期间发生变化；本次批准已失效。",
            ));
        }
        let grant = self.provider.approve_activation(approval)?;
        let policy = self.policy(&capability_id)?;
        let grant_validation_now = unix_timestamp();
        if grant.activation_id != activation_id
            || grant.created_at < approval.created_at
            || grant.created_at > grant_validation_now
            || grant.expires_at.checked_sub(grant.created_at)
                != Some(BUILTIN_CAPABILITY_GRANT_TTL_SECONDS)
            || approval.policy_revision != policy.revision
            || !grant.is_live_for(&approval.run_id, manifest, &policy, grant_validation_now)
        {
            let _ = self
                .provider
                .revoke_activation(&grant.activation_id, &approval.action_id);
            return Err(AgentError::new(
                "Host 返回的内置能力 grant 与冻结批准不一致。",
            ));
        }
        Ok(grant)
    }

    pub fn revoke_activation(
        &self,
        activation_id: &CapabilityActivationId,
        action_id: &str,
    ) -> AgentResult<()> {
        self.provider.revoke_activation(activation_id, action_id)
    }

    pub fn revoke_grants(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<()> {
        if self.manifest(capability_id).is_none() {
            return Err(AgentError::new("撤销的内置能力未注册。"));
        }
        self.provider.revoke_grants(capability_id)
    }

    pub fn revoke_run_grants(&self, run_id: &str) -> AgentResult<()> {
        if run_id.trim().is_empty() {
            return Err(AgentError::new("撤销的内置能力 run identity 无效。"));
        }
        self.provider.revoke_run_grants(run_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare_builtin_mcp_tool_approval(
        &self,
        invocation: BuiltinCapabilityInvocation,
        call_reason: String,
        operation_category: String,
        resource_summary: BuiltinMcpToolResourceSummary,
        origin: Option<String>,
        mut risk_kinds: Vec<BuiltinMcpToolRiskKind>,
    ) -> AgentResult<AgentBuiltinMcpToolApproval> {
        invocation.validate_identity()?;
        if invocation.builtin_tool_grant.is_some() {
            return Err(AgentError::new("未批准的内置 MCP Tool 请求携带了 grant。"));
        }
        let manifest = self
            .manifest(&invocation.capability_id)
            .ok_or_else(|| AgentError::new("内置 MCP Tool 所属能力已不存在。"))?;
        let policy = self.policy(&invocation.capability_id)?;
        let capability_grant = self
            .live_grant(&invocation.run_id, &invocation.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的内置能力授权。"))?;
        let descriptor = manifest
            .tools
            .iter()
            .find(|tool| exact_invocation_matches_descriptor(&invocation, manifest, tool))
            .ok_or_else(|| AgentError::new("敏感 Tool 不在当前审核 manifest 中。"))?;
        risk_kinds.sort_unstable();
        risk_kinds.dedup();
        if descriptor.builtin_approval_mode == BuiltinMcpToolApprovalMode::Never
            || !builtin_tool_requires_approval(descriptor, &invocation.arguments)
            || risk_kinds.is_empty()
            || risk_kinds != descriptor.builtin_risk_kinds
            || invocation.policy_revision != policy.revision
            || invocation.activation_id != capability_grant.activation_id
        {
            return Err(AgentError::new(
                "内置 MCP Tool 敏感审批与 reviewed policy 不一致。",
            ));
        }
        validate_builtin_sensitive_target_scope(&invocation)?;
        let arguments_digest = builtin_mcp_tool_arguments_digest(&invocation.arguments)?;
        let resource_scope_digest = builtin_mcp_tool_resource_scope_digest(
            &invocation.tool_id,
            &arguments_digest,
            origin.as_deref(),
            &risk_kinds,
            &resource_summary.scope,
        )?;
        let request = BuiltinMcpToolApprovalRequest {
            invocation,
            capability_grant,
            capability_display_name: manifest.descriptor.display_name.clone(),
            tool_display_name: descriptor.model_name.clone(),
            call_reason,
            operation_category,
            resource_summary,
            resource_scope_digest,
            origin,
            risk_kinds,
            target_binding: None,
        };
        let approval = self.provider.prepare_builtin_mcp_tool_approval(request)?;
        validate_builtin_mcp_tool_approval_shape(&approval)?;
        Ok(approval)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_builtin_mcp_tool_approval_async(
        &self,
        invocation: BuiltinCapabilityInvocation,
        call_reason: String,
        operation_category: String,
        mut resource_summary: BuiltinMcpToolResourceSummary,
        file_preparation: Option<BuiltinMcpToolFilePreparation>,
        mut risk_kinds: Vec<BuiltinMcpToolRiskKind>,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentBuiltinMcpToolApproval> {
        invocation.validate_identity()?;
        cancellation.check()?;
        if invocation.builtin_tool_grant.is_some() {
            return Err(AgentError::new("未批准的内置 MCP Tool 请求携带了 grant。"));
        }
        let manifest = self
            .manifest(&invocation.capability_id)
            .ok_or_else(|| AgentError::new("内置 MCP Tool 所属能力已不存在。"))?;
        let policy = self.policy(&invocation.capability_id)?;
        let capability_grant = self
            .live_grant(&invocation.run_id, &invocation.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的内置能力授权。"))?;
        let descriptor = manifest
            .tools
            .iter()
            .find(|tool| exact_invocation_matches_descriptor(&invocation, manifest, tool))
            .ok_or_else(|| AgentError::new("敏感 Tool 不在当前审核 manifest 中。"))?;
        risk_kinds.sort_unstable();
        risk_kinds.dedup();
        if descriptor.builtin_approval_mode == BuiltinMcpToolApprovalMode::Never
            || !builtin_tool_requires_approval(descriptor, &invocation.arguments)
            || risk_kinds.is_empty()
            || risk_kinds != descriptor.builtin_risk_kinds
            || invocation.policy_revision != policy.revision
            || invocation.activation_id != capability_grant.activation_id
        {
            return Err(AgentError::new(
                "内置 MCP Tool 敏感审批与 reviewed policy 不一致。",
            ));
        }
        validate_builtin_sensitive_target_scope(&invocation)?;
        let arguments_digest = builtin_mcp_tool_arguments_digest(&invocation.arguments)?;
        let created_at = unix_timestamp();
        let expires_at = created_at
            .checked_add(BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS)
            .ok_or_else(|| AgentError::new("内置 MCP Tool 页面绑定时间溢出。"))?
            .min(capability_grant.expires_at);
        if expires_at <= created_at {
            return Err(AgentError::new(
                "内置 MCP Tool capability grant 已接近过期。",
            ));
        }
        let target_binding = self
            .provider
            .prepare_builtin_mcp_tool_target_binding(BuiltinMcpToolTargetBindingRequest {
                binding_request_id: Uuid::new_v4().to_string(),
                binding_scope: builtin_mcp_tool_binding_scope(&invocation),
                invocation: invocation.clone(),
                capability_grant: capability_grant.clone(),
                arguments_digest: arguments_digest.clone(),
                created_at,
                expires_at,
                cancellation: cancellation.clone(),
                file_preparation,
            })
            .await?;
        let target_binding_id = Uuid::parse_str(&target_binding.binding_id);
        if cancellation.is_cancelled() {
            let _ = self.provider.release_builtin_mcp_tool_target_binding(
                &target_binding,
                &invocation.run_id,
                &invocation.activation_id,
                &invocation.call_id,
                BuiltinMcpToolTargetBindingReleaseReason::Cancelled,
            );
            cancellation.check()?;
            return Err(AgentError::new(
                "内置 MCP Tool 页面绑定在审批发布前已取消。",
            ));
        }
        if target_binding_id.ok().and_then(|id| id.get_version()) != Some(uuid::Version::Random)
            || !valid_sha256_digest(&target_binding.target_binding_digest)
            || target_binding.created_at != created_at
            || target_binding.expires_at != expires_at
        {
            let _ = self.provider.release_builtin_mcp_tool_target_binding(
                &target_binding,
                &invocation.run_id,
                &invocation.activation_id,
                &invocation.call_id,
                BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
            );
            return Err(AgentError::new(
                "Main 返回的敏感 Tool 页面绑定 identity 无效或 origin 已漂移。",
            ));
        }
        if let Err(error) = validate_optional_origin(target_binding.origin.as_deref()) {
            let _ = self.provider.release_builtin_mcp_tool_target_binding(
                &target_binding,
                &invocation.run_id,
                &invocation.activation_id,
                &invocation.call_id,
                BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
            );
            return Err(error);
        }
        resource_summary.origin = target_binding.origin.clone();
        resource_summary.file_basenames = target_binding.file_basenames.clone();
        let resource_scope_digest = match builtin_mcp_tool_resource_scope_digest_v2(
            &invocation.tool_id,
            &arguments_digest,
            target_binding.origin.as_deref(),
            &risk_kinds,
            &resource_summary.scope,
            &target_binding.target_binding_digest,
        ) {
            Ok(digest) => digest,
            Err(error) => {
                let _ = self.provider.release_builtin_mcp_tool_target_binding(
                    &target_binding,
                    &invocation.run_id,
                    &invocation.activation_id,
                    &invocation.call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
                );
                return Err(error);
            }
        };
        let cleanup_binding = target_binding.clone();
        let cleanup_run_id = invocation.run_id.clone();
        let cleanup_activation_id = invocation.activation_id.clone();
        let cleanup_call_id = invocation.call_id.clone();
        let request = BuiltinMcpToolApprovalRequest {
            invocation,
            capability_grant,
            capability_display_name: manifest.descriptor.display_name.clone(),
            tool_display_name: descriptor.model_name.clone(),
            call_reason,
            operation_category,
            resource_summary,
            resource_scope_digest,
            origin: target_binding.origin.clone(),
            risk_kinds,
            target_binding: Some(target_binding),
        };
        match self.provider.prepare_builtin_mcp_tool_approval(request) {
            Ok(approval) => {
                if let Err(error) = validate_builtin_mcp_tool_approval_shape(&approval) {
                    let _ = self.provider.dismiss_builtin_mcp_tool_approval(&approval);
                    let _ = self.provider.release_builtin_mcp_tool_target_binding(
                        &cleanup_binding,
                        &cleanup_run_id,
                        &cleanup_activation_id,
                        &cleanup_call_id,
                        BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
                    );
                    return Err(error);
                }
                Ok(approval)
            }
            Err(error) => {
                let _ = self.provider.release_builtin_mcp_tool_target_binding(
                    &cleanup_binding,
                    &cleanup_run_id,
                    &cleanup_activation_id,
                    &cleanup_call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
                );
                Err(error)
            }
        }
    }

    pub fn approve_builtin_mcp_tool(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<BuiltinMcpToolGrant> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        let identity = &approval.identity;
        let capability_id = BuiltinCapabilityId::parse(identity.capability_id.clone())?;
        let activation_id =
            CapabilityActivationId::parse(identity.capability_activation_id.clone())?;
        let manifest = self
            .manifest(&capability_id)
            .ok_or_else(|| AgentError::new("批准的内置 MCP Tool 能力已不存在。"))?;
        let policy = self.policy(&capability_id)?;
        let capability_grant = self
            .live_grant(&identity.run_id, &capability_id)?
            .ok_or_else(|| AgentError::new("内置 MCP Tool capability grant 已失效。"))?;
        let descriptor = manifest
            .tools
            .iter()
            .find(|tool| exact_approval_matches_descriptor(identity, manifest, tool))
            .ok_or_else(|| AgentError::new("批准的内置 MCP Tool identity 已漂移。"))?;
        let now = unix_timestamp();
        if approval.approval_status != AgentApprovalStatus::Approved
            || descriptor.builtin_approval_mode == BuiltinMcpToolApprovalMode::Never
            || approval.risk_kinds != descriptor.builtin_risk_kinds
            || capability_grant.activation_id != activation_id
            || identity.policy_revision != policy.revision
            || approval.created_at > now
            || approval.expires_at <= now
        {
            return Err(AgentError::new(
                "内置 MCP Tool 批准在等待期间发生漂移；已安全失效。",
            ));
        }
        let grant = self.provider.approve_builtin_mcp_tool(approval)?;
        if !grant.is_live_for(approval, &capability_grant, now) {
            return Err(AgentError::new("Host 返回了无效的内置 MCP Tool grant。"));
        }
        Ok(grant)
    }

    pub fn dismiss_builtin_mcp_tool_approval(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        self.provider.dismiss_builtin_mcp_tool_approval(approval)
    }

    pub fn reject_builtin_mcp_tool_approval(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        self.provider.reject_builtin_mcp_tool_approval(approval)
    }

    pub fn revoke_builtin_mcp_tool_grant(
        &self,
        grant_id: &str,
        approval_id: &str,
    ) -> AgentResult<()> {
        self.provider
            .revoke_builtin_mcp_tool_grant(grant_id, approval_id)
    }

    pub async fn invoke_approved_builtin_mcp_tool(
        &self,
        approval: AgentBuiltinMcpToolApproval,
        grant: BuiltinMcpToolGrant,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<Value> {
        self.provider
            .invoke_approved_builtin_mcp_tool(approval, grant, cancellation)
            .await
    }

    pub fn live_browser_risk_grant(
        &self,
        request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<Option<BrowserRiskGrant>> {
        request.validate()?;
        let manifest = self
            .manifest(&request.capability_id)
            .ok_or_else(|| AgentError::new("浏览器风险请求的内置能力已不存在。"))?;
        let policy = self.policy(&request.capability_id)?;
        let capability_grant = self
            .live_grant(&request.run_id, &request.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的浏览器能力授权。"))?;
        if request.capability_activation_id != capability_grant.activation_id
            || request.manifest_digest != manifest.manifest_digest
            || request.policy_revision != policy.revision
            || request.display_name != manifest.descriptor.display_name
            || !manifest
                .tools
                .iter()
                .any(|tool| tool.tool_id == request.trigger_tool_name)
        {
            return Err(AgentError::new(
                "浏览器风险请求与当前 capability grant 不一致。",
            ));
        }
        let grant = self.provider.browser_risk_grant(request)?;
        Ok(grant.filter(|grant| {
            grant.is_live_for_request(&capability_grant, request, unix_timestamp())
        }))
    }

    pub fn prepare_browser_risk_approval(
        &self,
        request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<AgentBrowserRiskApproval> {
        // Run the same final live capability checks used by grant lookup before publishing an
        // approval. The Provider stores any Host-only resolver binding process-locally.
        if self.live_browser_risk_grant(request)?.is_some() {
            return Err(AgentError::new(
                "浏览器目标已由当前任务的精确 risk grant 授权。",
            ));
        }
        let approval = self.provider.prepare_browser_risk_approval(request)?;
        validate_browser_risk_approval_shape(&approval)?;
        if approval.run_id != request.run_id
            || approval.call_id != request.call_id
            || approval.trigger_tool_name != request.trigger_tool_name
            || approval.capability_id != request.capability_id.as_str()
            || approval.capability_activation_id != request.capability_activation_id.as_str()
            || approval.manifest_digest != request.manifest_digest
            || approval.policy_revision != request.policy_revision
            || approval.destination != request.destination
            || approval.trigger != request.trigger
            || approval.risk_kinds != request.risk_kinds
        {
            return Err(AgentError::new(
                "Host 生成的浏览器风险批准与冻结请求不一致。",
            ));
        }
        Ok(approval)
    }

    pub fn approve_browser_risk(
        &self,
        approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<BrowserRiskGrant> {
        validate_browser_risk_approval_shape(approval)?;
        let capability_id = BuiltinCapabilityId::parse(approval.capability_id.clone())?;
        let activation_id =
            CapabilityActivationId::parse(approval.capability_activation_id.clone())?;
        let manifest = self
            .manifest(&capability_id)
            .ok_or_else(|| AgentError::new("批准的浏览器能力已不存在。"))?;
        let policy = self.policy(&capability_id)?;
        let capability_grant = self
            .live_grant(&approval.run_id, &capability_id)?
            .ok_or_else(|| AgentError::new("浏览器能力 grant 已失效。"))?;
        let now = unix_timestamp();
        if approval.approval_status != AgentApprovalStatus::Approved
            || activation_id != capability_grant.activation_id
            || approval.manifest_digest != manifest.manifest_digest
            || approval.policy_revision != policy.revision
            || approval.display_name != manifest.descriptor.display_name
            || !manifest
                .tools
                .iter()
                .any(|tool| tool.tool_id == approval.trigger_tool_name)
            || approval.created_at > now
            || approval.expires_at <= now
        {
            return Err(AgentError::new(
                "浏览器风险批准在等待期间发生漂移；已安全失效。",
            ));
        }
        let grant = self.provider.approve_browser_risk(approval)?;
        if !grant.is_live_for_approval(&capability_grant, approval, now) {
            return Err(AgentError::new("Host 返回了无效的浏览器风险 grant。"));
        }
        Ok(grant)
    }

    /// Atomically consumes a process-only risk approval without creating a grant.
    pub fn dismiss_browser_risk_approval(
        &self,
        approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<()> {
        validate_browser_risk_approval_shape(approval)?;
        self.provider.dismiss_browser_risk_approval(approval)
    }

    pub fn revoke_browser_risk_grants(
        &self,
        run_id: Option<&str>,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<()> {
        if self.manifest(capability_id).is_none() {
            return Err(AgentError::new("撤销的内置能力未注册。"));
        }
        self.provider
            .revoke_browser_risk_grants(run_id, capability_id)
    }

    pub(crate) async fn invoke(
        &self,
        request: BuiltinCapabilityDispatchRequest,
    ) -> AgentResult<Value> {
        let manifest = self
            .manifest(&request.capability_id)
            .ok_or_else(|| AgentError::new("内置能力已不可用。"))?;
        let reviewed_tool = manifest
            .tools
            .iter()
            .find(|tool| tool.tool_id == request.tool_id && tool.model_name == request.model_name);
        if manifest.managed_mcp_id != request.managed_mcp_id
            || manifest.provider_contract.package_name != request.package_name
            || manifest.provider_contract.package_version != request.package_version
            || manifest.provider_contract.upstream_catalog_digest != request.upstream_catalog_digest
            || manifest.provider_contract.policy_digest != request.policy_digest
            || manifest.manifest_digest != request.manifest_digest
            || reviewed_tool.is_none_or(|tool| {
                tool.raw_name != request.raw_name
                    || tool.upstream_schema_digest != request.upstream_schema_digest
                    || tool.host_overlay_digest != request.host_overlay_digest
                    || tool.schema_digest != request.host_input_schema_digest
            })
        {
            return Err(AgentError::new("内置能力工具不在已审核 manifest 中。"));
        }
        if reviewed_tool
            .is_some_and(|tool| builtin_tool_requires_approval(tool, &request.arguments))
        {
            return Err(AgentError::new(
                "敏感内置 MCP Tool 缺少原调用生命周期批准。",
            ));
        }
        let policy = self.policy(&request.capability_id)?;
        let grant = self
            .live_grant(&request.run_id, &request.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的内置能力授权。"))?;
        let invocation = BuiltinCapabilityInvocation {
            run_id: request.run_id,
            capability_id: request.capability_id,
            managed_mcp_id: request.managed_mcp_id,
            package_name: request.package_name,
            package_version: request.package_version,
            upstream_catalog_digest: request.upstream_catalog_digest,
            policy_digest: request.policy_digest,
            activation_id: grant.activation_id.clone(),
            manifest_digest: request.manifest_digest,
            policy_revision: policy.revision,
            tool_id: request.tool_id,
            raw_name: request.raw_name,
            model_name: request.model_name,
            upstream_schema_digest: request.upstream_schema_digest,
            host_overlay_digest: request.host_overlay_digest,
            host_input_schema_digest: request.host_input_schema_digest,
            call_id: request.call_id,
            arguments: request.arguments,
            builtin_tool_grant: None,
        };
        self.provider
            .invoke_authorized(invocation, grant, request.cancellation)
            .await
    }
}

pub fn build_browser_risk_approval(
    request: &BrowserRiskAuthorizationRequest,
    now: u64,
) -> AgentResult<AgentBrowserRiskApproval> {
    request.validate()?;
    let expires_at = now
        .checked_add(BROWSER_RISK_APPROVAL_TTL_SECONDS)
        .ok_or_else(|| AgentError::new("浏览器风险批准时间溢出。"))?;
    Ok(AgentBrowserRiskApproval {
        schema_version: BROWSER_RISK_APPROVAL_SCHEMA_VERSION,
        action_id: Uuid::new_v4().to_string(),
        risk_approval_id: Uuid::new_v4().to_string(),
        run_id: request.run_id.clone(),
        call_id: request.call_id.clone(),
        trigger_tool_name: request.trigger_tool_name.clone(),
        capability_id: request.capability_id.as_str().to_string(),
        capability_activation_id: request.capability_activation_id.as_str().to_string(),
        display_name: request.display_name.clone(),
        reason: request.reason.clone(),
        destination: request.destination.clone(),
        trigger: request.trigger,
        risk_kinds: request.risk_kinds.clone(),
        manifest_digest: request.manifest_digest.clone(),
        policy_revision: request.policy_revision,
        created_at: now,
        expires_at,
        approval_status: AgentApprovalStatus::Required,
    })
}

pub fn build_builtin_mcp_tool_approval(
    request: &BuiltinMcpToolApprovalRequest,
    now: u64,
) -> AgentResult<AgentBuiltinMcpToolApproval> {
    request.validate()?;
    let (created_at, expires_at) = if let Some(binding) = &request.target_binding {
        (binding.created_at, binding.expires_at)
    } else {
        (
            now,
            now.checked_add(BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS)
                .ok_or_else(|| AgentError::new("内置 MCP Tool 审批时间溢出。"))?,
        )
    };
    let arguments_digest = builtin_mcp_tool_arguments_digest(&request.invocation.arguments)?;
    let invocation = &request.invocation;
    Ok(AgentBuiltinMcpToolApproval {
        schema_version: BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION,
        identity: BuiltinMcpToolApprovalIdentity {
            action_id: Uuid::new_v4().to_string(),
            approval_id: Uuid::new_v4().to_string(),
            run_id: invocation.run_id.clone(),
            call_id: invocation.call_id.clone(),
            capability_id: invocation.capability_id.as_str().to_string(),
            capability_activation_id: invocation.activation_id.as_str().to_string(),
            managed_mcp_id: invocation.managed_mcp_id.clone(),
            package_name: invocation.package_name.clone(),
            package_version: invocation.package_version.clone(),
            upstream_catalog_digest: invocation.upstream_catalog_digest.clone(),
            manifest_digest: invocation.manifest_digest.clone(),
            policy_digest: invocation.policy_digest.clone(),
            policy_revision: invocation.policy_revision,
            tool_id: invocation.tool_id.clone(),
            raw_name: invocation.raw_name.clone(),
            model_name: invocation.model_name.clone(),
            upstream_schema_digest: invocation.upstream_schema_digest.clone(),
            host_overlay_digest: invocation.host_overlay_digest.clone(),
            host_input_schema_digest: invocation.host_input_schema_digest.clone(),
            arguments_digest,
            resource_scope_digest: request.resource_scope_digest.clone(),
            origin: request.origin.clone(),
        },
        capability_display_name: request.capability_display_name.clone(),
        tool_display_name: request.tool_display_name.clone(),
        call_reason: request.call_reason.clone(),
        operation_category: request.operation_category.clone(),
        resource_summary: request.resource_summary.clone(),
        risk_kinds: request.risk_kinds.clone(),
        created_at,
        expires_at,
        approval_status: AgentApprovalStatus::Required,
    })
}

/// Deterministic safe resource binding shared with Main.
///
/// `arguments_digest` is computed over the complete model/Host input object before stripping
/// `call_reason`. Opaque FileBroker handles are bound separately without revealing a path or file
/// content. Main recomputes this digest before applying overlays.
pub fn builtin_mcp_tool_resource_scope_digest(
    tool_id: &str,
    arguments_digest: &str,
    origin: Option<&str>,
    risk_kinds: &[BuiltinMcpToolRiskKind],
    scope: &str,
) -> AgentResult<String> {
    if BuiltinCapabilityId::parse(tool_id.to_string()).is_err()
        || !valid_sha256_digest(arguments_digest)
        || !valid_policy_token(scope)
    {
        return Err(AgentError::new(
            "内置 MCP Tool resource scope identity 无效。",
        ));
    }
    validate_optional_origin(origin)?;
    let mut risks = risk_kinds.to_vec();
    risks.sort_unstable();
    if risks.is_empty()
        || risks.len() > BUILTIN_MCP_RISK_MAX_COUNT
        || risks.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(AgentError::new("内置 MCP Tool resource scope 风险无效。"));
    }
    let material = json!({
        "schemaVersion": 1,
        "toolId": tool_id,
        "argumentsDigest": arguments_digest,
        "origin": origin,
        "riskKinds": risks,
        "scope": scope,
    });
    let bytes = serde_json::to_vec(&material)
        .map_err(|_| AgentError::new("无法计算内置 MCP Tool resource scope digest。"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Version 2 additionally binds the Main-owned exact Surface generation digest. The opaque
/// binding itself stays process-only; this digest is the only durable approval projection.
pub fn builtin_mcp_tool_resource_scope_digest_v2(
    tool_id: &str,
    arguments_digest: &str,
    origin: Option<&str>,
    risk_kinds: &[BuiltinMcpToolRiskKind],
    scope: &str,
    target_binding_digest: &str,
) -> AgentResult<String> {
    if BuiltinCapabilityId::parse(tool_id.to_string()).is_err()
        || !valid_sha256_digest(arguments_digest)
        || !valid_sha256_digest(target_binding_digest)
        || !valid_policy_token(scope)
    {
        return Err(AgentError::new(
            "内置 MCP Tool resource scope identity 无效。",
        ));
    }
    validate_optional_origin(origin)?;
    let mut risks = risk_kinds.to_vec();
    risks.sort_unstable();
    if risks.is_empty()
        || risks.len() > BUILTIN_MCP_RISK_MAX_COUNT
        || risks.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(AgentError::new("内置 MCP Tool resource scope 风险无效。"));
    }
    let material = json!({
        "schemaVersion": 2,
        "toolId": tool_id,
        "argumentsDigest": arguments_digest,
        "origin": origin,
        "riskKinds": risks,
        "scope": scope,
        "targetBindingDigest": target_binding_digest,
    });
    let bytes = serde_json::to_vec(&material)
        .map_err(|_| AgentError::new("无法计算内置 MCP Tool resource scope digest。"))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

/// Canonical SHA-256 binding used only by the built-in sensitive-Tool authorization wire.
///
/// External MCP approval identities intentionally retain their historical bare-hex digest. The
/// managed Playwright bridge and Main validator require an explicit algorithm prefix, so this
/// adapter must remain separate rather than changing `mcp_tool_arguments_digest` globally.
pub fn builtin_mcp_tool_arguments_digest(arguments: &Value) -> AgentResult<String> {
    Ok(format!(
        "sha256:{}",
        crate::tools::mcp_tool_arguments_digest(arguments)?
    ))
}

pub fn validate_builtin_mcp_tool_approval_shape(
    approval: &AgentBuiltinMcpToolApproval,
) -> AgentResult<()> {
    let identity = &approval.identity;
    parse_v4_uuid(&identity.action_id, "内置 MCP Tool action")?;
    parse_v4_uuid(&identity.approval_id, "内置 MCP Tool approval")?;
    BuiltinCapabilityId::parse(identity.capability_id.clone())?;
    CapabilityActivationId::parse(identity.capability_activation_id.clone())?;
    if approval.schema_version != BUILTIN_MCP_TOOL_APPROVAL_SCHEMA_VERSION
        || identity.run_id.trim().is_empty()
        || identity.call_id.trim().is_empty()
        || BuiltinCapabilityId::parse(identity.managed_mcp_id.clone()).is_err()
        || identity.package_name.trim().is_empty()
        || identity.package_version.trim().is_empty()
        || !valid_sha256_digest(&identity.upstream_catalog_digest)
        || !valid_sha256_digest(&identity.manifest_digest)
        || !valid_sha256_digest(&identity.policy_digest)
        || BuiltinCapabilityId::parse(identity.tool_id.clone()).is_err()
        || BuiltinCapabilityId::parse(identity.raw_name.clone()).is_err()
        || identity.model_name.trim().is_empty()
        || !valid_sha256_digest(&identity.upstream_schema_digest)
        || !valid_sha256_digest(&identity.host_overlay_digest)
        || !valid_sha256_digest(&identity.host_input_schema_digest)
        || !valid_sha256_digest(&identity.arguments_digest)
        || !valid_sha256_digest(&identity.resource_scope_digest)
        || approval.capability_display_name.trim().is_empty()
        || approval.capability_display_name.len() > DISPLAY_NAME_MAX_BYTES
        || approval.tool_display_name.trim().is_empty()
        || approval.tool_display_name.len() > DISPLAY_NAME_MAX_BYTES
        || approval.call_reason.trim().is_empty()
        || approval.call_reason.len() > REASON_MAX_BYTES
        || !valid_policy_token(&approval.operation_category)
        || approval.risk_kinds.is_empty()
        || approval.risk_kinds.len() > BUILTIN_MCP_RISK_MAX_COUNT
        || approval.expires_at.checked_sub(approval.created_at)
            != Some(BUILTIN_MCP_TOOL_APPROVAL_TTL_SECONDS)
    {
        return Err(AgentError::new("内置 MCP Tool 审批字段无效。"));
    }
    let mut risks = approval.risk_kinds.clone();
    risks.sort_unstable();
    if risks.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(AgentError::new("内置 MCP Tool 审批风险分类重复。"));
    }
    validate_builtin_resource_summary(&approval.resource_summary)?;
    validate_optional_origin(identity.origin.as_deref())?;
    if approval.resource_summary.origin != identity.origin {
        return Err(AgentError::new("内置 MCP Tool 审批 origin 投影不一致。"));
    }
    Ok(())
}

pub fn builtin_mcp_tool_rejected_result(
    approval: &AgentBuiltinMcpToolApproval,
    user_feedback: Option<&str>,
) -> AgentToolResult {
    let feedback = user_feedback.and_then(sanitize_user_feedback);
    AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.model_name.clone(),
        ok: true,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "builtin_mcp_tool_approval",
            "status": "rejected",
            "decision": "rejected",
            "userFeedback": feedback,
            "retryable": false,
            "recovery": "user_refused_do_not_retry",
            "dispatchCertainty": "definitely_not_dispatched",
            "contentOmitted": true,
        })),
        error: None,
    }
}

pub fn builtin_mcp_tool_cancelled_result(
    approval: &AgentBuiltinMcpToolApproval,
) -> AgentToolResult {
    builtin_mcp_tool_terminal_result(
        approval,
        "cancelled",
        "mcp.tool_cancelled_before_dispatch",
        "definitely_not_dispatched",
        false,
    )
}

pub fn builtin_mcp_tool_expired_result(approval: &AgentBuiltinMcpToolApproval) -> AgentToolResult {
    builtin_mcp_tool_terminal_result(
        approval,
        "expired",
        "mcp.tool_approval_expired",
        "definitely_not_dispatched",
        false,
    )
}

pub fn builtin_mcp_tool_payload_unavailable_result(
    approval: &AgentBuiltinMcpToolApproval,
) -> AgentToolResult {
    builtin_mcp_tool_terminal_result(
        approval,
        "payload_unavailable",
        "mcp.approval_payload_unavailable",
        "definitely_not_dispatched",
        false,
    )
}

pub fn builtin_mcp_tool_outcome_unknown_result(
    approval: &AgentBuiltinMcpToolApproval,
) -> AgentToolResult {
    builtin_mcp_tool_terminal_result(
        approval,
        "outcome_unknown",
        "mcp.tool_outcome_unknown",
        "possibly_dispatched",
        false,
    )
}

fn builtin_mcp_tool_terminal_result(
    approval: &AgentBuiltinMcpToolApproval,
    status: &str,
    error_code: &str,
    dispatch_certainty: &str,
    ok: bool,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.model_name.clone(),
        ok,
        result: Some(json!({
            "schemaVersion": 1,
            "type": "builtin_mcp_tool_approval",
            "status": status,
            "errorCode": error_code,
            "retryable": false,
            "dispatchCertainty": dispatch_certainty,
            "contentOmitted": true,
        })),
        error: (!ok).then(|| {
            "The sensitive built-in MCP tool did not produce an authoritative response.".to_string()
        }),
    }
}

fn validate_builtin_resource_summary(summary: &BuiltinMcpToolResourceSummary) -> AgentResult<()> {
    if !valid_policy_token(&summary.scope)
        || summary.display_name.trim().is_empty()
        || summary.display_name.len() > BUILTIN_MCP_SAFE_DISPLAY_MAX_BYTES
        || summary.file_basenames.len() > BUILTIN_MCP_FILE_BASENAME_MAX_COUNT
        || summary.file_basenames.iter().any(|name| {
            name.trim().is_empty()
                || name.len() > 255
                || name.contains(['/', '\\'])
                || name.chars().any(char::is_control)
        })
    {
        return Err(AgentError::new("内置 MCP Tool 资源摘要无效。"));
    }
    validate_optional_origin(summary.origin.as_deref())
}

fn validate_optional_origin(origin: Option<&str>) -> AgentResult<()> {
    let Some(origin) = origin else {
        return Ok(());
    };
    let lower = origin.to_ascii_lowercase();
    let has_http_scheme = lower.starts_with("https://") || lower.starts_with("http://");
    let authority = origin.split_once("://").map(|(_, rest)| rest).unwrap_or("");
    if !has_http_scheme
        || origin.len() > BROWSER_DESTINATION_MAX_BYTES
        || authority.is_empty()
        || authority.contains(['/', '?', '#', '@'])
        || origin.chars().any(char::is_control)
    {
        return Err(AgentError::new(
            "内置 MCP Tool Host origin 必须是无凭据、无路径的 HTTP(S) origin。",
        ));
    }
    Ok(())
}

fn valid_policy_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn exact_invocation_matches_descriptor(
    invocation: &BuiltinCapabilityInvocation,
    manifest: &BuiltinCapabilityManifest,
    descriptor: &BuiltinCapabilityToolDescriptor,
) -> bool {
    invocation.managed_mcp_id == manifest.managed_mcp_id
        && invocation.package_name == manifest.provider_contract.package_name
        && invocation.package_version == manifest.provider_contract.package_version
        && invocation.upstream_catalog_digest == manifest.provider_contract.upstream_catalog_digest
        && invocation.policy_digest == manifest.provider_contract.policy_digest
        && invocation.manifest_digest == manifest.manifest_digest
        && invocation.tool_id == descriptor.tool_id
        && invocation.raw_name == descriptor.raw_name
        && invocation.model_name == descriptor.model_name
        && invocation.upstream_schema_digest == descriptor.upstream_schema_digest
        && invocation.host_overlay_digest == descriptor.host_overlay_digest
        && invocation.host_input_schema_digest == descriptor.schema_digest
}

fn exact_approval_matches_descriptor(
    identity: &BuiltinMcpToolApprovalIdentity,
    manifest: &BuiltinCapabilityManifest,
    descriptor: &BuiltinCapabilityToolDescriptor,
) -> bool {
    identity.managed_mcp_id == manifest.managed_mcp_id
        && identity.package_name == manifest.provider_contract.package_name
        && identity.package_version == manifest.provider_contract.package_version
        && identity.upstream_catalog_digest == manifest.provider_contract.upstream_catalog_digest
        && identity.policy_digest == manifest.provider_contract.policy_digest
        && identity.manifest_digest == manifest.manifest_digest
        && identity.tool_id == descriptor.tool_id
        && identity.raw_name == descriptor.raw_name
        && identity.model_name == descriptor.model_name
        && identity.upstream_schema_digest == descriptor.upstream_schema_digest
        && identity.host_overlay_digest == descriptor.host_overlay_digest
        && identity.host_input_schema_digest == descriptor.schema_digest
}

fn sanitize_user_feedback(value: &str) -> Option<String> {
    sanitize_builtin_mcp_safe_text(value, 1_024)
}

pub(crate) fn sanitize_builtin_mcp_safe_text(value: &str, max_bytes: usize) -> Option<String> {
    let value = value
        .chars()
        .map(|character| {
            if character.is_control()
                || matches!(
                    character,
                    '\u{00ad}'
                        | '\u{061c}'
                        | '\u{200b}'..='\u{200f}'
                        | '\u{2028}'..='\u{202e}'
                        | '\u{2060}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{feff}'
                )
            {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    let lower = value.to_ascii_lowercase();
    const SECRET_MARKERS: &[&str] = &[
        "authorization:",
        "authorization=",
        "authorization bearer ",
        "proxy-authorization:",
        "cookie:",
        "cookie=",
        "set-cookie:",
        "password:",
        "password=",
        "passwd:",
        "passwd=",
        "token:",
        "token=",
        "secret:",
        "secret=",
        "api_key:",
        "api_key=",
        "apikey:",
        "apikey=",
        "access_token:",
        "access_token=",
        "refresh_token:",
        "refresh_token=",
    ];
    let contains_url_private_data = (lower.contains("http://") || lower.contains("https://"))
        && (lower.contains('?') || lower.contains('@'));
    if contains_url_private_data || SECRET_MARKERS.iter().any(|marker| lower.contains(marker)) {
        return Some("[sensitive text omitted]".to_string());
    }
    let mut end = value.len().min(max_bytes);
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (end > 0).then(|| value[..end].to_string())
}

pub fn browser_risk_rejected_result(
    approval: &AgentBrowserRiskApproval,
    user_feedback: Option<&str>,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: approval.call_id.clone(),
        tool: "browser_risk_authorization".to_string(),
        ok: true,
        result: Some(json!({
            "status": "rejected",
            "destinationOrigin": approval.destination.origin,
            "riskKinds": approval.risk_kinds,
            "userFeedback": user_feedback,
            "recovery": "userRefusedDoNotRetry",
            "message": "The user rejected this browser destination. Do not repeat the same request unless the user changes direction."
        })),
        error: None,
    }
}

pub fn validate_browser_risk_approval_shape(
    approval: &AgentBrowserRiskApproval,
) -> AgentResult<()> {
    parse_v4_uuid(&approval.action_id, "浏览器风险 action")?;
    parse_v4_uuid(&approval.risk_approval_id, "浏览器风险 approval")?;
    if approval.schema_version != BROWSER_RISK_APPROVAL_SCHEMA_VERSION
        || approval.run_id.trim().is_empty()
        || approval.call_id.trim().is_empty()
        || !valid_browser_tool_name(&approval.trigger_tool_name)
        || approval.display_name.trim().is_empty()
        || approval.reason.trim().is_empty()
        || approval.reason.len() > REASON_MAX_BYTES
        || approval.expires_at.checked_sub(approval.created_at)
            != Some(BROWSER_RISK_APPROVAL_TTL_SECONDS)
        || !valid_sha256_digest(&approval.manifest_digest)
    {
        return Err(AgentError::new("浏览器风险批准字段无效。"));
    }
    BuiltinCapabilityId::parse(approval.capability_id.clone())?;
    CapabilityActivationId::parse(approval.capability_activation_id.clone())?;
    validate_browser_destination(&approval.destination)?;
    validate_browser_risk_kinds(&approval.risk_kinds)
}

fn validate_browser_destination(destination: &BrowserDestinationIdentity) -> AgentResult<()> {
    let value_size = destination
        .normalized_url
        .len()
        .saturating_add(destination.origin.len())
        .saturating_add(destination.scheme.len())
        .saturating_add(destination.ascii_host.len());
    if value_size > BROWSER_DESTINATION_MAX_BYTES
        || destination.normalized_url.len() > 2_048
        || destination.origin.len() > 512
        || !matches!(destination.scheme.as_str(), "http" | "https")
        || !valid_ascii_url_host(&destination.ascii_host)
        || destination.effective_port == 0
        || destination.normalized_url.contains(['?', '#'])
        || [
            destination.normalized_url.as_str(),
            destination.origin.as_str(),
            destination.scheme.as_str(),
            destination.ascii_host.as_str(),
        ]
        .iter()
        .any(|value| value.is_empty() || value.contains('\0'))
    {
        return Err(AgentError::new("浏览器目标安全 identity 无效。"));
    }

    // Parse and reconstruct instead of trusting several correlated wire fields independently.
    // An exact canonical comparison rejects URL parser normalization tricks, credentials hidden
    // in authority, default-port aliases, and mismatched origin/host identities.
    let parsed = reqwest::Url::parse(&destination.normalized_url)
        .map_err(|_| AgentError::new("浏览器目标 URL 无法规范解析。"))?;
    let parsed_host = parsed
        .host_str()
        .ok_or_else(|| AgentError::new("浏览器目标 URL 缺少 Host。"))?;
    let parsed_port = parsed
        .port_or_known_default()
        .ok_or_else(|| AgentError::new("浏览器目标 URL 缺少有效端口。"))?;
    let canonical_url = parsed.to_string();
    let canonical_origin = parsed.origin().ascii_serialization();
    if parsed.scheme() != destination.scheme
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed_host != destination.ascii_host
        || parsed_port != destination.effective_port
        || canonical_origin != destination.origin
        || canonical_url != destination.normalized_url
    {
        return Err(AgentError::new(
            "浏览器目标 URL、origin、Host 或端口不一致。",
        ));
    }
    Ok(())
}

fn valid_ascii_url_host(host: &str) -> bool {
    if host.is_empty()
        || host.trim() != host
        || host.starts_with('[')
        || host.ends_with(']')
        || !host.is_ascii()
        || host.bytes().any(|byte| {
            byte.is_ascii_uppercase()
                || byte.is_ascii_control()
                || byte.is_ascii_whitespace()
                || matches!(byte, b'/' | b'\\' | b'@' | b'?' | b'#')
        })
    {
        return false;
    }
    // URL host serialization is either an IPv6 literal (without brackets here), an IPv4
    // literal, or an IDNA ASCII domain. Its remaining punctuation is deliberately tiny.
    host.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b':' | b'_'))
}

fn validate_browser_risk_kinds(risk_kinds: &[BrowserRiskKind]) -> AgentResult<()> {
    if risk_kinds.is_empty() || risk_kinds.len() > BROWSER_RISK_MAX_COUNT {
        return Err(AgentError::new("浏览器风险分类数量无效。"));
    }
    if risk_kinds.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(AgentError::new("浏览器风险分类必须去重并按稳定顺序排列。"));
    }
    Ok(())
}

fn requires_exact_browser_risk_scope(risk_kinds: &[BrowserRiskKind]) -> bool {
    risk_kinds.iter().any(|kind| {
        matches!(
            kind,
            BrowserRiskKind::UrlUserinfo
                | BrowserRiskKind::RiskEscalation
                | BrowserRiskKind::NewWindow
                | BrowserRiskKind::FileUpload
                | BrowserRiskKind::FileDownload
        )
    })
}

fn valid_sha256_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_resolution_fingerprint(value: &str) -> bool {
    value.len() == 76
        && value.starts_with("hmac-sha256:")
        && value[12..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn valid_browser_tool_name(value: &str) -> bool {
    value.starts_with("browser_")
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn parse_v4_uuid(value: &str, label: &str) -> AgentResult<Uuid> {
    let uuid = Uuid::parse_str(value).map_err(|_| AgentError::new(format!("{label} id 无效。")))?;
    if uuid.is_nil()
        || uuid.get_version() != Some(uuid::Version::Random)
        || uuid.to_string() != value
    {
        return Err(AgentError::new(format!("{label} id 必须是规范 UUID v4。")));
    }
    Ok(uuid)
}

pub fn builtin_capability_activation_result(
    approval: &AgentBuiltinCapabilityActivationApproval,
    state: CapabilityActivationState,
    user_feedback: Option<&str>,
) -> AgentToolResult {
    AgentToolResult {
        exact_archive_file: None,
        call_id: approval.call_id.clone(),
        tool: "activate_capability".to_string(),
        ok: true,
        result: Some(json!({
            "status": state,
            "capability": approval.capability_id,
            "userFeedback": user_feedback,
        })),
        error: None,
    }
}

pub fn builtin_capability_activation_rejected_result(
    approval: &AgentBuiltinCapabilityActivationApproval,
    user_feedback: Option<&str>,
) -> AgentToolResult {
    builtin_capability_activation_result(
        approval,
        CapabilityActivationState::Rejected,
        user_feedback,
    )
}

/// Verifies that a durable checkpoint still contains the exact model-authored activation call
/// frozen into the typed approval.
///
/// Checkpoint arguments are protocol history, not authorization. Nevertheless, allowing them to
/// drift would pair a grant for one capability/reason with a different ToolCall in the model
/// transcript. Keep this validator strict and value-free in its errors so rejected records cannot
/// leak a model-authored reason into logs.
pub fn validate_frozen_builtin_capability_activation_args(
    approval: &AgentBuiltinCapabilityActivationApproval,
    args: &Value,
) -> AgentResult<()> {
    let object = args
        .as_object()
        .ok_or_else(|| AgentError::new("内置能力激活 checkpoint 参数必须是精确 JSON object。"))?;
    if object.len() != 2
        || object.get("capability").and_then(Value::as_str) != Some(approval.capability_id.as_str())
        || object.get("reason").and_then(Value::as_str) != Some(approval.reason.as_str())
    {
        return Err(AgentError::new(
            "内置能力激活 checkpoint 参数与冻结批准不一致。",
        ));
    }
    Ok(())
}

pub(crate) fn build_activation_approval(
    run_id: &str,
    call_id: &str,
    manifest: &BuiltinCapabilityManifest,
    policy: &BuiltinCapabilityPolicy,
    reason: String,
) -> AgentResult<AgentBuiltinCapabilityActivationApproval> {
    if reason.trim().is_empty() || reason.len() > REASON_MAX_BYTES {
        return Err(AgentError::new("激活理由不能为空且不能超过 4 KiB。"));
    }
    let now = unix_timestamp();
    Ok(AgentBuiltinCapabilityActivationApproval {
        action_id: Uuid::new_v4().to_string(),
        activation_id: CapabilityActivationId::generate().as_str().to_string(),
        run_id: run_id.to_string(),
        call_id: call_id.to_string(),
        capability_id: manifest.descriptor.id.as_str().to_string(),
        display_name: manifest.descriptor.display_name.clone(),
        reason,
        manifest_digest: manifest.manifest_digest.clone(),
        policy_revision: policy.revision,
        created_at: now,
        expires_at: now.saturating_add(BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS),
        approval_status: AgentApprovalStatus::Required,
    })
}

pub(crate) fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn schema_digest(schema: &Value) -> AgentResult<String> {
    let canonical = canonical_json(schema);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|error| AgentError::new(format!("无法计算内置能力 schema digest：{error}")))?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = serde_json::Map::with_capacity(values.len());
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding_scope_invocation(tool_id: &str, arguments: Value) -> BuiltinCapabilityInvocation {
        BuiltinCapabilityInvocation {
            run_id: "run-binding-scope".to_string(),
            capability_id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
            managed_mcp_id: "builtin.browser_automation.mcp".to_string(),
            package_name: "@playwright/mcp".to_string(),
            package_version: "0.0.79".to_string(),
            upstream_catalog_digest: format!("sha256:{}", "a".repeat(64)),
            policy_digest: format!("sha256:{}", "b".repeat(64)),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: format!("sha256:{}", "c".repeat(64)),
            policy_revision: 1,
            tool_id: tool_id.to_string(),
            raw_name: tool_id.to_string(),
            model_name: tool_id.to_string(),
            upstream_schema_digest: format!("sha256:{}", "d".repeat(64)),
            host_overlay_digest: format!("sha256:{}", "e".repeat(64)),
            host_input_schema_digest: format!("sha256:{}", "f".repeat(64)),
            call_id: "call-binding-scope".to_string(),
            arguments,
            builtin_tool_grant: None,
        }
    }

    #[test]
    fn sensitive_binding_scope_distinguishes_profile_and_page_authority() {
        assert_eq!(
            builtin_mcp_tool_binding_scope(&binding_scope_invocation(
                "browser_cookie_set",
                json!({"name":"session", "value":"reviewed"}),
            )),
            BuiltinMcpToolBindingScope::ManagedSurface,
            "an omitted cookie domain is derived from the frozen current page"
        );
        assert_eq!(
            builtin_mcp_tool_binding_scope(&binding_scope_invocation(
                "browser_cookie_set",
                json!({
                    "name":"session",
                    "value":"reviewed",
                    "domain":"fixture.invalid"
                }),
            )),
            BuiltinMcpToolBindingScope::ManagedBrowserProfile
        );
        assert_eq!(
            builtin_mcp_tool_binding_scope(&binding_scope_invocation(
                "browser_cookie_list",
                json!({}),
            )),
            BuiltinMcpToolBindingScope::ManagedBrowserProfile
        );
        assert_eq!(
            builtin_mcp_tool_binding_scope(&binding_scope_invocation(
                "browser_storage_state",
                json!({}),
            )),
            BuiltinMcpToolBindingScope::ManagedBrowserProfile
        );
        assert_eq!(
            builtin_mcp_tool_binding_scope(&binding_scope_invocation(
                "browser_evaluate",
                json!({"function":"() => 1"}),
            )),
            BuiltinMcpToolBindingScope::ManagedSurface
        );
    }

    #[test]
    fn builtin_sensitive_argument_digest_matches_the_main_canonical_vector() {
        let arguments = json!({
            "function": "() => document.title",
            "call_reason": "Read the current page title.",
        });
        assert_eq!(
            builtin_mcp_tool_arguments_digest(&arguments).unwrap(),
            "sha256:44679ad41b476dfb14d002b9d22a010f72f3c60ae4d060868c9ceacab2aabcd1"
        );
        assert_eq!(
            crate::tools::mcp_tool_arguments_digest(&arguments).unwrap(),
            "44679ad41b476dfb14d002b9d22a010f72f3c60ae4d060868c9ceacab2aabcd1",
            "external MCP keeps its historical bare digest"
        );
        assert_eq!(
            builtin_mcp_tool_resource_scope_digest(
                "browser_evaluate",
                "sha256:44679ad41b476dfb14d002b9d22a010f72f3c60ae4d060868c9ceacab2aabcd1",
                Some("https://mail.example.test"),
                &[BuiltinMcpToolRiskKind::PageScriptExecution],
                "managed_surface",
            )
            .unwrap(),
            "sha256:7a94511d5d3fa315c32237ca09c5bc7510fbe7cec14a40456b578f8e55181c7a"
        );
    }

    #[test]
    fn managed_profile_scope_digest_matches_the_main_canonical_vector() {
        let arguments = json!({
            "call_reason": "List reviewed managed browser cookies.",
        });
        let arguments_digest = builtin_mcp_tool_arguments_digest(&arguments).unwrap();
        assert_eq!(
            arguments_digest,
            "sha256:7eedba5a990eb2faeeb27bebfccd4ac2b5fa1b6237b308ccaf402fc5772c72b7"
        );
        assert_eq!(
            builtin_mcp_tool_resource_scope_digest(
                "browser_cookie_list",
                &arguments_digest,
                None,
                &[BuiltinMcpToolRiskKind::CookieRead],
                "managed_browser_profile",
            )
            .unwrap(),
            "sha256:e94817429ef8eba75b2b368825326e524b43f524c5dc38d9961f69e2d32f5bb2"
        );
    }

    #[test]
    fn rejection_feedback_redacts_common_credential_assignments() {
        for feedback in [
            "Authorization: Bearer FEEDBACK_SECRET",
            "Set-Cookie: session=FEEDBACK_SECRET",
            "password=FEEDBACK_SECRET",
            "api_key: FEEDBACK_SECRET",
        ] {
            let projected = sanitize_user_feedback(feedback).unwrap();
            assert_eq!(projected, "[sensitive text omitted]");
            assert!(!projected.contains("FEEDBACK_SECRET"));
        }
        assert_eq!(
            sanitize_user_feedback("No, use the public status page instead.").as_deref(),
            Some("No, use the public status page instead.")
        );
    }

    #[test]
    fn technical_gate_manifest_may_start_without_tools() {
        let manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
                display_name: "Browser automation".to_string(),
                description: "Managed browser technical gate".to_string(),
            },
            "builtin.browser_automation.mcp",
            "1",
            Vec::new(),
        )
        .unwrap();
        manifest.validate().unwrap();
        assert!(manifest.tools.is_empty());
        assert!(manifest.manifest_digest.starts_with("sha256:"));
    }

    #[test]
    fn manifest_revalidates_transparently_deserialized_capability_identity() {
        let descriptor: BuiltinCapabilityDescriptor = serde_json::from_value(json!({
            "id": "INVALID CAPABILITY",
            "displayName": "Browser automation",
            "description": "Managed browser technical gate"
        }))
        .unwrap();
        assert!(BuiltinCapabilityManifest::new(
            descriptor,
            "builtin.browser_automation.mcp",
            "1",
            Vec::new(),
        )
        .is_err());
    }

    #[test]
    fn manifest_digest_detects_reviewed_tool_drift() {
        let mut manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
                display_name: "Browser automation".to_string(),
                description: "Managed browser".to_string(),
            },
            "builtin.browser_automation.mcp",
            "1",
            vec![BuiltinCapabilityToolDescriptor::new(
                "browser.snapshot",
                "browser_snapshot",
                "Read page",
                json!({"type":"object","properties":{}}),
                crate::protocol::AgentToolSafety::ReadOnly,
                false,
            )
            .unwrap()],
        )
        .unwrap();
        manifest.tools[0].model_name = "browser_unreviewed".to_string();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn schema_digest_is_canonical_and_detects_schema_drift() {
        let left = BuiltinCapabilityToolDescriptor::new(
            "browser.snapshot",
            "browser_snapshot",
            "Read page",
            serde_json::from_str(
                r#"{"type":"object","properties":{"url":{"type":"string"},"limit":{"type":"integer"}}}"#,
            )
            .unwrap(),
            crate::protocol::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap();
        let right = BuiltinCapabilityToolDescriptor::new(
            "browser.snapshot",
            "browser_snapshot",
            "Read page",
            serde_json::from_str(
                r#"{"properties":{"limit":{"type":"integer"},"url":{"type":"string"}},"type":"object"}"#,
            )
            .unwrap(),
            crate::protocol::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap();
        assert_eq!(left.schema_digest, right.schema_digest);

        let mut drifted = left;
        drifted.input_schema["properties"]["url"]["maxLength"] = json!(2048);
        assert!(drifted.validate().is_err());
    }

    #[test]
    fn frozen_activation_arguments_require_exact_capability_reason_and_shape() {
        let approval = AgentBuiltinCapabilityActivationApproval {
            action_id: Uuid::new_v4().to_string(),
            activation_id: CapabilityActivationId::generate().as_str().to_string(),
            run_id: "run-1".to_string(),
            call_id: "call-1".to_string(),
            capability_id: "browser.automation".to_string(),
            display_name: "Browser automation".to_string(),
            reason: "Inspect the current page".to_string(),
            manifest_digest: format!("sha256:{}", "1".repeat(64)),
            policy_revision: 1,
            created_at: 1,
            expires_at: 901,
            approval_status: AgentApprovalStatus::Required,
        };
        assert!(validate_frozen_builtin_capability_activation_args(
            &approval,
            &json!({
                "reason": "Inspect the current page",
                "capability": "browser.automation"
            })
        )
        .is_ok());
        for drifted in [
            json!({
                "capability": "browser.automation",
                "reason": "Different reason"
            }),
            json!({
                "capability": "another.capability",
                "reason": "Inspect the current page"
            }),
            json!({
                "capability": "browser.automation",
                "reason": "Inspect the current page",
                "extra": true
            }),
            json!(["browser.automation", "Inspect the current page"]),
        ] {
            assert!(
                validate_frozen_builtin_capability_activation_args(&approval, &drifted).is_err()
            );
        }
    }

    #[test]
    fn manifest_digest_is_stable_across_reviewed_tool_input_order() {
        let descriptor = BuiltinCapabilityDescriptor {
            id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Managed browser".to_string(),
        };
        let snapshot = BuiltinCapabilityToolDescriptor::new(
            "browser.snapshot",
            "browser_snapshot",
            "Read page",
            json!({"type":"object","properties":{}}),
            crate::protocol::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap();
        let navigate = BuiltinCapabilityToolDescriptor::new(
            "browser.navigate",
            "browser_navigate",
            "Navigate",
            json!({
                "type":"object",
                "properties":{"url":{"type":"string"}},
                "required":["url"]
            }),
            crate::protocol::AgentToolSafety::RequiresApproval,
            false,
        )
        .unwrap();
        let left = BuiltinCapabilityManifest::new(
            descriptor.clone(),
            "builtin.browser_automation.mcp",
            "1",
            vec![snapshot.clone(), navigate.clone()],
        )
        .unwrap();
        let right = BuiltinCapabilityManifest::new(
            descriptor,
            "builtin.browser_automation.mcp",
            "1",
            vec![navigate, snapshot],
        )
        .unwrap();
        assert_eq!(left.manifest_digest, right.manifest_digest);
        assert_eq!(left.tools, right.tools);
    }

    #[test]
    fn capability_grant_binds_catalog_policy_and_manifest_digests() {
        let descriptor = BuiltinCapabilityDescriptor {
            id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
            display_name: "Browser automation".to_string(),
            description: "Managed browser".to_string(),
        };
        let tool = BuiltinCapabilityToolDescriptor::new(
            "browser.snapshot",
            "browser_snapshot",
            "Read page",
            json!({"type":"object","properties":{}}),
            crate::protocol::AgentToolSafety::ReadOnly,
            false,
        )
        .unwrap();
        let provider_contract = BuiltinCapabilityProviderContract::new(
            "@playwright/mcp",
            "0.0.79",
            format!("sha256:{}", "a".repeat(64)),
            format!("sha256:{}", "b".repeat(64)),
        )
        .unwrap();
        let manifest = BuiltinCapabilityManifest::new_with_provider_contract(
            descriptor.clone(),
            "builtin.browser_automation.mcp",
            "v2",
            provider_contract.clone(),
            vec![tool.clone()],
        )
        .unwrap();
        let policy = BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 7,
        };
        let grant = CapabilityGrant {
            run_id: "run-1".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            upstream_catalog_digest: provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: provider_contract.policy_digest.clone(),
            policy_revision: policy.revision,
            created_at: 1,
            expires_at: 100,
        };
        assert!(grant.is_live_for("run-1", &manifest, &policy, 2));

        let catalog_drift = BuiltinCapabilityManifest::new_with_provider_contract(
            descriptor.clone(),
            "builtin.browser_automation.mcp",
            "v2",
            BuiltinCapabilityProviderContract::new(
                "@playwright/mcp",
                "0.0.79",
                format!("sha256:{}", "c".repeat(64)),
                provider_contract.policy_digest.clone(),
            )
            .unwrap(),
            vec![tool.clone()],
        )
        .unwrap();
        assert!(!grant.is_live_for("run-1", &catalog_drift, &policy, 2));

        let policy_drift = BuiltinCapabilityManifest::new_with_provider_contract(
            descriptor,
            "builtin.browser_automation.mcp",
            "v2",
            BuiltinCapabilityProviderContract::new(
                "@playwright/mcp",
                "0.0.79",
                provider_contract.upstream_catalog_digest,
                format!("sha256:{}", "d".repeat(64)),
            )
            .unwrap(),
            vec![tool],
        )
        .unwrap();
        assert!(!grant.is_live_for("run-1", &policy_drift, &policy, 2));
        assert_ne!(manifest.manifest_digest, policy_drift.manifest_digest);
    }
}
