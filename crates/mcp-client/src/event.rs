use std::fmt;

use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::{
    McpCatalogCompleteness, McpConfigDigest, McpConnectionState, McpError, McpErrorKind,
    McpRegistryChangeKind, McpServerId, McpServerScope,
};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpStderrSnapshot {
    /// Ephemeral user-requested diagnostics. Hosts must not persist this field
    /// into traces, checkpoints, or ordinary application events.
    #[serde(skip_serializing, default)]
    pub retained: String,
    pub retained_bytes: usize,
    pub dropped_bytes: u64,
    pub truncated: bool,
}

impl fmt::Debug for McpStderrSnapshot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpStderrSnapshot")
            .field("retained", &"<redacted>")
            .field("retained_bytes", &self.retained_bytes)
            .field("dropped_bytes", &self.dropped_bytes)
            .field("truncated", &self.truncated)
            .finish()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpConnectionEvent {
    StateChanged {
        server_id: McpServerId,
        state: McpConnectionState,
    },
    StderrLimited {
        server_id: McpServerId,
        dropped_bytes: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpPeerNotificationState {
    Unknown,
    Unsupported,
    Active,
    Unavailable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpPeerSignalSnapshot {
    pub sequence: u64,
    pub tools_revision: u64,
    pub notification_state: McpPeerNotificationState,
    pub transport_closed: bool,
    pub exit_code: Option<i32>,
}

impl Default for McpPeerSignalSnapshot {
    fn default() -> Self {
        Self {
            sequence: 0,
            tools_revision: 0,
            notification_state: McpPeerNotificationState::Unknown,
            transport_closed: false,
            exit_code: None,
        }
    }
}

pub struct McpPeerSignalReceiver {
    receiver: watch::Receiver<McpPeerSignalSnapshot>,
}

impl fmt::Debug for McpPeerSignalReceiver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpPeerSignalReceiver")
            .field("snapshot", &*self.receiver.borrow())
            .finish()
    }
}

impl McpPeerSignalReceiver {
    pub fn snapshot(&self) -> McpPeerSignalSnapshot {
        *self.receiver.borrow()
    }

    pub async fn changed(&mut self) -> Option<McpPeerSignalSnapshot> {
        self.receiver.changed().await.ok()?;
        Some(*self.receiver.borrow_and_update())
    }
}

#[derive(Clone)]
pub(crate) struct McpPeerSignalPublisher {
    sender: watch::Sender<McpPeerSignalSnapshot>,
}

impl McpPeerSignalPublisher {
    pub(crate) fn new() -> Self {
        let (sender, _) = watch::channel(McpPeerSignalSnapshot::default());
        Self { sender }
    }

    pub(crate) fn subscribe(&self) -> McpPeerSignalReceiver {
        McpPeerSignalReceiver {
            receiver: self.sender.subscribe(),
        }
    }

    pub(crate) fn set_notification_state(&self, state: McpPeerNotificationState) {
        self.sender.send_modify(|snapshot| {
            snapshot.sequence = snapshot.sequence.saturating_add(1);
            snapshot.notification_state = state;
        });
    }

    pub(crate) fn tools_changed(&self) {
        self.sender.send_modify(|snapshot| {
            snapshot.sequence = snapshot.sequence.saturating_add(1);
            snapshot.tools_revision = snapshot.tools_revision.saturating_add(1);
        });
    }

    pub(crate) fn notifications_unavailable(&self) {
        self.sender.send_modify(|snapshot| {
            snapshot.sequence = snapshot.sequence.saturating_add(1);
            snapshot.tools_revision = snapshot.tools_revision.saturating_add(1);
            snapshot.notification_state = McpPeerNotificationState::Unavailable;
        });
    }

    pub(crate) fn transport_closed(&self, exit_code: Option<i32>) {
        self.sender.send_modify(|snapshot| {
            if !snapshot.transport_closed {
                snapshot.sequence = snapshot.sequence.saturating_add(1);
                snapshot.transport_closed = true;
                snapshot.exit_code = exit_code;
            } else if snapshot.exit_code.is_none() && exit_code.is_some() {
                snapshot.sequence = snapshot.sequence.saturating_add(1);
                snapshot.exit_code = exit_code;
            }
        });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpServerState {
    Disabled,
    Starting,
    Discovering,
    Ready,
    Stopping,
    Error,
    Backoff,
    Degraded,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSafeError {
    pub kind: McpErrorKind,
    pub code: String,
    pub message: String,
    pub exit_code: Option<i32>,
}

impl From<&McpError> for McpSafeError {
    fn from(error: &McpError) -> Self {
        let (code, message) = match error.kind {
            McpErrorKind::Config => (
                "mcp_config_error",
                "The MCP server configuration is invalid.",
            ),
            McpErrorKind::Spawn => ("mcp_spawn_error", "The MCP server could not be started."),
            McpErrorKind::Negotiation => (
                "mcp_negotiation_error",
                "The MCP protocol negotiation failed.",
            ),
            McpErrorKind::Protocol => (
                "mcp_protocol_error",
                "The MCP server returned an invalid protocol response.",
            ),
            McpErrorKind::OutputTooLarge => (
                "mcp_output_too_large",
                "The MCP server returned a tool result that exceeded safety limits.",
            ),
            McpErrorKind::Timeout => ("mcp_timeout", "The MCP operation timed out."),
            McpErrorKind::Cancelled => ("mcp_cancelled", "The MCP operation was cancelled."),
            McpErrorKind::OutcomeUnknown => (
                "mcp_outcome_unknown",
                "The MCP request may have reached the server, but its outcome is unknown.",
            ),
            McpErrorKind::ServerExited => ("mcp_server_exited", "The MCP server process exited."),
            McpErrorKind::Shutdown => (
                "mcp_shutdown_error",
                "The MCP server could not be shut down cleanly.",
            ),
        };
        Self {
            kind: error.kind,
            code: code.to_string(),
            message: message.to_string(),
            exit_code: error.exit_code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpEvent {
    /// A committed Registry mutation containing only routing-safe identity metadata.
    ///
    /// In particular, this event never carries the Server configuration, command, environment,
    /// headers, or secret references.
    RegistryChanged {
        revision: u64,
        kind: McpRegistryChangeKind,
        server_id: McpServerId,
        scope: McpServerScope,
        config_digest: McpConfigDigest,
        config_epoch: crate::McpConfigEpoch,
    },
    /// The Registry broadcast receiver skipped one or more mutations.
    ///
    /// A Host must treat this as a fail-closed signal for approval state because a removed
    /// configuration source may no longer be present in the current Registry snapshot.
    RegistryReconciliationRequired { skipped_changes: u64 },
    ServerStateChanged {
        server_id: McpServerId,
        sequence: u64,
        previous: McpServerState,
        current: McpServerState,
    },
    CatalogChanged {
        server_id: McpServerId,
        sequence: u64,
        generation: u64,
        config_epoch: crate::McpConfigEpoch,
        registry_revision: u64,
        config_digest: McpConfigDigest,
        completeness: McpCatalogCompleteness,
        tool_count: usize,
    },
    ServerError {
        server_id: McpServerId,
        sequence: u64,
        error: McpSafeError,
    },
    ServerExited {
        server_id: McpServerId,
        sequence: u64,
        exit_code: Option<i32>,
    },
}

pub trait McpEventSink: Send + Sync {
    fn emit(&self, event: McpEvent);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoopMcpEventSink;

impl McpEventSink for NoopMcpEventSink {
    fn emit(&self, _event: McpEvent) {}
}

impl<F> McpEventSink for F
where
    F: Fn(McpEvent) + Send + Sync,
{
    fn emit(&self, event: McpEvent) {
        self(event);
    }
}
