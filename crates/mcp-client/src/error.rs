use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpErrorKind {
    Config,
    Spawn,
    Negotiation,
    Protocol,
    Timeout,
    Cancelled,
    ServerExited,
    Shutdown,
}

#[derive(Clone, Debug, Error, PartialEq, Eq, Serialize, Deserialize)]
#[error("{kind}: {message}")]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct McpError {
    pub kind: McpErrorKind,
    pub message: String,
    pub operation: Option<String>,
    pub timeout_ms: Option<u64>,
    pub exit_code: Option<i32>,
}

impl McpError {
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Config, message)
    }

    pub fn spawn(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Spawn, message)
    }

    pub fn negotiation(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Negotiation, message)
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Protocol, message)
    }

    pub fn timeout(operation: impl Into<String>, timeout_ms: u64) -> Self {
        let operation = operation.into();
        Self {
            kind: McpErrorKind::Timeout,
            message: format!("{operation} timed out"),
            operation: Some(operation),
            timeout_ms: Some(timeout_ms),
            exit_code: None,
        }
    }

    pub fn cancelled(operation: impl Into<String>) -> Self {
        let operation = operation.into();
        Self {
            kind: McpErrorKind::Cancelled,
            message: format!("{operation} was cancelled"),
            operation: Some(operation),
            timeout_ms: None,
            exit_code: None,
        }
    }

    pub fn server_exited(exit_code: Option<i32>) -> Self {
        Self {
            kind: McpErrorKind::ServerExited,
            message: "MCP server exited".to_string(),
            operation: None,
            timeout_ms: None,
            exit_code,
        }
    }

    pub fn shutdown(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Shutdown, message)
    }

    fn new(kind: McpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            operation: None,
            timeout_ms: None,
            exit_code: None,
        }
    }
}

impl fmt::Display for McpErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Config => "config",
            Self::Spawn => "spawn",
            Self::Negotiation => "negotiation",
            Self::Protocol => "protocol",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::ServerExited => "server_exited",
            Self::Shutdown => "shutdown",
        };
        formatter.write_str(value)
    }
}
