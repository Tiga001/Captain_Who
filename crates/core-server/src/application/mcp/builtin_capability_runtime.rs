use super::builtin_capability_policy::{
    BuiltinCapabilityId as StoredCapabilityId, BuiltinCapabilityPolicyError,
    SqliteBuiltinCapabilityPolicyStore,
};
use super::managed_playwright_bridge::ManagedPlaywrightMcpRuntime;
#[cfg(test)]
use super::playwright_manifest::BROWSER_AUTOMATION_MANAGED_MCP_ID;
use super::playwright_manifest::{
    load_playwright_browser_manifest, BROWSER_AUTOMATION_CAPABILITY_ID,
};
use mycopilot_core::{
    build_browser_risk_approval, build_builtin_mcp_tool_approval,
    validate_browser_risk_approval_shape, validate_builtin_mcp_tool_approval_shape,
    AgentApprovalStatus, AgentBrowserRiskApproval, AgentBuiltinCapabilityActivationApproval,
    AgentBuiltinMcpToolApproval, AgentCancellationToken, AgentError, AgentResult,
    BrowserRiskAuthorizationRequest, BrowserRiskGrant, BuiltinCapabilityFuture,
    BuiltinCapabilityId, BuiltinCapabilityInvocation, BuiltinCapabilityManifest,
    BuiltinCapabilityPolicy, BuiltinCapabilityProvider, BuiltinCapabilityRuntime,
    BuiltinMcpToolApprovalRequest, BuiltinMcpToolBindingScope, BuiltinMcpToolFilePreparation,
    BuiltinMcpToolGrant, BuiltinMcpToolTargetBindingReleaseReason,
    BuiltinMcpToolTargetBindingRequest, CapabilityActivationId, CapabilityGrant,
    McpOmittedContentKind, McpRuntimeProjectionLimits, McpToolContentBlock,
    PreparedBuiltinMcpToolTargetBinding, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    BUILTIN_CAPABILITY_GRANT_TTL_SECONDS,
};
use mycopilot_mcp_client::{McpContentBlock, McpInvocationId, McpToolResult};
use mycopilot_protocol_rs::{
    BuiltinMcpToolRiskKindDto, ManagedPlaywrightAuthorizationContext,
    ManagedPlaywrightBuiltinToolGrantContext, ManagedPlaywrightCompletionOutcome,
    ManagedPlaywrightPrepareSensitiveToolInput, ManagedPlaywrightSensitiveBindingReleaseReason,
    ManagedPlaywrightSensitiveBindingScopeDto, ManagedPlaywrightSensitiveFilePreparation,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::{Uuid, Version};

const MAX_PENDING_BROWSER_RISK_APPROVALS: usize = 64;
const MAX_PENDING_BUILTIN_TOOL_APPROVALS: usize = 128;
const MAX_DENIED_BUILTIN_TOOL_SCOPES: usize = 256;

fn builtin_mcp_cancelled_before_dispatch_error() -> AgentError {
    AgentError::cancelled_structured(
        "mcp.tool_cancelled_before_dispatch",
        "The approved built-in MCP Tool call was cancelled before Host dispatch.",
        json!({
            "schemaVersion": 1,
            "type": "builtin_mcp_tool_approval",
            "status": "cancelled",
            "errorCode": "mcp.tool_cancelled_before_dispatch",
            "retryable": false,
            "dispatchCertainty": "definitely_not_dispatched",
            "contentOmitted": true,
        }),
    )
}

#[derive(Default)]
struct ProcessGrantState {
    grants: HashMap<(String, String), CapabilityGrant>,
    consumed_action_ids: HashMap<String, u64>,
    consumed_activation_ids: HashMap<String, u64>,
    browser_risk_grants: HashMap<String, BrowserRiskGrant>,
    pending_browser_risks: HashMap<String, PendingBrowserRiskBinding>,
    consumed_browser_risk_action_ids: HashMap<String, u64>,
    pending_builtin_tools: HashMap<String, PendingBuiltinToolBinding>,
    approved_builtin_tools: HashMap<String, ApprovedBuiltinToolBinding>,
    consumed_builtin_tool_action_ids: HashMap<String, u64>,
    denied_builtin_tool_scopes: HashSet<BuiltinToolDenialScope>,
    deny_all_builtin_tool_runs: HashSet<String>,
}

#[derive(Clone)]
struct PendingBrowserRiskBinding {
    approval: AgentBrowserRiskApproval,
    resolution_fingerprint: String,
    target_fingerprint: String,
}

#[derive(Clone)]
struct PendingBuiltinToolBinding {
    approval: AgentBuiltinMcpToolApproval,
    invocation: BuiltinCapabilityInvocation,
    capability_grant: CapabilityGrant,
    denial_scope: BuiltinToolDenialScope,
    target_binding: PreparedBuiltinMcpToolTargetBinding,
}

#[derive(Clone)]
struct ApprovedBuiltinToolBinding {
    approval: AgentBuiltinMcpToolApproval,
    invocation: BuiltinCapabilityInvocation,
    capability_grant: CapabilityGrant,
    grant: BuiltinMcpToolGrant,
    target_binding: PreparedBuiltinMcpToolTargetBinding,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct BuiltinToolDenialScope {
    run_id: String,
    capability_activation_id: String,
    tool_id: String,
    arguments_digest: String,
    origin: Option<String>,
    operation_category: String,
    resource_scope: String,
    risk_kinds: Vec<mycopilot_core::BuiltinMcpToolRiskKind>,
}

impl BuiltinToolDenialScope {
    fn from_approval(approval: &AgentBuiltinMcpToolApproval) -> Self {
        Self {
            run_id: approval.identity.run_id.clone(),
            capability_activation_id: approval.identity.capability_activation_id.clone(),
            tool_id: approval.identity.tool_id.clone(),
            arguments_digest: approval.identity.arguments_digest.clone(),
            origin: approval.identity.origin.clone(),
            operation_category: approval.operation_category.clone(),
            resource_scope: approval.resource_summary.scope.clone(),
            risk_kinds: approval.risk_kinds.clone(),
        }
    }
}

impl ProcessGrantState {
    fn prune_expired(&mut self, now: u64) {
        self.grants.retain(|_, grant| grant.expires_at > now);
        self.consumed_action_ids
            .retain(|_, expires_at| *expires_at > now);
        self.consumed_activation_ids
            .retain(|_, expires_at| *expires_at > now);
        self.browser_risk_grants
            .retain(|_, grant| grant.expires_at > now);
        self.pending_browser_risks
            .retain(|_, binding| binding.approval.expires_at > now);
        self.consumed_browser_risk_action_ids
            .retain(|_, expires_at| *expires_at > now);
        self.pending_builtin_tools
            .retain(|_, binding| binding.approval.expires_at > now);
        self.approved_builtin_tools
            .retain(|_, binding| binding.grant.expires_at > now);
        self.consumed_builtin_tool_action_ids
            .retain(|_, expires_at| *expires_at > now);
    }
}

/// Host-owned process-memory authority for reviewed built-in capabilities.
///
/// This provider does not start a browser or an MCP Server. Round 1 registers an intentionally
/// empty browser manifest; later rounds may add reviewed tools whose first invocation lazily starts
/// the harness behind `invoke_authorized`.
pub(crate) struct HostBuiltinCapabilityProvider {
    policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    manifest: BuiltinCapabilityManifest,
    grants: Mutex<ProcessGrantState>,
    activation_transition: Mutex<()>,
    managed_runtime: Mutex<Option<Arc<ManagedPlaywrightMcpRuntime>>>,
    storage: Option<Arc<mycopilot_core::storage::service::StorageService>>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    approval_before_start_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    sensitive_before_grant_consume_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl std::fmt::Debug for HostBuiltinCapabilityProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HostBuiltinCapabilityProvider")
            .field("capability_id", &self.manifest.descriptor.id)
            .field("manifest_digest", &self.manifest.manifest_digest)
            .finish_non_exhaustive()
    }
}

impl HostBuiltinCapabilityProvider {
    pub(crate) fn runtime_and_provider(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        storage: Option<Arc<mycopilot_core::storage::service::StorageService>>,
    ) -> AgentResult<(BuiltinCapabilityRuntime, Arc<Self>)> {
        let provider = Arc::new(Self::new_with_storage(policies, storage)?);
        let runtime = BuiltinCapabilityRuntime::new(provider.clone())?;
        Ok((runtime, provider))
    }

    pub(crate) fn attach_managed_runtime(
        &self,
        runtime: Arc<ManagedPlaywrightMcpRuntime>,
    ) -> AgentResult<()> {
        let mut slot = self
            .managed_runtime
            .lock()
            .map_err(|_| AgentError::new("内置浏览器 runtime 状态不可用。"))?;
        if slot.is_some() {
            return Err(AgentError::new("内置浏览器 runtime 已注册。"));
        }
        *slot = Some(runtime);
        Ok(())
    }

    fn new_with_storage(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        storage: Option<Arc<mycopilot_core::storage::service::StorageService>>,
    ) -> AgentResult<Self> {
        Self::with_clock(policies, Arc::new(unix_timestamp), storage)
    }

    fn with_clock(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        storage: Option<Arc<mycopilot_core::storage::service::StorageService>>,
    ) -> AgentResult<Self> {
        let manifest = load_playwright_browser_manifest()?;
        Ok(Self {
            policies,
            manifest,
            grants: Mutex::new(ProcessGrantState::default()),
            activation_transition: Mutex::new(()),
            managed_runtime: Mutex::new(None),
            storage,
            clock,
            #[cfg(test)]
            approval_before_start_hook: Mutex::new(None),
            #[cfg(test)]
            sensitive_before_grant_consume_hook: Mutex::new(None),
        })
    }

    fn stored_id(capability_id: &BuiltinCapabilityId) -> AgentResult<StoredCapabilityId> {
        match capability_id.as_str() {
            BROWSER_AUTOMATION_CAPABILITY_ID => Ok(StoredCapabilityId::BrowserAutomation),
            _ => Err(AgentError::new("未注册的内置能力。")),
        }
    }

    fn current_policy(
        &self,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<BuiltinCapabilityPolicy> {
        let record = self
            .policies
            .get(Self::stored_id(capability_id)?)
            .map_err(policy_error)?;
        Ok(BuiltinCapabilityPolicy {
            user_allowed: record.user_allowed,
            revision: record.policy_revision,
        })
    }

    fn lock_grants(&self) -> AgentResult<MutexGuard<'_, ProcessGrantState>> {
        self.grants
            .lock()
            .map_err(|_| AgentError::new("内置能力 grant 状态不可用。"))
    }

    fn lock_activation_transition(&self) -> AgentResult<MutexGuard<'_, ()>> {
        self.activation_transition
            .lock()
            .map_err(|_| AgentError::new("内置能力 activation 状态不可用。"))
    }

    fn rollback_exact_approval(&self, grant: &CapabilityGrant, action_id: &str) -> AgentResult<()> {
        let key = Self::grant_key(&grant.run_id, &grant.capability_id);
        let mut state = self.lock_grants()?;
        if state.grants.get(&key) == Some(grant) {
            state.grants.remove(&key);
        }
        state
            .consumed_activation_ids
            .remove(grant.activation_id.as_str());
        state.consumed_action_ids.remove(action_id);
        Ok(())
    }

    fn grant_key(run_id: &str, capability_id: &BuiltinCapabilityId) -> (String, String) {
        (run_id.to_string(), capability_id.as_str().to_string())
    }

    fn exact_manifest_for(
        &self,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<&BuiltinCapabilityManifest> {
        if capability_id == &self.manifest.descriptor.id {
            Ok(&self.manifest)
        } else {
            Err(AgentError::new("未注册的内置能力。"))
        }
    }

    fn live_grant(
        &self,
        run_id: &str,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<Option<CapabilityGrant>> {
        let manifest = self.exact_manifest_for(capability_id)?;
        let policy = self.current_policy(capability_id)?;
        let now = (self.clock)();
        let key = Self::grant_key(run_id, capability_id);
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        let grant = state.grants.get(&key).cloned();
        match grant {
            Some(grant) if grant.is_live_for(run_id, manifest, &policy, now) => Ok(Some(grant)),
            Some(_) => {
                state.grants.remove(&key);
                Ok(None)
            }
            None => Ok(None),
        }
    }

    fn validate_approval(
        &self,
        approval: &AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<(
        BuiltinCapabilityId,
        CapabilityActivationId,
        BuiltinCapabilityPolicy,
    )> {
        let capability_id = BuiltinCapabilityId::parse(approval.capability_id.clone())?;
        let activation_id = CapabilityActivationId::parse(approval.activation_id.clone())?;
        let action_id = parse_v4_uuid(&approval.action_id, "action")?;
        let manifest = self.exact_manifest_for(&capability_id)?;
        let policy = self.current_policy(&capability_id)?;
        let now = (self.clock)();
        if approval.approval_status != AgentApprovalStatus::Approved
            || approval.run_id.trim().is_empty()
            || approval.call_id.trim().is_empty()
            || action_id.to_string() != approval.action_id
            || approval.display_name != manifest.descriptor.display_name
            || approval.manifest_digest != manifest.manifest_digest
            || approval.policy_revision != policy.revision
            || !policy.user_allowed
            || approval.created_at > now
            || approval.expires_at <= now
            || approval.expires_at.checked_sub(approval.created_at)
                != Some(BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS)
        {
            return Err(AgentError::new(
                "内置能力批准与当前 Host policy/manifest 不一致。",
            ));
        }
        Ok((capability_id, activation_id, policy))
    }

    fn managed_runtime(&self) -> AgentResult<Option<Arc<ManagedPlaywrightMcpRuntime>>> {
        self.managed_runtime
            .lock()
            .map(|runtime| runtime.clone())
            .map_err(|_| AgentError::new("内置浏览器 runtime 状态不可用。"))
    }

    fn start_managed_runtime(&self) -> AgentResult<()> {
        let Some(runtime) = self.managed_runtime()? else {
            return Ok(());
        };
        runtime
            .request_start()
            .map_err(|_| AgentError::new("内置浏览器 activation 无法启动。"))
    }

    fn stop_managed_runtime(&self) -> AgentResult<()> {
        let Some(runtime) = self.managed_runtime()? else {
            return Ok(());
        };
        runtime
            .request_stop()
            .map_err(|_| AgentError::new("内置浏览器 activation 无法停止。"))
    }
}

impl BuiltinCapabilityProvider for HostBuiltinCapabilityProvider {
    fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
        Ok(vec![self.manifest.clone()])
    }

    fn policy(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
        self.current_policy(capability_id)
    }

    fn grant(
        &self,
        run_id: &str,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<Option<CapabilityGrant>> {
        self.live_grant(run_id, capability_id)
    }

    fn approve_activation(
        &self,
        approval: &AgentBuiltinCapabilityActivationApproval,
    ) -> AgentResult<CapabilityGrant> {
        // Serialize the final policy check, grant publication and desired-runtime mutation with
        // grant revocation. The durable policy store remains authoritative: if disable commits
        // while approval is being settled, the second policy read below fails closed.
        let _transition = self.lock_activation_transition()?;
        let (capability_id, activation_id, policy) = self.validate_approval(approval)?;
        let key = Self::grant_key(&approval.run_id, &capability_id);
        let mut state = self.lock_grants()?;
        let now = (self.clock)();
        state.prune_expired(now);
        let current_policy = self.current_policy(&capability_id)?;
        if current_policy != policy || !current_policy.user_allowed {
            return Err(AgentError::new(
                "内置能力批准期间 policy 已改变；本次批准已失效。",
            ));
        }
        if state.consumed_action_ids.contains_key(&approval.action_id)
            || state
                .consumed_activation_ids
                .contains_key(activation_id.as_str())
            || state.grants.contains_key(&key)
        {
            return Err(AgentError::new("内置能力批准已消费或当前任务已有 grant。"));
        }
        let grant = CapabilityGrant {
            run_id: approval.run_id.clone(),
            capability_id,
            activation_id,
            manifest_digest: approval.manifest_digest.clone(),
            upstream_catalog_digest: self
                .manifest
                .provider_contract
                .upstream_catalog_digest
                .clone(),
            provider_policy_digest: self.manifest.provider_contract.policy_digest.clone(),
            policy_revision: policy.revision,
            created_at: now,
            expires_at: now.saturating_add(BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
        };
        state
            .consumed_action_ids
            .insert(approval.action_id.clone(), grant.expires_at);
        state
            .consumed_activation_ids
            .insert(approval.activation_id.clone(), grant.expires_at);
        state.grants.insert(key, grant.clone());
        drop(state);

        #[cfg(test)]
        if let Some(hook) = self
            .approval_before_start_hook
            .lock()
            .map_err(|_| AgentError::new("内置能力 activation 测试状态不可用。"))?
            .clone()
        {
            hook();
        }

        let final_policy = self.current_policy(&grant.capability_id)?;
        if final_policy != policy || !final_policy.user_allowed {
            self.rollback_exact_approval(&grant, &approval.action_id)?;
            return Err(AgentError::new(
                "内置能力批准期间 policy 已改变；本次批准已回滚。",
            ));
        }
        if let Err(error) = self.start_managed_runtime() {
            self.rollback_exact_approval(&grant, &approval.action_id)?;
            return Err(error);
        }
        Ok(grant)
    }

    fn revoke_activation(
        &self,
        activation_id: &CapabilityActivationId,
        action_id: &str,
    ) -> AgentResult<()> {
        let _transition = self.lock_activation_transition()?;
        let mut state = self.lock_grants()?;
        let released = state
            .pending_builtin_tools
            .values()
            .filter(|binding| {
                binding.approval.identity.capability_activation_id == activation_id.as_str()
            })
            .map(|binding| {
                (
                    binding.target_binding.clone(),
                    binding.approval.identity.run_id.clone(),
                    binding.approval.identity.call_id.clone(),
                )
            })
            .chain(
                state
                    .approved_builtin_tools
                    .values()
                    .filter(|binding| binding.grant.capability_activation_id == *activation_id)
                    .map(|binding| {
                        (
                            binding.target_binding.clone(),
                            binding.grant.run_id.clone(),
                            binding.grant.call_id.clone(),
                        )
                    }),
            )
            .collect::<Vec<_>>();
        state
            .grants
            .retain(|_, grant| grant.activation_id != *activation_id);
        state
            .browser_risk_grants
            .retain(|_, grant| grant.capability_activation_id != *activation_id);
        state.pending_browser_risks.retain(|_, binding| {
            binding.approval.capability_activation_id != activation_id.as_str()
        });
        state.pending_builtin_tools.retain(|_, binding| {
            binding.approval.identity.capability_activation_id != activation_id.as_str()
        });
        state
            .approved_builtin_tools
            .retain(|_, binding| binding.grant.capability_activation_id != *activation_id);
        state
            .denied_builtin_tool_scopes
            .retain(|scope| scope.capability_activation_id != activation_id.as_str());
        state.consumed_activation_ids.remove(activation_id.as_str());
        state.consumed_action_ids.remove(action_id);
        let should_stop = state.grants.is_empty();
        drop(state);
        for (binding, run_id, call_id) in released {
            let _ = self.release_builtin_mcp_tool_target_binding(
                &binding,
                &run_id,
                activation_id,
                &call_id,
                BuiltinMcpToolTargetBindingReleaseReason::CapabilityRevoked,
            );
        }
        if should_stop {
            self.stop_managed_runtime()?;
        }
        Ok(())
    }

    fn revoke_grants(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<()> {
        let _transition = self.lock_activation_transition()?;
        self.exact_manifest_for(capability_id)?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        let released = state
            .pending_builtin_tools
            .values()
            .filter(|binding| binding.approval.identity.capability_id == capability_id.as_str())
            .map(|binding| {
                (
                    binding.target_binding.clone(),
                    binding.approval.identity.run_id.clone(),
                    binding.approval.identity.capability_activation_id.clone(),
                    binding.approval.identity.call_id.clone(),
                )
            })
            .chain(
                state
                    .approved_builtin_tools
                    .values()
                    .filter(|binding| binding.grant.capability_id == *capability_id)
                    .map(|binding| {
                        (
                            binding.target_binding.clone(),
                            binding.grant.run_id.clone(),
                            binding.grant.capability_activation_id.as_str().to_string(),
                            binding.grant.call_id.clone(),
                        )
                    }),
            )
            .collect::<Vec<_>>();
        state
            .grants
            .retain(|(_, stored_capability_id), _| stored_capability_id != capability_id.as_str());
        state
            .browser_risk_grants
            .retain(|_, grant| grant.capability_id != *capability_id);
        state
            .pending_browser_risks
            .retain(|_, binding| binding.approval.capability_id != capability_id.as_str());
        state
            .pending_builtin_tools
            .retain(|_, binding| binding.approval.identity.capability_id != capability_id.as_str());
        state
            .approved_builtin_tools
            .retain(|_, binding| binding.grant.capability_id != *capability_id);
        drop(state);
        for (binding, run_id, activation_id, call_id) in released {
            if let Ok(activation_id) = CapabilityActivationId::parse(activation_id) {
                let _ = self.release_builtin_mcp_tool_target_binding(
                    &binding,
                    &run_id,
                    &activation_id,
                    &call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::CapabilityRevoked,
                );
            }
        }
        self.stop_managed_runtime()?;
        Ok(())
    }

    fn revoke_run_grants(&self, run_id: &str) -> AgentResult<()> {
        if run_id.trim().is_empty() {
            return Err(AgentError::new("内置能力 run identity 无效。"));
        }
        let _transition = self.lock_activation_transition()?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        let released = state
            .pending_builtin_tools
            .values()
            .filter(|binding| binding.approval.identity.run_id == run_id)
            .map(|binding| {
                (
                    binding.target_binding.clone(),
                    binding.approval.identity.capability_activation_id.clone(),
                    binding.approval.identity.call_id.clone(),
                )
            })
            .chain(
                state
                    .approved_builtin_tools
                    .values()
                    .filter(|binding| binding.grant.run_id == run_id)
                    .map(|binding| {
                        (
                            binding.target_binding.clone(),
                            binding.grant.capability_activation_id.as_str().to_string(),
                            binding.grant.call_id.clone(),
                        )
                    }),
            )
            .collect::<Vec<_>>();
        state
            .grants
            .retain(|(stored_run_id, _), _| stored_run_id != run_id);
        state
            .browser_risk_grants
            .retain(|_, grant| grant.run_id != run_id);
        state
            .pending_browser_risks
            .retain(|_, binding| binding.approval.run_id != run_id);
        state
            .pending_builtin_tools
            .retain(|_, binding| binding.approval.identity.run_id != run_id);
        state
            .approved_builtin_tools
            .retain(|_, binding| binding.grant.run_id != run_id);
        state
            .denied_builtin_tool_scopes
            .retain(|scope| scope.run_id != run_id);
        state.deny_all_builtin_tool_runs.remove(run_id);
        let should_stop = state.grants.is_empty();
        drop(state);
        for (binding, activation_id, call_id) in released {
            if let Ok(activation_id) = CapabilityActivationId::parse(activation_id) {
                let _ = self.release_builtin_mcp_tool_target_binding(
                    &binding,
                    run_id,
                    &activation_id,
                    &call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::RunRevoked,
                );
            }
        }
        if should_stop {
            // This stops/detaches managed automation only. BrowserSurfaceManager deliberately
            // keeps the user's visible guest page alive for manual browsing.
            self.stop_managed_runtime()?;
        }
        Ok(())
    }

    fn prepare_builtin_mcp_tool_target_binding<'a>(
        &'a self,
        request: BuiltinMcpToolTargetBindingRequest,
    ) -> BuiltinCapabilityFuture<'a, PreparedBuiltinMcpToolTargetBinding> {
        Box::pin(async move {
            request.cancellation.check()?;
            mycopilot_core::validate_builtin_sensitive_target_scope(&request.invocation)?;
            if request.invocation.builtin_tool_grant.is_some()
                || request.arguments_digest
                    != mycopilot_core::builtin_mcp_tool_arguments_digest(
                        &request.invocation.arguments,
                    )?
                || request.created_at >= request.expires_at
                || request.expires_at > request.capability_grant.expires_at
            {
                return Err(AgentError::new(
                    "内置 MCP Tool 页面绑定请求 identity 无效。",
                ));
            }
            let manifest = self.exact_manifest_for(&request.invocation.capability_id)?;
            let live = self
                .live_grant(
                    &request.invocation.run_id,
                    &request.invocation.capability_id,
                )?
                .ok_or_else(|| AgentError::new("内置 MCP Tool capability grant 已失效。"))?;
            if live != request.capability_grant
                || request.invocation.activation_id != live.activation_id
                || request.invocation.managed_mcp_id != manifest.managed_mcp_id
                || request.invocation.package_name != manifest.provider_contract.package_name
                || request.invocation.package_version != manifest.provider_contract.package_version
                || request.invocation.upstream_catalog_digest
                    != manifest.provider_contract.upstream_catalog_digest
                || request.invocation.policy_digest != manifest.provider_contract.policy_digest
                || request.invocation.manifest_digest != manifest.manifest_digest
                || !manifest.tools.iter().any(|tool| {
                    tool.tool_id == request.invocation.tool_id
                        && tool.raw_name == request.invocation.raw_name
                        && mycopilot_core::builtin_tool_requires_approval(
                            tool,
                            &request.invocation.arguments,
                        )
                })
            {
                return Err(AgentError::new(
                    "内置 MCP Tool 页面绑定与当前 manifest/grant 不一致。",
                ));
            }
            let runtime = self
                .managed_runtime()?
                .ok_or_else(|| AgentError::new("内置浏览器 runtime 尚未注册。"))?;
            let bridge = runtime.bridge();
            let release_run_id = request.invocation.run_id.clone();
            let release_activation_id = request.invocation.activation_id.as_str().to_string();
            let release_call_id = request.invocation.call_id.clone();
            let prepare =
                bridge.prepare_sensitive_tool(ManagedPlaywrightPrepareSensitiveToolInput {
                    binding_request_id: request.binding_request_id,
                    binding_scope: match request.binding_scope {
                        BuiltinMcpToolBindingScope::ManagedSurface => {
                            ManagedPlaywrightSensitiveBindingScopeDto::ManagedSurface
                        }
                        BuiltinMcpToolBindingScope::ManagedBrowserProfile => {
                            ManagedPlaywrightSensitiveBindingScopeDto::ManagedBrowserProfile
                        }
                    },
                    run_id: request.invocation.run_id,
                    capability_id: request.invocation.capability_id.as_str().to_string(),
                    activation_id: request.invocation.activation_id.as_str().to_string(),
                    manifest_digest: request.invocation.manifest_digest,
                    policy_revision: request.invocation.policy_revision,
                    grant_expires_at_ms: request.capability_grant.expires_at.saturating_mul(1_000),
                    call_id: request.invocation.call_id,
                    tool_name: request.invocation.tool_id,
                    arguments_digest: request.arguments_digest,
                    created_at_ms: request.created_at.saturating_mul(1_000),
                    expires_at_ms: request.expires_at.saturating_mul(1_000),
                    file_preparation: request.file_preparation.map(
                        |preparation| match preparation {
                            BuiltinMcpToolFilePreparation::ResolvedPaths(paths) => {
                                ManagedPlaywrightSensitiveFilePreparation::ResolvedPaths { paths }
                            }
                        },
                    ),
                });
            tokio::pin!(prepare);
            let outcome = tokio::select! {
                biased;
                _ = request.cancellation.cancelled() => {
                    // Drain the bounded prepare request. Main may already have created and
                    // successfully completed a binding by the time cancellation wins this select;
                    // discarding that opaque binding id would leak proposal authority until TTL.
                    if let Ok(ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                        binding_id,
                        ..
                    }) = prepare.await {
                        let _ = bridge.release_sensitive_tool_binding_now(
                            binding_id,
                            release_run_id,
                            release_activation_id,
                            release_call_id,
                            ManagedPlaywrightSensitiveBindingReleaseReason::Cancelled,
                        );
                    }
                    return Err(builtin_mcp_cancelled_before_dispatch_error());
                }
                result = &mut prepare => result,
            }
            .map_err(|_| AgentError::new("Main 无法冻结敏感浏览器页面 identity。"))?;
            let ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                binding_id,
                target_binding_digest,
                origin,
                created_at_ms,
                expires_at_ms,
                file_basenames,
                file_revision_digest,
            } = outcome
            else {
                return Err(AgentError::new("Main 返回了无效的敏感页面绑定结果。"));
            };
            if created_at_ms != request.created_at.saturating_mul(1_000)
                || expires_at_ms != request.expires_at.saturating_mul(1_000)
            {
                let _ = bridge.release_sensitive_tool_binding_now(
                    binding_id,
                    release_run_id,
                    release_activation_id,
                    release_call_id,
                    ManagedPlaywrightSensitiveBindingReleaseReason::ProposalFailed,
                );
                return Err(AgentError::new("Main 返回的敏感页面绑定时间已漂移。"));
            }
            Ok(PreparedBuiltinMcpToolTargetBinding {
                binding_id,
                target_binding_digest,
                origin,
                created_at: request.created_at,
                expires_at: request.expires_at,
                file_basenames,
                file_revision_digest,
            })
        })
    }

    fn release_builtin_mcp_tool_target_binding(
        &self,
        binding: &PreparedBuiltinMcpToolTargetBinding,
        run_id: &str,
        activation_id: &CapabilityActivationId,
        call_id: &str,
        reason: BuiltinMcpToolTargetBindingReleaseReason,
    ) -> AgentResult<()> {
        let Some(runtime) = self.managed_runtime()? else {
            return Ok(());
        };
        runtime
            .bridge()
            .release_sensitive_tool_binding_now(
                binding.binding_id.clone(),
                run_id.to_string(),
                activation_id.as_str().to_string(),
                call_id.to_string(),
                map_target_binding_release_reason(reason),
            )
            .map_err(|_| AgentError::new("Main 敏感页面绑定清理消息无法发送。"))
    }

    fn prepare_builtin_mcp_tool_approval(
        &self,
        request: BuiltinMcpToolApprovalRequest,
    ) -> AgentResult<AgentBuiltinMcpToolApproval> {
        let cleanup_binding = request.target_binding.clone();
        let cleanup_run_id = request.invocation.run_id.clone();
        let cleanup_activation_id = request.invocation.activation_id.clone();
        let cleanup_call_id = request.invocation.call_id.clone();
        let result = (|| {
            request.validate()?;
            mycopilot_core::validate_builtin_sensitive_target_scope(&request.invocation)?;
            let target_binding = request
                .target_binding
                .clone()
                .ok_or_else(|| AgentError::new("敏感内置 MCP Tool 缺少 Main 页面绑定。"))?;
            let now = (self.clock)();
            let manifest = self.exact_manifest_for(&request.invocation.capability_id)?;
            let live = self
                .live_grant(
                    &request.invocation.run_id,
                    &request.invocation.capability_id,
                )?
                .ok_or_else(|| AgentError::new("内置 MCP Tool capability grant 已失效。"))?;
            let reviewed_tool = manifest.tools.iter().find(|tool| {
                tool.tool_id == request.invocation.tool_id
                    && tool.raw_name == request.invocation.raw_name
                    && tool.model_name == request.invocation.model_name
                    && tool.upstream_schema_digest == request.invocation.upstream_schema_digest
                    && tool.host_overlay_digest == request.invocation.host_overlay_digest
                    && tool.schema_digest == request.invocation.host_input_schema_digest
            });
            let mut requested_risks = request.risk_kinds.clone();
            requested_risks.sort_unstable();
            let reviewed_risk_match = reviewed_tool.is_some_and(|tool| {
                let mut reviewed_risks = tool.builtin_risk_kinds.clone();
                reviewed_risks.sort_unstable();
                tool.builtin_approval_mode != mycopilot_core::BuiltinMcpToolApprovalMode::Never
                    && mycopilot_core::builtin_tool_requires_approval(
                        tool,
                        &request.invocation.arguments,
                    )
                    && reviewed_risks == requested_risks
            });
            if live != request.capability_grant
                || request.invocation.activation_id != live.activation_id
                || request.invocation.managed_mcp_id != manifest.managed_mcp_id
                || request.invocation.package_name != manifest.provider_contract.package_name
                || request.invocation.package_version != manifest.provider_contract.package_version
                || request.invocation.upstream_catalog_digest
                    != manifest.provider_contract.upstream_catalog_digest
                || request.invocation.policy_digest != manifest.provider_contract.policy_digest
                || request.invocation.manifest_digest != manifest.manifest_digest
                || !reviewed_risk_match
            {
                return Err(AgentError::new(
                    "内置 MCP Tool 审批请求与当前 manifest/grant/reviewed risk policy 不一致。",
                ));
            }
            let approval = build_builtin_mcp_tool_approval(&request, now)?;
            let denial_scope = BuiltinToolDenialScope::from_approval(&approval);
            let mut state = self.lock_grants()?;
            state.prune_expired(now);
            if state
                .grants
                .get(&(live.run_id.clone(), live.capability_id.as_str().to_string()))
                != Some(&live)
            {
                return Err(AgentError::new(
                    "内置 MCP Tool capability grant 在审批创建前已撤销。",
                ));
            }
            if state
                .deny_all_builtin_tool_runs
                .contains(&approval.identity.run_id)
                || state.denied_builtin_tool_scopes.contains(&denial_scope)
            {
                return Err(AgentError::structured(
                    "builtin_mcp_tool.previously_rejected",
                    "The user already rejected this exact sensitive browser request. Do not retry it unless the request changes.",
                    json!({
                        "type": "builtin_mcp_tool_approval",
                        "status": "rejected",
                        "retryable": false,
                        "dispatchCertainty": "definitely_not_dispatched",
                    }),
                ));
            }
            if state
                .pending_builtin_tools
                .values()
                .any(|binding| binding.denial_scope == denial_scope)
            {
                return Err(AgentError::structured(
                    "builtin_mcp_tool.approval_already_pending",
                    "The same sensitive built-in Tool request already has a pending approval.",
                    json!({
                        "type": "builtin_mcp_tool_approval",
                        "status": "pending",
                        "retryable": false,
                        "dispatchCertainty": "definitely_not_dispatched",
                    }),
                ));
            }
            if state.pending_builtin_tools.len() >= MAX_PENDING_BUILTIN_TOOL_APPROVALS {
                return Err(AgentError::structured(
                    "builtin_mcp_tool.approval_busy",
                    "Sensitive built-in Tool approval capacity is exhausted.",
                    json!({"retryable": false, "dispatchCertainty": "definitely_not_dispatched"}),
                ));
            }
            state.pending_builtin_tools.insert(
                approval.identity.approval_id.clone(),
                PendingBuiltinToolBinding {
                    approval: approval.clone(),
                    invocation: request.invocation,
                    capability_grant: live,
                    denial_scope,
                    target_binding,
                },
            );
            Ok(approval)
        })();
        if result.is_err() {
            if let Some(binding) = cleanup_binding {
                let _ = self.release_builtin_mcp_tool_target_binding(
                    &binding,
                    &cleanup_run_id,
                    &cleanup_activation_id,
                    &cleanup_call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed,
                );
            }
        }
        result
    }

    fn approve_builtin_mcp_tool(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<BuiltinMcpToolGrant> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        let now = (self.clock)();
        let capability_id = BuiltinCapabilityId::parse(approval.identity.capability_id.clone())?;
        let activation_id =
            CapabilityActivationId::parse(approval.identity.capability_activation_id.clone())?;
        let live = self
            .live_grant(&approval.identity.run_id, &capability_id)?
            .ok_or_else(|| AgentError::new("内置 MCP Tool capability grant 已失效。"))?;
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        if state
            .grants
            .get(&(live.run_id.clone(), live.capability_id.as_str().to_string()))
            != Some(&live)
        {
            return Err(AgentError::new(
                "内置 MCP Tool capability grant 在批准前已撤销。",
            ));
        }
        if state
            .consumed_builtin_tool_action_ids
            .contains_key(&approval.identity.action_id)
        {
            return Err(AgentError::new("内置 MCP Tool 审批已消费。"));
        }
        let binding = state
            .pending_builtin_tools
            .get(&approval.identity.approval_id)
            .cloned()
            .ok_or_else(|| AgentError::new("内置 MCP Tool 审批不存在或已失效。"))?;
        let mut frozen = approval.clone();
        frozen.approval_status = AgentApprovalStatus::Required;
        if binding.approval != frozen
            || binding.capability_grant != live
            || approval.approval_status != AgentApprovalStatus::Approved
            || approval.created_at > now
            || approval.expires_at <= now
        {
            return Err(AgentError::new("内置 MCP Tool 审批 identity 已漂移。"));
        }
        state
            .pending_builtin_tools
            .remove(&approval.identity.approval_id);
        let grant = BuiltinMcpToolGrant {
            grant_id: Uuid::new_v4().to_string(),
            approval_id: approval.identity.approval_id.clone(),
            run_id: approval.identity.run_id.clone(),
            call_id: approval.identity.call_id.clone(),
            capability_id,
            capability_activation_id: activation_id,
            managed_mcp_id: approval.identity.managed_mcp_id.clone(),
            package_name: approval.identity.package_name.clone(),
            package_version: approval.identity.package_version.clone(),
            upstream_catalog_digest: approval.identity.upstream_catalog_digest.clone(),
            manifest_digest: approval.identity.manifest_digest.clone(),
            policy_digest: approval.identity.policy_digest.clone(),
            policy_revision: approval.identity.policy_revision,
            tool_id: approval.identity.tool_id.clone(),
            raw_name: approval.identity.raw_name.clone(),
            model_name: approval.identity.model_name.clone(),
            upstream_schema_digest: approval.identity.upstream_schema_digest.clone(),
            host_overlay_digest: approval.identity.host_overlay_digest.clone(),
            host_input_schema_digest: approval.identity.host_input_schema_digest.clone(),
            arguments_digest: approval.identity.arguments_digest.clone(),
            resource_scope_digest: approval.identity.resource_scope_digest.clone(),
            target_binding_id: Some(binding.target_binding.binding_id.clone()),
            target_binding_digest: Some(binding.target_binding.target_binding_digest.clone()),
            origin: approval.identity.origin.clone(),
            risk_kinds: approval.risk_kinds.clone(),
            created_at: now,
            expires_at: approval.expires_at.min(live.expires_at),
        };
        if !grant.is_live_for(approval, &live, now) {
            return Err(AgentError::new("内置 MCP Tool grant 构造失败。"));
        }
        state
            .consumed_builtin_tool_action_ids
            .insert(approval.identity.action_id.clone(), approval.expires_at);
        state.approved_builtin_tools.insert(
            grant.grant_id.clone(),
            ApprovedBuiltinToolBinding {
                approval: frozen,
                invocation: binding.invocation,
                capability_grant: live,
                grant: grant.clone(),
                target_binding: binding.target_binding,
            },
        );
        Ok(grant)
    }

    fn dismiss_builtin_mcp_tool_approval(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        let mut frozen = approval.clone();
        frozen.approval_status = AgentApprovalStatus::Required;
        let mut released = Vec::new();
        if let Some(binding) = state
            .pending_builtin_tools
            .get(&approval.identity.approval_id)
        {
            if binding.approval != frozen {
                return Err(AgentError::new("内置 MCP Tool 审批 identity 已漂移。"));
            }
            if let Some(binding) = state
                .pending_builtin_tools
                .remove(&approval.identity.approval_id)
            {
                released.push(binding.target_binding);
            }
        }
        let approved_grant_ids = state
            .approved_builtin_tools
            .iter()
            .filter_map(|(grant_id, binding)| {
                (binding.approval == frozen).then_some(grant_id.clone())
            })
            .collect::<Vec<_>>();
        for grant_id in approved_grant_ids {
            if let Some(binding) = state.approved_builtin_tools.remove(&grant_id) {
                released.push(binding.target_binding);
            }
        }
        // Cleanup is intentionally idempotent: deletion, cancellation, expiry and shutdown may
        // converge on the same durable action. An already-settled historical action has no live
        // authority, so a second invalidation is a successful no-op.
        state
            .consumed_builtin_tool_action_ids
            .insert(approval.identity.action_id.clone(), approval.expires_at);
        drop(state);
        let activation_id =
            CapabilityActivationId::parse(approval.identity.capability_activation_id.clone())?;
        for binding in released {
            let _ = self.release_builtin_mcp_tool_target_binding(
                &binding,
                &approval.identity.run_id,
                &activation_id,
                &approval.identity.call_id,
                BuiltinMcpToolTargetBindingReleaseReason::Cancelled,
            );
        }
        Ok(())
    }

    fn reject_builtin_mcp_tool_approval(
        &self,
        approval: &AgentBuiltinMcpToolApproval,
    ) -> AgentResult<()> {
        validate_builtin_mcp_tool_approval_shape(approval)?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        let binding = state
            .pending_builtin_tools
            .get(&approval.identity.approval_id)
            .cloned()
            .ok_or_else(|| AgentError::new("内置 MCP Tool 审批不存在或已结算。"))?;
        let mut frozen = approval.clone();
        frozen.approval_status = AgentApprovalStatus::Required;
        if binding.approval != frozen {
            return Err(AgentError::new("内置 MCP Tool 审批 identity 已漂移。"));
        }
        state
            .pending_builtin_tools
            .remove(&approval.identity.approval_id);
        if state.denied_builtin_tool_scopes.len() >= MAX_DENIED_BUILTIN_TOOL_SCOPES
            && !state
                .denied_builtin_tool_scopes
                .contains(&binding.denial_scope)
        {
            state
                .deny_all_builtin_tool_runs
                .insert(approval.identity.run_id.clone());
        } else {
            state
                .denied_builtin_tool_scopes
                .insert(binding.denial_scope.clone());
        }
        state
            .consumed_builtin_tool_action_ids
            .insert(approval.identity.action_id.clone(), approval.expires_at);
        drop(state);
        let activation_id =
            CapabilityActivationId::parse(approval.identity.capability_activation_id.clone())?;
        let _ = self.release_builtin_mcp_tool_target_binding(
            &binding.target_binding,
            &approval.identity.run_id,
            &activation_id,
            &approval.identity.call_id,
            BuiltinMcpToolTargetBindingReleaseReason::Rejected,
        );
        Ok(())
    }

    fn revoke_builtin_mcp_tool_grant(&self, grant_id: &str, approval_id: &str) -> AgentResult<()> {
        let mut state = self.lock_grants()?;
        let released = if state
            .approved_builtin_tools
            .get(grant_id)
            .is_some_and(|binding| binding.grant.approval_id == approval_id)
        {
            state.approved_builtin_tools.remove(grant_id)
        } else {
            None
        };
        drop(state);
        if let Some(binding) = released {
            let _ = self.release_builtin_mcp_tool_target_binding(
                &binding.target_binding,
                &binding.grant.run_id,
                &binding.grant.capability_activation_id,
                &binding.grant.call_id,
                BuiltinMcpToolTargetBindingReleaseReason::GrantRevoked,
            );
        }
        Ok(())
    }

    fn invoke_approved_builtin_mcp_tool<'a>(
        &'a self,
        approval: AgentBuiltinMcpToolApproval,
        grant: BuiltinMcpToolGrant,
        cancellation: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async move {
            validate_builtin_mcp_tool_approval_shape(&approval)?;
            let now = (self.clock)();
            #[cfg(test)]
            if let Some(hook) = self
                .sensitive_before_grant_consume_hook
                .lock()
                .map_err(|_| AgentError::new("内置 MCP Tool 测试状态不可用。"))?
                .clone()
            {
                hook();
            }
            let binding = {
                let mut state = self.lock_grants()?;
                state.prune_expired(now);
                let binding = state
                    .approved_builtin_tools
                    .get(&grant.grant_id)
                    .cloned()
                    .ok_or_else(|| AgentError::new("内置 MCP Tool grant 不存在或已消费。"))?;
                let mut frozen = approval.clone();
                frozen.approval_status = AgentApprovalStatus::Required;
                if binding.approval != frozen
                    || binding.grant != grant
                    || !grant.is_live_for(&approval, &binding.capability_grant, now)
                    || mycopilot_core::builtin_mcp_tool_arguments_digest(
                        &binding.invocation.arguments,
                    )? != grant.arguments_digest
                {
                    return Err(AgentError::new("内置 MCP Tool grant identity 已漂移。"));
                }
                if cancellation.is_cancelled() {
                    state.approved_builtin_tools.remove(&grant.grant_id);
                    drop(state);
                    let _ = self.release_builtin_mcp_tool_target_binding(
                        &binding.target_binding,
                        &binding.grant.run_id,
                        &binding.grant.capability_activation_id,
                        &binding.grant.call_id,
                        BuiltinMcpToolTargetBindingReleaseReason::Cancelled,
                    );
                    return Err(builtin_mcp_cancelled_before_dispatch_error());
                }
                // Validation and one-time consumption happen under one lock. A stale or forged
                // queue attempt must not consume the valid grant that the original call owns.
                state.approved_builtin_tools.remove(&grant.grant_id);
                binding
            };
            // Consumption is the dispatch-certainty boundary owned by this Provider. A cancel
            // observed immediately after that atomic claim still proves that Main was never
            // called, so keep the result definite and do not hand the invocation to the Host.
            if cancellation.is_cancelled() {
                let _ = self.release_builtin_mcp_tool_target_binding(
                    &binding.target_binding,
                    &binding.grant.run_id,
                    &binding.grant.capability_activation_id,
                    &binding.grant.call_id,
                    BuiltinMcpToolTargetBindingReleaseReason::Cancelled,
                );
                return Err(builtin_mcp_cancelled_before_dispatch_error());
            }
            let mut invocation = binding.invocation;
            invocation.builtin_tool_grant = Some(grant);
            self.invoke_authorized(invocation, binding.capability_grant, cancellation)
                .await
        })
    }

    fn browser_risk_grant(
        &self,
        request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<Option<BrowserRiskGrant>> {
        let capability_grant = self
            .live_grant(&request.run_id, &request.capability_id)?
            .ok_or_else(|| AgentError::new("浏览器 capability grant 已失效。"))?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        Ok(state
            .browser_risk_grants
            .values()
            .find(|grant| grant.is_live_for_request(&capability_grant, request, now))
            .cloned())
    }

    fn prepare_browser_risk_approval(
        &self,
        request: &BrowserRiskAuthorizationRequest,
    ) -> AgentResult<AgentBrowserRiskApproval> {
        request.validate()?;
        let now = (self.clock)();
        // This Provider method is an authority boundary, not merely a DTO factory. Revalidate the
        // task grant and reviewed manifest here even though BuiltinCapabilityRuntime normally does
        // the same check. A forged Host request must never create a spurious approval surface.
        let manifest = self.exact_manifest_for(&request.capability_id)?;
        let policy = self.current_policy(&request.capability_id)?;
        let capability_grant = self
            .live_grant(&request.run_id, &request.capability_id)?
            .ok_or_else(|| AgentError::new("浏览器 capability grant 已失效。"))?;
        if !policy.user_allowed
            || request.capability_activation_id != capability_grant.activation_id
            || request.manifest_digest != manifest.manifest_digest
            || request.policy_revision != policy.revision
            || request.display_name != manifest.descriptor.display_name
            || !manifest
                .tools
                .iter()
                .any(|tool| tool.tool_id == request.trigger_tool_name)
        {
            return Err(AgentError::new(
                "浏览器风险请求与当前 grant 或 reviewed manifest 不一致。",
            ));
        }
        let approval = build_browser_risk_approval(request, now)?;
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        if state.pending_browser_risks.len() >= MAX_PENDING_BROWSER_RISK_APPROVALS {
            return Err(AgentError::structured(
                "browser.risk.busy",
                "浏览器风险审批数量已达到上限。",
                json!({"retryable": false}),
            ));
        }
        state.pending_browser_risks.insert(
            approval.risk_approval_id.clone(),
            PendingBrowserRiskBinding {
                approval: approval.clone(),
                resolution_fingerprint: request.resolution_fingerprint.clone(),
                target_fingerprint: request.target_fingerprint.clone(),
            },
        );
        Ok(approval)
    }

    fn approve_browser_risk(
        &self,
        approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<BrowserRiskGrant> {
        validate_browser_risk_approval_shape(approval)?;
        let _transition = self.lock_activation_transition()?;
        let now = (self.clock)();
        let capability_id = BuiltinCapabilityId::parse(approval.capability_id.clone())?;
        let activation_id =
            CapabilityActivationId::parse(approval.capability_activation_id.clone())?;
        let capability_grant = self
            .live_grant(&approval.run_id, &capability_id)?
            .ok_or_else(|| AgentError::new("浏览器 capability grant 已失效。"))?;
        let policy = self.current_policy(&capability_id)?;
        let manifest = self.exact_manifest_for(&capability_id)?;
        if !policy.user_allowed
            || policy.revision != approval.policy_revision
            || manifest.manifest_digest != approval.manifest_digest
            || capability_grant.activation_id != activation_id
            || approval.display_name != manifest.descriptor.display_name
            || !manifest
                .tools
                .iter()
                .any(|tool| tool.tool_id == approval.trigger_tool_name)
            || approval.approval_status != AgentApprovalStatus::Approved
            || approval.created_at > now
            || approval.expires_at <= now
        {
            return Err(AgentError::new(
                "浏览器风险批准在等待期间发生漂移；已安全失效。",
            ));
        }
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        if state
            .consumed_browser_risk_action_ids
            .contains_key(&approval.action_id)
        {
            return Err(AgentError::new("浏览器风险批准已消费。"));
        }
        let binding = state
            .pending_browser_risks
            .remove(&approval.risk_approval_id)
            .ok_or_else(|| AgentError::new("浏览器风险批准不存在或已失效。"))?;
        let mut frozen = approval.clone();
        frozen.approval_status = AgentApprovalStatus::Required;
        if binding.approval != frozen {
            return Err(AgentError::new("浏览器风险批准 identity 已漂移。"));
        }
        let expires_at = capability_grant.expires_at;
        let grant = BrowserRiskGrant {
            grant_id: Uuid::new_v4().to_string(),
            run_id: approval.run_id.clone(),
            capability_id,
            capability_activation_id: activation_id,
            manifest_digest: approval.manifest_digest.clone(),
            policy_revision: approval.policy_revision,
            destination: approval.destination.clone(),
            resolution_fingerprint: binding.resolution_fingerprint,
            target_fingerprint: binding.target_fingerprint,
            trigger: approval.trigger,
            trigger_tool_name: approval.trigger_tool_name.clone(),
            risk_kinds: approval.risk_kinds.clone(),
            created_at: now,
            expires_at,
        };
        state
            .consumed_browser_risk_action_ids
            .insert(approval.action_id.clone(), approval.expires_at);
        state
            .browser_risk_grants
            .insert(grant.grant_id.clone(), grant.clone());
        Ok(grant)
    }

    fn dismiss_browser_risk_approval(
        &self,
        approval: &AgentBrowserRiskApproval,
    ) -> AgentResult<()> {
        validate_browser_risk_approval_shape(approval)?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        if state
            .consumed_browser_risk_action_ids
            .contains_key(&approval.action_id)
        {
            return Err(AgentError::new("浏览器风险批准已消费。"));
        }
        let binding = state
            .pending_browser_risks
            .remove(&approval.risk_approval_id)
            .ok_or_else(|| AgentError::new("浏览器风险批准不存在或已失效。"))?;
        let mut frozen = approval.clone();
        frozen.approval_status = AgentApprovalStatus::Required;
        if binding.approval != frozen {
            return Err(AgentError::new("浏览器风险批准 identity 已漂移。"));
        }
        state
            .consumed_browser_risk_action_ids
            .insert(approval.action_id.clone(), approval.expires_at);
        Ok(())
    }

    fn revoke_browser_risk_grants(
        &self,
        run_id: Option<&str>,
        capability_id: &BuiltinCapabilityId,
    ) -> AgentResult<()> {
        self.exact_manifest_for(capability_id)?;
        let mut state = self.lock_grants()?;
        state.browser_risk_grants.retain(|_, grant| {
            grant.capability_id != *capability_id
                || run_id.is_some_and(|run_id| grant.run_id != run_id)
        });
        state.pending_browser_risks.retain(|_, binding| {
            binding.approval.capability_id != capability_id.as_str()
                || run_id.is_some_and(|run_id| binding.approval.run_id != run_id)
        });
        Ok(())
    }

    fn invoke_authorized<'a>(
        &'a self,
        invocation: BuiltinCapabilityInvocation,
        expected_grant: CapabilityGrant,
        cancellation: AgentCancellationToken,
    ) -> BuiltinCapabilityFuture<'a, Value> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                if invocation.builtin_tool_grant.is_some() {
                    return Err(builtin_mcp_cancelled_before_dispatch_error());
                }
                cancellation.check()?;
            }
            let manifest = self.exact_manifest_for(&invocation.capability_id)?;
            let live = self
                .live_grant(&invocation.run_id, &invocation.capability_id)?
                .ok_or_else(|| AgentError::new("当前任务的内置能力 grant 已失效。"))?;
            let reviewed_tool = manifest.tools.iter().find(|tool| {
                tool.tool_id == invocation.tool_id
                    && tool.raw_name == invocation.raw_name
                    && tool.model_name == invocation.model_name
                    && tool.upstream_schema_digest == invocation.upstream_schema_digest
                    && tool.host_overlay_digest == invocation.host_overlay_digest
                    && tool.schema_digest == invocation.host_input_schema_digest
            });
            if live != expected_grant
                || invocation.activation_id != live.activation_id
                || invocation.managed_mcp_id != manifest.managed_mcp_id
                || invocation.package_name != manifest.provider_contract.package_name
                || invocation.package_version != manifest.provider_contract.package_version
                || invocation.upstream_catalog_digest
                    != manifest.provider_contract.upstream_catalog_digest
                || invocation.policy_digest != manifest.provider_contract.policy_digest
                || invocation.manifest_digest != manifest.manifest_digest
                || invocation.policy_revision != live.policy_revision
                || reviewed_tool.is_none()
            {
                return Err(AgentError::new(
                    "内置能力调用不在当前审核 manifest/grant 中。",
                ));
            }
            let requires_sensitive_approval = reviewed_tool.is_some_and(|tool| {
                mycopilot_core::builtin_tool_requires_approval(tool, &invocation.arguments)
            });
            if requires_sensitive_approval != invocation.builtin_tool_grant.is_some() {
                return Err(AgentError::new(
                    "内置 MCP Tool 的敏感 grant 与 reviewed policy 不一致。",
                ));
            }
            if let Some(grant) = invocation.builtin_tool_grant.as_ref() {
                if grant.run_id != invocation.run_id
                    || grant.call_id != invocation.call_id
                    || grant.capability_id != invocation.capability_id
                    || grant.capability_activation_id != invocation.activation_id
                    || grant.manifest_digest != invocation.manifest_digest
                    || grant.policy_digest != invocation.policy_digest
                    || grant.policy_revision != invocation.policy_revision
                    || grant.tool_id != invocation.tool_id
                    || grant.arguments_digest
                        != mycopilot_core::builtin_mcp_tool_arguments_digest(&invocation.arguments)?
                    || grant.expires_at <= (self.clock)()
                {
                    return Err(AgentError::new("内置 MCP Tool 的敏感 grant 已漂移或过期。"));
                }
            }
            let runtime = self
                .managed_runtime()?
                .ok_or_else(|| AgentError::new("内置浏览器 runtime 尚未注册。"))?;
            let call_reason = invocation
                .arguments
                .get("call_reason")
                .and_then(Value::as_str)
                .filter(|reason| !reason.trim().is_empty() && reason.len() <= 512)
                .ok_or_else(|| AgentError::new("内置浏览器调用缺少有效的调用理由。"))?
                .to_string();
            let invocation_id = McpInvocationId::new();
            let builtin_tool_grant = invocation
                .builtin_tool_grant
                .as_ref()
                .map(|grant| {
                    Ok::<_, AgentError>(Box::new(ManagedPlaywrightBuiltinToolGrantContext {
                        grant_id: grant.grant_id.clone(),
                        approval_id: grant.approval_id.clone(),
                        arguments_digest: grant.arguments_digest.clone(),
                        resource_scope_digest: grant.resource_scope_digest.clone(),
                        target_binding_id: grant.target_binding_id.clone().ok_or_else(|| {
                            AgentError::new("敏感 Tool grant 缺少 Main 页面绑定 identity。")
                        })?,
                        target_binding_digest: grant.target_binding_digest.clone().ok_or_else(
                            || AgentError::new("敏感 Tool grant 缺少 Main 页面绑定 digest。"),
                        )?,
                        origin: grant.origin.clone(),
                        risk_kinds: grant
                            .risk_kinds
                            .iter()
                            .copied()
                            .map(map_builtin_tool_risk_kind)
                            .collect(),
                        expires_at_ms: grant.expires_at.saturating_mul(1_000),
                    }))
                })
                .transpose()?;
            let authorization_context = ManagedPlaywrightAuthorizationContext {
                conversation_id: invocation.conversation_id.clone(),
                run_id: invocation.run_id.clone(),
                capability_id: invocation.capability_id.as_str().to_string(),
                activation_id: invocation.activation_id.as_str().to_string(),
                manifest_digest: invocation.manifest_digest.clone(),
                policy_revision: invocation.policy_revision,
                grant_expires_at_ms: live
                    .expires_at
                    .checked_mul(1_000)
                    .ok_or_else(|| AgentError::new("内置浏览器 grant 时间无法安全投影。"))?,
                invocation_id: invocation_id.to_string(),
                call_id: invocation.call_id.clone(),
                trigger_tool_name: invocation.tool_id.clone(),
                call_reason,
                builtin_tool_grant,
            };
            let mcp_cancellation = mycopilot_mcp_client::McpCancellationToken::new();
            let invoke = runtime.invoke(
                &invocation.tool_id,
                &invocation.call_id,
                invocation.arguments,
                authorization_context,
                mcp_cancellation.clone(),
            );
            tokio::pin!(invoke);
            let result = tokio::select! {
                biased;
                _ = cancellation.cancelled() => {
                    mcp_cancellation.cancel();
                    invoke.await
                }
                result = &mut invoke => result,
            }
            .map_err(crate::adapters::mcp_runtime::map_invocation_error);
            let publish_path = runtime.take_host_artifact_publish_path(&invocation.call_id);
            let result = result?;
            let result = attach_browser_artifact_read_path(
                result,
                publish_path,
                self.storage.as_deref(),
                invocation
                    .conversation_id
                    .as_deref()
                    .map(|conversation_id| {
                        mycopilot_core::storage::service::ManagedArtifactAuthority {
                            conversation_id,
                            run_id: invocation.run_id.as_str(),
                            call_id: invocation.call_id.as_str(),
                        }
                    }),
            );
            project_managed_browser_result(result)
        })
    }
}

fn map_builtin_tool_risk_kind(
    risk: mycopilot_core::BuiltinMcpToolRiskKind,
) -> BuiltinMcpToolRiskKindDto {
    use mycopilot_core::BuiltinMcpToolRiskKind as Core;
    match risk {
        Core::FileRead => BuiltinMcpToolRiskKindDto::FileRead,
        Core::FileWrite => BuiltinMcpToolRiskKindDto::FileWrite,
        Core::FileUpload => BuiltinMcpToolRiskKindDto::FileUpload,
        Core::FileDownload => BuiltinMcpToolRiskKindDto::FileDownload,
        Core::CookieRead => BuiltinMcpToolRiskKindDto::CookieRead,
        Core::CookieWrite => BuiltinMcpToolRiskKindDto::CookieWrite,
        Core::LocalStorageRead => BuiltinMcpToolRiskKindDto::LocalStorageRead,
        Core::LocalStorageWrite => BuiltinMcpToolRiskKindDto::LocalStorageWrite,
        Core::SessionStorageRead => BuiltinMcpToolRiskKindDto::SessionStorageRead,
        Core::SessionStorageWrite => BuiltinMcpToolRiskKindDto::SessionStorageWrite,
        Core::StorageStateImport => BuiltinMcpToolRiskKindDto::StorageStateImport,
        Core::StorageStateExport => BuiltinMcpToolRiskKindDto::StorageStateExport,
        Core::NetworkSensitiveRead => BuiltinMcpToolRiskKindDto::NetworkSensitiveRead,
        Core::PageScriptExecution => BuiltinMcpToolRiskKindDto::PageScriptExecution,
        Core::UnsafeCodeExecution => BuiltinMcpToolRiskKindDto::UnsafeCodeExecution,
    }
}

fn map_target_binding_release_reason(
    reason: BuiltinMcpToolTargetBindingReleaseReason,
) -> ManagedPlaywrightSensitiveBindingReleaseReason {
    match reason {
        BuiltinMcpToolTargetBindingReleaseReason::ProposalFailed => {
            ManagedPlaywrightSensitiveBindingReleaseReason::ProposalFailed
        }
        BuiltinMcpToolTargetBindingReleaseReason::Rejected => {
            ManagedPlaywrightSensitiveBindingReleaseReason::Rejected
        }
        BuiltinMcpToolTargetBindingReleaseReason::Cancelled => {
            ManagedPlaywrightSensitiveBindingReleaseReason::Cancelled
        }
        BuiltinMcpToolTargetBindingReleaseReason::Expired => {
            ManagedPlaywrightSensitiveBindingReleaseReason::Expired
        }
        BuiltinMcpToolTargetBindingReleaseReason::RunRevoked => {
            ManagedPlaywrightSensitiveBindingReleaseReason::RunRevoked
        }
        BuiltinMcpToolTargetBindingReleaseReason::CapabilityRevoked => {
            ManagedPlaywrightSensitiveBindingReleaseReason::CapabilityRevoked
        }
        BuiltinMcpToolTargetBindingReleaseReason::GrantRevoked => {
            ManagedPlaywrightSensitiveBindingReleaseReason::GrantRevoked
        }
        BuiltinMcpToolTargetBindingReleaseReason::Shutdown => {
            ManagedPlaywrightSensitiveBindingReleaseReason::Shutdown
        }
    }
}

/// Publishes a Host-owned image or PDF into the managed Artifact store and returns a canonical
/// `readPath` for subsequent tools. The source path never survives this function.
fn attach_browser_artifact_read_path(
    mut result: McpToolResult,
    publish_path: Option<String>,
    storage: Option<&mycopilot_core::storage::service::StorageService>,
    authority: Option<mycopilot_core::storage::service::ManagedArtifactAuthority<'_>>,
) -> McpToolResult {
    if let Some(structured) = result
        .structured_content
        .as_mut()
        .and_then(Value::as_object_mut)
    {
        structured.remove("hostArtifactPublishPath");
        structured.remove("managedPath");
        structured.remove("privatePath");
        structured.remove("outputDir");
        structured.remove("path");
    }
    let Some(source) = publish_path.filter(|path| Path::new(path).is_absolute()) else {
        return result;
    };
    let Some(storage) = storage else {
        return result;
    };
    let Some(authority) = authority else {
        return result;
    };
    let kind = result
        .structured_content
        .as_ref()
        .and_then(|structured| structured.get("artifacts"))
        .and_then(Value::as_array)
        .and_then(|artifacts| artifacts.first())
        .and_then(|artifact| artifact.get("kind"))
        .and_then(Value::as_str);
    let mime = result
        .structured_content
        .as_ref()
        .and_then(|structured| structured.get("artifacts"))
        .and_then(Value::as_array)
        .and_then(|artifacts| artifacts.first())
        .and_then(|artifact| artifact.get("mimeType"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let extension = match (kind, mime) {
        (Some("image"), "image/png") => "png",
        (Some("image"), "image/jpeg") => "jpg",
        (Some("image"), "image/webp") => "webp",
        (Some("pdf"), "application/pdf") => "pdf",
        _ => return result,
    };
    let source_path = Path::new(&source);
    let staged = source_path.with_extension(extension);
    if staged != source_path && fs::copy(source_path, &staged).is_err() {
        return result;
    }
    let published = match storage.publish_managed_artifact_file(&staged, authority) {
        Ok(published) => published,
        Err(_) => {
            if staged != source_path {
                let _ = fs::remove_file(&staged);
            }
            return result;
        }
    };
    if staged != source_path {
        let _ = fs::remove_file(&staged);
    }
    let read_path = published.read_path();
    if let Some(structured) = result
        .structured_content
        .as_mut()
        .and_then(Value::as_object_mut)
    {
        structured.insert("readPath".to_string(), json!(read_path));
    }
    if let Some(McpContentBlock::Text { text }) = result.content.first_mut() {
        let input_field = if extension == "pdf" {
            "run_command.inputs[].path"
        } else {
            "read_image.path"
        };
        if !text.contains(input_field) {
            text.push_str(&format!("\n{input_field}: {read_path}"));
        }
    }
    result
}

/// Applies the same bounded MCP content policy used by external Servers before a result enters
/// the generic built-in Tool path. Binary data and resource URIs are represented only by omission
/// metadata; an authoritative `isError` response remains a completed Tool-level error rather than
/// a transport failure or an unknown outcome.
fn project_managed_browser_result(result: McpToolResult) -> AgentResult<Value> {
    let projected = crate::adapters::mcp_runtime::map_tool_result(
        result,
        &McpRuntimeProjectionLimits::default(),
    )?;
    let content = projected
        .content
        .into_iter()
        .map(|block| match block {
            McpToolContentBlock::Text { text } => json!({
                "type": "text",
                "text": text,
            }),
            McpToolContentBlock::Omitted {
                kind,
                mime_type,
                encoded_bytes,
            } => json!({
                "type": "omitted",
                "kind": omitted_content_kind(kind),
                "mimeType": mime_type,
                "encodedBytes": encoded_bytes,
            }),
        })
        .collect::<Vec<_>>();
    let structured_content = projected.structured_content.and_then(|structured| {
        if structured.get("artifacts").is_none() {
            return Some(structured);
        }
        let artifacts =
            mycopilot_core::browser_artifacts::safe_browser_artifact_references(&structured)?;
        let mut projected = json!({"artifacts": artifacts});
        if let Some(read_path) = structured
            .get("readPath")
            .and_then(mycopilot_core::browser_artifacts::safe_browser_artifact_read_path)
        {
            projected["readPath"] = json!(read_path);
        }
        Some(projected)
    });
    let envelope = json!({
        "schemaVersion": 1,
        "type": "managed_mcp_tool_result",
        "outcome": if projected.is_error { "tool_error" } else { "completed" },
        "isError": projected.is_error,
        "content": content,
        "structuredContent": structured_content,
        "truncated": projected.truncated_at_source,
        "dispatchCertainty": "response_received",
    });
    if projected.is_error {
        return Err(AgentError::structured(
            "builtin.browser_automation.tool_error",
            "The managed browser tool returned an authoritative Tool-level error.",
            envelope,
        ));
    }
    Ok(envelope)
}

fn omitted_content_kind(kind: McpOmittedContentKind) -> &'static str {
    match kind {
        McpOmittedContentKind::Image => "image",
        McpOmittedContentKind::Audio => "audio",
        McpOmittedContentKind::EmbeddedResource => "embedded_resource",
        McpOmittedContentKind::ResourceLink => "resource_link",
    }
}

fn parse_v4_uuid(value: &str, kind: &str) -> AgentResult<Uuid> {
    let id = Uuid::parse_str(value)
        .map_err(|_| AgentError::new(format!("内置能力 {kind} id 无效。")))?;
    if id.is_nil() || id.get_version() != Some(Version::Random) || id.to_string() != value {
        return Err(AgentError::new(format!(
            "内置能力 {kind} id 必须是规范 UUID v4。"
        )));
    }
    Ok(id)
}

fn policy_error(error: BuiltinCapabilityPolicyError) -> AgentError {
    let code = match error {
        BuiltinCapabilityPolicyError::Conflict => "conflict",
        BuiltinCapabilityPolicyError::CorruptRecord => "corrupt_record",
        BuiltinCapabilityPolicyError::InvalidInput => "invalid_input",
        BuiltinCapabilityPolicyError::StorageUnavailable => "storage_unavailable",
        BuiltinCapabilityPolicyError::RevisionExhausted => "revision_exhausted",
    };
    AgentError::new(format!("内置能力 policy 不可用：{code}。"))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests;
