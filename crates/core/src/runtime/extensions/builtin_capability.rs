use super::{ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeExtension};
use crate::builtin_capabilities::{
    BuiltinCapabilityDescriptor, BuiltinCapabilityPolicy, BuiltinCapabilityRuntime,
};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::protocol::{AgentError, AgentResult};
use crate::tools::{
    builtin_tool_capability_id, ActivateCapabilityTool, AgentTool, BuiltinCapabilityAgentTool,
    ToolCapabilityId, ToolRegistry, BUILTIN_ACTIVATION_CAPABILITY,
};
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) const BUILTIN_CAPABILITY_EXTENSION_ID: &str =
    crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID;
const BUILTIN_CAPABILITY_EXTENSION_VERSION: u32 = 1;

pub(super) struct BuiltinCapabilityExtension {
    run_id: String,
    runtime: BuiltinCapabilityRuntime,
    request: Vec<BuiltinCapabilityRequestState>,
}

/// Presentation facts only. Authority remains in the Host and is never restored from a checkpoint.
struct BuiltinCapabilityRequestState {
    descriptor: BuiltinCapabilityDescriptor,
    policy: BuiltinCapabilityPolicy,
    active: bool,
}

impl BuiltinCapabilityExtension {
    pub(super) fn new(run_id: String, runtime: BuiltinCapabilityRuntime) -> Self {
        Self {
            run_id,
            runtime,
            request: Vec::new(),
        }
    }

    pub(super) fn register_manifest_tools(&self, registry: &mut ToolRegistry) -> AgentResult<()> {
        for manifest in self.runtime.manifests() {
            for descriptor in manifest.tools.iter().cloned() {
                registry.register_builtin_capability_tool(BuiltinCapabilityAgentTool::new(
                    manifest,
                    descriptor,
                    self.runtime.clone(),
                )?)?;
            }
        }
        Ok(())
    }
}

impl RuntimeExtension for BuiltinCapabilityExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: BUILTIN_CAPABILITY_EXTENSION_ID,
            version: BUILTIN_CAPABILITY_EXTENSION_VERSION,
            order: -50,
        }
    }

    fn prepare_model_request(&mut self) -> AgentResult<()> {
        // Build all projections from one frozen policy/grant pair per capability. Do not leave
        // the previous request's contract usable if reading fresh authority fails.
        self.request.clear();
        let request = self
            .runtime
            .manifests()
            .iter()
            .map(|manifest| {
                let (policy, grant) = self
                    .runtime
                    .policy_and_live_grant(&self.run_id, &manifest.descriptor.id)?;
                Ok(BuiltinCapabilityRequestState {
                    descriptor: manifest.descriptor.clone(),
                    policy,
                    active: grant.is_some(),
                })
            })
            .collect::<AgentResult<Vec<_>>>()?;
        self.request = request;
        Ok(())
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        vec![Box::new(ActivateCapabilityTool::new(
            self.run_id.clone(),
            self.runtime.clone(),
        ))]
    }

    fn register_additional_tools(&self, registry: &mut ToolRegistry) -> AgentResult<()> {
        self.register_manifest_tools(registry)
    }

    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        let mut active = BTreeSet::new();
        for capability in &self.request {
            if capability.policy.user_allowed {
                active.insert(ToolCapabilityId::application_owned(
                    BUILTIN_ACTIVATION_CAPABILITY,
                ));
            }
            if capability.active {
                active.insert(builtin_tool_capability_id(&capability.descriptor.id)?);
            }
        }
        Ok(active)
    }

    fn request_context(&self, request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        if request.purpose != ModelRequestPurpose::AgentWork {
            return Ok(Vec::new());
        }
        let available = self
            .request
            .iter()
            .filter(|capability| capability.policy.user_allowed)
            .map(|capability| {
                format!(
                    "- {} (`{}`): {} [{}]",
                    capability.descriptor.display_name,
                    capability.descriptor.id.as_str(),
                    capability.descriptor.description,
                    if capability.active {
                        "active"
                    } else {
                        "awaiting approval"
                    },
                )
            })
            .collect::<Vec<_>>();
        if available.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![ContextItem::text(
            LlmMessageRole::System,
            format!(
                "## Enabled built-in capabilities\nUse activate_capability to request task-scoped approval before using a capability awaiting approval. Enabling a capability in settings does not grant this task access.\n{}",
                available.join("\n"),
            ),
            ContextSource::RuntimeGuard,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        ).with_source(ContextSource::CapabilityInstructions)])
    }

    fn conversation_world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let state = json!({
            "capabilities": self.request.iter().map(|capability| json!({
                "capabilityId": capability.descriptor.id,
                "userAllowed": capability.policy.user_allowed,
                "policyRevision": capability.policy.revision,
            })).collect::<Vec<_>>()
        });
        let projection = json!({
            "capabilities": self.request.iter().map(|capability| json!({
                "capabilityId": capability.descriptor.id,
                "userAllowed": capability.policy.user_allowed,
            })).collect::<Vec<_>>()
        });
        Ok(vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::extension("builtin.capabilities.policy")
                .map_err(|error| AgentError::new(error.to_string()))?,
            WorldStateLifetime::Conversation,
            state,
            projection,
        )
        .map_err(|error| AgentError::new(error.to_string()))?])
    }

    fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let state = json!({
            "capabilities": self.request.iter().map(|capability| json!({
                "capabilityId": capability.descriptor.id,
                "active": capability.active,
            })).collect::<Vec<_>>()
        });
        Ok(vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::extension(BUILTIN_CAPABILITY_EXTENSION_ID)
                .map_err(|error| AgentError::new(error.to_string()))?,
            WorldStateLifetime::Run,
            state.clone(),
            state,
        )
        .map_err(|error| AgentError::new(error.to_string()))?])
    }

    /// Grants are deliberately absent: they live only in the Host process-memory provider.
    fn snapshot_state(&self) -> AgentResult<Value> {
        Ok(json!({"schemaVersion": BUILTIN_CAPABILITY_EXTENSION_VERSION}))
    }

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()> {
        if version != BUILTIN_CAPABILITY_EXTENSION_VERSION
            || state != json!({"schemaVersion": BUILTIN_CAPABILITY_EXTENSION_VERSION})
        {
            return Err(AgentError::new(
                "无法恢复内置能力扩展：checkpoint 状态版本无效。",
            ));
        }
        self.request.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin_capabilities::{
        BuiltinCapabilityDescriptor, BuiltinCapabilityFuture, BuiltinCapabilityId,
        BuiltinCapabilityInvocation, BuiltinCapabilityManifest, BuiltinCapabilityPolicy,
        BuiltinCapabilityProvider, CapabilityActivationId, CapabilityGrant,
    };
    use crate::{AgentBuiltinCapabilityActivationApproval, AgentCancellationToken};
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    struct GrantedProvider {
        manifests: Vec<BuiltinCapabilityManifest>,
        policies: Mutex<BTreeMap<BuiltinCapabilityId, BuiltinCapabilityPolicy>>,
        grants: Mutex<BTreeMap<BuiltinCapabilityId, CapabilityGrant>>,
        policy_reads: AtomicUsize,
        grant_reads: AtomicUsize,
    }

    impl BuiltinCapabilityProvider for GrantedProvider {
        fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
            Ok(self.manifests.clone())
        }

        fn policy(&self, id: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
            self.policy_reads.fetch_add(1, Ordering::SeqCst);
            self.policies
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or_else(|| AgentError::new("fixture policy unavailable"))
        }

        fn grant(&self, _: &str, id: &BuiltinCapabilityId) -> AgentResult<Option<CapabilityGrant>> {
            self.grant_reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.grants.lock().unwrap().get(id).cloned())
        }

        fn approve_activation(
            &self,
            _: &AgentBuiltinCapabilityActivationApproval,
        ) -> AgentResult<CapabilityGrant> {
            Err(AgentError::new("not used"))
        }

        fn revoke_grants(&self, _: &BuiltinCapabilityId) -> AgentResult<()> {
            Ok(())
        }

        fn revoke_activation(&self, _: &CapabilityActivationId, _: &str) -> AgentResult<()> {
            Ok(())
        }

        fn invoke_authorized<'a>(
            &'a self,
            _: BuiltinCapabilityInvocation,
            _: CapabilityGrant,
            _: AgentCancellationToken,
        ) -> BuiltinCapabilityFuture<'a, Value> {
            Box::pin(async { Err(AgentError::new("not used")) })
        }
    }

    fn manifest(id: &str, label: &str) -> BuiltinCapabilityManifest {
        BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse(id).unwrap(),
                display_name: label.to_string(),
                description: format!("Managed {label} operations"),
            },
            format!("builtin.{id}.mcp"),
            "1",
            vec![crate::BuiltinCapabilityToolDescriptor::new(
                format!("{id}.snapshot"),
                format!("{}_snapshot", id.replace('.', "_")),
                format!("Read the managed {label}"),
                json!({"type":"object","properties":{},"additionalProperties":false}),
                crate::AgentToolSafety::ReadOnly,
                false,
            )
            .unwrap()],
        )
        .unwrap()
    }

    fn grant(manifest: &BuiltinCapabilityManifest) -> CapabilityGrant {
        let now = crate::builtin_capabilities::unix_timestamp();
        CapabilityGrant {
            run_id: "run-secret-authority".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            upstream_catalog_digest: manifest.provider_contract.upstream_catalog_digest.clone(),
            provider_policy_digest: manifest.provider_contract.policy_digest.clone(),
            policy_revision: 3,
            created_at: now,
            expires_at: now + 60,
        }
    }

    fn fixture(
        manifests: Vec<BuiltinCapabilityManifest>,
    ) -> (
        BuiltinCapabilityExtension,
        Arc<GrantedProvider>,
        ToolRegistry,
    ) {
        let provider = Arc::new(GrantedProvider {
            policies: Mutex::new(
                manifests
                    .iter()
                    .map(|manifest| {
                        (
                            manifest.descriptor.id.clone(),
                            BuiltinCapabilityPolicy {
                                user_allowed: true,
                                revision: 3,
                            },
                        )
                    })
                    .collect(),
            ),
            grants: Mutex::new(
                manifests
                    .iter()
                    .map(|manifest| (manifest.descriptor.id.clone(), grant(manifest)))
                    .collect(),
            ),
            manifests,
            policy_reads: AtomicUsize::new(0),
            grant_reads: AtomicUsize::new(0),
        });
        let runtime = BuiltinCapabilityRuntime::new(provider.clone()).unwrap();
        let extension =
            BuiltinCapabilityExtension::new("run-secret-authority".to_string(), runtime);
        let mut registry = ToolRegistry::empty();
        extension.register_manifest_tools(&mut registry).unwrap();
        for tool in extension.tools() {
            registry
                .register_extension_tool(BUILTIN_CAPABILITY_EXTENSION_ID, tool)
                .unwrap();
        }
        (extension, provider, registry)
    }

    fn effective_tools(
        extension: &BuiltinCapabilityExtension,
        registry: &ToolRegistry,
    ) -> crate::tools::EffectiveToolSet {
        registry
            .effective_tool_set(
                registry.definitions(),
                &extension.active_tool_capabilities().unwrap(),
            )
            .unwrap()
    }

    fn request_text(extension: &BuiltinCapabilityExtension) -> String {
        crate::context::ContextFrame::new(
            extension
                .request_context(&ModelRequestContext::agent_work())
                .unwrap(),
        )
        .into_messages()
        .iter()
        .map(|message| message.content())
        .collect::<Vec<_>>()
        .join("\n")
    }

    #[test]
    fn checkpoint_snapshot_contains_no_task_grant() {
        let (mut extension, provider, _) = fixture(vec![manifest("browser.automation", "Browser")]);
        extension.prepare_model_request().unwrap();
        assert_eq!(extension.active_tool_capabilities().unwrap().len(), 2);
        let encoded = serde_json::to_string(&extension.snapshot_state().unwrap()).unwrap();
        assert_eq!(encoded, r#"{"schemaVersion":1}"#);
        assert!(!encoded.contains("run-secret-authority"));
        assert!(!encoded.contains("browser.automation"));
        extension
            .restore_state(1, serde_json::from_str(&encoded).unwrap())
            .unwrap();
        assert!(extension.active_tool_capabilities().unwrap().is_empty());
        provider.grants.lock().unwrap().clear();
        extension.prepare_model_request().unwrap();
        let state = extension.world_state_sections().unwrap().remove(0);
        assert_eq!(
            state.model_projection.unwrap()["capabilities"][0]["active"],
            false
        );
    }

    #[test]
    fn settings_switch_freezes_one_contract_and_preserves_disabled_world_state_fact() {
        let (mut extension, provider, registry) =
            fixture(vec![manifest("browser.automation", "Browser")]);
        extension.prepare_model_request().unwrap();
        let enabled = effective_tools(&extension, &registry);
        let enabled_context = request_text(&extension);
        let enabled_state = extension.world_state_sections().unwrap();
        let enabled_policy = extension.conversation_world_state_sections().unwrap();
        assert_eq!(enabled_state[0].lifetime, WorldStateLifetime::Run);
        assert_eq!(enabled_policy[0].lifetime, WorldStateLifetime::Conversation);
        assert!(enabled.contains("activate_capability"));
        assert!(enabled.contains("browser_automation_snapshot"));
        assert!(enabled_context.contains("Managed Browser operations"));
        assert_eq!(
            enabled_state[0].model_projection.as_ref().unwrap()["capabilities"][0]["active"],
            true
        );

        let id = BuiltinCapabilityId::parse("browser.automation").unwrap();
        *provider.policies.lock().unwrap().get_mut(&id).unwrap() = BuiltinCapabilityPolicy {
            user_allowed: false,
            revision: 4,
        };
        // A setting changed after preparation cannot tear the current schema/context/state apart.
        assert_eq!(
            serde_json::to_value(enabled.all_definitions()).unwrap(),
            serde_json::to_value(effective_tools(&extension, &registry).all_definitions()).unwrap()
        );
        assert_eq!(enabled_context, request_text(&extension));
        assert_eq!(enabled_state, extension.world_state_sections().unwrap());
        assert_eq!(
            enabled_policy,
            extension.conversation_world_state_sections().unwrap()
        );
        assert_eq!(provider.policy_reads.load(Ordering::SeqCst), 1);
        assert_eq!(provider.grant_reads.load(Ordering::SeqCst), 1);

        extension.prepare_model_request().unwrap();
        let disabled = effective_tools(&extension, &registry);
        assert!(!disabled.contains("activate_capability"));
        assert!(!disabled.contains("browser_automation_snapshot"));
        assert_eq!(
            serde_json::to_value(enabled.stable_definitions()).unwrap(),
            serde_json::to_value(disabled.stable_definitions()).unwrap(),
        );
        assert_eq!(enabled.stable_revision(), disabled.stable_revision());
        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
        let state = extension.world_state_sections().unwrap();
        assert_eq!(
            state[0].model_projection.as_ref().unwrap(),
            &json!({"capabilities": [{
                "capabilityId": "browser.automation",
                "active": false
            }]})
        );
        assert!(state[0].state["capabilities"][0]
            .get("policyRevision")
            .is_none());
        let policy = extension.conversation_world_state_sections().unwrap();
        assert_eq!(
            policy[0].model_projection.as_ref().unwrap(),
            &json!({
                "capabilities": [{"capabilityId":"browser.automation", "userAllowed":false}]
            })
        );
        assert_eq!(policy[0].state["capabilities"][0]["policyRevision"], 4);
        assert!(!serde_json::to_string(&state)
            .unwrap()
            .contains("Managed Browser operations"));
        assert!(!serde_json::to_string(&state)
            .unwrap()
            .contains("activate_capability"));

        // Turning settings back on cannot reuse the old revision's task authority.
        *provider.policies.lock().unwrap().get_mut(&id).unwrap() = BuiltinCapabilityPolicy {
            user_allowed: true,
            revision: 5,
        };
        extension.prepare_model_request().unwrap();
        let awaiting = effective_tools(&extension, &registry);
        assert!(awaiting.contains("activate_capability"));
        assert!(!awaiting.contains("browser_automation_snapshot"));
        assert_eq!(
            extension.world_state_sections().unwrap()[0]
                .model_projection
                .as_ref()
                .unwrap()["capabilities"][0]["active"],
            false
        );
    }

    #[test]
    fn policy_revision_changes_without_status_changes_stay_out_of_model_diffs() {
        use crate::world_state::{WorldStateDiff, WorldStateSnapshot};

        let (mut extension, provider, _) = fixture(vec![manifest("browser.automation", "Browser")]);
        let id = BuiltinCapabilityId::parse("browser.automation").unwrap();
        *provider.policies.lock().unwrap().get_mut(&id).unwrap() = BuiltinCapabilityPolicy {
            user_allowed: false,
            revision: 4,
        };
        extension.prepare_model_request().unwrap();
        let before = WorldStateSnapshot::new(
            "conversation-epoch",
            0,
            extension.conversation_world_state_sections().unwrap(),
        )
        .unwrap();
        provider
            .policies
            .lock()
            .unwrap()
            .get_mut(&id)
            .unwrap()
            .revision = 5;
        extension.prepare_model_request().unwrap();
        let after = WorldStateSnapshot::new(
            "conversation-epoch",
            1,
            extension.conversation_world_state_sections().unwrap(),
        )
        .unwrap();
        assert_ne!(before.revision, after.revision);
        assert_eq!(
            WorldStateDiff::between(&before, &after)
                .unwrap()
                .model_projection_against(&before, WorldStateLifetime::Conversation)
                .unwrap(),
            None,
        );
        assert_eq!(
            before
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap(),
            after
                .model_projection_revision(WorldStateLifetime::Conversation)
                .unwrap(),
        );
    }

    #[test]
    fn enabled_catalog_omits_disabled_capabilities_and_empty_catalog_has_no_activation_tool() {
        let (mut extension, provider, registry) = fixture(vec![
            manifest("browser.automation", "Browser"),
            manifest("documents.reader", "Documents"),
        ]);
        provider
            .policies
            .lock()
            .unwrap()
            .get_mut(&BuiltinCapabilityId::parse("browser.automation").unwrap())
            .unwrap()
            .user_allowed = false;
        extension.prepare_model_request().unwrap();
        let tools = effective_tools(&extension, &registry);
        let schema = serde_json::to_string(&tools.all_definitions()).unwrap();
        assert!(tools.contains("activate_capability"));
        assert!(!schema.to_lowercase().contains("browser"));
        let context = request_text(&extension);
        assert!(context.contains("Documents"));
        assert!(!context.to_lowercase().contains("browser"));
        assert!(extension
            .request_context(&ModelRequestContext {
                purpose: ModelRequestPurpose::ContextCompaction
            })
            .unwrap()
            .is_empty());

        let (mut empty, _, registry) = fixture(Vec::new());
        empty.prepare_model_request().unwrap();
        assert!(!effective_tools(&empty, &registry).contains("activate_capability"));
        assert!(empty
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn failed_policy_refresh_clears_previous_contract() {
        let (mut extension, provider, registry) =
            fixture(vec![manifest("browser.automation", "Browser")]);
        extension.prepare_model_request().unwrap();
        assert!(effective_tools(&extension, &registry).contains("browser_automation_snapshot"));
        provider.policies.lock().unwrap().clear();
        assert!(extension.prepare_model_request().is_err());
        assert!(effective_tools(&extension, &registry)
            .all_definitions()
            .is_empty());
        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn live_activation_changes_run_state_without_rewriting_conversation_policy() {
        let (mut extension, provider, _) = fixture(vec![manifest("browser.automation", "Browser")]);
        extension.prepare_model_request().unwrap();
        let policy = extension.conversation_world_state_sections().unwrap();
        let active = extension.world_state_sections().unwrap();
        provider.grants.lock().unwrap().clear();
        extension.prepare_model_request().unwrap();
        assert_eq!(
            extension.conversation_world_state_sections().unwrap(),
            policy
        );
        let inactive = extension.world_state_sections().unwrap();
        assert_ne!(active, inactive);
        assert_eq!(
            inactive[0].model_projection.as_ref().unwrap()["capabilities"][0]["active"],
            false
        );
        assert_eq!(provider.policy_reads.load(Ordering::SeqCst), 2);
        assert_eq!(provider.grant_reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_browser_policy_read_remains_an_error_instead_of_disabled_user_state() {
        let (mut extension, provider, _) = fixture(vec![manifest("browser.automation", "Browser")]);
        extension.prepare_model_request().unwrap();
        provider.policies.lock().unwrap().clear();
        assert!(extension.prepare_model_request().is_err());
        assert!(extension.active_tool_capabilities().unwrap().is_empty());
        let rendered =
            serde_json::to_string(&extension.conversation_world_state_sections().unwrap()).unwrap();
        assert!(!rendered.contains("disabled_by_user"));
        assert!(!rendered.contains("userAllowed"));
    }
}
