use super::*;

/// Live Host authority shared by root, child and automation runtime segments and previews.
/// It intentionally owns no run-scoped settings snapshot or credential cache.
struct StoredWebSearchPolicy {
    storage: Arc<StorageService>,
}

impl mycopilot_core::WebSearchPolicySource for StoredWebSearchPolicy {
    fn snapshot(&self) -> AgentResult<mycopilot_core::WebSearchPolicySnapshot> {
        self.storage.load_web_search_policy_snapshot().map_err(|_| {
            AgentError::structured(
                "web_search.configuration_required",
                "联网搜索尚未配置可用凭据。",
                serde_json::json!({ "reason": "configuration_required" }),
            )
        })
    }

    fn authorize_execution(&self) -> AgentResult<mycopilot_core::WebSearchExecutionCredential> {
        self.storage.authorize_web_search_execution()
    }
}

impl AgentService {
    pub(super) fn web_search_policy_source(
        &self,
    ) -> Arc<dyn mycopilot_core::WebSearchPolicySource> {
        Arc::new(StoredWebSearchPolicy {
            storage: Arc::clone(&self.storage),
        })
    }
}
