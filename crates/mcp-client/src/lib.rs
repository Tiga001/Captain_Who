//! Stable MyCopilot-facing MCP client boundary.
//!
//! The public API in this crate deliberately does not expose SDK-specific
//! types. `rmcp` is an implementation detail confined to the connector and
//! mapping modules.
//!
//! Hosts must install a non-overridable target filter for `rmcp` before
//! handling sensitive tool arguments or results. The SDK can emit
//! peer-controlled protocol content at multiple tracing levels, outside this
//! crate's redacted domain-type `Debug` implementations.
//!
//! ```compile_fail
//! use mycopilot_mcp_client::rmcp;
//! ```

mod catalog;
mod config;
mod connection;
mod connector;
mod digest;
mod error;
mod event;
mod invocation;
mod limits;
mod manager;
mod registry;
mod transports;
mod types;

pub use catalog::{
    McpCatalogCompleteness, McpCatalogDiagnostic, McpCatalogDiagnosticKind, McpCatalogIssue,
    McpCatalogLimits, McpCatalogPolicy, McpCatalogSnapshot, McpCatalogTool, McpCatalogToolCall,
    McpCatalogToolCallIdentity, McpToolId,
};
pub use config::{
    McpApprovalMode, McpEnvBinding, McpServerConfig, McpServerScope, McpStdioConfig,
    McpTransportConfig, McpTrustLevel,
};
pub use connection::{McpClientHandle, McpPeer};
pub use connector::{BoxMcpFuture, McpConnector};
pub use digest::{config_digest, McpCatalogDigest, McpConfigDigest, McpSchemaDigest};
pub use error::{McpError, McpErrorKind};
pub(crate) use event::McpPeerSignalPublisher;
pub use event::{
    McpConnectionEvent, McpEvent, McpEventSink, McpPeerNotificationState, McpPeerSignalReceiver,
    McpPeerSignalSnapshot, McpSafeError, McpServerState, McpStderrSnapshot, NoopMcpEventSink,
};
pub use invocation::{
    McpActiveCallId, McpActiveCallProvenance, McpActiveCallSnapshot, McpDispatchCertainty,
    McpDispatchPhase, McpDispatchTracker, McpInvocationId, McpInvocationState, McpModelCallId,
    McpOutcomeUnknownReason,
};
pub use limits::McpSecurityLimits;
pub use manager::{
    McpBatchOperationResult, McpConnectionManager, McpManagerPolicy, McpServerStatus,
    McpShutdownReport,
};
pub use registry::{
    allocate_model_namespace, InMemoryMcpRegistry, McpRegistry, McpRegistryChange,
    McpRegistryChangeKind, McpRegistryEntry, McpRegistryMutation, McpRegistrySubscription,
    McpRegistrySubscriptionError,
};
pub use transports::stdio::{McpStdioConnector, McpStdioPolicy};
pub use types::{
    McpCacheScope, McpCapabilitySnapshot, McpConfigEpoch, McpConnectionState, McpContentBlock,
    McpEmbeddedResource, McpImplementationInfo, McpLifecycleKind, McpModelNamespace,
    McpProtocolSnapshot, McpResourceLink, McpServerId, McpToolAnnotations, McpToolCall,
    McpToolDescriptor, McpToolPage, McpToolResult, MCP_MODEL_NAMESPACE_MAX_BYTES,
};

/// Re-export the cancellation primitive as part of our stable API boundary.
///
/// This is intentionally `tokio-util`, not MyCopilot Agent Runtime's
/// cancellation type. A later adapter can bridge the two without coupling this
/// protocol crate to the runtime.
pub use tokio_util::sync::CancellationToken as McpCancellationToken;
