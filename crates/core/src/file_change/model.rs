use super::committer::{FileChangeDeleteJournal, FileChangeDeleteJournalState};
use super::digest::content_digest;
use super::digest::valid_digest;
use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::observation::{FileObservationCheckpoint, FileObservationState};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const FILE_CHANGE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeOperation {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStatus {
    Drafting,
    Ready,
    WaitingApproval,
    Applying,
    Applied,
    AlreadyApplied,
    Rejected,
    Conflict,
    Failed,
    OutcomeUnknown,
    Aborted,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeOutcome {
    DefinitelyNotExecuted,
    Applied,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FileChangeEdit {
    Replace {
        old_text: String,
        new_text: String,
        replace_all: bool,
    },
    InsertBefore {
        anchor: String,
        text: String,
    },
    InsertAfter {
        anchor: String,
        text: String,
    },
    Append {
        text: String,
    },
    Prepend {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FileChangeContentState {
    Missing,
    Present {
        revision: String,
        digest: String,
        byte_count: u64,
    },
}

impl FileChangeContentState {
    pub fn validate(&self) -> FileChangeResultValue<()> {
        match self {
            Self::Missing => Ok(()),
            Self::Present {
                revision, digest, ..
            } if !revision.trim().is_empty() && valid_digest(digest) => Ok(()),
            Self::Present { .. } => {
                Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
            }
        }
    }

    pub fn digest(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Present { digest, .. } => Some(digest),
        }
    }

    pub fn revision(&self) -> Option<&str> {
        match self {
            Self::Missing => None,
            Self::Present { revision, .. } => Some(revision),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeTransaction {
    pub schema_version: u32,
    pub id: String,
    pub operation: FileChangeOperation,
    pub file_path: String,
    pub status: FileChangeStatus,
    pub outcome: FileChangeOutcome,
    pub base: FileChangeContentState,
    pub target: FileChangeContentState,
    pub proposal_digest: String,
    pub created_at: u64,
    pub updated_at: u64,
}

/// Host-private execution material for one Direct file change.
///
/// The public approval projection removes this object before it crosses into the Renderer. The
/// durable pending-action record retains it so manual approval and restart recovery can rebuild
/// exactly the same authoritative plan without executing the presentation diff.
#[derive(Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeDirectBinding {
    pub schema_version: u32,
    pub transaction: FileChangeTransaction,
    pub proposal: FileChangeProposal,
    pub observation_id: String,
    pub observation: FileObservationCheckpoint,
    pub source_tool_name: String,
    pub source_call_id: String,
    pub source_args_digest: String,
    /// Required-nullable reference to the canonical persistent Staged transaction. Direct
    /// `action=apply` carries `null`; Staged commit and the temporary `write_file` adapter carry
    /// the exact transaction id so approval settlement cannot target another draft.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub staged_transaction_id: Option<String>,
    pub conversation_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub project_id: Option<String>,
    pub run_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub staged_transaction_revision: Option<u64>,
    pub canonical_target: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub base_content: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub target_content: Option<String>,
    /// Required-nullable durable delete recovery state. Direct delete freezes a deterministic
    /// prepared journal before approval; create and update must carry `null`.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub delete_journal: Option<FileChangeDeleteJournal>,
    /// Required-nullable durable commit evidence. A proposal awaiting approval must carry
    /// `null`; after publication this receipt must exactly match the frozen transaction.
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub receipt: Option<FileChangeReceipt>,
    pub permission_revision: String,
    pub tool_set_revision: String,
    pub provider_wire_revision: String,
}

impl std::fmt::Debug for FileChangeDirectBinding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FileChangeDirectBinding([REDACTED])")
    }
}

impl FileChangeDirectBinding {
    pub fn validate(&self) -> FileChangeResultValue<()> {
        if self.schema_version != FILE_CHANGE_SCHEMA_VERSION
            || self.observation_id.trim().is_empty()
            || !matches!(self.source_tool_name.as_str(), "apply_patch" | "write_file")
            || self.source_call_id.trim().is_empty()
            || !valid_digest(&self.source_args_digest)
            || self.conversation_id.trim().is_empty()
            || self
                .project_id
                .as_deref()
                .is_some_and(|id| id.trim().is_empty())
            || self.run_id.trim().is_empty()
            || self.permission_revision.trim().is_empty()
            || self.tool_set_revision.trim().is_empty()
            || self.provider_wire_revision.trim().is_empty()
            || !Path::new(&self.canonical_target).is_absolute()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        self.transaction.validate()?;
        self.proposal.validate()?;
        self.observation.validate_frozen_binding(
            &self.conversation_id,
            &self.run_id,
            Path::new(&self.canonical_target),
        )?;
        let observation_matches_base = self.observation_id == self.observation.observation_id
            && match (self.transaction.operation, &self.observation.state) {
                (FileChangeOperation::Create, FileObservationState::Missing) => {
                    matches!(self.transaction.base, FileChangeContentState::Missing)
                }
                (
                    FileChangeOperation::Update | FileChangeOperation::Delete,
                    FileObservationState::Existing { revision, .. },
                ) => self.transaction.base.revision() == Some(revision.as_str()),
                _ => false,
            };
        let delete_journal_is_valid =
            match (self.transaction.operation, self.delete_journal.as_ref()) {
                (FileChangeOperation::Create | FileChangeOperation::Update, None) => true,
                (FileChangeOperation::Delete, Some(journal)) => {
                    journal.validate().is_ok()
                        && journal.transaction_id == self.transaction.id
                        && journal.target_path == self.canonical_target
                        && journal.proposal_digest == self.proposal.proposal_digest
                        && Some(journal.base_digest.as_str()) == self.transaction.base.digest()
                }
                _ => false,
            };
        let staged_reference_is_valid = match (
            self.staged_transaction_id.as_deref(),
            self.staged_transaction_revision,
        ) {
            (None, None) => self.source_tool_name == "apply_patch",
            (Some(transaction_id), Some(_)) => {
                self.transaction.operation != FileChangeOperation::Delete
                    && transaction_id == self.transaction.id
            }
            _ => false,
        };
        if self.transaction.id != self.proposal.transaction_id
            || self.proposal.id != self.source_call_id
            || self.transaction.operation != self.proposal.operation
            || self.transaction.file_path != self.proposal.file_path
            || self.transaction.base != self.proposal.base
            || self.transaction.target != self.proposal.target
            || self.transaction.proposal_digest != self.proposal.proposal_digest
            || !content_matches_binding_state(&self.transaction.base, self.base_content.as_deref())
            || !content_matches_binding_state(
                &self.transaction.target,
                self.target_content.as_deref(),
            )
            || !observation_matches_base
            || !delete_journal_is_valid
            || !staged_reference_is_valid
        {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let prepared = self.transaction.status == FileChangeStatus::WaitingApproval
            && self.transaction.outcome == FileChangeOutcome::DefinitelyNotExecuted
            && self.receipt.is_none()
            && self
                .delete_journal
                .as_ref()
                .is_none_or(|journal| journal.state == FileChangeDeleteJournalState::Prepared);
        let committed = matches!(
            self.transaction.status,
            FileChangeStatus::Applied | FileChangeStatus::AlreadyApplied
        ) && self.transaction.outcome == FileChangeOutcome::Applied
            && self.receipt.as_ref().is_some_and(|receipt| {
                receipt.validate().is_ok()
                    && receipt.schema_version == self.schema_version
                    && receipt.transaction_id == self.transaction.id
                    && receipt.operation == self.transaction.operation
                    && receipt.file_path == self.transaction.file_path
                    && receipt.outcome == self.transaction.outcome
                    && receipt.base == self.transaction.base
                    && receipt.target == self.transaction.target
                    && receipt.proposal_digest == self.transaction.proposal_digest
                    && receipt.committed_at == self.transaction.updated_at
            })
            && self.delete_journal.as_ref().is_none_or(|journal| {
                matches!(
                    journal.state,
                    FileChangeDeleteJournalState::Tombstoned
                        | FileChangeDeleteJournalState::Finalized
                ) && journal.updated_at >= self.transaction.updated_at
            });
        if prepared || committed {
            Ok(())
        } else {
            Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ))
        }
    }

    pub fn is_prepared(&self) -> bool {
        self.validate().is_ok()
            && self.transaction.status == FileChangeStatus::WaitingApproval
            && self.receipt.is_none()
    }

    pub fn with_commit(
        &self,
        commit: &super::committer::FileChangeCommit,
    ) -> FileChangeResultValue<Self> {
        self.validate()?;
        if !self.is_prepared()
            || commit.receipt.transaction_id != self.transaction.id
            || commit.receipt.operation != self.transaction.operation
            || commit.receipt.file_path != self.transaction.file_path
            || commit.receipt.base != self.transaction.base
            || commit.receipt.target != self.transaction.target
            || commit.receipt.proposal_digest != self.transaction.proposal_digest
            || commit.receipt.outcome != FileChangeOutcome::Applied
            || !matches!(
                commit.status,
                FileChangeStatus::Applied | FileChangeStatus::AlreadyApplied
            )
        {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let mut committed = self.clone();
        committed.transaction.status = commit.status;
        committed.transaction.outcome = FileChangeOutcome::Applied;
        committed.transaction.updated_at = commit.receipt.committed_at;
        committed.delete_journal = commit.delete_journal.clone();
        committed.receipt = Some(commit.receipt.clone());
        committed.validate()?;
        Ok(committed)
    }

    pub fn is_commit_successor_of(&self, prepared: &Self) -> bool {
        if !prepared.is_prepared() || self.validate().is_err() || self.receipt.is_none() {
            return false;
        }
        let mut expected = prepared.clone();
        expected.transaction.status = self.transaction.status;
        expected.transaction.outcome = self.transaction.outcome;
        expected.transaction.updated_at = self.transaction.updated_at;
        expected.delete_journal = self.delete_journal.clone();
        expected.receipt = self.receipt.clone();
        self == &expected
    }

    /// Advances a durably committed delete from its recoverable tombstone state to the exact
    /// finalized journal returned by the committer. No other execution material may change.
    pub fn with_finalized_delete_journal(
        &self,
        finalized_journal: FileChangeDeleteJournal,
    ) -> FileChangeResultValue<Self> {
        self.validate()?;
        let current_journal = self
            .delete_journal
            .as_ref()
            .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))?;
        let mut expected_journal = current_journal.clone();
        expected_journal.state = FileChangeDeleteJournalState::Finalized;
        expected_journal.updated_at = finalized_journal.updated_at;
        if self.transaction.operation != FileChangeOperation::Delete
            || self.receipt.is_none()
            || current_journal.state != FileChangeDeleteJournalState::Tombstoned
            || finalized_journal.state != FileChangeDeleteJournalState::Finalized
            || finalized_journal.updated_at < current_journal.updated_at
            || finalized_journal != expected_journal
        {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let mut finalized = self.clone();
        finalized.delete_journal = Some(finalized_journal);
        finalized.validate()?;
        Ok(finalized)
    }

    /// Returns whether `self` is the exact tombstone-finalization successor of `committed`.
    pub fn is_delete_finalization_successor_of(&self, committed: &Self) -> bool {
        let Some(finalized_journal) = self.delete_journal.clone() else {
            return false;
        };
        committed
            .with_finalized_delete_journal(finalized_journal)
            .is_ok_and(|expected| self == &expected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeProposal {
    pub schema_version: u32,
    pub id: String,
    pub transaction_id: String,
    pub operation: FileChangeOperation,
    pub file_path: String,
    pub base: FileChangeContentState,
    pub target: FileChangeContentState,
    pub diff_digest: String,
    pub proposal_digest: String,
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeResult {
    pub schema_version: u32,
    pub transaction_id: String,
    pub operation: FileChangeOperation,
    pub file_path: String,
    pub status: FileChangeStatus,
    pub outcome: FileChangeOutcome,
    pub base: FileChangeContentState,
    pub target: FileChangeContentState,
    pub proposal_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeReceipt {
    pub schema_version: u32,
    pub transaction_id: String,
    pub operation: FileChangeOperation,
    pub file_path: String,
    pub outcome: FileChangeOutcome,
    pub base: FileChangeContentState,
    pub target: FileChangeContentState,
    pub proposal_digest: String,
    pub committed_at: u64,
}

macro_rules! impl_validate {
    ($type:ty, $body:expr) => {
        impl $type {
            pub fn validate(&self) -> FileChangeResultValue<()> {
                if self.schema_version != FILE_CHANGE_SCHEMA_VERSION {
                    return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
                }
                if self.file_path.trim().is_empty() || !valid_digest(&self.proposal_digest) {
                    return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
                }
                self.base.validate()?;
                self.target.validate()?;
                ($body)(self)
            }
        }
    };
}

impl_validate!(FileChangeTransaction, |value: &FileChangeTransaction| {
    if value.id.trim().is_empty() || value.updated_at < value.created_at {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        validate_operation_states(value.operation, &value.base, &value.target)?;
        validate_status_outcome(value.status, value.outcome)
    }
});

impl_validate!(FileChangeProposal, |value: &FileChangeProposal| {
    if value.id.trim().is_empty()
        || value.transaction_id.trim().is_empty()
        || !valid_digest(&value.diff_digest)
    {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        validate_operation_states(value.operation, &value.base, &value.target)
    }
});

impl_validate!(FileChangeResult, |value: &FileChangeResult| {
    if value.transaction_id.trim().is_empty() {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        validate_operation_states(value.operation, &value.base, &value.target)?;
        validate_status_outcome(value.status, value.outcome)
    }
});

impl_validate!(FileChangeReceipt, |value: &FileChangeReceipt| {
    if value.transaction_id.trim().is_empty() {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        validate_operation_states(value.operation, &value.base, &value.target)?;
        if value.outcome == FileChangeOutcome::DefinitelyNotExecuted {
            Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        } else {
            Ok(())
        }
    }
});

fn validate_operation_states(
    operation: FileChangeOperation,
    base: &FileChangeContentState,
    target: &FileChangeContentState,
) -> FileChangeResultValue<()> {
    let valid = matches!(
        (operation, base, target),
        (
            FileChangeOperation::Create,
            FileChangeContentState::Missing,
            FileChangeContentState::Present { .. }
        ) | (
            FileChangeOperation::Update,
            FileChangeContentState::Present { .. },
            FileChangeContentState::Present { .. }
        ) | (
            FileChangeOperation::Delete,
            FileChangeContentState::Present { .. },
            FileChangeContentState::Missing
        )
    );
    if valid {
        Ok(())
    } else {
        Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ))
    }
}

fn validate_status_outcome(
    status: FileChangeStatus,
    outcome: FileChangeOutcome,
) -> FileChangeResultValue<()> {
    let valid = match status {
        FileChangeStatus::Applied | FileChangeStatus::AlreadyApplied => {
            outcome == FileChangeOutcome::Applied
        }
        FileChangeStatus::Applying | FileChangeStatus::OutcomeUnknown => {
            outcome == FileChangeOutcome::OutcomeUnknown
        }
        FileChangeStatus::Drafting
        | FileChangeStatus::Ready
        | FileChangeStatus::WaitingApproval
        | FileChangeStatus::Rejected
        | FileChangeStatus::Conflict
        | FileChangeStatus::Failed
        | FileChangeStatus::Aborted
        | FileChangeStatus::Expired => outcome == FileChangeOutcome::DefinitelyNotExecuted,
    };
    if valid {
        Ok(())
    } else {
        Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ))
    }
}

fn content_matches_binding_state(state: &FileChangeContentState, content: Option<&str>) -> bool {
    match (state, content) {
        (FileChangeContentState::Missing, None) => true,
        (
            FileChangeContentState::Present {
                revision,
                digest,
                byte_count,
            },
            Some(content),
        ) => {
            crate::content_revision(content.as_bytes()) == *revision
                && content_digest(content.as_bytes()) == *digest
                && content.len() as u64 == *byte_count
        }
        _ => false,
    }
}

fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}
