#[allow(dead_code)]
pub(crate) mod approval_payload_store;
#[allow(dead_code)] // Wired into the production Manager composition by the Round 5 host bootstrap.
pub(crate) mod authorized_stdio_connector;
pub(crate) mod browser_risk;
pub(crate) mod builtin_capability_policy;
pub(crate) mod builtin_capability_runtime;
pub(crate) mod managed_playwright_bridge;
pub(crate) mod management;
pub(crate) mod playwright_manifest;
pub(crate) mod registry_event_sink;
#[allow(dead_code)]
pub(crate) mod sqlite_envelope_repository;
pub(crate) mod sqlite_registry;
