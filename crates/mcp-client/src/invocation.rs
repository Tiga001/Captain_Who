use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::{
    McpCatalogDigest, McpConfigDigest, McpConfigEpoch, McpError, McpSchemaDigest, McpServerId,
    McpToolId,
};

const MAX_MODEL_CALL_ID_BYTES: usize = 512;

/// Stable Host-owned identity for one logical tool invocation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct McpInvocationId(Uuid);

impl McpInvocationId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub const fn from_uuid(value: Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for McpInvocationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for McpInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("McpInvocationId")
            .field(&self.0)
            .finish()
    }
}

impl fmt::Display for McpInvocationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for McpInvocationId {
    type Err = McpError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(value)
            .map(Self)
            .map_err(|error| McpError::config(format!("invalid MCP invocation ID: {error}")))
    }
}

/// Provider/Agent Runtime call identity, kept separate from the MCP raw tool name.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct McpModelCallId(String);

impl McpModelCallId {
    pub fn new(value: impl Into<String>) -> Result<Self, McpError> {
        let value = value.into();
        validate_model_call_id(&value)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for McpModelCallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpModelCallId")
            .field("bytes", &self.0.len())
            .finish()
    }
}

impl fmt::Display for McpModelCallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl FromStr for McpModelCallId {
    type Err = McpError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for McpModelCallId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

fn validate_model_call_id(value: &str) -> Result<(), McpError> {
    if value.is_empty()
        || value.len() > MAX_MODEL_CALL_ID_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(McpError::config(
            "MCP model call ID must be non-empty, bounded, and contain no control characters",
        ));
    }
    Ok(())
}

/// Exact key used by the Manager's active-call registry.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpActiveCallId {
    pub server_id: McpServerId,
    pub invocation_id: McpInvocationId,
    pub model_call_id: McpModelCallId,
}

/// Secret-free route identity retained while one MCP call is active.
///
/// Raw arguments, server configuration, environment bindings and transport data are deliberately
/// absent. The route is captured before dispatch so shutdown and diagnostics never need to parse a
/// provider-visible name to determine which server/tool is running.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpActiveCallProvenance {
    pub tool_id: McpToolId,
    pub model_name: String,
    pub config_epoch: McpConfigEpoch,
    pub registry_revision: u64,
    pub config_digest: McpConfigDigest,
    pub catalog_generation: u64,
    pub catalog_digest: McpCatalogDigest,
    pub schema_digest: McpSchemaDigest,
}

impl fmt::Debug for McpActiveCallProvenance {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpActiveCallProvenance")
            .field("server_id", &self.tool_id.server_id)
            .field("raw_name", &"<redacted>")
            .field("model_name", &"<redacted>")
            .field("config_epoch", &self.config_epoch)
            .field("registry_revision", &self.registry_revision)
            .field("config_digest", &self.config_digest)
            .field("catalog_generation", &self.catalog_generation)
            .field("catalog_digest", &self.catalog_digest)
            .field("schema_digest", &self.schema_digest)
            .finish()
    }
}

impl McpActiveCallId {
    pub fn new(
        server_id: McpServerId,
        invocation_id: McpInvocationId,
        model_call_id: McpModelCallId,
    ) -> Self {
        Self {
            server_id,
            invocation_id,
            model_call_id,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
#[repr(u8)]
pub enum McpDispatchPhase {
    #[default]
    NotStarted = 0,
    Dispatching = 1,
    RequestQueued = 2,
    ResponseReceived = 3,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpDispatchCertainty {
    #[default]
    DefinitelyNotDispatched,
    PossiblyDispatched,
    ResponseReceived,
}

/// Shared monotonic dispatch evidence. The Manager owns the conservative
/// transition to `RequestQueued` immediately before entering a peer.
#[derive(Clone, Default)]
pub struct McpDispatchTracker {
    phase: Arc<AtomicU8>,
}

impl fmt::Debug for McpDispatchTracker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("McpDispatchTracker")
            .field("phase", &self.phase())
            .field("certainty", &self.certainty())
            .finish()
    }
}

impl McpDispatchTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn phase(&self) -> McpDispatchPhase {
        match self.phase.load(Ordering::Acquire) {
            0 => McpDispatchPhase::NotStarted,
            1 => McpDispatchPhase::Dispatching,
            2 => McpDispatchPhase::RequestQueued,
            _ => McpDispatchPhase::ResponseReceived,
        }
    }

    pub fn certainty(&self) -> McpDispatchCertainty {
        match self.phase() {
            McpDispatchPhase::NotStarted | McpDispatchPhase::Dispatching => {
                McpDispatchCertainty::DefinitelyNotDispatched
            }
            McpDispatchPhase::RequestQueued => McpDispatchCertainty::PossiblyDispatched,
            McpDispatchPhase::ResponseReceived => McpDispatchCertainty::ResponseReceived,
        }
    }

    pub fn mark_dispatching(&self) {
        self.advance(McpDispatchPhase::Dispatching);
    }

    pub fn mark_request_queued(&self) {
        self.advance(McpDispatchPhase::RequestQueued);
    }

    pub fn mark_response_received(&self) {
        self.advance(McpDispatchPhase::ResponseReceived);
    }

    fn advance(&self, phase: McpDispatchPhase) {
        let target = phase as u8;
        let _ = self
            .phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < target).then_some(target)
            });
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpInvocationState {
    Dispatching,
    Running,
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum McpOutcomeUnknownReason {
    Cancelled,
    TimedOut,
    Shutdown,
    ServerStopped,
    ServerRestarted,
    ServerRemoved,
    ServerExited,
    TransportClosed,
    ProtocolFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpActiveCallSnapshot {
    pub id: McpActiveCallId,
    pub provenance: McpActiveCallProvenance,
    pub state: McpInvocationState,
    pub dispatch_phase: McpDispatchPhase,
    pub dispatch_certainty: McpDispatchCertainty,
    pub timeout_ms: u64,
    pub elapsed_ms: u64,
    pub deadline_remaining_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invocation_id_round_trips() {
        let id = McpInvocationId::new();
        assert_eq!(id.to_string().parse::<McpInvocationId>().unwrap(), id);
    }

    #[test]
    fn model_call_id_is_bounded_and_safe() {
        assert_eq!(
            "provider-call-1"
                .parse::<McpModelCallId>()
                .unwrap()
                .as_str(),
            "provider-call-1"
        );
        assert!("\n".parse::<McpModelCallId>().is_err());
        assert!("x"
            .repeat(MAX_MODEL_CALL_ID_BYTES + 1)
            .parse::<McpModelCallId>()
            .is_err());
    }

    #[test]
    fn dispatch_tracker_is_monotonic() {
        let tracker = McpDispatchTracker::new();
        tracker.mark_request_queued();
        tracker.mark_dispatching();
        assert_eq!(tracker.phase(), McpDispatchPhase::RequestQueued);
        assert_eq!(
            tracker.certainty(),
            McpDispatchCertainty::PossiblyDispatched
        );
        tracker.mark_response_received();
        assert_eq!(tracker.certainty(), McpDispatchCertainty::ResponseReceived);
    }

    #[test]
    fn active_call_provenance_debug_redacts_server_authored_names() {
        let provenance = McpActiveCallProvenance {
            tool_id: McpToolId {
                server_id: McpServerId::new(),
                raw_name: "RAW_TOOL_NAME_CANARY".to_string(),
            },
            model_name: "MODEL_TOOL_NAME_CANARY".to_string(),
            config_epoch: McpConfigEpoch::new(),
            registry_revision: 7,
            config_digest: "a".repeat(64).parse().unwrap(),
            catalog_generation: 7,
            catalog_digest: "b".repeat(64).parse().unwrap(),
            schema_digest: "c".repeat(64).parse().unwrap(),
        };
        let debug = format!("{provenance:?}");
        assert!(!debug.contains("RAW_TOOL_NAME_CANARY"));
        assert!(!debug.contains("MODEL_TOOL_NAME_CANARY"));
        assert!(debug.contains("<redacted>"));
    }
}
