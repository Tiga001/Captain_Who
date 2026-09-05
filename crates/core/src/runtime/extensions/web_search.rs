use super::{ExtensionDescriptor, ModelRequestContext, ModelRequestPurpose, RuntimeExtension};
use crate::context::{ContextItem, ContextRetention, ContextScope, ContextSource};
use crate::llm::LlmMessageRole;
use crate::tools::{AgentTool, ToolCapabilityId, WebFetchTool, WebSearchTool, WEB_SEARCH_CAPABILITY};
use crate::world_state::{WorldStateLifetime, WorldStateSectionEnvelope, WorldStateSectionId};
use crate::{AgentError, AgentResult, WebSearchPolicySnapshot, WebSearchPolicySource};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::Arc;

pub(super) const WEB_SEARCH_EXTENSION_ID: &str = "web.search";
const WEB_SEARCH_EXTENSION_VERSION: u32 = 1;

pub(super) struct WebSearchExtension {
    source: Arc<dyn WebSearchPolicySource>,
    request: Option<WebSearchPolicySnapshot>,
}

impl WebSearchExtension {
    pub(super) fn new(source: Arc<dyn WebSearchPolicySource>) -> Self {
        Self { source, request: None }
    }

    fn available(&self) -> bool {
        self.request.is_some_and(WebSearchPolicySnapshot::available)
    }
}

impl RuntimeExtension for WebSearchExtension {
    fn descriptor(&self) -> ExtensionDescriptor {
        ExtensionDescriptor {
            id: WEB_SEARCH_EXTENSION_ID,
            version: WEB_SEARCH_EXTENSION_VERSION,
            order: 40,
        }
    }

    fn prepare_model_request(&mut self) -> AgentResult<()> {
        // Fail closed on a Host read error without retaining the previous request's permission.
        self.request = self.source.snapshot().ok();
        Ok(())
    }

    fn tools(&self) -> Vec<Box<dyn AgentTool>> {
        // The implementation registry stays complete so an initially disabled run can enable
        // this capability at its next natural model request without creating a new run.
        vec![
            Box::new(WebSearchTool::with_policy(Arc::clone(&self.source))),
            Box::new(WebFetchTool::with_policy(Arc::clone(&self.source))),
        ]
    }

    fn active_tool_capabilities(&self) -> AgentResult<BTreeSet<ToolCapabilityId>> {
        Ok(if self.available() {
            BTreeSet::from([ToolCapabilityId::application_owned(WEB_SEARCH_CAPABILITY)])
        } else {
            BTreeSet::new()
        })
    }

    fn request_context(&self, request: &ModelRequestContext) -> AgentResult<Vec<ContextItem>> {
        if !self.available() || request.purpose != ModelRequestPurpose::AgentWork {
            return Ok(Vec::new());
        }
        Ok(vec![ContextItem::text(
            LlmMessageRole::System,
            "## 联网搜索\n对当前状态、近期变化、陌生实体或需要来源核实的信息使用 web_search；它返回的是 Provider 生成的搜索摘要/片段，不是网页全文。用它定位和比较来源，查询应围绕明确的信息缺口，并优先官方或一手来源；需要精确正文时再用 web_fetch 打开少量关键页面。已有结果足以回答时停止搜索；追加搜索应补充具体缺口，不要重复高度重叠的查询。本地项目问题不能用网页搜索替代 workspace 检查。\nweb_fetch 用于深读用户明确提供的公开 URL，或从 web_search 结果中筛选出的少量关键页面；仅在搜索摘要不足以支撑结论时读取正文，不要猜测 URL。获取失败时回到搜索结果或说明限制。",
            ContextSource::RuntimeGuard,
            ContextScope::Run,
            ContextRetention::RequestOnly,
        )])
    }

    fn world_state_sections(&self) -> AgentResult<Vec<WorldStateSectionEnvelope>> {
        let reason = match self.request {
            None => "host_unavailable",
            Some(snapshot) if !snapshot.enabled => "disabled_by_user",
            Some(snapshot) if !snapshot.credential_ready => "configuration_required",
            Some(_) => "available",
        };
        let state = json!({ "available": self.available(), "reason": reason });
        Ok(vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::extension(WEB_SEARCH_EXTENSION_ID)
                .map_err(|error| AgentError::new(error.to_string()))?,
            WorldStateLifetime::Run,
            state.clone(),
            state,
        ).map_err(|error| AgentError::new(error.to_string()))?])
    }

    fn snapshot_state(&self) -> AgentResult<Value> {
        Ok(json!({ "schemaVersion": WEB_SEARCH_EXTENSION_VERSION }))
    }

    fn restore_state(&mut self, version: u32, state: Value) -> AgentResult<()> {
        if version != WEB_SEARCH_EXTENSION_VERSION
            || state != json!({ "schemaVersion": WEB_SEARCH_EXTENSION_VERSION })
        {
            return Err(AgentError::new("无法恢复联网搜索扩展：checkpoint 状态版本无效。"));
        }
        // Only restore the shell. A checkpoint cannot grant policy or preserve a credential.
        self.request = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{EffectiveToolSet, ToolExecutionContext, ToolRegistry};
    use crate::WebSearchExecutionCredential;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Policy {
        enabled: AtomicBool,
        credential_ready: AtomicBool,
        fail_read: AtomicBool,
        reads: AtomicUsize,
        executions: AtomicUsize,
    }

    impl Policy {
        fn new(enabled: bool, credential_ready: bool) -> Arc<Self> {
            Arc::new(Self {
                enabled: AtomicBool::new(enabled),
                credential_ready: AtomicBool::new(credential_ready),
                fail_read: AtomicBool::new(false),
                reads: AtomicUsize::new(0),
                executions: AtomicUsize::new(0),
            })
        }
    }

    impl WebSearchPolicySource for Policy {
        fn snapshot(&self) -> AgentResult<WebSearchPolicySnapshot> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.fail_read.load(Ordering::SeqCst) {
                return Err(AgentError::new("Host policy read failed"));
            }
            Ok(WebSearchPolicySnapshot {
                enabled: self.enabled.load(Ordering::SeqCst),
                credential_ready: self.credential_ready.load(Ordering::SeqCst),
            })
        }

        fn authorize_execution(&self) -> AgentResult<WebSearchExecutionCredential> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            self.snapshot()?.ensure_available()?;
            panic!("these tests must never admit a network operation")
        }
    }

    fn registry(extension: &WebSearchExtension) -> ToolRegistry {
        let mut registry = ToolRegistry::defaults_with_search(None);
        for tool in extension.tools() {
            registry.register_extension_tool(WEB_SEARCH_EXTENSION_ID, tool).unwrap();
        }
        registry
    }

    fn tool_set(extension: &WebSearchExtension, registry: &ToolRegistry) -> EffectiveToolSet {
        registry.effective_tool_set(registry.definitions(), &extension.active_tool_capabilities().unwrap()).unwrap()
    }

    #[test]
    fn one_request_snapshot_controls_web_schema_instructions_and_world_state() {
        let source = Policy::new(false, true);
        let mut extension = WebSearchExtension::new(source.clone());
        let registry = registry(&extension);
        extension.prepare_model_request().unwrap();
        let off = tool_set(&extension, &registry);
        assert!(registry.contains_tool("web_search"));
        assert!(!off.contains("web_search"));
        assert!(!off.contains("web_fetch"));
        assert_eq!(extension.world_state_sections().unwrap()[0].state["reason"], "disabled_by_user");
        assert!(extension.request_context(&ModelRequestContext::agent_work()).unwrap().is_empty());

        source.enabled.store(true, Ordering::SeqCst);
        // Every projection retains the same disabled decision until the next natural request.
        assert!(!tool_set(&extension, &registry).contains("web_search"));
        assert!(extension.request_context(&ModelRequestContext::agent_work()).unwrap().is_empty());
        assert_eq!(source.reads.load(Ordering::SeqCst), 1);
        extension.prepare_model_request().unwrap();
        let on = tool_set(&extension, &registry);
        assert!(on.contains("web_search"));
        assert!(on.contains("web_fetch"));
        assert_eq!(on.dynamic_definitions().len(), 2);
        let instructions = extension.request_context(&ModelRequestContext::agent_work()).unwrap();
        assert_eq!(instructions.len(), 1);
        let frame = crate::context::ContextFrame::new(instructions);
        assert_eq!(frame.manifest().entries[0].retention, "request_only");
        let messages = frame.to_messages();
        assert!(messages[0].content().contains("web_search"));
        assert!(messages[0].content().contains("web_fetch"));
        assert!(extension.request_context(&ModelRequestContext { purpose: ModelRequestPurpose::ContextCompaction }).unwrap().is_empty());
        assert_eq!(extension.world_state_sections().unwrap()[0].state["reason"], "available");
        assert_eq!(source.reads.load(Ordering::SeqCst), 2);
        assert_eq!(serde_json::to_vec(off.stable_definitions()).unwrap(), serde_json::to_vec(on.stable_definitions()).unwrap());
        assert_eq!(off.stable_revision(), on.stable_revision());

        source.enabled.store(false, Ordering::SeqCst);
        extension.prepare_model_request().unwrap();
        let disabled_again = tool_set(&extension, &registry);
        assert_eq!(off.revision(), disabled_again.revision());
        // Preserve the already accepted batch contract. Executions still recheck live policy.
        let restored = disabled_again.restore_frozen_checkpoint(&on.checkpoint()).unwrap();
        assert_eq!(restored.revision(), on.revision());
        assert!(extension.request_context(&ModelRequestContext::agent_work()).unwrap().is_empty());
    }

    #[test]
    fn unavailable_credentials_and_host_failures_replace_previous_web_admission() {
        let source = Policy::new(true, true);
        let mut extension = WebSearchExtension::new(source.clone());
        extension.prepare_model_request().unwrap();
        assert!(extension.available());
        source.credential_ready.store(false, Ordering::SeqCst);
        extension.prepare_model_request().unwrap();
        assert!(!extension.available());
        assert_eq!(extension.world_state_sections().unwrap()[0].state["reason"], "configuration_required");
        source.fail_read.store(true, Ordering::SeqCst);
        extension.prepare_model_request().unwrap();
        assert!(extension.active_tool_capabilities().unwrap().is_empty());
        assert!(extension.request_context(&ModelRequestContext::agent_work()).unwrap().is_empty());
        assert_eq!(extension.world_state_sections().unwrap()[0].state["reason"], "host_unavailable");
    }

    #[test]
    fn newly_executed_web_calls_recheck_live_policy_even_for_an_enabled_request() {
        let source = Policy::new(true, true);
        let mut extension = WebSearchExtension::new(source.clone());
        extension.prepare_model_request().unwrap();
        assert!(extension.available());
        source.enabled.store(false, Ordering::SeqCst);
        let context = ToolExecutionContext::from_run_context(None);
        for tool in extension.tools() {
            let args = if tool.definition().name == "web_search" {
                json!({ "query": "owned test query" })
            } else {
                json!({ "url": "https://example.com/owned-test" })
            };
            let error = tool.execute(&context, args).unwrap_err();
            assert_eq!(error.code(), Some("web_search.disabled_by_user"));
        }
        assert_eq!(source.executions.load(Ordering::SeqCst), 2);
        assert!(extension.available(), "already accepted request stays frozen");
    }

    #[test]
    fn checkpoints_restore_only_a_web_shell_and_never_a_policy_or_credential() {
        let source = Policy::new(true, true);
        let mut extension = WebSearchExtension::new(source.clone());
        extension.prepare_model_request().unwrap();
        let saved = extension.snapshot_state().unwrap();
        assert_eq!(saved, json!({ "schemaVersion": 1 }));
        source.enabled.store(false, Ordering::SeqCst);
        extension.restore_state(1, saved).unwrap();
        assert!(!extension.available());
        extension.prepare_model_request().unwrap();
        assert!(!extension.available());
        assert!(extension.restore_state(1, json!({ "schemaVersion": 1, "enabled": true })).is_err());
        assert!(!format!("{:?}", WebSearchExecutionCredential::new("test-secret".to_string()).unwrap()).contains("test-secret"));
    }
}
