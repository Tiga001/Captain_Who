//! Stable MyCopilot-facing MCP client boundary.
//!
//! The public API in this crate deliberately does not expose SDK-specific
//! types. `rmcp` is an implementation detail confined to the connector and
//! mapping modules.
//!
//! Hosts must disable the `rmcp` tracing target before handling sensitive tool
//! arguments or results. The SDK can emit peer-controlled protocol content
//! even above debug level, outside this crate's redacted domain-type `Debug`
//! implementations.
//!
//! ```compile_fail
//! use mycopilot_mcp_client::rmcp;
//! ```

mod config;
mod connection;
mod connector;
mod error;
mod event;
mod transports;
mod types;

pub use config::{
    McpEnvBinding, McpServerConfig, McpServerScope, McpStdioConfig, McpTransportConfig,
    McpTrustLevel,
};
pub use connection::{McpClientHandle, McpPeer};
pub use connector::{BoxMcpFuture, McpConnector};
pub use error::{McpError, McpErrorKind};
pub use event::{McpConnectionEvent, McpStderrSnapshot};
pub use transports::stdio::{McpStdioConnector, McpStdioPolicy};
pub use types::{
    McpCacheScope, McpCapabilitySnapshot, McpConnectionState, McpContentBlock, McpEmbeddedResource,
    McpImplementationInfo, McpLifecycleKind, McpProtocolSnapshot, McpResourceLink, McpServerId,
    McpToolAnnotations, McpToolCall, McpToolDescriptor, McpToolPage, McpToolResult,
};

/// Re-export the cancellation primitive as part of our stable API boundary.
///
/// This is intentionally `tokio-util`, not MyCopilot Agent Runtime's
/// cancellation type. A later adapter can bridge the two without coupling this
/// protocol crate to the runtime.
pub use tokio_util::sync::CancellationToken as McpCancellationToken;
