//! Canonical single-target text-file mutation primitives.
//!
//! This module is deliberately independent from the model-visible `apply_patch` tool. It defines
//! the shared domain model used by both Direct and Staged file changes.

mod bound_io;
mod committer;
mod digest;
mod error;
mod model;
mod observation;
mod planner;
mod policy;
mod run_grant;
mod staged;

pub(crate) use bound_io::BoundParent;
#[cfg(unix)]
pub(crate) use bound_io::BoundReadParent;
pub use committer::{
    FileChangeCommit, FileChangeCommitter, FileChangeDeleteJournal, FileChangeDeleteJournalState,
    FileChangeReconciliation,
};
pub(crate) use digest::valid_digest;
pub use digest::{content_digest, diff_digest, proposal_digest};
pub use error::{
    FileChangeError, FileChangeErrorCategory, FileChangeErrorCode, FileChangeFailure,
    FileChangeRecovery,
};
pub use model::{
    FileChangeContentState, FileChangeDirectBinding, FileChangeEdit, FileChangeOperation,
    FileChangeOutcome, FileChangeProposal, FileChangeReceipt, FileChangeResult, FileChangeStatus,
    FileChangeTransaction, FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
};
pub(crate) use observation::FileObservationOwner;
pub use observation::{
    FileObservation, FileObservationCheckpoint, FileObservationDirectoryIdentity,
    FileObservationIdentity, FileObservationRegistry, FileObservationState,
    FILE_OBSERVATION_CHECKPOINT_SCHEMA_VERSION, FILE_OBSERVATION_TTL_MS,
};
pub use planner::{
    FileChangeBase, FileChangeMutation, FileChangePlan, FileChangePlanRequest, FileChangePlanner,
};
pub use policy::{FileChangePathPolicy, ResolvedFileChangeTarget};
pub use run_grant::*;
pub use staged::{
    allowed_staged_actions, is_unsettled_staged_status, FileChangeMutationReceipt,
    FileChangeStagedAction, FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION,
};

#[cfg(test)]
mod tests;
