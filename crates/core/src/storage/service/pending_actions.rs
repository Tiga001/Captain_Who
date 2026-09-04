include!("pending_actions/identity.rs");
include!("pending_actions/file_effects.rs");

// Each shard extends the same StorageService; no transaction, CAS, lease, or effect boundary moves
// out of its original method body.
include!("pending_actions/startup_reconciliation.rs");
include!("pending_actions/store_and_recover.rs");
include!("pending_actions/transitions.rs");
include!("pending_actions/trace_commit.rs");
include!("pending_actions/settlement_validation.rs");
include!("pending_actions/tests.rs");
