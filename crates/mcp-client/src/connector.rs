use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use crate::{McpError, McpPeer, McpServerConfig};

pub type BoxMcpFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, McpError>> + Send + 'a>>;

/// Object-safe connector boundary for future stdio and HTTP implementations.
pub trait McpConnector: Send + Sync {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>>;
}
