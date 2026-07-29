use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{McpError, McpServerId};

pub(crate) const MAX_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1_000;

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
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerConfig {
    pub id: McpServerId,
    pub display_name: String,
    pub scope: McpServerScope,
    pub trust: McpTrustLevel,
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
        30_000
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
        for (name, value) in [
            ("connect", self.connect_timeout_ms),
            ("request", self.request_timeout_ms),
            ("shutdown", self.shutdown_timeout_ms),
        ] {
            if !(1..=MAX_TIMEOUT_MS).contains(&value) {
                return Err(McpError::config(format!(
                    "MCP {name} timeout must be between 1 ms and 24 hours"
                )));
            }
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
            .field("enabled", &self.enabled)
            .field("transport", &self.transport)
            .field("connect_timeout_ms", &self.connect_timeout_ms)
            .field("request_timeout_ms", &self.request_timeout_ms)
            .field("shutdown_timeout_ms", &self.shutdown_timeout_ms)
            .finish()
    }
}
