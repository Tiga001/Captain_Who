use super::digest::valid_digest;
use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use serde::{Deserialize, Serialize};

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
