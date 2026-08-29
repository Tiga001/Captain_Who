use super::{FileChangeError, FileChangeErrorCode};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION: u32 = 1;
pub const MAX_STAGED_FILE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeStagedAction {
    Append,
    Edit,
    Commit,
    Status,
    Abort,
}

impl FileChangeStagedAction {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Append => "append",
            Self::Edit => "edit",
            Self::Commit => "commit",
            Self::Status => "status",
            Self::Abort => "abort",
        }
    }
}

pub fn allowed_staged_actions(status: &str) -> Vec<FileChangeStagedAction> {
    match status {
        "drafting" | "ready" => vec![
            FileChangeStagedAction::Append,
            FileChangeStagedAction::Edit,
            FileChangeStagedAction::Commit,
            FileChangeStagedAction::Status,
            FileChangeStagedAction::Abort,
        ],
        _ => vec![FileChangeStagedAction::Status],
    }
}

pub fn is_unsettled_staged_status(status: &str) -> bool {
    matches!(
        status,
        "drafting" | "ready" | "waiting_approval" | "applying"
    ) || !matches!(
        status,
        "applied" | "already_applied" | "rejected" | "conflict" | "failed" | "aborted" | "expired"
    )
}

/// Durable, content-free receipt for one accepted Staged mutation.
///
/// The operation row separately binds the exact payload digest. Keeping text out of this receipt
/// lets idempotent replay and Fork validation remain safe for model/Renderer projections.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeMutationReceipt {
    pub schema_version: u32,
    pub transaction_id: String,
    pub index: u64,
    pub draft_revision: u64,
    pub next_index: u64,
    pub byte_count: u64,
    pub line_count: u64,
    pub allowed_next_actions: Vec<FileChangeStagedAction>,
    pub requires_commit_before_response: bool,
}

impl FileChangeMutationReceipt {
    pub fn validate(&self) -> Result<(), FileChangeError> {
        let allowed = self
            .allowed_next_actions
            .iter()
            .copied()
            .collect::<HashSet<_>>();
        if self.schema_version != FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION
            || self.transaction_id.trim().is_empty()
            || self.next_index != self.index.saturating_add(1)
            || self.draft_revision != self.next_index
            || self.byte_count > MAX_STAGED_FILE_BYTES
            || allowed.len() != self.allowed_next_actions.len()
            || allowed != allowed_staged_actions("drafting").into_iter().collect()
            || !self.requires_commit_before_response
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn receipt() -> FileChangeMutationReceipt {
        FileChangeMutationReceipt {
            schema_version: FILE_CHANGE_MUTATION_RECEIPT_SCHEMA_VERSION,
            transaction_id: "file-change-staged-v1:fixture".to_string(),
            index: 0,
            draft_revision: 1,
            next_index: 1,
            byte_count: 6,
            line_count: 1,
            allowed_next_actions: vec![
                FileChangeStagedAction::Append,
                FileChangeStagedAction::Edit,
                FileChangeStagedAction::Commit,
                FileChangeStagedAction::Status,
                FileChangeStagedAction::Abort,
            ],
            requires_commit_before_response: true,
        }
    }

    #[test]
    fn mutation_receipt_round_trips_and_rejects_unknown_or_missing_fields() {
        let value = serde_json::to_value(receipt()).unwrap();
        let decoded: FileChangeMutationReceipt = serde_json::from_value(value.clone()).unwrap();
        decoded.validate().unwrap();

        let mut extra = value.clone();
        extra["content"] = json!("secret");
        assert!(serde_json::from_value::<FileChangeMutationReceipt>(extra).is_err());
        let mut missing = value;
        missing.as_object_mut().unwrap().remove("draftRevision");
        assert!(serde_json::from_value::<FileChangeMutationReceipt>(missing).is_err());
    }
}
