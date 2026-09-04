use super::*;

#[cfg(test)]
static AUTO_ACTION_AUDIT_FAILURES: Mutex<Vec<(String, String, String)>> = Mutex::new(Vec::new());

#[cfg(test)]
static AUTO_ACTION_AUDIT_POST_COMMIT_FAILURES: Mutex<Vec<(String, String, String)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
static MANUAL_ACTION_AUDIT_FAILURES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

#[cfg(test)]
static MANUAL_ACTION_AUDIT_POST_COMMIT_FAILURES: Mutex<Vec<(String, String)>> =
    Mutex::new(Vec::new());

#[cfg(test)]
static PENDING_STATUS_TRANSITION_FAILURES: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// Installs a one-shot, action-scoped audit persistence failure for Host boundary tests.
///
/// Matching on run, provider action id, and status keeps parallel tests isolated without adding
/// production-only dependency injection surface to `AgentService`.
#[cfg(test)]
pub(super) fn inject_auto_action_audit_failure(
    run_id: &str,
    provider_action_id: &str,
    status: &str,
) {
    AUTO_ACTION_AUDIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((
            run_id.to_string(),
            provider_action_id.to_string(),
            status.to_string(),
        ));
}

/// Simulates a storage/transport error reported after SQLite has committed the terminal receipt.
///
/// This is deliberately separate from [`inject_auto_action_audit_failure`]: callers must prove
/// that a commit-unknown response is reconciled from durable state instead of overwriting a
/// successful terminal receipt or replaying the process.
#[cfg(test)]
pub(super) fn inject_auto_action_audit_post_commit_failure(
    run_id: &str,
    provider_action_id: &str,
    status: &str,
) {
    AUTO_ACTION_AUDIT_POST_COMMIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((
            run_id.to_string(),
            provider_action_id.to_string(),
            status.to_string(),
        ));
}

#[cfg(test)]
pub(super) fn inject_manual_action_audit_failure(storage_id: &str, status: &str) {
    MANUAL_ACTION_AUDIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((storage_id.to_string(), status.to_string()));
}

#[cfg(test)]
pub(super) fn inject_manual_action_audit_post_commit_failure(storage_id: &str, status: &str) {
    MANUAL_ACTION_AUDIT_POST_COMMIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((storage_id.to_string(), status.to_string()));
}

/// Installs a one-shot, action-scoped pending status persistence failure. This exercises the
/// independent terminal/pending settlement retry without adding a production fault surface.
#[cfg(test)]
pub(super) fn inject_pending_status_transition_failure(storage_id: &str, status: &str) {
    PENDING_STATUS_TRANSITION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .push((storage_id.to_string(), status.to_string()));
}

#[cfg(test)]
fn take_pending_status_transition_failure(storage_id: &str, status: &str) -> Option<String> {
    let mut failures = PENDING_STATUS_TRANSITION_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures
        .iter()
        .position(|candidate| candidate.0 == storage_id && candidate.1 == status)?;
    failures.swap_remove(index);
    Some(format!(
        "injected pending status persistence failure for action={storage_id}, status={status}"
    ))
}

#[cfg(test)]
fn take_auto_action_audit_failure(
    run_id: &str,
    provider_action_id: &str,
    status: &str,
) -> Option<String> {
    let mut failures = AUTO_ACTION_AUDIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures.iter().position(|candidate| {
        candidate.0 == run_id && candidate.1 == provider_action_id && candidate.2 == status
    })?;
    failures.swap_remove(index);
    Some(format!(
        "injected auto action audit persistence failure for run={run_id}, action={provider_action_id}, status={status}"
    ))
}

#[cfg(test)]
fn take_auto_action_audit_post_commit_failure(
    run_id: &str,
    provider_action_id: &str,
    status: &str,
) -> Option<String> {
    let mut failures = AUTO_ACTION_AUDIT_POST_COMMIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures.iter().position(|candidate| {
        candidate.0 == run_id && candidate.1 == provider_action_id && candidate.2 == status
    })?;
    failures.swap_remove(index);
    Some(format!(
        "injected post-commit auto action audit failure for run={run_id}, action={provider_action_id}, status={status}"
    ))
}

#[cfg(test)]
fn take_manual_action_audit_failure(storage_id: &str, status: &str) -> Option<String> {
    let mut failures = MANUAL_ACTION_AUDIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures
        .iter()
        .position(|candidate| candidate.0 == storage_id && candidate.1 == status)?;
    failures.swap_remove(index);
    Some(format!(
        "injected manual action audit persistence failure for action={storage_id}, status={status}"
    ))
}

#[cfg(test)]
fn take_manual_action_audit_post_commit_failure(storage_id: &str, status: &str) -> Option<String> {
    let mut failures = MANUAL_ACTION_AUDIT_POST_COMMIT_FAILURES
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let index = failures
        .iter()
        .position(|candidate| candidate.0 == storage_id && candidate.1 == status)?;
    failures.swap_remove(index);
    Some(format!(
        "injected post-commit manual action audit failure for action={storage_id}, status={status}"
    ))
}
