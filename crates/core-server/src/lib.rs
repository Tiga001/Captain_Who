//! Reusable process-host adapters.
//!
//! The production binary continues to own bootstrap and IPC. This library surface is deliberately
//! narrow so repository-owned integration fixtures can exercise the real MCP adapter without
//! duplicating Host security logic.

pub mod application {
    #[allow(dead_code)]
    pub(crate) mod mcp {
        pub(crate) mod approval_payload_store;
    }
}

pub mod adapters {
    pub mod mcp_runtime;
}

pub mod mcp_trace_safety;

pub(crate) mod pending_action_identity;
