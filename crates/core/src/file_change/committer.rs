use super::bound_io::{BoundParent, BoundUpdateExchange};
use super::digest::{content_digest, valid_digest};
use super::error::{FileChangeError, FileChangeErrorCode, FileChangeResultValue};
use super::model::{
    FileChangeContentState, FileChangeOperation, FileChangeOutcome, FileChangeReceipt,
    FileChangeStatus, FILE_CHANGE_SCHEMA_VERSION,
};
use super::planner::FileChangePlan;
use super::policy::ResolvedFileChangeTarget;
use crate::content_revision;
use serde::{Deserialize, Serialize};
use std::fs::Permissions;
use std::io;
use std::path::Path;

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
        let parent = BoundParent::open(target)?;
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
        if parent.read_optional(&tombstone)?.is_some() {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
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

    /// Executes a transaction that is authoritatively known not to have started yet.
    ///
    /// `commit` is deliberately idempotent for commit-unknown recovery and may adopt an exact
    /// target as `already_applied`. A live approval path must not make that inference: another
    /// actor can produce the same bytes while approval is pending. This entry point requires the
    /// frozen Base to still be current and rejects an exact Target as a conflict instead.
    pub fn commit_fresh(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        mut delete_journal: Option<&mut FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeCommit> {
        validate_transaction_and_plan(transaction_id, target, plan)?;
        match self.reconcile(target, plan, delete_journal.as_deref())? {
            FileChangeReconciliation::DefinitelyNotExecuted => {}
            FileChangeReconciliation::AlreadyApplied | FileChangeReconciliation::OutcomeUnknown => {
                return Err(FileChangeError::new(fresh_conflict_code(plan.operation)));
            }
        }
        let first_attempt = self.commit(
            transaction_id,
            target,
            plan,
            committed_at,
            delete_journal.as_deref_mut(),
        );
        let (commit, reconciled_after_attempt) = match first_attempt {
            Ok(commit) => (commit, false),
            Err(error) if error.code() == FileChangeErrorCode::OutcomeUnknown => {
                // The initial fresh-state check proved that this transaction had not started. An
                // outcome-unknown error returned after entering `commit` may therefore be
                // reconciled idempotently without adopting an unrelated pre-existing Target.
                (
                    self.reconcile_attempted_outcome_unknown(
                        transaction_id,
                        target,
                        plan,
                        committed_at,
                        delete_journal,
                        error,
                    )?,
                    true,
                )
            }
            Err(error) => return Err(error),
        };
        if commit.status == FileChangeStatus::AlreadyApplied && !reconciled_after_attempt {
            Err(FileChangeError::new(fresh_conflict_code(plan.operation)))
        } else {
            Ok(commit)
        }
    }

    pub(crate) fn reconcile_attempted_outcome_unknown(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        delete_journal: Option<&mut FileChangeDeleteJournal>,
        original_error: FileChangeError,
    ) -> FileChangeResultValue<FileChangeCommit> {
        let reconciliation = self
            .reconcile(target, plan, delete_journal.as_deref())
            .map_err(|_| FileChangeError::new(FileChangeErrorCode::OutcomeUnknown))?;
        match reconciliation {
            FileChangeReconciliation::AlreadyApplied => self
                .commit(transaction_id, target, plan, committed_at, delete_journal)
                .map_err(|_| FileChangeError::new(FileChangeErrorCode::OutcomeUnknown)),
            FileChangeReconciliation::DefinitelyNotExecuted => {
                Err(FileChangeError::new(FileChangeErrorCode::Failed))
            }
            FileChangeReconciliation::OutcomeUnknown => Err(original_error),
        }
    }

    pub fn finalize_delete(
        &self,
        target: &ResolvedFileChangeTarget,
        journal: &mut FileChangeDeleteJournal,
        now: u64,
    ) -> FileChangeResultValue<()> {
        journal.validate()?;
        validate_delete_target(target, journal)?;
        if journal.state == FileChangeDeleteJournalState::Finalized {
            return Ok(());
        }
        if journal.state != FileChangeDeleteJournalState::Tombstoned {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let parent = BoundParent::open(target)?;
        if parent.read_optional(journal.tombstone())?.is_none() {
            // `Tombstoned` is persisted only with the exact commit receipt. If the recovery file
            // is already absent, cleanup either completed before the caller observed its result or
            // another authorized cleanup removed it. A newly created target belongs to a later
            // transaction and must never be removed or used to roll this delete back.
            sync_after_publication(&parent)?;
            journal.state = FileChangeDeleteJournalState::Finalized;
            journal.updated_at = now;
            return Ok(());
        }
        ensure_path_digest(&parent, journal.tombstone(), &journal.base_digest)?;
        parent.revalidate()?;
        parent
            .remove(journal.tombstone())
            .map_err(|error| io_error(error, false))?;
        sync_after_publication(&parent)?;
        journal.state = FileChangeDeleteJournalState::Finalized;
        journal.updated_at = now;
        Ok(())
    }

    pub fn rollback_delete(
        &self,
        target: &ResolvedFileChangeTarget,
        journal: &mut FileChangeDeleteJournal,
        now: u64,
    ) -> FileChangeResultValue<()> {
        journal.validate()?;
        validate_delete_target(target, journal)?;
        if journal.state == FileChangeDeleteJournalState::RolledBack {
            return Ok(());
        }
        if journal.state != FileChangeDeleteJournalState::Tombstoned {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        let parent = BoundParent::open(target)?;
        if let Some(current) = parent.read_optional(target.absolute_path())? {
            if state_digest_matches(&journal.base_digest, &current.bytes)
                && parent.read_optional(journal.tombstone())?.is_none()
            {
                journal.state = FileChangeDeleteJournalState::RolledBack;
                journal.updated_at = now;
                return Ok(());
            }
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        ensure_path_digest(&parent, journal.tombstone(), &journal.base_digest)?;
        parent.revalidate()?;
        parent
            .rename_noreplace(journal.tombstone(), target.absolute_path())
            .map_err(rename_conflict_error)?;
        sync_after_publication(&parent)?;
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
        let parent = BoundParent::open(target)?;
        self.reconcile_bound(&parent, plan, delete_journal)
    }

    fn reconcile_bound(
        &self,
        parent: &BoundParent<'_>,
        plan: &FileChangePlan,
        delete_journal: Option<&FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeReconciliation> {
        let current = parent.read_optional(parent.target_path())?;
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
                None => self.reconcile_deleted(parent, plan, delete_journal),
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
        let parent = BoundParent::open(target)?;
        match self.reconcile_bound(&parent, plan, None)? {
            FileChangeReconciliation::AlreadyApplied => {
                sync_after_publication(&parent)?;
                return Ok(applied_commit(
                    transaction_id,
                    target,
                    plan,
                    committed_at,
                    FileChangeStatus::AlreadyApplied,
                    None,
                ));
            }
            FileChangeReconciliation::DefinitelyNotExecuted => {}
            FileChangeReconciliation::OutcomeUnknown => {
                return Err(FileChangeError::new(FileChangeErrorCode::FileExists));
            }
        }
        if parent.read_optional(target.absolute_path())?.is_some() {
            return Err(FileChangeError::new(FileChangeErrorCode::FileExists));
        }
        let target_content = required_target_content(plan)?;
        let staged = parent.stage(target_content, None)?;
        parent.revalidate()?;
        parent
            .publish_noreplace(staged, target.absolute_path())
            .map_err(|error| io_error(error, true))?;
        // Once publication succeeds, a directory sync failure makes the durable outcome unknown.
        sync_after_publication(&parent)?;
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
        self.commit_update_inner(transaction_id, target, plan, committed_at, || {})
    }

    #[cfg(test)]
    pub(super) fn commit_update_with_before_exchange(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        before_exchange: impl FnOnce(),
    ) -> FileChangeResultValue<FileChangeCommit> {
        validate_transaction_and_plan(transaction_id, target, plan)?;
        self.commit_update_inner(transaction_id, target, plan, committed_at, before_exchange)
    }

    fn commit_update_inner(
        &self,
        transaction_id: &str,
        target: &ResolvedFileChangeTarget,
        plan: &FileChangePlan,
        committed_at: u64,
        before_exchange: impl FnOnce(),
    ) -> FileChangeResultValue<FileChangeCommit> {
        let parent = BoundParent::open(target)?;
        match self.reconcile_bound(&parent, plan, None)? {
            FileChangeReconciliation::AlreadyApplied => {
                sync_after_publication(&parent)?;
                return Ok(applied_commit(
                    transaction_id,
                    target,
                    plan,
                    committed_at,
                    FileChangeStatus::AlreadyApplied,
                    None,
                ));
            }
            FileChangeReconciliation::DefinitelyNotExecuted => {}
            FileChangeReconciliation::OutcomeUnknown => {
                return Err(FileChangeError::new(FileChangeErrorCode::RevisionConflict));
            }
        }
        let first = parent.read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &first.bytes)?;
        let target_content = required_target_content(plan)?;
        let staged = parent.stage(target_content, Some(first.permissions.clone()))?;

        // Atomically retain whatever generation occupies the target at the publication
        // linearization point. Verification therefore cannot overwrite-and-forget a write that
        // lands after this final Base read.
        parent.revalidate()?;
        let second = parent.read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &second.bytes)?;
        if !same_permissions(&first.permissions, &second.permissions) {
            return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
        }
        parent.revalidate()?;
        before_exchange();
        let exchanged = parent
            .publish_exchange(staged, target.absolute_path())
            .map_err(|error| io_error(error, false))?;

        let displaced_validation = parent.read_exchanged(&exchanged).and_then(|displaced| {
            if !parent.same_file_identity(&second, &displaced) {
                return Err(FileChangeError::new(FileChangeErrorCode::RevisionConflict));
            }
            ensure_state_matches(&plan.base, &displaced.bytes)?;
            if !same_permissions(&second.permissions, &displaced.permissions) {
                return Err(FileChangeError::new(FileChangeErrorCode::Conflict));
            }
            Ok(())
        });
        if let Err(validation_error) = displaced_validation {
            return match rollback_update_exchange(
                &parent,
                target.absolute_path(),
                plan,
                &second.permissions,
                exchanged,
            ) {
                Ok(()) => Err(validation_error),
                Err(rollback_error) => Err(rollback_error),
            };
        }

        // Only the exact, verified Base generation may be unlinked. From the exchange onward,
        // cleanup or durability failures are commit-unknown because Target was already visible.
        parent
            .remove_exchanged(exchanged)
            .map_err(outcome_unknown_io)?;
        sync_after_publication(&parent)?;
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
        let parent = BoundParent::open(target)?;
        match journal.state {
            FileChangeDeleteJournalState::Tombstoned | FileChangeDeleteJournalState::Finalized => {
                if self.reconcile_bound(&parent, plan, Some(journal))?
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
                match self.reconcile_bound(&parent, plan, Some(journal))? {
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
                        let code = if parent.read_optional(target.absolute_path())?.is_some() {
                            FileChangeErrorCode::RevisionConflict
                        } else {
                            FileChangeErrorCode::OutcomeUnknown
                        };
                        return Err(FileChangeError::new(code));
                    }
                }
            }
        }

        parent.revalidate()?;
        let current = parent.read_required(target.absolute_path())?;
        ensure_state_matches(&plan.base, &current.bytes)?;
        parent.revalidate()?;
        parent
            .rename_noreplace(target.absolute_path(), journal.tombstone())
            .map_err(rename_conflict_error)?;
        if let Err(error) = ensure_path_digest(&parent, journal.tombstone(), &journal.base_digest) {
            let restored = parent.rename_noreplace(journal.tombstone(), target.absolute_path());
            let _ = parent.sync();
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
        sync_after_publication(&parent)?;
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
        parent: &BoundParent<'_>,
        plan: &FileChangePlan,
        journal: Option<&FileChangeDeleteJournal>,
    ) -> FileChangeResultValue<FileChangeReconciliation> {
        let Some(journal) = journal else {
            return Ok(FileChangeReconciliation::OutcomeUnknown);
        };
        journal.validate()?;
        if journal.target_path != parent.target_path().to_string_lossy() {
            return Err(FileChangeError::new(
                FileChangeErrorCode::IllegalFieldCombination,
            ));
        }
        if journal.proposal_digest != plan.proposal_digest {
            return Ok(FileChangeReconciliation::OutcomeUnknown);
        }
        match journal.state {
            FileChangeDeleteJournalState::Tombstoned => {
                let tombstone = parent.read_optional(journal.tombstone())?;
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
                match parent.read_optional(journal.tombstone())? {
                    None => Ok(FileChangeReconciliation::AlreadyApplied),
                    Some(_) => Ok(FileChangeReconciliation::OutcomeUnknown),
                }
            }
            FileChangeDeleteJournalState::Prepared => {
                let tombstone = parent.read_optional(journal.tombstone())?;
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

fn fresh_conflict_code(operation: FileChangeOperation) -> FileChangeErrorCode {
    match operation {
        FileChangeOperation::Create => FileChangeErrorCode::FileExists,
        FileChangeOperation::Update => FileChangeErrorCode::RevisionConflict,
        FileChangeOperation::Delete => FileChangeErrorCode::OutcomeUnknown,
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

fn validate_delete_target(
    target: &ResolvedFileChangeTarget,
    journal: &FileChangeDeleteJournal,
) -> FileChangeResultValue<()> {
    if journal.target_path == target.absolute_path().to_string_lossy() {
        Ok(())
    } else {
        Err(FileChangeError::new(
            FileChangeErrorCode::IllegalFieldCombination,
        ))
    }
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

fn ensure_path_digest(
    parent: &BoundParent<'_>,
    path: &Path,
    expected: &str,
) -> FileChangeResultValue<()> {
    let current = parent.read_required(path)?;
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

fn rollback_update_exchange(
    parent: &BoundParent<'_>,
    target: &Path,
    plan: &FileChangePlan,
    staged_permissions: &Permissions,
    mut exchanged: BoundUpdateExchange,
) -> FileChangeResultValue<()> {
    parent
        .rollback_exchange(&mut exchanged, target)
        .map_err(outcome_unknown_io)?;

    // The exchange syscall is atomic, but a second actor may have changed Target after our first
    // exchange. Only discard the private entry when it is provably our staged Target generation;
    // otherwise every unverified generation remains linked and the caller receives commit-unknown.
    let restored_staged = parent.read_exchanged(&exchanged).map_err(|error| {
        outcome_unknown_diagnostic(format!("rollback verification failed: {error}"))
    })?;
    let staged_identity_matches = parent
        .exchanged_is_staged_target(&exchanged, &restored_staged)
        .map_err(outcome_unknown_io)?;
    if !staged_identity_matches
        || !state_matches(&plan.target, &restored_staged.bytes)
        || !same_permissions(staged_permissions, &restored_staged.permissions)
    {
        return Err(outcome_unknown_diagnostic(
            "rollback was concurrently interfered with; preserved every unverified generation",
        ));
    }

    // At this point the target name has been restored and the private name is exactly our staged
    // inode, so removing it cannot discard the concurrent generation that caused validation to
    // fail.
    parent
        .remove_exchanged(exchanged)
        .map_err(outcome_unknown_io)?;
    sync_after_publication(parent)
}

fn outcome_unknown_io(error: io::Error) -> FileChangeError {
    outcome_unknown_diagnostic(error.to_string())
}

fn outcome_unknown_diagnostic(diagnostic: impl Into<String>) -> FileChangeError {
    FileChangeError::with_diagnostic(
        FileChangeErrorCode::OutcomeUnknown,
        diagnostic.into().into_boxed_str(),
    )
}

fn sync_after_publication(parent: &BoundParent<'_>) -> FileChangeResultValue<()> {
    parent.sync().map_err(|error| {
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

fn rename_conflict_error(error: io::Error) -> FileChangeError {
    let code = match error.kind() {
        io::ErrorKind::AlreadyExists => FileChangeErrorCode::Conflict,
        io::ErrorKind::NotFound => FileChangeErrorCode::FileMissing,
        io::ErrorKind::PermissionDenied => FileChangeErrorCode::PermissionDenied,
        _ => FileChangeErrorCode::Failed,
    };
    FileChangeError::with_diagnostic(code, error.to_string())
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
