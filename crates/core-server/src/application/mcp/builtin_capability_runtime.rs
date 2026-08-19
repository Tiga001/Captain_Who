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
    build_browser_risk_approval, validate_browser_risk_approval_shape, AgentApprovalStatus,
    AgentBrowserRiskApproval, AgentBuiltinCapabilityActivationApproval, AgentCancellationToken,
    AgentError, AgentResult, BrowserRiskAuthorizationRequest, BrowserRiskGrant,
    BuiltinCapabilityFuture, BuiltinCapabilityId, BuiltinCapabilityInvocation,
    BuiltinCapabilityManifest, BuiltinCapabilityPolicy, BuiltinCapabilityProvider,
    BuiltinCapabilityRuntime, CapabilityActivationId, CapabilityGrant, McpOmittedContentKind,
    McpRuntimeProjectionLimits, McpToolContentBlock, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    BUILTIN_CAPABILITY_GRANT_TTL_SECONDS,
};
use mycopilot_mcp_client::{McpInvocationId, McpToolResult};
use mycopilot_protocol_rs::ManagedPlaywrightAuthorizationContext;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::{Uuid, Version};

const MAX_PENDING_BROWSER_RISK_APPROVALS: usize = 64;

#[derive(Default)]
struct ProcessGrantState {
    grants: HashMap<(String, String), CapabilityGrant>,
    consumed_action_ids: HashMap<String, u64>,
    consumed_activation_ids: HashMap<String, u64>,
    browser_risk_grants: HashMap<String, BrowserRiskGrant>,
    pending_browser_risks: HashMap<String, PendingBrowserRiskBinding>,
    consumed_browser_risk_action_ids: HashMap<String, u64>,
}

#[derive(Clone)]
struct PendingBrowserRiskBinding {
    approval: AgentBrowserRiskApproval,
    resolution_fingerprint: String,
    target_fingerprint: String,
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
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    #[cfg(test)]
    approval_before_start_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
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
    #[cfg(test)]
    pub(crate) fn runtime(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    ) -> AgentResult<BuiltinCapabilityRuntime> {
        BuiltinCapabilityRuntime::new(Arc::new(Self::new(policies)?))
    }

    pub(crate) fn runtime_and_provider(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    ) -> AgentResult<(BuiltinCapabilityRuntime, Arc<Self>)> {
        let provider = Arc::new(Self::new(policies)?);
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

    fn new(policies: Arc<SqliteBuiltinCapabilityPolicyStore>) -> AgentResult<Self> {
        Self::with_clock(policies, Arc::new(unix_timestamp))
    }

    fn with_clock(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> AgentResult<Self> {
        let manifest = load_playwright_browser_manifest()?;
        Ok(Self {
            policies,
            manifest,
            grants: Mutex::new(ProcessGrantState::default()),
            activation_transition: Mutex::new(()),
            managed_runtime: Mutex::new(None),
            clock,
            #[cfg(test)]
            approval_before_start_hook: Mutex::new(None),
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
            policy_revision: policy.revision,
            created_at: now,
            expires_at: now.saturating_add(BUILTIN_CAPABILITY_GRANT_TTL_SECONDS),
        };
        state
            .consumed_action_ids
            .insert(approval.action_id.clone(), approval.expires_at);
        state
            .consumed_activation_ids
            .insert(approval.activation_id.clone(), approval.expires_at);
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
        state
            .grants
            .retain(|_, grant| grant.activation_id != *activation_id);
        state
            .browser_risk_grants
            .retain(|_, grant| grant.capability_activation_id != *activation_id);
        state.pending_browser_risks.retain(|_, binding| {
            binding.approval.capability_activation_id != activation_id.as_str()
        });
        state.consumed_activation_ids.remove(activation_id.as_str());
        state.consumed_action_ids.remove(action_id);
        let should_stop = state.grants.is_empty();
        drop(state);
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
        state
            .grants
            .retain(|(_, stored_capability_id), _| stored_capability_id != capability_id.as_str());
        state
            .browser_risk_grants
            .retain(|_, grant| grant.capability_id != *capability_id);
        state
            .pending_browser_risks
            .retain(|_, binding| binding.approval.capability_id != capability_id.as_str());
        drop(state);
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
        state
            .grants
            .retain(|(stored_run_id, _), _| stored_run_id != run_id);
        state
            .browser_risk_grants
            .retain(|_, grant| grant.run_id != run_id);
        state
            .pending_browser_risks
            .retain(|_, binding| binding.approval.run_id != run_id);
        let should_stop = state.grants.is_empty();
        drop(state);
        if should_stop {
            // This stops/detaches managed automation only. BrowserSurfaceManager deliberately
            // keeps the user's visible guest page alive for manual browsing.
            self.stop_managed_runtime()?;
        }
        Ok(())
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
            cancellation.check()?;
            let manifest = self.exact_manifest_for(&invocation.capability_id)?;
            let live = self
                .live_grant(&invocation.run_id, &invocation.capability_id)?
                .ok_or_else(|| AgentError::new("当前任务的内置能力 grant 已失效。"))?;
            if live != expected_grant
                || invocation.activation_id != live.activation_id
                || invocation.managed_mcp_id != manifest.managed_mcp_id
                || invocation.manifest_digest != manifest.manifest_digest
                || invocation.policy_revision != live.policy_revision
                || !manifest.tools.iter().any(|tool| {
                    tool.tool_id == invocation.tool_id && tool.model_name == invocation.model_name
                })
            {
                return Err(AgentError::new(
                    "内置能力调用不在当前审核 manifest/grant 中。",
                ));
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
            let authorization_context = ManagedPlaywrightAuthorizationContext {
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
            .map_err(crate::adapters::mcp_runtime::map_invocation_error)?;
            project_managed_browser_result(result)
        })
    }
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
    let envelope = json!({
        "schemaVersion": 1,
        "type": "managed_mcp_tool_result",
        "outcome": if projected.is_error { "tool_error" } else { "completed" },
        "isError": projected.is_error,
        "content": content,
        "structuredContent": projected.structured_content,
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
mod tests {
    use super::*;
    use mycopilot_core::{
        builtin_capability_activation_result, BrowserDestinationIdentity,
        BrowserResolvedAddressClass, BrowserRiskKind, BrowserRiskTrigger,
        CapabilityActivationState,
    };
    use mycopilot_mcp_client::{
        McpContentBlock, McpEmbeddedResource, McpResourceLink, McpToolResult,
    };
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Barrier;

    struct Harness {
        _directory: tempfile::TempDir,
        _storage: mycopilot_core::storage::service::StorageService,
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        provider: Arc<HostBuiltinCapabilityProvider>,
        runtime: BuiltinCapabilityRuntime,
        now: Arc<AtomicU64>,
    }

    fn harness() -> Harness {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("storage.sqlite");
        let storage = mycopilot_core::storage::service::StorageService::open(&path).unwrap();
        let policies = Arc::new(SqliteBuiltinCapabilityPolicyStore::open(&path).unwrap());
        let now = Arc::new(AtomicU64::new(unix_timestamp()));
        let clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst)) as Arc<dyn Fn() -> u64 + Send + Sync>
        };
        let provider = Arc::new(
            HostBuiltinCapabilityProvider::with_clock(Arc::clone(&policies), clock).unwrap(),
        );
        let runtime = BuiltinCapabilityRuntime::new(provider.clone()).unwrap();
        Harness {
            _directory: directory,
            _storage: storage,
            policies,
            provider,
            runtime,
            now,
        }
    }

    fn approved(
        harness: &Harness,
        action_id: Uuid,
        activation_id: Uuid,
    ) -> AgentBuiltinCapabilityActivationApproval {
        let manifest = &harness.runtime.manifests()[0];
        let now = harness.now.load(Ordering::SeqCst);
        AgentBuiltinCapabilityActivationApproval {
            action_id: action_id.to_string(),
            activation_id: activation_id.to_string(),
            run_id: "run-1".to_string(),
            call_id: "call-1".to_string(),
            capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
            display_name: manifest.descriptor.display_name.clone(),
            reason: "Use the managed browser".to_string(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_revision: 1,
            created_at: now,
            expires_at: now + BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
            approval_status: AgentApprovalStatus::Approved,
        }
    }

    fn browser_risk_request(
        harness: &Harness,
        grant: &CapabilityGrant,
        normalized_url: &str,
        target_byte: char,
        trigger: BrowserRiskTrigger,
        risk_kinds: Vec<BrowserRiskKind>,
    ) -> BrowserRiskAuthorizationRequest {
        BrowserRiskAuthorizationRequest {
            run_id: grant.run_id.clone(),
            call_id: format!("call-risk-{target_byte}"),
            trigger_tool_name: "browser_navigate".to_string(),
            capability_id: grant.capability_id.clone(),
            capability_activation_id: grant.activation_id.clone(),
            manifest_digest: grant.manifest_digest.clone(),
            policy_revision: grant.policy_revision,
            display_name: harness.runtime.manifests()[0]
                .descriptor
                .display_name
                .clone(),
            reason: "Open the reviewed destination.".to_string(),
            destination: BrowserDestinationIdentity {
                normalized_url: normalized_url.to_string(),
                origin: "http://127.0.0.1:8765".to_string(),
                scheme: "http".to_string(),
                ascii_host: "127.0.0.1".to_string(),
                effective_port: 8765,
                address_class: BrowserResolvedAddressClass::Loopback,
            },
            resolution_fingerprint: format!("hmac-sha256:{}", "b".repeat(64)),
            target_fingerprint: format!("hmac-sha256:{}", target_byte.to_string().repeat(64)),
            trigger,
            risk_kinds,
        }
    }

    #[test]
    fn defaults_disabled_and_reviewed_manifest_does_not_start_any_harness() {
        let harness = harness();
        let manifest = &harness.runtime.manifests()[0];
        assert_eq!(
            manifest.descriptor.id.as_str(),
            BROWSER_AUTOMATION_CAPABILITY_ID
        );
        assert!(!manifest.tools.is_empty());
        assert!(
            !harness
                .runtime
                .policy(&manifest.descriptor.id)
                .unwrap()
                .user_allowed
        );
    }

    #[test]
    fn approval_is_exact_atomic_and_one_time() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let grant = harness.runtime.approve_activation(&approval).unwrap();
        assert_eq!(grant.run_id, "run-1");
        assert_eq!(
            harness
                .runtime
                .live_grant("run-1", &grant.capability_id)
                .unwrap(),
            Some(grant.clone())
        );
        assert!(harness.runtime.approve_activation(&approval).is_err());

        let result = builtin_capability_activation_result(
            &approval,
            CapabilityActivationState::Active,
            None,
        );
        assert!(result.ok);
    }

    #[test]
    fn browser_risk_grants_reuse_only_the_frozen_safe_scope() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
        let request = browser_risk_request(
            &harness,
            &capability_grant,
            "http://127.0.0.1:8765/a",
            'c',
            BrowserRiskTrigger::ToolArgument,
            vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
        );
        let mut approval = harness
            .runtime
            .prepare_browser_risk_approval(&request)
            .unwrap();
        approval.approval_status = AgentApprovalStatus::Approved;
        let risk_grant = harness.runtime.approve_browser_risk(&approval).unwrap();
        assert!(harness.runtime.approve_browser_risk(&approval).is_err());
        assert_eq!(
            harness.runtime.live_browser_risk_grant(&request).unwrap(),
            Some(risk_grant.clone())
        );

        // A stable resolver answer and risk set may reuse an origin grant across paths.
        let same_origin = browser_risk_request(
            &harness,
            &capability_grant,
            "http://127.0.0.1:8765/b",
            'd',
            BrowserRiskTrigger::Redirect,
            vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
        );
        assert!(harness
            .runtime
            .live_browser_risk_grant(&same_origin)
            .unwrap()
            .is_some());

        let mut resolver_drift = same_origin.clone();
        resolver_drift.resolution_fingerprint = format!("hmac-sha256:{}", "e".repeat(64));
        assert!(harness
            .runtime
            .live_browser_risk_grant(&resolver_drift)
            .unwrap()
            .is_none());

        let mut risk_drift = same_origin;
        risk_drift.risk_kinds.push(BrowserRiskKind::NonStandardPort);
        risk_drift.risk_kinds.sort_unstable();
        assert!(harness
            .runtime
            .live_browser_risk_grant(&risk_drift)
            .unwrap()
            .is_none());

        harness
            .now
            .store(risk_grant.expires_at.saturating_add(1), Ordering::SeqCst);
        // Browser risk authority never outlives its task-level capability grant. Once that
        // parent grant expires, lookup fails closed instead of treating the request as a fresh,
        // independently approvable browser destination.
        assert!(harness.runtime.live_browser_risk_grant(&request).is_err());
    }

    #[test]
    fn sensitive_browser_actions_bind_exact_target_trigger_and_tool() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
        let request = browser_risk_request(
            &harness,
            &capability_grant,
            "http://127.0.0.1:8765/upload",
            'c',
            BrowserRiskTrigger::Upload,
            vec![
                BrowserRiskKind::InsecureHttp,
                BrowserRiskKind::Loopback,
                BrowserRiskKind::FileUpload,
            ],
        );
        let mut approval = harness
            .runtime
            .prepare_browser_risk_approval(&request)
            .unwrap();
        approval.approval_status = AgentApprovalStatus::Approved;
        harness.runtime.approve_browser_risk(&approval).unwrap();

        let mut target_drift = request.clone();
        target_drift.target_fingerprint = format!("hmac-sha256:{}", "d".repeat(64));
        assert!(harness
            .runtime
            .live_browser_risk_grant(&target_drift)
            .unwrap()
            .is_none());
        let mut trigger_drift = request.clone();
        trigger_drift.trigger = BrowserRiskTrigger::NewWindow;
        assert!(harness
            .runtime
            .live_browser_risk_grant(&trigger_drift)
            .unwrap()
            .is_none());
        let mut tool_drift = request;
        tool_drift.trigger_tool_name = "browser_click".to_string();
        assert!(harness
            .runtime
            .live_browser_risk_grant(&tool_drift)
            .unwrap()
            .is_none());
    }

    #[test]
    fn terminal_run_retires_capability_risk_and_pending_authority() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let activation = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let capability_grant = harness.runtime.approve_activation(&activation).unwrap();
        let request = browser_risk_request(
            &harness,
            &capability_grant,
            "http://127.0.0.1:8765/a",
            'c',
            BrowserRiskTrigger::ToolArgument,
            vec![BrowserRiskKind::InsecureHttp, BrowserRiskKind::Loopback],
        );
        let approval = harness
            .runtime
            .prepare_browser_risk_approval(&request)
            .unwrap();
        assert_eq!(
            harness
                .provider
                .lock_grants()
                .unwrap()
                .pending_browser_risks
                .len(),
            1
        );

        harness.runtime.revoke_run_grants("run-1").unwrap();
        let state = harness.provider.lock_grants().unwrap();
        assert!(state.grants.is_empty());
        assert!(state.browser_risk_grants.is_empty());
        assert!(state.pending_browser_risks.is_empty());
        drop(state);
        assert!(harness.runtime.approve_browser_risk(&approval).is_err());
    }

    #[test]
    fn exact_activation_rollback_removes_grant_and_restores_one_shot_ids() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let grant = harness.runtime.approve_activation(&approval).unwrap();
        assert!(harness
            .runtime
            .live_grant("run-1", &grant.capability_id)
            .unwrap()
            .is_some());

        harness
            .runtime
            .revoke_activation(&grant.activation_id, &approval.action_id)
            .unwrap();
        assert!(harness
            .runtime
            .live_grant("run-1", &grant.capability_id)
            .unwrap()
            .is_none());
        assert!(harness.provider.lock_grants().unwrap().grants.is_empty());

        // A durable settlement failure may retry the exact pending approval; rollback must not
        // leave either one-shot identity consumed.
        assert!(harness.runtime.approve_activation(&approval).is_ok());
    }

    #[test]
    fn durable_disable_committed_during_approval_rolls_back_grant_before_start() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let hook = {
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            Arc::new(move || {
                entered.wait();
                release.wait();
            }) as Arc<dyn Fn() + Send + Sync>
        };
        *harness.provider.approval_before_start_hook.lock().unwrap() = Some(hook);

        let provider = Arc::clone(&harness.provider);
        let approval_for_thread = approval.clone();
        let settlement =
            std::thread::spawn(move || provider.approve_activation(&approval_for_thread));
        entered.wait();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
            .unwrap();
        release.wait();

        assert!(settlement.join().unwrap().is_err());
        let state = harness.provider.lock_grants().unwrap();
        assert!(state.grants.is_empty());
        assert!(state.consumed_action_ids.is_empty());
        assert!(state.consumed_activation_ids.is_empty());
    }

    #[test]
    fn policy_disable_and_expiry_make_process_grant_immediately_unusable() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let grant = harness.runtime.approve_activation(&approval).unwrap();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
            .unwrap();
        harness.runtime.revoke_grants(&grant.capability_id).unwrap();
        assert!(harness.provider.lock_grants().unwrap().grants.is_empty());
        assert!(harness
            .runtime
            .live_grant("run-1", &grant.capability_id)
            .unwrap()
            .is_none());

        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 2, true)
            .unwrap();
        assert!(harness
            .runtime
            .live_grant("run-1", &grant.capability_id)
            .unwrap()
            .is_none());
        harness
            .now
            .store(grant.expires_at.saturating_add(1), Ordering::SeqCst);
        assert!(harness
            .provider
            .grant("run-1", &grant.capability_id)
            .unwrap()
            .is_none());
        let state = harness.provider.lock_grants().unwrap();
        assert!(state.grants.is_empty());
        assert!(state.consumed_action_ids.is_empty());
        assert!(state.consumed_activation_ids.is_empty());
    }

    #[test]
    fn approval_rejects_manifest_policy_expiry_and_id_drift() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let base = approved(&harness, Uuid::new_v4(), Uuid::new_v4());

        let mut drifted = base.clone();
        drifted.manifest_digest = "sha256:deadbeef".to_string();
        assert!(harness.runtime.approve_activation(&drifted).is_err());

        let mut expired = base.clone();
        expired.expires_at = expired.created_at;
        assert!(harness.runtime.approve_activation(&expired).is_err());

        let mut malformed = base;
        malformed.action_id = Uuid::nil().to_string();
        assert!(harness.runtime.approve_activation(&malformed).is_err());

        let policy_drift = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 1, false)
            .unwrap();
        assert!(harness.provider.approve_activation(&policy_drift).is_err());
        assert!(harness.provider.lock_grants().unwrap().grants.is_empty());
    }

    #[tokio::test]
    async fn unreviewed_tool_invocation_fails_closed_without_starting_a_harness() {
        let harness = harness();
        harness
            .policies
            .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
            .unwrap();
        let approval = approved(&harness, Uuid::new_v4(), Uuid::new_v4());
        let grant = harness.runtime.approve_activation(&approval).unwrap();
        let invocation = BuiltinCapabilityInvocation {
            run_id: grant.run_id.clone(),
            capability_id: grant.capability_id.clone(),
            managed_mcp_id: BROWSER_AUTOMATION_MANAGED_MCP_ID.to_string(),
            activation_id: grant.activation_id.clone(),
            manifest_digest: grant.manifest_digest.clone(),
            policy_revision: grant.policy_revision,
            tool_id: "browser_unreviewed".to_string(),
            model_name: "browser_unreviewed".to_string(),
            call_id: "call-unreviewed".to_string(),
            arguments: serde_json::json!({}),
        };
        let error = harness
            .provider
            .invoke_authorized(invocation, grant, AgentCancellationToken::new())
            .await
            .unwrap_err();
        assert!(error.to_string().contains("不在当前审核 manifest"));
    }

    #[test]
    fn managed_result_projection_omits_binary_and_resource_identity() {
        let projected = project_managed_browser_result(McpToolResult {
            content: vec![
                McpContentBlock::Text {
                    text: "visible text".to_string(),
                },
                McpContentBlock::Image {
                    data: "private-image-canary".to_string(),
                    mime_type: "image/png".to_string(),
                },
                McpContentBlock::Audio {
                    data: "private-audio-canary".to_string(),
                    mime_type: "audio/wav".to_string(),
                },
                McpContentBlock::EmbeddedResource {
                    resource: McpEmbeddedResource::Text {
                        uri: "private-resource-uri-canary".to_string(),
                        mime_type: Some("text/plain".to_string()),
                        text: "private-resource-body-canary".to_string(),
                    },
                },
                McpContentBlock::ResourceLink {
                    resource: McpResourceLink {
                        uri: "private-link-uri-canary".to_string(),
                        name: "private-link-name-canary".to_string(),
                        title: None,
                        description: None,
                        mime_type: None,
                        size: Some(42),
                    },
                },
            ],
            structured_content: Some(json!({"safe": true})),
            is_error: false,
        })
        .unwrap();
        let serialized = serde_json::to_string(&projected).unwrap();
        assert!(serialized.contains("visible text"));
        assert!(serialized.contains("embedded_resource"));
        for canary in [
            "private-image-canary",
            "private-audio-canary",
            "private-resource-uri-canary",
            "private-resource-body-canary",
            "private-link-uri-canary",
            "private-link-name-canary",
        ] {
            assert!(!serialized.contains(canary), "leaked {canary}");
        }
    }

    #[test]
    fn authoritative_managed_tool_error_is_failed_with_response_received() {
        let error = project_managed_browser_result(McpToolResult {
            content: vec![McpContentBlock::Text {
                text: "reviewed tool error".to_string(),
            }],
            structured_content: None,
            is_error: true,
        })
        .unwrap_err();
        assert_eq!(error.code(), Some("builtin.browser_automation.tool_error"));
        let details = error.details().unwrap();
        assert_eq!(details["outcome"], "tool_error");
        assert_eq!(details["isError"], true);
        assert_eq!(details["dispatchCertainty"], "response_received");
        assert_eq!(details["content"][0]["text"], "reviewed tool error");
    }
}
