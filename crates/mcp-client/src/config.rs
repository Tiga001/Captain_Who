use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{McpError, McpServerId};

pub(crate) const MAX_TIMEOUT_MS: u64 = 300_000;
const MAX_CONNECT_TIMEOUT_MS: u64 = 10_000;
const MAX_SHUTDOWN_TIMEOUT_MS: u64 = 2_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpServerScope {
    Builtin,
    User,
    Project { project_id: String },
    Plugin { plugin_id: String },
    Managed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTrustLevel {
    Untrusted,
    UserApproved,
    Managed,
    Builtin,
}

/// Host-side policy for every tool invocation from this server.
///
/// `Prompt` means the Host must obtain a distinct per-call approval before it
/// invokes the Manager. `Auto` allows the Host to dispatch without that
/// per-call prompt after all other identity, trust and catalog checks pass.
/// `Deny` is an emergency/configuration kill switch and is enforced again
/// inside the Manager.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpApprovalMode {
    Prompt,
    Auto,
    Deny,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpEnvBinding {
    /// A non-secret literal value. Credentials must use `SecretRef` instead.
    Plain { name: String, value: String },
    /// An opaque secret-store reference. Resolution is fail-closed in round 1.
    SecretRef { name: String, secret_id: String },
    /// A host value copied only when the connector policy names it explicitly.
    AllowlistedHostVariable { name: String },
}

impl McpEnvBinding {
    pub fn name(&self) -> &str {
        match self {
            Self::Plain { name, .. }
            | Self::SecretRef { name, .. }
            | Self::AllowlistedHostVariable { name } => name,
        }
    }
}

impl fmt::Debug for McpEnvBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plain { name, .. } => formatter
                .debug_struct("Plain")
                .field("name", name)
                .field("value", &"<redacted>")
                .finish(),
            Self::SecretRef { name, .. } => formatter
                .debug_struct("SecretRef")
                .field("name", name)
                .field("secret_id", &"<redacted>")
                .finish(),
            Self::AllowlistedHostVariable { name } => formatter
                .debug_struct("AllowlistedHostVariable")
                .field("name", name)
                .finish(),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStdioConfig {
    pub program: PathBuf,
    #[serde(default)]
    pub arguments: Vec<String>,
    pub cwd: PathBuf,
    #[serde(default)]
    pub environment: Vec<McpEnvBinding>,
}

/// Host-owned in-process bridge identity for a managed MCP implementation.
///
/// This transport carries no executable, endpoint, credential, or user-editable parameter. Hosts
/// must construct it from a compiled allowlist; persistent external Registry adapters are expected
/// to reject it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpHostBridgeConfig {
    pub channel: String,
}

impl McpHostBridgeConfig {
    pub fn new(channel: impl Into<String>) -> Result<Self, McpError> {
        let config = Self {
            channel: channel.into(),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), McpError> {
        if self.channel.is_empty()
            || self.channel.len() > 128
            || !self.channel.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'.' | b'_' | b'-')
            })
        {
            return Err(McpError::config(
                "MCP Host bridge channel is not a canonical Host-owned identity",
            ));
        }
        Ok(())
    }
}

impl fmt::Debug for McpHostBridgeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpHostBridgeConfig")
            .field("channel", &self.channel)
            .finish()
    }
}

impl fmt::Debug for McpStdioConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpStdioConfig")
            .field("program", &self.program)
            .field("argument_count", &self.arguments.len())
            .field("cwd", &self.cwd)
            .field(
                "environment_names",
                &self
                    .environment
                    .iter()
                    .map(McpEnvBinding::name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "transport", content = "config", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpTransportConfig {
    Stdio(McpStdioConfig),
    HostBridge(McpHostBridgeConfig),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServerConfig {
    pub id: McpServerId,
    pub display_name: String,
    pub scope: McpServerScope,
    pub trust: McpTrustLevel,
    pub approval_mode: McpApprovalMode,
    pub enabled: bool,
    pub transport: McpTransportConfig,
    #[serde(default = "McpServerConfig::default_connect_timeout_ms")]
    pub connect_timeout_ms: u64,
    #[serde(default = "McpServerConfig::default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    #[serde(default = "McpServerConfig::default_shutdown_timeout_ms")]
    pub shutdown_timeout_ms: u64,
}

impl McpServerConfig {
    pub const fn default_connect_timeout_ms() -> u64 {
        10_000
    }

    pub const fn default_request_timeout_ms() -> u64 {
        60_000
    }

    pub const fn default_shutdown_timeout_ms() -> u64 {
        2_000
    }

    pub(crate) fn connect_timeout(&self) -> Duration {
        Duration::from_millis(self.connect_timeout_ms.max(1))
    }

    pub(crate) fn request_timeout(&self) -> Duration {
        Duration::from_millis(self.request_timeout_ms.max(1))
    }

    pub(crate) fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.shutdown_timeout_ms.max(1))
    }

    pub(crate) fn validate_timeouts(&self) -> Result<(), McpError> {
        if !(1..=MAX_CONNECT_TIMEOUT_MS).contains(&self.connect_timeout_ms) {
            return Err(McpError::config(
                "MCP connect timeout must be between 1 ms and 10 seconds",
            ));
        }
        if !(1..=MAX_TIMEOUT_MS).contains(&self.request_timeout_ms) {
            return Err(McpError::config(
                "MCP request timeout must be between 1 ms and 300 seconds",
            ));
        }
        if !(1..=MAX_SHUTDOWN_TIMEOUT_MS).contains(&self.shutdown_timeout_ms) {
            return Err(McpError::config(
                "MCP shutdown timeout must be between 1 ms and 2 seconds",
            ));
        }
        if let McpTransportConfig::HostBridge(bridge) = &self.transport {
            bridge.validate()?;
        }
        Ok(())
    }
}

impl fmt::Debug for McpServerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpServerConfig")
            .field("id", &self.id)
            .field("display_name", &self.display_name)
            .field("scope", &self.scope)
            .field("trust", &self.trust)
            .field("approval_mode", &self.approval_mode)
            .field("enabled", &self.enabled)
            .field("transport", &self.transport)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("shutdown_timeout_ms", &self.shutdown_timeout_ms)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> McpServerConfig {
        McpServerConfig {
            id: McpServerId::new(),
            display_name: "owned fixture".to_string(),
            scope: McpServerScope::User,
            trust: McpTrustLevel::UserApproved,
            approval_mode: McpApprovalMode::Prompt,
            enabled: true,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: PathBuf::from("/owned-fixture"),
                arguments: Vec::new(),
                cwd: PathBuf::from("/"),
                environment: Vec::new(),
            }),
            connect_timeout_ms: McpServerConfig::default_connect_timeout_ms(),
            request_timeout_ms: McpServerConfig::default_request_timeout_ms(),
            shutdown_timeout_ms: McpServerConfig::default_shutdown_timeout_ms(),
        }
    }

    #[test]
    fn transport_timeouts_cannot_exceed_host_hard_limits() {
        let mut value = config();
        value.validate_timeouts().unwrap();

        value.connect_timeout_ms = MAX_CONNECT_TIMEOUT_MS + 1;
        assert!(value.validate_timeouts().is_err());
        value = config();
        value.request_timeout_ms = MAX_TIMEOUT_MS + 1;
        assert!(value.validate_timeouts().is_err());
        value = config();
        value.shutdown_timeout_ms = MAX_SHUTDOWN_TIMEOUT_MS + 1;
        assert!(value.validate_timeouts().is_err());
    }

    #[test]
    fn current_config_requires_approval_mode_and_rejects_extra_fields() {
        let expected = config();
        let serialized = serde_json::to_value(&expected).unwrap();
        assert_eq!(
            serde_json::from_value::<McpServerConfig>(serialized.clone()).unwrap(),
            expected
        );

        let mut missing_approval_mode = serialized.clone();
        missing_approval_mode
            .as_object_mut()
            .unwrap()
            .remove("approvalMode");
        assert!(serde_json::from_value::<McpServerConfig>(missing_approval_mode).is_err());

        let mut extra_field = serialized;
        extra_field
            .as_object_mut()
            .unwrap()
            .insert("legacyApproval".to_string(), serde_json::json!("prompt"));
        assert!(serde_json::from_value::<McpServerConfig>(extra_field).is_err());
    }

    #[test]
    fn host_bridge_is_explicit_and_contains_no_launch_or_secret_fields() {
        let transport = McpTransportConfig::HostBridge(
            McpHostBridgeConfig::new("builtin.playwright.v1").unwrap(),
        );
        let value = serde_json::to_value(&transport).unwrap();
        assert_eq!(value["transport"], "host_bridge");
        assert_eq!(value["config"]["channel"], "builtin.playwright.v1");
        assert!(value.get("program").is_none());
        assert!(value.get("environment").is_none());
        assert!(McpHostBridgeConfig::new("INVALID CHANNEL").is_err());
    }
}
