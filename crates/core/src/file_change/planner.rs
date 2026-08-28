use super::digest::{content_digest, diff_digest, proposal_digest};
use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::model::{
    FileChangeContentState, FileChangeDirectBinding, FileChangeEdit, FileChangeOperation,
};
use crate::content_revision;
use serde::Serialize;
use similar::{ChangeTag, TextDiff};

const MAX_PLANNED_CONTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_EDITS: usize = 128;

#[derive(Debug, Clone, Copy)]
pub enum FileChangeBase<'a> {
    Missing,
    Existing { content: &'a str, revision: &'a str },
}

#[derive(Debug, Clone)]
pub enum FileChangeMutation {
    Complete(String),
    Edits(Vec<FileChangeEdit>),
    Delete,
}

#[derive(Debug, Clone)]
pub struct FileChangePlanRequest<'a> {
    pub operation: FileChangeOperation,
    pub file_path: &'a str,
    pub base: FileChangeBase<'a>,
    pub mutation: FileChangeMutation,
}

#[derive(Debug, Clone)]
pub struct FileChangePlan {
    pub operation: FileChangeOperation,
    pub file_path: String,
    pub base: FileChangeContentState,
    pub target: FileChangeContentState,
    pub base_content: Option<String>,
    pub target_content: Option<String>,
    pub diff: String,
    pub diff_digest: String,
    pub proposal_digest: String,
    pub additions: u64,
    pub deletions: u64,
}

impl FileChangePlan {
    pub fn validate(&self) -> FileChangeResultValue<()> {
        if self.file_path.trim().is_empty()
            || self
                .base_content
                .as_ref()
                .is_some_and(|content| content.contains('\0'))
            || self.target_content.as_ref().is_some_and(|content| {
                content.contains('\0') || content.len() > MAX_PLANNED_CONTENT_BYTES
            })
            || !content_matches_state(&self.base, self.base_content.as_deref())
            || !content_matches_state(&self.target, self.target_content.as_deref())
            || !operation_states_match(self.operation, &self.base, &self.target)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        if self.operation == FileChangeOperation::Update && self.base_content == self.target_content
        {
            return Err(FileChangeError::new(FileChangeErrorCode::NoChange));
        }

        let old = self.base_content.as_deref().unwrap_or_default();
        let new = self.target_content.as_deref().unwrap_or_default();
        let expected_diff = build_diff(&self.file_path, self.operation, old, new);
        let expected_counts = diff_counts(old, new);
        if self.diff != expected_diff
            || self.diff_digest != diff_digest(&expected_diff)
            || (self.additions, self.deletions) != expected_counts
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let material = ProposalDigestMaterial {
            operation: self.operation,
            file_path: &self.file_path,
            base: &self.base,
            target: &self.target,
            diff_digest: &self.diff_digest,
            additions: self.additions,
            deletions: self.deletions,
        };
        let expected_proposal = proposal_digest(&material).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
        })?;
        if self.proposal_digest == expected_proposal {
            Ok(())
        } else {
            Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
        }
    }

    pub fn from_direct_binding(
        binding: &FileChangeDirectBinding,
        diff: String,
    ) -> FileChangeResultValue<Self> {
        binding.validate()?;
        let plan = Self {
            operation: binding.transaction.operation,
            file_path: binding.transaction.file_path.clone(),
            base: binding.transaction.base.clone(),
            target: binding.transaction.target.clone(),
            base_content: binding.base_content.clone(),
            target_content: binding.target_content.clone(),
            diff,
            diff_digest: binding.proposal.diff_digest.clone(),
            proposal_digest: binding.proposal.proposal_digest.clone(),
            additions: binding.proposal.additions,
            deletions: binding.proposal.deletions,
        };
        plan.validate()?;
        Ok(plan)
    }
}

#[derive(Debug, Default)]
pub struct FileChangePlanner;

impl FileChangePlanner {
    pub fn plan(
        &self,
        request: FileChangePlanRequest<'_>,
    ) -> FileChangeResultValue<FileChangePlan> {
        let path = request.file_path.trim();
        if path.is_empty() {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let (base_state, base_content) = validate_base(request.operation, request.base)?;
        let target_content = target_content(request.operation, &base_content, request.mutation)?;
        if target_content
            .as_ref()
            .is_some_and(|content| content.len() > MAX_PLANNED_CONTENT_BYTES)
        {
            return Err(FileChangeError::new(FileChangeErrorCode::ContentTooLarge));
        }
        if request.operation == FileChangeOperation::Update
            && target_content.as_deref() == base_content.as_deref()
        {
            return Err(FileChangeError::new(FileChangeErrorCode::NoChange));
        }
        let target_state = target_content
            .as_ref()
            .map(|content| state_for_content(content))
            .unwrap_or(FileChangeContentState::Missing);
        let old = base_content.as_deref().unwrap_or_default();
        let new = target_content.as_deref().unwrap_or_default();
        let diff = build_diff(path, request.operation, old, new);
        let (additions, deletions) = diff_counts(old, new);
        let diff_digest = diff_digest(&diff);
        let digest_material = ProposalDigestMaterial {
            operation: request.operation,
            file_path: path,
            base: &base_state,
            target: &target_state,
            diff_digest: &diff_digest,
            additions,
            deletions,
        };
        let proposal_digest = proposal_digest(&digest_material).map_err(|error| {
            FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
        })?;
        let plan = FileChangePlan {
            operation: request.operation,
            file_path: path.to_string(),
            base: base_state,
            target: target_state,
            base_content,
            target_content,
            diff,
            diff_digest,
            proposal_digest,
            additions,
            deletions,
        };
        plan.validate()?;
        Ok(plan)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProposalDigestMaterial<'a> {
    operation: FileChangeOperation,
    file_path: &'a str,
    base: &'a FileChangeContentState,
    target: &'a FileChangeContentState,
    diff_digest: &'a str,
    additions: u64,
    deletions: u64,
}

fn validate_base(
    operation: FileChangeOperation,
    base: FileChangeBase<'_>,
) -> FileChangeResultValue<(FileChangeContentState, Option<String>)> {
    match (operation, base) {
        (FileChangeOperation::Create, FileChangeBase::Missing) => {
            Ok((FileChangeContentState::Missing, None))
        }
        (
            FileChangeOperation::Update | FileChangeOperation::Delete,
            FileChangeBase::Existing { content, revision },
        ) => {
            if content_revision(content.as_bytes()) != revision {
                return Err(FileChangeError::new(FileChangeErrorCode::RevisionConflict));
            }
            Ok((state_for_content(content), Some(content.to_string())))
        }
        (FileChangeOperation::Create, FileChangeBase::Existing { .. }) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileExists))
        }
        (FileChangeOperation::Update | FileChangeOperation::Delete, FileChangeBase::Missing) => {
            Err(FileChangeError::new(FileChangeErrorCode::FileMissing))
        }
    }
}

fn target_content(
    operation: FileChangeOperation,
    base: &Option<String>,
    mutation: FileChangeMutation,
) -> FileChangeResultValue<Option<String>> {
    match (operation, mutation) {
        (
            FileChangeOperation::Create | FileChangeOperation::Update,
            FileChangeMutation::Complete(content),
        ) => {
            reject_nul(&content)?;
            Ok(Some(content))
        }
        (FileChangeOperation::Update, FileChangeMutation::Edits(edits)) => {
            if edits.is_empty() || edits.len() > MAX_EDITS {
                return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
            }
            let current = base.as_deref().ok_or_else(|| {
                FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination)
            })?;
            let updated = apply_edits(current, &edits)?;
            reject_nul(&updated)?;
            Ok(Some(updated))
        }
        (FileChangeOperation::Delete, FileChangeMutation::Delete) => Ok(None),
        _ => Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        )),
    }
}

fn apply_edits(current: &str, edits: &[FileChangeEdit]) -> FileChangeResultValue<String> {
    let mut updated = current.to_string();
    for edit in edits {
        updated = match edit {
            FileChangeEdit::Replace {
                old_text,
                new_text,
                replace_all,
            } => {
                if old_text.is_empty() {
                    return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
                }
                let matches = updated.match_indices(old_text).count();
                if matches == 0 {
                    return Err(FileChangeError::new(FileChangeErrorCode::MatchNotFound));
                }
                if *replace_all {
                    updated.replace(old_text, new_text)
                } else if matches == 1 {
                    updated.replacen(old_text, new_text, 1)
                } else {
                    return Err(FileChangeError::new(FileChangeErrorCode::AmbiguousMatch));
                }
            }
            FileChangeEdit::InsertBefore { anchor, text }
            | FileChangeEdit::InsertAfter { anchor, text } => {
                if anchor.is_empty() {
                    return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
                }
                let matches = updated.match_indices(anchor).count();
                if matches == 0 {
                    return Err(FileChangeError::new(FileChangeErrorCode::MatchNotFound));
                }
                if matches > 1 {
                    return Err(FileChangeError::new(FileChangeErrorCode::AmbiguousMatch));
                }
                if matches!(edit, FileChangeEdit::InsertBefore { .. }) {
                    updated.replacen(anchor, &format!("{text}{anchor}"), 1)
                } else {
                    updated.replacen(anchor, &format!("{anchor}{text}"), 1)
                }
            }
            FileChangeEdit::Append { text } => format!("{updated}{text}"),
            FileChangeEdit::Prepend { text } => format!("{text}{updated}"),
        };
    }
    Ok(updated)
}

fn state_for_content(content: &str) -> FileChangeContentState {
    FileChangeContentState::Present {
        revision: content_revision(content.as_bytes()),
        digest: content_digest(content.as_bytes()),
        byte_count: content.len() as u64,
    }
}

fn content_matches_state(state: &FileChangeContentState, content: Option<&str>) -> bool {
    match (state, content) {
        (FileChangeContentState::Missing, None) => true,
        (FileChangeContentState::Present { .. }, Some(content)) => {
            state == &state_for_content(content)
        }
        (FileChangeContentState::Missing, Some(_))
        | (FileChangeContentState::Present { .. }, None) => false,
    }
}

fn operation_states_match(
    operation: FileChangeOperation,
    base: &FileChangeContentState,
    target: &FileChangeContentState,
) -> bool {
    matches!(
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
    )
}

fn reject_nul(content: &str) -> FileChangeResultValue<()> {
    if content.contains('\0') {
        Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments))
    } else {
        Ok(())
    }
}

fn build_diff(path: &str, operation: FileChangeOperation, old: &str, new: &str) -> String {
    let old_header = if operation == FileChangeOperation::Create {
        "/dev/null".to_string()
    } else {
        format!("a/{path}")
    };
    let new_header = if operation == FileChangeOperation::Delete {
        "/dev/null".to_string()
    } else {
        format!("b/{path}")
    };
    TextDiff::from_lines(old, new)
        .unified_diff()
        .header(&old_header, &new_header)
        .to_string()
}

fn diff_counts(old: &str, new: &str) -> (u64, u64) {
    TextDiff::from_lines(old, new).iter_all_changes().fold(
        (0_u64, 0_u64),
        |(additions, deletions), change| match change.tag() {
            ChangeTag::Insert => (additions.saturating_add(1), deletions),
            ChangeTag::Delete => (additions, deletions.saturating_add(1)),
            ChangeTag::Equal => (additions, deletions),
        },
    )
}
