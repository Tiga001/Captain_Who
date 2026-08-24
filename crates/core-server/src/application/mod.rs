pub(crate) mod agent;
#[allow(dead_code)]
// Internal collaboration boundary; Host wiring is deliberately deferred to round 4.
pub(crate) mod agent_collaboration;
#[allow(dead_code)]
// Persistent Agent scheduling is internal until the round-4 Host/Harness wiring.
pub(crate) mod agent_dispatcher;
pub(crate) mod agent_harness;
pub(crate) mod agent_support;
#[allow(dead_code)]
// Reliable collaboration wait kernel; its model Tool adapter belongs to round 4.
pub(crate) mod agent_wait;
pub(crate) mod automation;
pub(crate) mod collaboration_authorization;
pub(crate) mod mcp;
pub(crate) mod notification;
