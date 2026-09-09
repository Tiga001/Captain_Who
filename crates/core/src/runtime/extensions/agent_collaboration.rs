use super::{ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeExtension};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::prompts::collaboration_harness_section;
use crate::tools::{ToolCapabilityId, ToolRegistry, AGENT_COLLABORATION_CAPABILITY};
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use crate::{
    AgentCollaborationPolicySource, AgentCollaborationRuntimeServices, AgentCollaborationSettings,
    AgentError, AgentResult,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

const AGENT_COLLABORATION_EXTENSION_ID: &str = "agent.collaboration";
const AGENT_COLLABORATION_EXTENSION_VERSION: u32 = 1;

pub(super) struct AgentCollaborationExtension {
    services: Option<AgentCollaborationRuntimeServices>,
    source: Option<Arc<dyn AgentCollaborationPolicySource>>,
    request: Option<AgentCollaborationSettings>,
    prepared: bool,
    restored: Option<Snapshot>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Snapshot {
    schema_version: u32,
    policy: Option<AgentCollaborationSettings>,
}

impl AgentCollaborationExtension {
    pub(super) fn new(
        services: Option<AgentCollaborationRuntimeServices>,
        source: Option<Arc<dyn AgentCollaborationPolicySource>>,
    ) -> Self {
        Self {
            services,
            source,
            request: None,
            prepared: false,
            restored: None,
        }
    }

    fn available(&self) -> bool {
        self.services.is_some() && self.request.as_ref().is_some_and(|policy| policy.enabled)
    }
}

impl RuntimeExtension for AgentCollaborationExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: AGENT_COLLABORATION_EXTENSION_ID,
            version: AGENT_COLLABORATION_EXTENSION_VERSION,
            order: 45,
        }
    }

    fn prepare_model_request(&mut self) -> AgentResult<()> {
        if self.prepared {
            return Ok(());
        }
        // One policy governs every request in the admitted logical run. A new Host segment
        // must prove the same binding when resuming; checkpoint/model input cannot grant it.
        let policy = match self.source.as_ref() {
            Some(source) => Some(source.snapshot()?),
            None if self.services.is_some() => {
                return Err(AgentError::new(
                    "智能体协作配置无效：Host 未提供本轮协作策略。",
                ));
            }
            None => None,
        };
        if self
            .restored
            .as_ref()
            .is_some_and(|snapshot| snapshot.policy != policy)
        {
            return Err(AgentError::new(
                "无法恢复智能体协作扩展：Host 本轮策略与冻结的检查点不一致。",
            ));
        }
        self.request = policy;
        self.prepared = true;
        Ok(())
    }

    fn register_additional_tools(&self, registry: &mut ToolRegistry) -> AgentResult<()> {
        // Keep implementations registered even when the model cannot use this capability.
        // In-flight batches are restored independently from a later run's user settings.
        registry.register_agent_collaboration_tools();
        Ok(())
    }

    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        Ok(if self.available() {
            BTreeSet::from([ToolCapabilityId::application_owned(
                AGENT_COLLABORATION_CAPABILITY,
            )])
        } else {
            BTreeSet::new()
        })
    }

    fn request_context(&self, request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        if !self.available() || request.purpose != ModelRequestPurpose::AgentWork {
            return Ok(Vec::new());
        }
        let services = self
            .services
            .as_ref()
            .expect("available collaboration has Host services");
        Ok(vec![ContextItem::text(
            LlmMessageRole::System,
            collaboration_harness_section(&services.caller, &services.selector_directory),
            ContextSource::CapabilityInstructions,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )])
    }

    fn conversation_world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let enabled = self.request.as_ref().is_some_and(|policy| policy.enabled);
        let reason = if self.request.as_ref().is_some_and(|policy| !policy.enabled) {
            "disabled_by_user"
        } else if self.services.is_none() || self.request.is_none() {
            "host_unavailable"
        } else {
            "available"
        };
        let projection = json!({
            "enabled": enabled,
            "available": self.available(),
            "reason": reason,
        });
        let mut state = projection.clone();
        state["policyRevision"] = self.request.as_ref().map(|policy| policy.revision).into();
        Ok(vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::extension(AGENT_COLLABORATION_EXTENSION_ID)
                .map_err(|error| AgentError::new(error.to_string()))?,
            WorldStateLifetime::Conversation,
            state,
            projection,
        )
        .map_err(|error| AgentError::new(error.to_string()))?])
    }

    fn snapshot_state(&self) -> AgentResult<Value> {
        serde_json::to_value(Snapshot {
            schema_version: AGENT_COLLABORATION_EXTENSION_VERSION,
            policy: self.request.clone(),
        })
        .map_err(|error| AgentError::new(error.to_string()))
    }

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()> {
        let snapshot = serde_json::from_value::<Snapshot>(state)
            .map_err(|_| AgentError::new("无法恢复智能体协作扩展：checkpoint 状态无效。"))?;
        if version != AGENT_COLLABORATION_EXTENSION_VERSION
            || snapshot.schema_version != AGENT_COLLABORATION_EXTENSION_VERSION
        {
            return Err(AgentError::new(
                "无法恢复智能体协作扩展：checkpoint 状态版本无效。",
            ));
        }
        self.request = None;
        self.prepared = false;
        self.restored = Some(snapshot);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{EffectiveToolSet, ToolUnavailability};
    use crate::{
        AgentCollaborationCaller, AgentCollaborationExecutor, AgentCollaborationSelectorDirectory,
        AGENT_COLLABORATION_TOOL_NAMES,
    };
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Policy {
        enabled: AtomicBool,
        fail: AtomicBool,
        reads: AtomicUsize,
    }

    impl Policy {
        fn new(enabled: bool) -> Arc<Self> {
            Arc::new(Self {
                enabled: AtomicBool::new(enabled),
                fail: AtomicBool::new(false),
                reads: AtomicUsize::new(0),
            })
        }
    }

    impl AgentCollaborationPolicySource for Policy {
        fn snapshot(&self) -> AgentResult<AgentCollaborationSettings> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.fail.load(Ordering::SeqCst) {
                return Err(AgentError::new("fixture policy unavailable"));
            }
            Ok(AgentCollaborationSettings {
                enabled: self.enabled.load(Ordering::SeqCst),
                ..Default::default()
            })
        }
    }

    struct Executor;
    impl AgentCollaborationExecutor for Executor {
        fn execute(
            &self,
            _: crate::AgentCollaborationInvocation,
            _: crate::AgentCollaborationExecutionControl,
        ) -> crate::AgentCollaborationExecutionFuture {
            panic!("projection must not execute collaboration")
        }
    }

    fn services(child: bool) -> AgentCollaborationRuntimeServices {
        AgentCollaborationRuntimeServices::new(
            Arc::new(Executor),
            AgentCollaborationCaller {
                agent_id: if child { "child" } else { "root" }.into(),
                root_agent_id: "root".into(),
                root_conversation_id: "root-conversation".into(),
                parent_agent_id: child.then(|| "root".into()),
                conversation_id: "conversation".into(),
                project_id: None,
                task_name: if child {
                    "核对"
                } else {
                    crate::ROOT_AGENT_TASK_NAME
                }
                .into(),
                task_path: if child { "/root/核对" } else { "/root" }.into(),
            },
            AgentCollaborationSelectorDirectory::default(),
        )
    }

    fn tool_set(extension: &AgentCollaborationExtension) -> EffectiveToolSet {
        let mut registry = ToolRegistry::defaults_with_search(None);
        extension.register_additional_tools(&mut registry).unwrap();
        registry
            .effective_tool_set(
                registry.definitions(),
                &extension.active_tool_capabilities().unwrap(),
            )
            .unwrap()
    }

    #[test]
    fn run_policy_atomically_controls_six_schemas_rules_directory_and_world_state() {
        for child in [false, true] {
            let source = Policy::new(true);
            let services = services(child);
            let expected =
                collaboration_harness_section(&services.caller, &services.selector_directory);
            let mut extension =
                AgentCollaborationExtension::new(Some(services.clone()), Some(source.clone()));
            extension.prepare_model_request().unwrap();
            let enabled = tool_set(&extension);
            assert!(AGENT_COLLABORATION_TOOL_NAMES
                .iter()
                .all(|name| enabled.contains(name)));
            assert_eq!(enabled.dynamic_definitions().len(), 6);
            assert!(enabled
                .stable_definitions()
                .iter()
                .all(|tool| !AGENT_COLLABORATION_TOOL_NAMES.contains(&tool.name.as_str())));
            let instructions = extension
                .request_context(&ModelRequestContext::agent_work())
                .unwrap();
            let frame = crate::context::ContextFrame::new(instructions);
            assert_eq!(frame.manifest().entries.len(), 1);
            assert_eq!(frame.manifest().entries[0].retention, "request_only");
            assert_eq!(frame.to_messages()[0].content(), expected);
            let section = &extension.conversation_world_state_sections().unwrap()[0];
            assert_eq!(section.state["enabled"], true);
            assert_eq!(section.state["available"], true);
            assert!(extension
                .request_context(&ModelRequestContext {
                    purpose: ModelRequestPurpose::ContextCompaction
                })
                .unwrap()
                .is_empty());

            source.enabled.store(false, Ordering::SeqCst);
            // A settings change cannot revoke a logical run that was already admitted.
            extension.prepare_model_request().unwrap();
            assert_eq!(tool_set(&extension).revision(), enabled.revision());
            assert_eq!(source.reads.load(Ordering::SeqCst), 1);
            let mut next_run = AgentCollaborationExtension::new(Some(services), Some(source));
            next_run.prepare_model_request().unwrap();
            let disabled = tool_set(&next_run);
            assert!(AGENT_COLLABORATION_TOOL_NAMES
                .iter()
                .all(|name| !disabled.contains(name)));
            assert!(next_run
                .request_context(&ModelRequestContext::agent_work())
                .unwrap()
                .is_empty());
            assert_eq!(
                next_run.conversation_world_state_sections().unwrap()[0].state["reason"],
                "disabled_by_user"
            );
            assert_eq!(
                next_run.conversation_world_state_sections().unwrap()[0].state["enabled"],
                false
            );
            assert_eq!(enabled.stable_revision(), disabled.stable_revision());
            assert!(matches!(
                disabled.unavailability("spawn_agent"),
                Some(ToolUnavailability::RuntimeCapabilityUnavailable { .. })
            ));
        }
    }

    #[test]
    fn approval_resume_requires_the_same_host_bound_policy() {
        let source = Policy::new(true);
        let mut original =
            AgentCollaborationExtension::new(Some(services(false)), Some(source.clone()));
        original.prepare_model_request().unwrap();
        let checkpoint = original.snapshot_state().unwrap();
        let original_tool_set = tool_set(&original).checkpoint();
        let mut resumed = AgentCollaborationExtension::new(Some(services(false)), Some(source));
        resumed.restore_state(1, checkpoint.clone()).unwrap();
        assert!(!resumed.available());
        resumed.prepare_model_request().unwrap();
        tool_set(&resumed)
            .restore_frozen_checkpoint(&original_tool_set)
            .unwrap();
        let mut mismatched =
            AgentCollaborationExtension::new(Some(services(false)), Some(Policy::new(false)));
        mismatched.restore_state(1, checkpoint).unwrap();
        assert!(mismatched
            .prepare_model_request()
            .unwrap_err()
            .to_string()
            .contains("本轮策略"));
        assert!(!mismatched.available());
    }

    #[test]
    fn missing_or_failed_host_policy_is_an_error_without_default_authorization() {
        let mut missing = AgentCollaborationExtension::new(Some(services(false)), None);
        assert!(missing
            .prepare_model_request()
            .unwrap_err()
            .to_string()
            .contains("未提供"));
        assert!(!missing.available());
        let source = Policy::new(true);
        source.fail.store(true, Ordering::SeqCst);
        let mut failing = AgentCollaborationExtension::new(Some(services(false)), Some(source));
        assert_eq!(
            failing.prepare_model_request().unwrap_err().to_string(),
            "fixture policy unavailable"
        );
        let mut unavailable = AgentCollaborationExtension::new(None, None);
        unavailable.prepare_model_request().unwrap();
        assert!(!unavailable.available());
        assert_eq!(
            unavailable.conversation_world_state_sections().unwrap()[0].state["reason"],
            "host_unavailable"
        );
        assert!(unavailable
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn disabled_run_without_services_retains_policy_in_world_state_and_checkpoint() {
        let policy = Policy::new(false);
        let mut extension = AgentCollaborationExtension::new(None, Some(policy.clone()));
        extension.prepare_model_request().unwrap();
        let section = &extension.conversation_world_state_sections().unwrap()[0];
        assert_eq!(section.state["enabled"], false);
        assert_eq!(section.state["available"], false);
        assert_eq!(section.state["reason"], "disabled_by_user");
        assert_eq!(section.state["policyRevision"], 1);
        let snapshot = extension.snapshot_state().unwrap();
        assert_eq!(snapshot["policy"]["enabled"], false);
        let mut resumed = AgentCollaborationExtension::new(None, Some(policy));
        resumed.restore_state(1, snapshot).unwrap();
        resumed.prepare_model_request().unwrap();
        assert_eq!(
            tool_set(&extension).revision(),
            tool_set(&resumed).revision()
        );
        assert!(AGENT_COLLABORATION_TOOL_NAMES
            .iter()
            .all(|name| !tool_set(&resumed).contains(name)));
    }
}
