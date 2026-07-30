use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{McpDispatchCertainty, McpOutcomeUnknownReason};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpErrorKind {
    Config,
    Spawn,
    Negotiation,
    Protocol,
    OutputTooLarge,
    Timeout,
    Cancelled,
    OutcomeUnknown,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_certainty: Option<McpDispatchCertainty>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome_unknown_reason: Option<McpOutcomeUnknownReason>,
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
            .with_dispatch_certainty(McpDispatchCertainty::DefinitelyNotDispatched)
    }

    pub fn output_too_large(operation: impl Into<String>, message: impl Into<String>) -> Self {
        let mut error = Self::new(McpErrorKind::OutputTooLarge, message);
        error.operation = Some(operation.into());
        error.dispatch_certainty = Some(McpDispatchCertainty::ResponseReceived);
        error
    }

    pub fn timeout(operation: impl Into<String>, timeout_ms: u64) -> Self {
        let operation = operation.into();
        Self {
            kind: McpErrorKind::Timeout,
            message: format!("{operation} timed out"),
            operation: Some(operation),
            timeout_ms: Some(timeout_ms),
            exit_code: None,
            dispatch_certainty: Some(McpDispatchCertainty::DefinitelyNotDispatched),
            outcome_unknown_reason: None,
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
            dispatch_certainty: Some(McpDispatchCertainty::DefinitelyNotDispatched),
            outcome_unknown_reason: None,
        }
    }

    pub fn outcome_unknown(
        operation: impl Into<String>,
        reason: McpOutcomeUnknownReason,
        certainty: McpDispatchCertainty,
    ) -> Self {
        let operation = operation.into();
        Self {
            kind: McpErrorKind::OutcomeUnknown,
            message: format!(
                "{operation} was interrupted after dispatch; the server outcome is unknown"
            ),
            operation: Some(operation),
            timeout_ms: None,
            exit_code: None,
            dispatch_certainty: Some(certainty),
            outcome_unknown_reason: Some(reason),
        }
    }

    pub fn server_exited(exit_code: Option<i32>) -> Self {
        Self {
            kind: McpErrorKind::ServerExited,
            message: "MCP server exited".to_string(),
            operation: None,
            timeout_ms: None,
            exit_code,
            dispatch_certainty: Some(McpDispatchCertainty::DefinitelyNotDispatched),
            outcome_unknown_reason: None,
        }
    }

    pub fn shutdown(message: impl Into<String>) -> Self {
        Self::new(McpErrorKind::Shutdown, message)
            .with_dispatch_certainty(McpDispatchCertainty::DefinitelyNotDispatched)
    }

    pub fn with_dispatch_certainty(mut self, certainty: McpDispatchCertainty) -> Self {
        self.dispatch_certainty = Some(certainty);
        self
    }

    fn new(kind: McpErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            operation: None,
            timeout_ms: None,
            exit_code: None,
            dispatch_certainty: None,
            outcome_unknown_reason: None,
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
            Self::OutputTooLarge => "output_too_large",
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::ServerExited => "server_exited",
            Self::Shutdown => "shutdown",
        };
        formatter.write_str(value)
    }
}
