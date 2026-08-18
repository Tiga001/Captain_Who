//! Host-owned, task-scoped built-in capability contracts.
//!
//! A built-in capability is not an external MCP configuration. The Host owns its manifest,
//! policy and process-memory grants; Core only exposes reviewed Tool contracts and routes typed
//! invocations. No executable, transport, credential or raw MCP type crosses this boundary.

use crate::protocol::{
    AgentApprovalStatus, AgentBuiltinCapabilityActivationApproval, AgentError, AgentResult,
    AgentToolDefinition, AgentToolResult,
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
    /// Stable Provider-facing name. It is presentation only and is never parsed for routing.
    pub model_name: String,
    pub description: String,
    pub input_schema: Value,
    pub schema_digest: String,
    pub safety: crate::protocol::AgentToolSafety,
    pub requires_workspace: bool,
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
        let schema_digest = schema_digest(&input_schema)?;
        let descriptor = Self {
            tool_id: tool_id.into(),
            model_name: model_name.into(),
            description: description.into(),
            input_schema,
            schema_digest,
            safety,
            requires_workspace,
        };
        descriptor.validate()?;
        Ok(descriptor)
    }

    pub fn validate(&self) -> AgentResult<()> {
        BuiltinCapabilityId::parse(self.tool_id.clone())?;
        if self.model_name.is_empty()
            || self.model_name.len() > 64
            || !self
                .model_name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
            || self.description.trim().is_empty()
            || self.description.len() > DESCRIPTION_MAX_BYTES
        {
            return Err(AgentError::new("内置能力 Tool identity/description 无效。"));
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
            requires_approval: false,
            approval_mode: crate::protocol::AgentToolApprovalMode::Never,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BuiltinCapabilityManifest {
    pub schema_version: u32,
    pub descriptor: BuiltinCapabilityDescriptor,
    /// Stable non-secret identity of the Host-managed MCP implementation.
    pub managed_mcp_id: String,
    pub version: String,
    pub tools: Vec<BuiltinCapabilityToolDescriptor>,
    pub manifest_digest: String,
}

impl BuiltinCapabilityManifest {
    pub fn new(
        descriptor: BuiltinCapabilityDescriptor,
        managed_mcp_id: impl Into<String>,
        version: impl Into<String>,
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
            && self.policy_revision == policy.revision
            && self.expires_at > now
    }
}

pub type BuiltinCapabilityFuture<'a, T> = Pin<Box<dyn Future<Output = AgentResult<T>> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct BuiltinCapabilityInvocation {
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub managed_mcp_id: String,
    pub activation_id: CapabilityActivationId,
    pub manifest_digest: String,
    pub policy_revision: u64,
    pub tool_id: String,
    pub model_name: String,
    pub call_id: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub(crate) struct BuiltinCapabilityDispatchRequest {
    pub run_id: String,
    pub capability_id: BuiltinCapabilityId,
    pub managed_mcp_id: String,
    pub manifest_digest: String,
    pub tool_id: String,
    pub model_name: String,
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
    fn invoke_authorized<'a>(
        &'a self,
        invocation: BuiltinCapabilityInvocation,
        expected_grant: CapabilityGrant,
        cancellation: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value>;
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
            || manifest.manifest_digest != request.manifest_digest
            || reviewed_tool.is_none()
        {
            return Err(AgentError::new("内置能力工具不在已审核 manifest 中。"));
        }
        let policy = self.policy(&request.capability_id)?;
        let grant = self
            .live_grant(&request.run_id, &request.capability_id)?
            .ok_or_else(|| AgentError::new("当前任务没有有效的内置能力授权。"))?;
        let invocation = BuiltinCapabilityInvocation {
            run_id: request.run_id,
            capability_id: request.capability_id,
            managed_mcp_id: request.managed_mcp_id,
            activation_id: grant.activation_id.clone(),
            manifest_digest: request.manifest_digest,
            policy_revision: policy.revision,
            tool_id: request.tool_id,
            model_name: request.model_name,
            call_id: request.call_id,
            arguments: request.arguments,
        };
        self.provider
            .invoke_authorized(invocation, grant, request.cancellation)
            .await
    }
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
}
