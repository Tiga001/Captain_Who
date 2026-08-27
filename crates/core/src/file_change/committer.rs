use super::digest::{content_digest, valid_digest};
use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::model::{
    FileChangeContentState, FileChangeOperation, FileChangeOutcome, FileChangeReceipt,
    FileChangeStatus, FILE_CHANGE_SCHEMA_VERSION,
};
use super::planner::FileChangePlan;
use super::policy::ResolvedFileChangeTarget;
use crate::content_revision;
use crate::durable_fs::{atomic_rename_noreplace, atomic_replace, sync_directory};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions, Permissions};
use std::io::{self, Read, Write};
use std::path::Path;
use tempfile::NamedTempFile;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeDeleteJournalState {
    Prepared,
    Tombstoned,
    Finalized,
    RolledBack,
}

/// Durable evidence for a recoverable delete.
///
/// The caller must persist `Prepared` before calling `commit`, persist `Tombstoned` together with
/// the receipt, and only then call `finalize_delete`. This module deliberately does not choose a
/// storage backend, so the model remains independent from the public tools and UI.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChangeDeleteJournal {
    pub schema_version: u32,
    pub journal_id: String,
    pub transaction_id: String,
    pub target_path: String,
    pub tombstone_path: String,
    pub base_digest: String,
    pub proposal_digest: String,
    pub state: FileChangeDeleteJournalState,
    pub created_at: u64,
    pub updated_at: u64,
}

impl FileChangeDeleteJournal {
    pub fn validate(&self) -> FileChangeResultValue<()> {
        if self.schema_version != FILE_CHANGE_SCHEMA_VERSION
            || self.journal_id.trim().is_empty()
            || self.transaction_id.trim().is_empty()
            || self.target_path.trim().is_empty()
            || self.tombstone_path.trim().is_empty()
            || !valid_digest(&self.base_digest)
            || !valid_digest(&self.proposal_digest)
            || self.updated_at < self.created_at
        {
            return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
        }
        let target = Path::new(&self.target_path);
        let tombstone = Path::new(&self.tombstone_path);
        let suffix = delete_journal_suffix(
            &self.transaction_id,
            &self.target_path,
            &self.base_digest,
            &self.proposal_digest,
        );
        let expected_id = format!("file-change-delete-{suffix}");
        let expected_tombstone = target
            .parent()
            .map(|parent| parent.join(format!(".{expected_id}.tombstone")));
        if !target.is_absolute()
            || !tombstone.is_absolute()
            || target.parent() != tombstone.parent()
            || target == tombstone
            || self.journal_id != expected_id
            || expected_tombstone.as_deref() != Some(tombstone)
        {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        Ok(())
    }

    fn tombstone(&self) -> &Path {
        Path::new(&self.tombstone_path)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChangeCommit {
    pub status: FileChangeStatus,
    pub receipt: FileChangeReceipt,
    pub delete_journal: Option<FileChangeDeleteJournal>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileChangeReconciliation {
    DefinitelyNotExecuted,
    AlreadyApplied,
    OutcomeUnknown,
}

#[derive(Debug, Default)]
pub struct FileChangeCommitter;

impl FileChangeCommitter {
    pub fn prepare_delete(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        now: u64,
    ) -> FileChangeResultValue<FileChangeDeleteJournal> {
        validate_transaction_and_plan(transaction_id, target, plan)?;
        if plan.operation != FileChangeOperation::Delete {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        target.revalidate()?;
        let base_digest = required_digest(&plan.base)?;
        let target_path = target.absolute_path().to_string_lossy().to_string();
        let suffix = delete_journal_suffix(
            transaction_id,
            &target_path,
            base_digest,
            &plan.proposal_digest,
        );
        let tombstone = target
            .parent()
            .join(format!(".file-change-delete-{suffix}.tombstone"));
        match fs::symlink_metadata(&tombstone) {
            Ok(_) => return Err(FileChangeError::new(FileChangeErrorCode::Conflict)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error, false)),
        }
        let journal = FileChangeDeleteJournal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            journal_id: format!("file-change-delete-{suffix}"),
            transaction_id: transaction_id.to_string(),
            target_path,
            tombstone_path: tombstone.to_string_lossy().to_string(),
            base_digest: base_digest.to_string(),
            proposal_digest: plan.proposal_digest.clone(),
            state: FileChangeDeleteJournalState::Prepared,
            created_at: now,
            updated_at: now,
        };
        journal.validate()?;
        Ok(journal)
    }

    pub fn commit(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        delete_journal: Option<&mut FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeCommit> {
        validate_transaction_and_plan(transaction_id, target, plan)?;
        match plan.operation {
            FileChangeOperation::Create => {
                if delete_journal.is_some() {
                    return Err(FileChangeError::new(
                        FileChangeErrorCode::IllegalFieldCombination,
                    ));
                }
                self.commit_create(transaction_id, target, plan, committed_at)
            }
            FileChangeOperation::Update => {
                if delete_journal.is_some() {
                    return Err(FileChangeError::new(
                        FileChangeErrorCode::IllegalFieldCombination,
                    ));
                }
                self.commit_update(transaction_id, target, plan, committed_at)
            }
            FileChangeOperation::Delete => {
                let journal = delete_journal.ok_or_else(|| {
                    FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination)
                })?;
                self.commit_delete(transaction_id, target, plan, committed_at, journal)
            }
        }
    }

    pub fn finalize_delete(
        &self,
        journal: &mut FileChangeDeleteJournal,
        now: u64,
    ) -> FileChangeResultValue<()> {
        journal.validate()?;
        if journal.state == FileChangeDeleteJournalState::Finalized {
            return Ok(());
        }
        if journal.state != FileChangeDeleteJournalState::Tombstoned {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        if read_optional(journal.tombstone())?.is_none() {
            if read_optional(Path::new(&journal.target_path))?.is_some() {
                return Err(FileChangeError::new(FileChangeErrorCode::OutcomeUnknown));
            }
            sync_after_publication(journal.tombstone().parent())?;
            journal.state = FileChangeDeleteJournalState::Finalized;
            journal.updated_at = now;
            return Ok(());
        }
        ensure_path_digest(journal.tombstone(), &journal.base_digest)?;
        fs::remove_file(journal.tombstone()).map_err(|error| io_error(error, false))?;
        sync_after_publication(journal.tombstone().parent())?;
        journal.state = FileChangeDeleteJournalState::Finalized;
        journal.updated_at = now;
        Ok(())
    }

    pub fn rollback_delete(
        &self,
        journal: &mut FileChangeDeleteJournal,
        now: u64,
    ) -> FileChangeResultValue<()> {
        journal.validate()?;
        if journal.state == FileChangeDeleteJournalState::RolledBack {
            return Ok(());
        }
        if journal.state != FileChangeDeleteJournalState::Tombstoned {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let target = Path::new(&journal.target_path);
        if let Some(current) = read_optional(target)? {
            if state_digest_matches(&journal.base_digest, &current.bytes)
                && read_optional(journal.tombstone())?.is_none()
            {
                journal.state = FileChangeDeleteJournalState::RolledBack;
                journal.updated_at = now;
                return Ok(());
            }
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        ensure_path_digest(journal.tombstone(), &journal.base_digest)?;
        atomic_rename_noreplace(journal.tombstone(), target).map_err(rename_conflict_error)?;
        sync_after_publication(target.parent())?;
        journal.state = FileChangeDeleteJournalState::RolledBack;
        journal.updated_at = now;
        Ok(())
    }

    pub fn reconcile(
        &self,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        delete_journal: Option<&FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeReconciliation> {
        let current = read_optional(target.absolute_path())?;
        match plan.operation {
            FileChangeOperation::Create => match current {
                None => Ok(FileChangeReconciliation::DefinitelyNotExecuted),
                Some(current) if state_matches(&plan.target, &current.bytes) => {
                    Ok(FileChangeReconciliation::AlreadyApplied)
                }
                Some(_) => Ok(FileChangeReconciliation::OutcomeUnknown),
            },
            FileChangeOperation::Update => match current {
                Some(current) if state_matches(&plan.base, &current.bytes) => {
                    Ok(FileChangeReconciliation::DefinitelyNotExecuted)
                }
                Some(current) if state_matches(&plan.target, &current.bytes) => {
                    Ok(FileChangeReconciliation::AlreadyApplied)
                }
                Some(_) | None => Ok(FileChangeReconciliation::OutcomeUnknown),
            },
            FileChangeOperation::Delete => match current {
                Some(current) if state_matches(&plan.base, &current.bytes) => {
                    Ok(FileChangeReconciliation::DefinitelyNotExecuted)
                }
                Some(_) => Ok(FileChangeReconciliation::OutcomeUnknown),
                None => self.reconcile_deleted(plan, delete_journal),
            },
        }
    }

    fn commit_create(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
    ) -> FileChangeResultValue<FileChangeCommit> {
        target.revalidate()?;
        match fs::symlink_metadata(target.absolute_path()) {
            Ok(_) => return Err(FileChangeError::new(FileChangeErrorCode::FileExists)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error, false)),
        }
        let target_content = required_target_content(plan)?;
        let staged = stage_content(target.parent(), target_content, None)?;
        target.revalidate()?;
        atomic_rename_noreplace(staged.as_ref(), target.absolute_path())
            .map_err(|error| io_error(error, true))?;
        // Once publication succeeds, a directory sync failure makes the durable outcome unknown.
        sync_after_publication(Some(target.parent()))?;
        Ok(applied_commit(
            transaction_id,
            target,
            plan,
            committed_at,
            FileChangeStatus::Applied,
            None,
        ))
    }

    fn commit_update(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
    ) -> FileChangeResultValue<FileChangeCommit> {
        target.revalidate()?;
        let first = read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &first.bytes)?;
        let target_content = required_target_content(plan)?;
        let staged = stage_content(
            target.parent(),
            target_content,
            Some(first.permissions.clone()),
        )?;

        // Bind publication to a freshly checked revision. The replace itself changes the directory
        // entry atomically and never follows a leaf symlink.
        target.revalidate()?;
        let second = read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &second.bytes)?;
        if !same_permissions(&first.permissions, &second.permissions) {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        atomic_replace(staged.as_ref(), target.absolute_path())
            .map_err(|error| io_error(error, false))?;
        sync_after_publication(Some(target.parent()))?;
        Ok(applied_commit(
            transaction_id,
            target,
            plan,
            committed_at,
            FileChangeStatus::Applied,
            None,
        ))
    }

    fn commit_delete(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        journal: &mut FileChangeDeleteJournal,
    ) -> FileChangeResultValue<FileChangeCommit> {
        validate_delete_journal(transaction_id, target, plan, journal)?;
        match journal.state {
            FileChangeDeleteJournalState::Tombstoned | FileChangeDeleteJournalState::Finalized => {
                if self.reconcile(target, plan, Some(journal))?
                    != FileChangeReconciliation::AlreadyApplied
                {
                    return Err(FileChangeError::new(FileChangeErrorCode::OutcomeUnknown));
                }
                return Ok(applied_commit(
                    transaction_id,
                    target,
                    plan,
                    committed_at,
                    FileChangeStatus::AlreadyApplied,
                    Some(journal.clone()),
                ));
            }
            FileChangeDeleteJournalState::RolledBack => {
                return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
            }
            FileChangeDeleteJournalState::Prepared => {
                match self.reconcile(target, plan, Some(journal))? {
                    FileChangeReconciliation::AlreadyApplied => {
                        journal.state = FileChangeDeleteJournalState::Tombstoned;
                        journal.updated_at = committed_at;
                        return Ok(applied_commit(
                            transaction_id,
                            target,
                            plan,
                            committed_at,
                            FileChangeStatus::AlreadyApplied,
                            Some(journal.clone()),
                        ));
                    }
                    FileChangeReconciliation::DefinitelyNotExecuted => {}
                    FileChangeReconciliation::OutcomeUnknown => {
                        let code = if read_optional(target.absolute_path())?.is_some() {
                            FileChangeErrorCode::RevisionConflict
                        } else {
                            FileChangeErrorCode::OutcomeUnknown
                        };
                        return Err(FileChangeError::new(code));
                    }
                }
            }
        }

        target.revalidate()?;
        let current = read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &current.bytes)?;
        atomic_rename_noreplace(target.absolute_path(), journal.tombstone())
            .map_err(rename_conflict_error)?;
        if let Err(error) = ensure_path_digest(journal.tombstone(), &journal.base_digest) {
            let restored = atomic_rename_noreplace(journal.tombstone(), target.absolute_path());
            let _ = sync_directory(target.parent());
            return if restored.is_ok() {
                Err(FileChangeError::with_diagnostic(
                    FileChangeErrorCode::Conflict,
                    format!("tombstoned revision changed: {error}"),
                ))
            } else {
                Err(FileChangeError::with_diagnostic(
                    FileChangeErrorCode::OutcomeUnknown,
                    format!("tombstoned revision changed and rollback failed: {error}"),
                ))
            };
        }
        sync_after_publication(Some(target.parent()))?;
        journal.state = FileChangeDeleteJournalState::Tombstoned;
        journal.updated_at = committed_at;
        Ok(applied_commit(
            transaction_id,
            target,
            plan,
            committed_at,
            FileChangeStatus::Applied,
            Some(journal.clone()),
        ))
    }

    fn reconcile_deleted(
        &self,
        plan: &FileChangePlan,
        journal: Option<&FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeReconciliation> {
        let Some(journal) = journal else {
            return Ok(FileChangeReconciliation::OutcomeUnknown);
        };
        journal.validate()?;
        if journal.proposal_digest != plan.proposal_digest {
            return Ok(FileChangeReconciliation::OutcomeUnknown);
        }
        match journal.state {
            FileChangeDeleteJournalState::Tombstoned => {
                let tombstone = read_optional(journal.tombstone())?;
                if tombstone
                    .as_ref()
                    .is_some_and(|file| content_digest(&file.bytes) == journal.base_digest)
                {
                    Ok(FileChangeReconciliation::AlreadyApplied)
                } else {
                    Ok(FileChangeReconciliation::OutcomeUnknown)
                }
            }
            FileChangeDeleteJournalState::Finalized => {
                match fs::symlink_metadata(journal.tombstone()) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        Ok(FileChangeReconciliation::AlreadyApplied)
                    }
                    Ok(_) | Err(_) => Ok(FileChangeReconciliation::OutcomeUnknown),
                }
            }
            FileChangeDeleteJournalState::Prepared => {
                let tombstone = read_optional(journal.tombstone())?;
                if tombstone
                    .as_ref()
                    .is_some_and(|file| content_digest(&file.bytes) == journal.base_digest)
                {
                    Ok(FileChangeReconciliation::AlreadyApplied)
                } else {
                    Ok(FileChangeReconciliation::OutcomeUnknown)
                }
            }
            FileChangeDeleteJournalState::RolledBack => {
                Ok(FileChangeReconciliation::OutcomeUnknown)
            }
        }
    }
}

fn validate_transaction_and_plan(
    transaction_id: &str,
    target: &ResolvedFileChangeTarget,
    plan: &FileChangePlan,
) -> FileChangeResultValue<()> {
    plan.validate()?;
    if transaction_id.trim().is_empty() || plan.file_path != target.display_path() {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    match plan.operation {
        FileChangeOperation::Create
            if matches!(plan.base, FileChangeContentState::Missing)
                && matches!(plan.target, FileChangeContentState::Present { .. }) => {}
        FileChangeOperation::Update
            if matches!(plan.base, FileChangeContentState::Present { .. })
                && matches!(plan.target, FileChangeContentState::Present { .. }) => {}
        FileChangeOperation::Delete
            if matches!(plan.base, FileChangeContentState::Present { .. })
                && matches!(plan.target, FileChangeContentState::Missing) => {}
        _ => {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
    }
    if !valid_digest(&plan.proposal_digest) || !valid_digest(&plan.diff_digest) {
        return Err(FileChangeError::new(FileChangeErrorCode::InvalidArguments));
    }
    Ok(())
}

fn validate_delete_journal(
    transaction_id: &str,
    target: &ResolvedFileChangeTarget,
    plan: &FileChangePlan,
    journal: &FileChangeDeleteJournal,
) -> FileChangeResultValue<()> {
    journal.validate()?;
    if journal.transaction_id != transaction_id
        || journal.target_path != target.absolute_path().to_string_lossy()
        || journal.proposal_digest != plan.proposal_digest
        || Some(journal.base_digest.as_str()) != plan.base.digest()
    {
        return Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ));
    }
    Ok(())
}

fn required_target_content(plan: &FileChangePlan) -> FileChangeResultValue<&str> {
    plan.target_content
        .as_deref()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))
}

fn required_digest(state: &FileChangeContentState) -> FileChangeResultValue<&str> {
    state
        .digest()
        .ok_or_else(|| FileChangeError::new(FileChangeErrorCode::IllegalFieldCombination))
}

fn delete_journal_suffix(
    transaction_id: &str,
    target_path: &str,
    base_digest: &str,
    proposal_digest: &str,
) -> String {
    let material = format!("{transaction_id}\0{target_path}\0{base_digest}\0{proposal_digest}");
    content_digest(material.as_bytes())
        .rsplit_once(':')
        .map(|(_, digest)| digest.to_string())
        .unwrap_or_else(|| "invalid".to_string())
}

fn stage_content(
    parent: &Path,
    content: &str,
    permissions: Option<Permissions>,
) -> FileChangeResultValue<tempfile::TempPath> {
    let mut staged = NamedTempFile::new_in(parent).map_err(staging_error)?;
    staged
        .write_all(content.as_bytes())
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(staging_error)?;
    if let Some(permissions) = permissions {
        staged
            .as_file()
            .set_permissions(permissions)
            .and_then(|()| staged.as_file().sync_all())
            .map_err(staging_error)?;
    }
    Ok(staged.into_temp_path())
}

struct CurrentFile {
    bytes: Vec<u8>,
    permissions: Permissions,
}

fn read_required(path: &Path) -> FileChangeResultValue<CurrentFile> {
    read_optional(path)?.ok_or_else(|| FileChangeError::new(FileChangeErrorCode::FileMissing))
}

fn read_optional(path: &Path) -> FileChangeResultValue<Option<CurrentFile>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(FileChangeError::new(FileChangeErrorCode::SymlinkForbidden));
        }
        Ok(metadata) => validate_open_metadata(&metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error, false)),
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(open_error(error)),
    };
    let before = file.metadata().map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    validate_open_metadata(&before)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes).map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    let after = file.metadata().map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::Failed, error.to_string())
    })?;
    if !same_open_file(&before, &after) || after.len() != bytes.len() as u64 {
        return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
    }
    Ok(Some(CurrentFile {
        bytes,
        permissions: after.permissions(),
    }))
}

fn ensure_path_digest(path: &Path, expected: &str) -> FileChangeResultValue<()> {
    let current = read_required(path)?;
    if state_digest_matches(expected, &current.bytes) {
        Ok(())
    } else {
        Err(FileChangeError::new(FileChangeErrorCode::RevisionConflict))
    }
}

fn state_digest_matches(expected: &str, bytes: &[u8]) -> bool {
    content_digest(bytes) == expected
}

fn ensure_state_matches(
    expected: &FileChangeContentState,
    bytes: &[u8],
) -> FileChangeResultValue<()> {
    if state_matches(expected, bytes) {
        Ok(())
    } else {
        Err(FileChangeError::new(FileChangeErrorCode::RevisionConflict))
    }
}

fn state_matches(expected: &FileChangeContentState, bytes: &[u8]) -> bool {
    let FileChangeContentState::Present {
        revision,
        digest,
        byte_count,
    } = expected
    else {
        return false;
    };
    *byte_count == bytes.len() as u64
        && revision == &content_revision(bytes)
        && digest == &content_digest(bytes)
}

fn applied_commit(
    transaction_id: &str,
    target: &ResolvedFileChangeTarget,
    plan: &FileChangePlan,
    committed_at: u64,
    status: FileChangeStatus,
    delete_journal: Option<FileChangeDeleteJournal>,
) -> FileChangeCommit {
    FileChangeCommit {
        status,
        receipt: FileChangeReceipt {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            transaction_id: transaction_id.to_string(),
            operation: plan.operation,
            file_path: target.display_path().to_string(),
            outcome: FileChangeOutcome::Applied,
            base: plan.base.clone(),
            target: plan.target.clone(),
            proposal_digest: plan.proposal_digest.clone(),
            committed_at,
        },
        delete_journal,
    }
}

fn sync_after_publication(parent: Option<&Path>) -> FileChangeResultValue<()> {
    let parent = parent.ok_or_else(|| FileChangeError::new(FileChangeErrorCode::OutcomeUnknown))?;
    sync_directory(parent).map_err(|error| {
        FileChangeError::with_diagnostic(FileChangeErrorCode::OutcomeUnknown, error.to_string())
    })
}

fn io_error(error: io::Error, no_replace: bool) -> FileChangeError {
    let code = match error.kind() {
        io::ErrorKind::AlreadyExists if no_replace => FileChangeErrorCode::FileExists,
        io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
        io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
        _ => FileChangeErrorCode::Failed,
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

fn staging_error(error: io::Error) -> FileChangeError {
    let code = if error.kind() == io::ErrorKind::PermissionDenied {
        FileChangeErrorCode::PermissionDenied
    } else {
        FileChangeErrorCode::Failed
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

fn rename_conflict_error(error: io::Error) -> FileChangeError {
    let code = match error.kind() {
        io::ErrorKind::AlreadyExists => FileChangeErrorCode::Conflict,
        io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
        io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
        _ => FileChangeErrorCode::Failed,
    };
    FileChangeError::with_diagnostic(code, error.to_string())
}

fn open_error(error: io::Error) -> FileChangeError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP) {
        return FileChangeError::with_diagnostic(
            FileChangeErrorCode::SymlinkForbidden,
            error.to_string(),
        );
    }
    io_error(error, false)
}

fn validate_open_metadata(metadata: &fs::Metadata) -> FileChangeResultValue<()> {
    if !metadata.is_file() {
        return Err(FileChangeError::new(FileChangeErrorCode::NotRegularFile));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(FileChangeError::new(FileChangeErrorCode::HardLinkForbidden));
        }
    }
    Ok(())
}

#[cfg(unix)]
fn same_open_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev()
        && left.ino() == right.ino()
        && left.len() == right.len()
        && left.mtime() == right.mtime()
        && left.mtime_nsec() == right.mtime_nsec()
}

#[cfg(unix)]
fn same_permissions(left: &Permissions, right: &Permissions) -> bool {
    use std::os::unix::fs::PermissionsExt;
    left.mode() == right.mode()
}

#[cfg(not(unix))]
fn same_permissions(left: &Permissions, right: &Permissions) -> bool {
    left.readonly() == right.readonly()
}

#[cfg(not(unix))]
fn same_open_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    left.len() == right.len() && left.modified().ok() == right.modified().ok()
}
