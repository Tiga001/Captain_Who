use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{McpConnectionState, McpServerId};

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
