use super::{ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeExtension};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::human_interaction::{HumanInteractionSettings, HUMAN_INTERACTION_MAX_SAFE_INTEGER};
use crate::llm::LlmMessageRole;
use crate::protocol::{AgentError, AgentResult, AgentToolDefinition};
use crate::tools::human_interaction::{
    human_interaction_tool_definitions, HUMAN_INTERACTION_CAPABILITY,
};
use crate::tools::ToolCapabilityId;
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use crate::HumanInteractionPolicySource;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) const HUMAN_INTERACTION_EXTENSION_ID: &str = "human.interaction";
const HUMAN_INTERACTION_EXTENSION_VERSION: u32 = 1;

// Deliberately closed during the foundation phase. Neither settings nor checkpoint data can
// claim that the durable synchronous suspension and asynchronous delivery paths are implemented.
const HUMAN_INTERACTION_EXECUTION_READY: bool = false;

pub(super) struct HumanInteractionExtension {
    source: Option<Arc<dyn HumanInteractionPolicySource>>,
    request: HumanInteractionRequestContract,
}

/// Every model-facing projection comes from this one immutable request snapshot. In particular,
/// a concurrent settings change cannot produce enabled schema with disabled instructions.
struct HumanInteractionRequestContract {
    settings: HumanInteractionSettings,
    execution_ready: bool,
}

impl HumanInteractionRequestContract {
    fn unavailable() -> Self {
        Self {
            settings: unavailable_policy(),
            execution_ready: false,
        }
    }

    fn new(settings: HumanInteractionSettings) -> AgentResult<Self> {
        if settings.revision > HUMAN_INTERACTION_MAX_SAFE_INTEGER
            || settings.updated_at < 0
            || settings.updated_at as u64 > HUMAN_INTERACTION_MAX_SAFE_INTEGER
        {
            return Err(AgentError::new("人机交互策略快照无效。"));
        }
        Ok(Self {
            settings,
            execution_ready: HUMAN_INTERACTION_EXECUTION_READY,
        })
    }

    fn available(&self) -> bool {
        self.settings.enabled && self.execution_ready
    }

    fn tool_definitions(&self) -> Vec<AgentToolDefinition> {
        if self.available() {
            human_interaction_tool_definitions()
        } else {
            Vec::new()
        }
    }

    fn capabilities(&self) -> BTreeSet<ToolCapabilityId> {
        if self.tool_definitions().is_empty() {
            BTreeSet::new()
        } else {
            BTreeSet::from([ToolCapabilityId::application_owned(
                HUMAN_INTERACTION_CAPABILITY,
            )])
        }
    }

    fn context(&self, purpose: ModelRequestPurpose) -> Vec<ContextItem> {
        if !self.available() || purpose != ModelRequestPurpose::AgentWork {
            return Vec::new();
        }
        vec![ContextItem::text(
            LlmMessageRole::System,
            "## 人机交互\n只有缺失信息会实质改变任务结果时才提问，能安全推断时说明假设并继续。使用 request_user_input 等待整批回答；还有独立工作可继续时，使用 request_user_input_async。异步 pending 只表示问题已记录，不代表用户回答；不要轮询、重复提问、猜测答案或把沉默当作同意。每题可提供可选选项；界面允许自由文本或不回答，整批处理后一次提交，提交后不可补答。异步批次可被忽略，忽略不会启动新运行；各题不回答后提交仍是正式提交。两种工具均不能用于权限或工具审批，用户回答不授予任何执行权限。",
            ContextSource::RuntimeGuard,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )]
    }

    fn world_state(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let id = WorldStateSectionId::extension(HUMAN_INTERACTION_EXTENSION_ID)
            .map_err(|error| AgentError::new(error.to_string()))?;
        let state = json!({
            "schemaVersion": 1,
            "policyRevision": self.settings.revision,
            "enabled": self.settings.enabled,
            "executionReady": self.execution_ready,
            "available": self.available(),
        });
        let section = if self.available() {
            WorldStateSectionEnvelope::model_visible(
                id,
                WorldStateLifetime::Run,
                state,
                json!({ "available": true }),
            )
        } else {
            WorldStateSectionEnvelope::host_only(id, WorldStateLifetime::Run, state)
        }
        .map_err(|error| AgentError::new(error.to_string()))?;
        Ok(vec![section])
    }
}

impl HumanInteractionExtension {
    pub(super) fn new(source: Option<Arc<dyn HumanInteractionPolicySource>>) -> Self {
        Self {
            source,
            request: HumanInteractionRequestContract::unavailable(),
        }
    }
}

fn unavailable_policy() -> HumanInteractionSettings {
    HumanInteractionSettings {
        enabled: false,
        revision: 0,
        updated_at: 0,
    }
}

impl RuntimeExtension for HumanInteractionExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: HUMAN_INTERACTION_EXTENSION_ID,
            version: HUMAN_INTERACTION_EXTENSION_VERSION,
            order: 50,
        }
    }

    fn prepare_model_request(&mut self) -> AgentResult<()> {
        // This optional capability must not turn a settings failure into an ordinary Agent-run
        // failure. Replace the complete previous snapshot on every boundary, including failures;
        // retaining its enabled policy would turn a transient read error into stale authority.
        // The user-facing settings RPC retains its own normal error reporting.
        self.request = self
            .source
            .as_ref()
            .and_then(|source| source.snapshot().ok())
            .and_then(|policy| HumanInteractionRequestContract::new(policy).ok())
            .unwrap_or_else(HumanInteractionRequestContract::unavailable);
        Ok(())
    }

    // No Tool implementation is registered in this phase. Merely supplying a policy source or
    // restoring this shell must never expose a Tool that acknowledges without durable execution.
    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        Ok(self.request.capabilities())
    }

    fn request_context(&self, request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        Ok(self.request.context(request.purpose))
    }

    fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        self.request.world_state()
    }

    fn snapshot_state(&self) -> AgentResult<Value> {
        // A snapshot restores the shell only. Policy, runtime readiness and delivery authority
        // always come from the current Host; none can be enabled by a persisted JSON field.
        Ok(json!({ "schemaVersion": HUMAN_INTERACTION_EXTENSION_VERSION }))
    }

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()> {
        if version != HUMAN_INTERACTION_EXTENSION_VERSION
            || state != json!({ "schemaVersion": HUMAN_INTERACTION_EXTENSION_VERSION })
        {
            return Err(AgentError::new("人机交互扩展快照无效或版本不受支持。"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolRegistry;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct Policy {
        settings: Mutex<HumanInteractionSettings>,
        reads: AtomicUsize,
    }

    impl HumanInteractionPolicySource for Policy {
        fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Ok(self.settings.lock().unwrap().clone())
        }
    }

    fn settings(enabled: bool, revision: u64) -> HumanInteractionSettings {
        HumanInteractionSettings {
            enabled,
            revision,
            updated_at: 1,
        }
    }

    #[test]
    fn human_interaction_unready_never_exposes_tools_or_model_instructions() {
        let policy = Arc::new(Policy {
            settings: Mutex::new(settings(true, 7)),
            reads: AtomicUsize::new(0),
        });
        let mut extension = HumanInteractionExtension::new(Some(policy));
        extension.prepare_model_request().unwrap();
        assert!(extension.request.tool_definitions().is_empty());
        assert!(extension.active_tool_capabilities().unwrap().is_empty());
        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
        assert!(extension.tools().is_empty());
        let mut registry = ToolRegistry::defaults_with_search(None);
        extension.register_additional_tools(&mut registry).unwrap();
        for name in crate::tools::human_interaction::HUMAN_INTERACTION_TOOL_NAMES {
            assert!(!registry.contains_tool(name));
        }
        let state = extension.world_state_sections().unwrap();
        assert_eq!(state[0].state["policyRevision"], 7);
        assert_eq!(state[0].state["executionReady"], false);
        assert!(state[0].model_projection.is_none());
    }

    #[test]
    fn human_interaction_projections_freeze_policy_once_per_request() {
        let policy = Arc::new(Policy {
            settings: Mutex::new(settings(true, 3)),
            reads: AtomicUsize::new(0),
        });
        let mut extension = HumanInteractionExtension::new(Some(policy.clone()));
        extension.prepare_model_request().unwrap();
        *policy.settings.lock().unwrap() = settings(false, 4);
        // Contract-only test: no executable handler or service is installed. Future ready
        // implementations must use this same snapshot for schema, prompt and World State.
        extension.request.execution_ready = true;
        assert_eq!(extension.request.tool_definitions().len(), 2);
        assert_eq!(extension.active_tool_capabilities().unwrap().len(), 1);
        assert_eq!(
            extension
                .request_context(&ModelRequestContext::agent_work())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            extension.world_state_sections().unwrap()[0].state["policyRevision"],
            3
        );
        assert_eq!(policy.reads.load(Ordering::SeqCst), 1);
        extension.prepare_model_request().unwrap();
        extension.request.execution_ready = true;
        assert!(extension.request.tool_definitions().is_empty());
        assert!(extension.active_tool_capabilities().unwrap().is_empty());
        assert!(extension
            .request_context(&ModelRequestContext::agent_work())
            .unwrap()
            .is_empty());
        assert_eq!(
            extension.world_state_sections().unwrap()[0].state["policyRevision"],
            4
        );
        assert_eq!(policy.reads.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn human_interaction_compaction_generation_never_inherits_question_instructions() {
        let request = HumanInteractionRequestContract {
            settings: settings(true, 1),
            execution_ready: true,
        };
        assert!(!request.context(ModelRequestPurpose::AgentWork).is_empty());
        assert!(request
            .context(ModelRequestPurpose::ContextCompaction)
            .is_empty());
    }

    #[test]
    fn human_interaction_restore_never_restores_readiness_or_policy() {
        let mut extension = HumanInteractionExtension::new(None);
        let snapshot = extension.snapshot_state().unwrap();
        extension.restore_state(1, snapshot.clone()).unwrap();
        extension.prepare_model_request().unwrap();
        assert!(extension.request.tool_definitions().is_empty());
        assert_eq!(snapshot, json!({ "schemaVersion": 1 }));
        assert!(extension
            .restore_state(1, json!({"schemaVersion":1,"enabled":true}))
            .is_err());
        assert!(extension
            .restore_state(1, json!({"schemaVersion":1,"executionReady":true}))
            .is_err());
        assert!(extension.restore_state(2, snapshot).is_err());
    }

    #[test]
    fn human_interaction_rejects_invalid_host_policy_revisions() {
        assert!(HumanInteractionRequestContract::new(settings(
            true,
            HUMAN_INTERACTION_MAX_SAFE_INTEGER + 1
        ))
        .is_err());
        let mut invalid = settings(true, 1);
        invalid.updated_at = -1;
        assert!(HumanInteractionRequestContract::new(invalid).is_err());
    }

    #[test]
    fn human_interaction_policy_failures_disable_only_the_capability_and_clear_old_snapshot() {
        struct FailingPolicy(AtomicUsize);
        impl HumanInteractionPolicySource for FailingPolicy {
            fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
                match self.0.load(Ordering::SeqCst) {
                    0 => Ok(settings(true, 7)),
                    1 => Err(AgentError::new("settings temporarily unavailable")),
                    2 => Ok(settings(true, HUMAN_INTERACTION_MAX_SAFE_INTEGER + 1)),
                    _ => Ok(HumanInteractionSettings {
                        enabled: true,
                        revision: 8,
                        updated_at: -1,
                    }),
                }
            }
        }
        let policy = Arc::new(FailingPolicy(AtomicUsize::new(0)));
        let mut extension = HumanInteractionExtension::new(Some(policy.clone()));
        for failure in 1..=3 {
            policy.0.store(0, Ordering::SeqCst);
            extension.prepare_model_request().unwrap();
            assert!(extension.request.settings.enabled);
            assert_eq!(extension.request.settings.revision, 7);
            policy.0.store(failure, Ordering::SeqCst);
            extension.prepare_model_request().unwrap();
            assert_eq!(extension.request.settings, unavailable_policy());
            assert!(!extension.request.execution_ready);
            assert!(extension.active_tool_capabilities().unwrap().is_empty());
            assert!(extension
                .request_context(&ModelRequestContext::agent_work())
                .unwrap()
                .is_empty());
            assert!(extension.world_state_sections().unwrap()[0]
                .model_projection
                .is_none());
        }
    }
}
