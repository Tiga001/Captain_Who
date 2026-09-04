pub(super) fn cancelled_staged_file_change_outcome_is_durable_or_unknown(
    storage: &StorageService,
    record: &PendingActionRecord,
) -> bool {
    let AgentProposedAction::FileChange { file_change } = &record.snapshot.action else {
        return false;
    };
    let Some(transaction_id) = file_change.execution.staged_transaction_id.as_deref() else {
        return false;
    };
    match storage.get_agent_file_change(transaction_id) {
        Ok(Some(draft)) => draft.status == "aborted",
        Ok(None) => false,
        // A storage read failure makes rollback unsafe: the abort write may have committed.
        Err(_) => true,
    }
}
