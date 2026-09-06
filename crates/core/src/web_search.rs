//! Host-owned web policy. Request snapshots contain no credentials; each new Tool execution
//! obtains fresh authorization separately. Policy is never restored from model/checkpoint input.

use crate::{AgentError, AgentResult, AgentSearchConfig, AgentSearchMode};
use serde_json::json;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebSearchPolicySnapshot {
    pub enabled: bool,
    pub credential_ready: bool,
}

impl WebSearchPolicySnapshot {
    pub fn available(self) -> bool {
        self.enabled && self.credential_ready
    }

    pub fn ensure_available(self) -> AgentResult<()> {
        let reason = if !self.enabled {
            "disabled_by_user"
        } else if !self.credential_ready {
            "configuration_required"
        } else {
            return Ok(());
        };
        Err(AgentError::structured(
            format!("web_search.{reason}"),
            if self.enabled {
                "联网搜索尚未配置可用凭据。"
            } else {
                "用户已关闭联网搜索。"
            },
            json!({ "reason": reason }),
        ))
    }
}

/// No serde implementation or plaintext Debug: this value belongs only to the execution path.
pub struct WebSearchExecutionCredential(String);

impl WebSearchExecutionCredential {
    pub fn new(value: String) -> AgentResult<Self> {
        let value = value.trim();
        if value.is_empty()
            || value.len() > 8_192
            || value
                .chars()
                .any(|character| character.is_whitespace() || character.is_control())
        {
            return Err(AgentError::structured(
                "web_search.configuration_required",
                "联网搜索尚未配置可用凭据。",
                json!({ "reason": "configuration_required" }),
            ));
        }
        Ok(Self(value.to_string()))
    }

    pub(crate) fn into_secret(self) -> String {
        self.0
    }
}

impl fmt::Debug for WebSearchExecutionCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("WebSearchExecutionCredential([REDACTED])")
    }
}

pub trait WebSearchPolicySource: Send + Sync {
    /// Called exactly once at each natural model-request boundary.
    fn snapshot(&self) -> AgentResult<WebSearchPolicySnapshot>;

    /// Recheck current user policy and credentials before admitting a new network operation.
    /// Already admitted operations keep their normal cancellation/completion lifecycle.
    fn authorize_execution(&self) -> AgentResult<WebSearchExecutionCredential>;
}

/// Explicit compatibility source for Core-only callers and previews without a live Host.
/// Its state stays frozen for this run; a supplied Host source always takes precedence.
pub struct FrozenWebSearchPolicySource {
    enabled: bool,
    credential: Option<String>,
}

impl FrozenWebSearchPolicySource {
    pub fn from_search_config(configuration: Option<&AgentSearchConfig>) -> Self {
        Self {
            enabled: configuration.is_some_and(|value| value.mode != AgentSearchMode::Disabled),
            credential: configuration
                .and_then(|value| value.tavily_api_key.as_ref())
                .filter(|value| WebSearchExecutionCredential::new((*value).clone()).is_ok())
                .cloned(),
        }
    }
}

impl fmt::Debug for FrozenWebSearchPolicySource {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("FrozenWebSearchPolicySource([REDACTED])")
    }
}

impl WebSearchPolicySource for FrozenWebSearchPolicySource {
    fn snapshot(&self) -> AgentResult<WebSearchPolicySnapshot> {
        Ok(WebSearchPolicySnapshot {
            enabled: self.enabled,
            credential_ready: self.credential.is_some(),
        })
    }

    fn authorize_execution(&self) -> AgentResult<WebSearchExecutionCredential> {
        self.snapshot()?.ensure_available()?;
        WebSearchExecutionCredential::new(self.credential.clone().unwrap_or_default())
    }
}
