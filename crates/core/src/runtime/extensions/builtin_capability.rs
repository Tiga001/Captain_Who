use super::{ExtensionDescriptor, RuntimeExtension};
use crate::builtin_capabilities::BuiltinCapabilityRuntime;
use crate::protocol::{AgentError, AgentResult};
use crate::tools::{
    builtin_tool_capability_id, ActivateCapabilityTool, AgentTool, BuiltinCapabilityAgentTool,
    ToolCapabilityId, ToolRegistry,
};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(super) const BUILTIN_CAPABILITY_EXTENSION_ID: &str =
    crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID;
const BUILTIN_CAPABILITY_EXTENSION_VERSION: u32 = 1;

pub(super) struct BuiltinCapabilityExtension {
    run_id: String,
    runtime: BuiltinCapabilityRuntime,
}

impl BuiltinCapabilityExtension {
    pub(super) fn new(run_id: String, runtime: BuiltinCapabilityRuntime) -> Self {
        Self { run_id, runtime }
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
        for manifest in self.runtime.manifests() {
            if self
                .runtime
                .live_grant(&self.run_id, &manifest.descriptor.id)?
                .is_some()
            {
                active.insert(builtin_tool_capability_id(&manifest.descriptor.id)?);
            }
        }
        Ok(active)
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
    use std::sync::Arc;

    struct GrantedProvider {
        manifest: BuiltinCapabilityManifest,
        grant: CapabilityGrant,
    }

    impl BuiltinCapabilityProvider for GrantedProvider {
        fn manifests(&self) -> AgentResult<Vec<BuiltinCapabilityManifest>> {
            Ok(vec![self.manifest.clone()])
        }

        fn policy(&self, _: &BuiltinCapabilityId) -> AgentResult<BuiltinCapabilityPolicy> {
            Ok(BuiltinCapabilityPolicy {
                user_allowed: true,
                revision: 3,
            })
        }

        fn grant(&self, _: &str, _: &BuiltinCapabilityId) -> AgentResult<Option<CapabilityGrant>> {
            Ok(Some(self.grant.clone()))
        }

        fn approve_activation(
            &self,
            _: &AgentBuiltinCapabilityActivationApproval,
        ) -> AgentResult<CapabilityGrant> {
            Ok(self.grant.clone())
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

    #[test]
    fn checkpoint_snapshot_contains_no_task_grant() {
        let manifest = BuiltinCapabilityManifest::new(
            BuiltinCapabilityDescriptor {
                id: BuiltinCapabilityId::parse("browser.automation").unwrap(),
                display_name: "Browser".to_string(),
                description: "Managed browser".to_string(),
            },
            "builtin.browser_automation.mcp",
            "1",
            Vec::new(),
        )
        .unwrap();
        let now = crate::builtin_capabilities::unix_timestamp();
        let grant = CapabilityGrant {
            run_id: "run-secret-authority".to_string(),
            capability_id: manifest.descriptor.id.clone(),
            activation_id: CapabilityActivationId::generate(),
            manifest_digest: manifest.manifest_digest.clone(),
            policy_revision: 3,
            created_at: now,
            expires_at: now + 60,
        };
        let runtime =
            crate::BuiltinCapabilityRuntime::new(Arc::new(GrantedProvider { manifest, grant }))
                .unwrap();
        let extension =
            BuiltinCapabilityExtension::new("run-secret-authority".to_string(), runtime);
        assert_eq!(extension.active_tool_capabilities().unwrap().len(), 1);
        let encoded = serde_json::to_string(&extension.snapshot_state().unwrap()).unwrap();
        assert_eq!(encoded, r#"{"schemaVersion":1}"#);
        assert!(!encoded.contains("run-secret-authority"));
        assert!(!encoded.contains("browser.automation"));
    }
}
