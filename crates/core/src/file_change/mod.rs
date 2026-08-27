//! Canonical single-target text-file mutation primitives.
//!
//! This module is deliberately independent from the model-visible `apply_patch` and `write_file`
//! adapters. During the staged writer consolidation, both adapters can be compared against this
//! domain model without changing the production tool registry or durable wire formats.

mod committer;
mod digest;
mod error;
mod model;
mod planner;
mod policy;

pub use committer::{
    FileChangeCommit, FileChangeCommitter, FileChangeDeleteJournal, FileChangeDeleteJournalState,
    FileChangeReconciliation,
};
pub use digest::{content_digest, diff_digest, proposal_digest};
pub use error::{
    FileChangeError, FileChangeErrorCategory, FileChangeErrorCode, FileChangeFailure,
    FileChangeRecovery,
};
pub use model::{
    FileChangeContentState, FileChangeEdit, FileChangeOperation, FileChangeOutcome,
    FileChangeProposal, FileChangeReceipt, FileChangeResult, FileChangeStatus,
    FileChangeTransaction, FILE_CHANGE_SCHEMA_VERSION,
};
pub use planner::{
    FileChangeBase, FileChangeMutation, FileChangePlan, FileChangePlanRequest, FileChangePlanner,
};
pub use policy::{FileChangePathPolicy, ResolvedFileChangeTarget};

#[cfg(test)]
mod tests;
