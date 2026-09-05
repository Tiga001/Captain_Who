use super::{ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeExtension};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::human_interaction::{HumanInteractionSettings, HUMAN_INTERACTION_MAX_SAFE_INTEGER};
use crate::llm::LlmMessageRole;
#[cfg(test)]
use crate::protocol::AgentToolDefinition;
use crate::protocol::{AgentError, AgentResult};
#[cfg(test)]
use crate::tools::human_interaction::human_interaction_tool_definitions;
use crate::tools::human_interaction::{
    HUMAN_INTERACTION_ASYNC_CAPABILITY, HUMAN_INTERACTION_CAPABILITY,
};
use crate::tools::ToolCapabilityId;
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use crate::HumanInteractionPolicySource;
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) const HUMAN_INTERACTION_EXTENSION_ID: &str = "human.interaction";
const HUMAN_INTERACTION_EXTENSION_VERSION: u32 = 1;

// Shared guidance stays identical for sync-only, async-only and combined capability snapshots.
// Only the routing paragraph below names the tools available at this model request boundary.
const HUMAN_INTERACTION_GUIDANCE: &str = "## 人机交互\n\n\
你可以通过人机交互工具邀请用户参与任务，包括补充信息、表达偏好、作出判断或决定、提供反馈，以及完成需要用户亲自参与的操作。用户明确要求发起交互时，也可使用这些工具。\n\n\
在交互前，先明确需要用户参与的具体事项。已有信息或当前可用能力足以解决的事情，应自行处理；需要用户提供独有信息、主观判断或实际协助时，清楚提出请求。\n\n\
每个条目围绕一个明确事项，提供足够的上下文，让用户能够理解并回应。请求实际协助时，说明需要做什么，以及希望用户反馈什么；必要时简短说明原因。\n\n\
选项应根据当前事项设计，准确表达有意义的不同回应，措辞简洁、清晰，避免诱导或预设用户立场。可以提供选择、判断、反馈或行动结果等不同形式的选项，不要求套用固定模板。开放式事项可以只提供文字输入，不必强行设计选项。\n\n\
一次批次中的条目由用户整批提交。提交前的选择和草稿都不是正式回应，不据此推进依赖用户回应的工作。";

const HUMAN_INTERACTION_RESPONSE_GUIDANCE: &str = "收到回应后，结合原交互内容理解用户实际表达的意思，并据此决定下一步。只作回应能够支持的判断：不要将偏好扩大为授权，将意向视为行动结果，或将用户陈述写成自己执行、观察或验证所得的事实。后续工作若依赖可核验的外部状态，使用当前可用且获授权的能力进行必要核验；无法核验时明确事实来源和仍存在的不确定性。\n\n\
用户的拒绝、跳过或忽略都应得到尊重，不能被解释为同意或所请求事项已经发生。根据已有信息调整方案；确实无法继续时说明缺少的条件，不反复催促。\n\n\
人机交互不替代应用的执行权限审批，也不能用于绕过已有权限限制。";

pub(super) struct HumanInteractionExtension {
    source: Option<Arc<dyn HumanInteractionPolicySource>>,
    execution_ready: bool,
    async_execution_ready: bool,
    request: HumanInteractionRequestContract,
}

/// Every model-facing projection comes from this one immutable request snapshot. In particular,
/// a concurrent settings change cannot produce enabled schema with disabled instructions.
struct HumanInteractionRequestContract {
    settings: HumanInteractionSettings,
    execution_ready: bool,
    async_execution_ready: bool,
}

impl HumanInteractionRequestContract {
    fn unavailable() -> Self {
        Self {
            settings: unavailable_policy(),
            execution_ready: false,
            async_execution_ready: false,
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
            execution_ready: false,
            async_execution_ready: false,
        })
    }

    fn available(&self) -> bool {
        self.settings.enabled && (self.execution_ready || self.async_execution_ready)
    }

    #[cfg(test)]
    fn tool_definitions(&self) -> Vec<AgentToolDefinition> {
        if self.available() {
            human_interaction_tool_definitions()
                .into_iter()
                .enumerate()
                .filter(|(index, _)| {
                    if *index == 0 {
                        self.execution_ready
                    } else {
                        self.async_execution_ready
                    }
                })
                .map(|(_, definition)| definition)
                .collect()
        } else {
            Vec::new()
        }
    }

    fn capabilities(&self) -> BTreeSet<ToolCapabilityId> {
        let mut capabilities = BTreeSet::new();
        if self.settings.enabled && self.execution_ready {
            capabilities.insert(ToolCapabilityId::application_owned(
                HUMAN_INTERACTION_CAPABILITY,
            ));
        }
        if self.settings.enabled && self.async_execution_ready {
            capabilities.insert(ToolCapabilityId::application_owned(
                HUMAN_INTERACTION_ASYNC_CAPABILITY,
            ));
        }
        capabilities
    }

    fn context(&self, purpose: ModelRequestPurpose) -> Vec<ContextItem> {
        if !self.available() || purpose != ModelRequestPurpose::AgentWork {
            return Vec::new();
        }
        let routing = if self.async_execution_ready && self.execution_ready {
            "后续推进必须等待用户参与时，使用 request_user_input；期间仍有独立工作可做时，使用 request_user_input_async。同步回应通过原工具调用的唯一结果返回；异步调用返回 accepted/requestId 仅表示请求已记录，回应随后作为用户输入送达，不会产生本次调用的第二个工具结果。应继续独立工作，等待正式回应，不轮询或重复请求。异步忽略本身不触发新的运行。"
        } else if self.async_execution_ready {
            "需要用户参与且期间仍有独立工作可做时，使用 request_user_input_async。调用返回 accepted/requestId 仅表示请求已记录，回应随后作为用户输入送达，不会产生本次调用的第二个工具结果。应继续不依赖回应的工作，等待正式回应，不轮询或重复请求。忽略本身不触发新的运行。"
        } else {
            "后续推进必须等待用户参与时，使用 request_user_input。当前运行暂停至用户整批提交，回应通过原工具调用的唯一结果返回，再继续后续工作。等待正式回应，不轮询或重复请求。"
        };
        let instructions = format!(
            "{HUMAN_INTERACTION_GUIDANCE}\n\n{routing}\n\n{HUMAN_INTERACTION_RESPONSE_GUIDANCE}"
        );
        vec![ContextItem::text(
            LlmMessageRole::System,
            instructions,
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
            "asyncExecutionReady": self.async_execution_ready,
            "asyncAvailable": self.settings.enabled && self.async_execution_ready,
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
            execution_ready: false,
            async_execution_ready: false,
            request: HumanInteractionRequestContract::unavailable(),
        }
    }

    pub(super) fn with_execution_ready(mut self, execution_ready: bool) -> Self {
        self.execution_ready = execution_ready;
        self
    }

    pub(super) fn with_async_execution_ready(mut self, execution_ready: bool) -> Self {
        self.async_execution_ready = execution_ready;
        self
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
        self.request.execution_ready = self.execution_ready && self.source.is_some();
        self.request.async_execution_ready = self.async_execution_ready && self.source.is_some();
        Ok(())
    }

    fn tools(&self) -> Vec<Box<dyn crate::tools::AgentTool>> {
        let mut tools: Vec<Box<dyn crate::tools::AgentTool>> = Vec::new();
        if self.execution_ready {
            tools.push(Box::new(
                crate::tools::human_interaction::RequestUserInputTool,
            ));
        }
        if self.async_execution_ready {
            tools.push(Box::new(
                crate::tools::human_interaction::RequestUserInputAsyncTool,
            ));
        }
        tools
    }
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
        let mut extension = HumanInteractionExtension::new(Some(policy.clone()))
            .with_execution_ready(true)
            .with_async_execution_ready(true);
        extension.prepare_model_request().unwrap();
        *policy.settings.lock().unwrap() = settings(false, 4);
        // The executable tool, schema, prompt and World State share the frozen request snapshot.
        assert_eq!(extension.tools().len(), 2);
        assert_eq!(extension.request.tool_definitions().len(), 2);
        assert_eq!(extension.active_tool_capabilities().unwrap().len(), 2);
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
    fn human_interaction_guidance_names_only_the_available_tools() {
        for enabled in [false, true] {
            for execution_ready in [false, true] {
                for async_execution_ready in [false, true] {
                    let request = HumanInteractionRequestContract {
                        settings: settings(enabled, 1),
                        execution_ready,
                        async_execution_ready,
                    };
                    let frame = crate::context::ContextFrame::new(
                        request.context(ModelRequestPurpose::AgentWork),
                    );
                    let messages = frame.to_messages();
                    let definitions = request.tool_definitions();
                    if definitions.is_empty() {
                        assert!(messages.is_empty());
                        continue;
                    }
                    assert_eq!(messages.len(), 1);
                    let text = messages[0].content();
                    let named_tools = text
                        .split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                        .filter(|word| word.starts_with("request_user_input"))
                        .collect::<BTreeSet<_>>();
                    let exposed_tools = definitions
                        .iter()
                        .map(|definition| definition.name.as_str())
                        .collect::<BTreeSet<_>>();
                    assert_eq!(named_tools, exposed_tools);
                    assert!(text.contains("不要求套用固定模板"));
                    assert!(text.contains("需要用户亲自参与的操作"));
                    assert!(!text.contains("已完成"));
                    assert!(request
                        .context(ModelRequestPurpose::ContextCompaction)
                        .is_empty());
                    assert_eq!(frame.manifest().entries[0].retention, "request_only");
                }
            }
        }
    }

    #[test]
    fn human_interaction_compaction_generation_never_inherits_question_instructions() {
        let request = HumanInteractionRequestContract {
            settings: settings(true, 1),
            execution_ready: true,
            async_execution_ready: false,
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
