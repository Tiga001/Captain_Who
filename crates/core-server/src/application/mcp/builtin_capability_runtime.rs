use super::builtin_capability_policy::{
    BuiltinCapabilityId as StoredCapabilityId, BuiltinCapabilityPolicyError,
    SqliteBuiltinCapabilityPolicyStore,
};
use mycopilot_core::{
    AgentApprovalStatus, AgentBuiltinCapabilityActivationApproval, AgentCancellationToken,
    AgentError, AgentResult, BuiltinCapabilityDescriptor, BuiltinCapabilityFuture,
    BuiltinCapabilityId, BuiltinCapabilityInvocation, BuiltinCapabilityManifest,
    BuiltinCapabilityPolicy, BuiltinCapabilityProvider, BuiltinCapabilityRuntime,
    CapabilityActivationId, CapabilityGrant, BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
    BUILTIN_CAPABILITY_GRANT_TTL_SECONDS,
};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::{Uuid, Version};

const BROWSER_AUTOMATION_CAPABILITY_ID: &str = "browser_automation";
const BROWSER_AUTOMATION_MANAGED_MCP_ID: &str = "builtin.browser_automation.mcp";
const BROWSER_AUTOMATION_MANIFEST_VERSION: &str = "builtin-browser-automation-v1";

#[derive(Default)]
struct ProcessGrantState {
    grants: HashMap<(String, String), CapabilityGrant>,
    consumed_action_ids: HashMap<String, u64>,
    consumed_activation_ids: HashMap<String, u64>,
}

impl ProcessGrantState {
    fn prune_expired(&mut self, now: u64) {
        self.grants.retain(|_, grant| grant.expires_at > now);
        self.consumed_action_ids
            .retain(|_, expires_at| *expires_at > now);
        self.consumed_activation_ids
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
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
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
    pub(crate) fn runtime(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
    ) -> AgentResult<BuiltinCapabilityRuntime> {
        BuiltinCapabilityRuntime::new(Arc::new(Self::new(policies)?))
    }

    fn new(policies: Arc<SqliteBuiltinCapabilityPolicyStore>) -> AgentResult<Self> {
        Self::with_clock(policies, Arc::new(unix_timestamp))
    }

    fn with_clock(
        policies: Arc<SqliteBuiltinCapabilityPolicyStore>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> AgentResult<Self> {
        let manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse(BROWSER_AUTOMATION_CAPABILITY_ID)?,
                display_name: "Browser automation".to_string(),
                description: "Control MyCopilot's managed in-app browser for the current task."
                    .to_string(),
            },
            BROWSER_AUTOMATION_MANAGED_MCP_ID,
            BROWSER_AUTOMATION_MANIFEST_VERSION,
            Vec::new(),
        )?;
        Ok(Self {
            policies,
            manifest,
            grants: Mutex::new(ProcessGrantState::default()),
            clock,
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
        Ok(grant)
    }

    fn revoke_activation(
        &self,
        activation_id: &CapabilityActivationId,
        action_id: &str,
    ) -> AgentResult<()> {
        let mut state = self.lock_grants()?;
        state
            .grants
            .retain(|_, grant| grant.activation_id != *activation_id);
        state.consumed_activation_ids.remove(activation_id.as_str());
        state.consumed_action_ids.remove(action_id);
        Ok(())
    }

    fn revoke_grants(&self, capability_id: &BuiltinCapabilityId) -> AgentResult<()> {
        self.exact_manifest_for(capability_id)?;
        let now = (self.clock)();
        let mut state = self.lock_grants()?;
        state.prune_expired(now);
        state
            .grants
            .retain(|(_, stored_capability_id), _| stored_capability_id != capability_id.as_str());
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
            Err(AgentError::new("当前版本尚未注册可执行的内置浏览器工具。"))
        })
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
    use mycopilot_core::{builtin_capability_activation_result, CapabilityActivationState};
    use std::sync::atomic::{AtomicU64, Ordering};

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

    #[test]
    fn defaults_disabled_and_empty_reviewed_manifest_never_starts_any_harness() {
        let harness = harness();
        let manifest = &harness.runtime.manifests()[0];
        assert_eq!(
            manifest.descriptor.id.as_str(),
            BROWSER_AUTOMATION_CAPABILITY_ID
        );
        assert!(manifest.tools.is_empty());
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
            tool_id: "browser.snapshot".to_string(),
            model_name: "browser_snapshot".to_string(),
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
}
