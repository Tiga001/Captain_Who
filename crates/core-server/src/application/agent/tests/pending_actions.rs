// Scenario-specific pending-action regression tests. Fixtures stay local unless
// multiple scenarios share the same durable storage/checkpoint setup.
use super::*;

mod approval_ordering;
mod atomic_transitions;
mod builtin_activation;
mod builtin_fixtures;
mod builtin_sensitive;
mod builtin_sensitive_execution;
mod builtin_sensitive_recovery;
mod continuation;
mod continuation_failures;
mod file_change_automatic_recovery;
mod file_change_fixtures;
mod file_change_manual_recovery;
mod file_changes;
mod fixtures;
mod identity_and_store;
mod mcp_approval;
mod mcp_automatic;
mod mcp_fixtures;
mod mcp_invalidation;
mod mcp_recovery;
mod provider_resume;
mod skill_persistence;
mod skill_workers;
mod startup_recovery;
