/// Reconciles current FileChanges from their frozen Base/Target digests without replaying a
/// mutation. Target equality produces `already_applied`, Base equality produces an explicit
/// definitely-not-executed failure, and every divergent or unreadable target produces
/// `outcome_unknown`. Each result is written as a typed ToolResult/audit before the generic
/// startup terminalizer moves the Pending Action to its durable target status.
pub(super) fn reconcile_interrupted_file_changes(
    storage: &Arc<StorageService>,
    reconciled_at: i64,
) -> Result<usize, String> {
    use mycopilot_core::file_change::{
        FileChangeCommitter, FileChangeOperation, FileChangeReconciliation,
    };
    use mycopilot_core::storage::service::{
        AgentPendingActionJsonCommitOutcome, AgentPendingActionResultCommitOutcome,
    };

    let committed_at = u64::try_from(reconciled_at)
        .map_err(|_| "Direct FileChange reconciliation timestamp is invalid".to_string())?;
    let candidates = storage.list_interrupted_agent_actions_for_host_reconciliation()?;
    let mut recovered = 0usize;
    for mut durable in candidates {
        if durable.status != "executing"
            || durable.action_type != "file_change"
            || durable.tool_name != "apply_patch"
        {
            continue;
        }
        let decoded = match PersistedAgentResumeInput::decode(&durable.agent_input_json) {
            Ok(decoded) => decoded,
            Err(_) => continue,
        };
        let prepared_action_json = durable.action_json.clone();
        let mut action = match serde_json::from_str::<AgentProposedAction>(&durable.action_json) {
            Ok(action @ AgentProposedAction::FileChange { .. }) => action,
            _ => continue,
        };
        let binding = match recovery_file_change_binding(&action) {
            Ok(binding) => binding.clone(),
            Err(_) => continue,
        };
        let provider_action_id = action_id_for_action(&action);
        if action_type_for_action(&action) != durable.action_type
            || tool_name_for_action(&action) != durable.tool_name
            || binding.run_id != durable.run_id
            || Some(binding.conversation_id.as_str()) != durable.conversation_id.as_deref()
            || durable.tool_call_id.as_deref() != Some(provider_action_id.as_str())
            || durable.action_id != pending_action_storage_id(&durable.run_id, &provider_action_id)
        {
            continue;
        }
        let preflight_record = PendingActionRecord {
            storage_id: durable.action_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id: provider_action_id.clone(),
                action_type: durable.action_type.clone(),
                tool_name: durable.tool_name.clone(),
                tool_call_id: durable.tool_call_id.clone(),
                run_id: durable.run_id.clone(),
                conversation_id: durable.conversation_id.clone(),
                assistant_message_id: durable.assistant_message_id.clone(),
                action: action.clone(),
                created_at: durable.created_at,
                status: PendingActionStatus::Executing,
            },
            agent_input: decoded.agent_input.clone(),
        };
        let existing_audit = storage.get_agent_action_audit(&durable.action_id)?;
        let is_exact_auto_journal = existing_audit.as_ref().is_some_and(|existing| {
            let lifecycle_matches = existing.status == "executing"
                || (durable.target_status.as_deref() == Some(existing.status.as_str())
                    && matches!(existing.status.as_str(), "completed" | "failed")
                    && existing.file_change_result_json.is_some()
                    && existing.tool_result_json.is_some()
                    && existing.completed_at.is_some());
            matches!(
                existing.decision_source.as_deref(),
                Some("auto" | "run_grant")
            ) && lifecycle_matches
                && existing.action_json == durable.action_json
                && existing.run_id == durable.run_id
                && existing.conversation_id == durable.conversation_id
                && existing.assistant_message_id == durable.assistant_message_id
                && existing.action_type == "file_change"
                && existing.tool_name == "apply_patch"
        });
        let binding_matches = if is_exact_auto_journal {
            pending_action_binding_matches_for_auto_journal(
                &durable.run_id,
                Some(&durable),
                &action,
                &decoded.agent_input,
            )
        } else {
            pending_action_binding_matches(
                &durable.run_id,
                Some(&durable),
                &action,
                &decoded.agent_input,
            )
        };
        let tool_call_valid = tool_call_for_pending_record(&preflight_record).is_ok();
        if !binding_matches || !tool_call_valid {
            continue;
        }
        if durable.target_status.as_deref() == Some("cancelled") {
            if existing_audit
                .as_ref()
                .is_some_and(|audit| audit.status == "cancelled")
            {
                // The normal cancellation path writes its typed audit and ToolResult timeline
                // before the final pending-status CAS. The generic pass independently validates
                // that exact durable bundle and refuses a missing or tampered result.
                continue;
            }
            let execution = cancelled_file_change_execution(storage, &preflight_record)?;
            let file_change_result = execution.file_change_result.as_ref().ok_or_else(|| {
                "cancelled FileChange recovery lacks its typed result".to_string()
            })?;
            if execution.final_pending_status != PendingActionStatus::Cancelled
                || execution.tool_result.call_id != provider_action_id
                || execution.tool_result.tool != "apply_patch"
            {
                return Err("cancelled FileChange recovery identity is invalid".to_string());
            }
            let assistant_message_id =
                durable.assistant_message_id.as_deref().ok_or_else(|| {
                    "cancelled FileChange recovery is missing its Assistant owner".to_string()
                })?;
            let trace = storage
                .get_conversation_turn_trace(assistant_message_id)?
                .ok_or_else(|| {
                    "cancelled FileChange recovery is missing its durable trace".to_string()
                })?;
            let model_context_items = storage
                .get_conversation_model_context_log(assistant_message_id)?
                .ok_or_else(|| {
                    "cancelled FileChange recovery is missing its durable model context".to_string()
                })?
                .items;
            let mut recovered_snapshot =
                mycopilot_core::conversation_trace_snapshot_with_recovered_tool_result(
                    &trace,
                    model_context_items,
                    &execution.tool_result,
                )?;
            set_recovered_file_change_result_approval(
                &mut recovered_snapshot,
                &execution.tool_result,
                AgentApprovalStatus::Rejected,
            )?;
            let conversation_id = durable.conversation_id.as_deref().ok_or_else(|| {
                "cancelled FileChange recovery is missing its conversation owner".to_string()
            })?;
            let recovered_trace = recovered_snapshot.in_progress_trace(
                &durable.run_id,
                conversation_id,
                assistant_message_id,
            );
            let recovered_model_context = recovered_snapshot.committed_prefix().model_context_items;
            let mut audit = action_audit_record(
                &preflight_record,
                Some("cancelled"),
                "cancelled",
                Some(file_change_result),
                None,
                Some(&execution.tool_result),
                execution.tool_result.error.as_deref(),
                Some(reconciled_at),
                Some(reconciled_at),
            );
            audit.decision_source = Some("manual".to_string());
            match storage.commit_pending_agent_action_audited_result_trace_with_model_context(
                &audit,
                "executing",
                "cancelled",
                &recovered_trace,
                &recovered_model_context,
                reconciled_at,
            )? {
                AgentPendingActionResultCommitOutcome::Committed { .. }
                | AgentPendingActionResultCommitOutcome::Idempotent => {}
            }
            recovered = recovered.saturating_add(1);
            continue;
        }
        if durable.target_status.is_some() {
            // Current automatic settlement commits the terminal audit, pending target, exact
            // ToolResult trace, and model context atomically. The generic startup pass validates
            // that authoritative bundle before advancing the lifecycle status.
            continue;
        }
        let (mut staged_record, staged_record_is_valid) = match staged_file_change_recovery_record(
            storage,
            &action,
            Some(&decoded.agent_input),
        ) {
            Ok(record) => (record, true),
            Err(_) => (None, false),
        };
        let plan = match recovery_file_change_plan(&action) {
            Ok(plan) => plan,
            Err(_) => continue,
        };
        let target =
            resolve_direct_file_change_recovery_target(&binding.canonical_target, &plan.file_path);
        let committer = FileChangeCommitter;
        let disposition = if !staged_record_is_valid {
            RecoveredFileChangeDisposition::OutcomeUnknown
        } else {
            match target.as_ref().and_then(|target| {
                committer
                    .reconcile(target, &plan, binding.delete_journal.as_ref())
                    .ok()
            }) {
                Some(FileChangeReconciliation::AlreadyApplied) => {
                    RecoveredFileChangeDisposition::AlreadyApplied
                }
                Some(FileChangeReconciliation::DefinitelyNotExecuted) => {
                    RecoveredFileChangeDisposition::DefinitelyNotExecuted
                }
                Some(FileChangeReconciliation::OutcomeUnknown) | None => {
                    RecoveredFileChangeDisposition::OutcomeUnknown
                }
            }
        };

        if disposition == RecoveredFileChangeDisposition::AlreadyApplied && binding.is_prepared() {
            let target = target.as_ref().ok_or_else(|| {
                "FileChange recovery lost its reconciled canonical target".to_string()
            })?;
            let commit = match plan.operation {
                FileChangeOperation::Delete => {
                    let mut journal = binding.delete_journal.clone().ok_or_else(|| {
                        "prepared Direct delete is missing its durable journal".to_string()
                    })?;
                    committer
                        .commit(
                            &binding.transaction.id,
                            target,
                            &plan,
                            committed_at,
                            Some(&mut journal),
                        )
                        .map_err(|error| error.to_string())?
                }
                FileChangeOperation::Create | FileChangeOperation::Update => committer
                    .commit(&binding.transaction.id, target, &plan, committed_at, None)
                    .map_err(|error| error.to_string())?,
            };
            let committed_binding = binding
                .with_commit(&commit)
                .map_err(|error| format!("FileChange recovery receipt is invalid: {error}"))?;
            action = file_change_action_with_binding(&action, committed_binding)?;
            let committed_action_json = serde_json::to_string(&action)
                .map_err(|_| "FileChange recovery receipt could not be encoded".to_string())?;
            let outcome = storage.commit_pending_direct_file_change_action_json(
                &durable,
                &prepared_action_json,
                &committed_action_json,
                reconciled_at,
            )?;
            if !matches!(
                outcome,
                AgentPendingActionJsonCommitOutcome::Updated
                    | AgentPendingActionJsonCommitOutcome::AlreadyCommitted
            ) {
                return Err(format!(
                    "FileChange recovery lost its pending receipt CAS: {outcome:?}"
                ));
            }
            durable.action_json = committed_action_json;
            durable.updated_at = reconciled_at;
        } else if disposition == RecoveredFileChangeDisposition::AlreadyApplied
            && binding.receipt.is_none()
        {
            return Err("FileChange recovery has no durable receipt after commit".to_string());
        }
        if disposition == RecoveredFileChangeDisposition::AlreadyApplied {
            let target = target.as_ref().ok_or_else(|| {
                "FileChange recovery lost its reconciled canonical target".to_string()
            })?;
            finalize_recovered_manual_direct_delete(
                storage,
                &mut durable,
                &mut action,
                target,
                committed_at,
                reconciled_at,
            )?;
        }
        settle_recovered_staged_file_change(
            storage,
            staged_record.as_mut(),
            disposition,
            reconciled_at,
        )?;

        let snapshot = PendingAgentActionSnapshot {
            action_id: provider_action_id,
            action_type: durable.action_type.clone(),
            tool_name: durable.tool_name.clone(),
            tool_call_id: durable.tool_call_id.clone(),
            run_id: durable.run_id.clone(),
            conversation_id: durable.conversation_id.clone(),
            assistant_message_id: durable.assistant_message_id.clone(),
            action: action.clone(),
            created_at: durable.created_at,
            status: PendingActionStatus::Executing,
        };
        let record = PendingActionRecord {
            storage_id: durable.action_id.clone(),
            snapshot,
            agent_input: decoded.agent_input,
        };
        if tool_call_for_pending_record(&record).is_err() {
            continue;
        }
        let (file_change_result, tool_result) =
            recovered_file_change_tool_result(&action, disposition)?;
        let assistant_message_id = durable.assistant_message_id.as_deref().ok_or_else(|| {
            "Direct FileChange recovery is missing its Assistant owner".to_string()
        })?;
        let trace = storage
            .get_conversation_turn_trace(assistant_message_id)?
            .ok_or_else(|| "Direct FileChange recovery is missing its durable trace".to_string())?;
        let model_context_items = storage
            .get_conversation_model_context_log(assistant_message_id)?
            .ok_or_else(|| {
                "Direct FileChange recovery is missing its durable model context".to_string()
            })?
            .items;
        let mut recovered_snapshot =
            mycopilot_core::conversation_trace_snapshot_with_recovered_tool_result(
                &trace,
                model_context_items,
                &tool_result,
            )?;
        if !is_exact_auto_journal {
            // The provider ToolCall remains immutable `required`, while a manual approval's
            // terminal ToolResult records the decision that authorized this recovered effect.
            // Normal execution already has this split; startup reconciliation must reproduce it
            // exactly instead of inheriting the unresolved ToolCall's approval marker.
            set_recovered_file_change_result_approval(
                &mut recovered_snapshot,
                &tool_result,
                AgentApprovalStatus::Approved,
            )?;
        }
        let conversation_id = durable.conversation_id.as_deref().ok_or_else(|| {
            "Direct FileChange recovery is missing its conversation owner".to_string()
        })?;
        let recovered_trace = recovered_snapshot.in_progress_trace(
            &durable.run_id,
            conversation_id,
            assistant_message_id,
        );
        let recovered_model_context = recovered_snapshot.committed_prefix().model_context_items;
        let decision_source = if is_exact_auto_journal {
            // The pending action JSON may have advanced from its prepared binding to the
            // reconciled committed binding above. The exact auto identity was frozen and checked
            // before that CAS, so do not compare the older audit JSON with the new binding again.
            existing_audit
                .as_ref()
                .and_then(|audit| audit.decision_source.as_deref())
                .expect("exact automatic FileChange journal has a decision source")
        } else {
            match existing_audit.as_ref() {
                Some(existing)
                    if matches!(
                        existing.decision_source.as_deref(),
                        Some("manual") | Some("manual_pending")
                    ) =>
                {
                    "manual"
                }
                None => "manual",
                Some(_) => continue,
            }
        };
        let mut audit = action_audit_record(
            &record,
            Some("approved"),
            disposition.pending_target_status(),
            Some(&file_change_result),
            None,
            Some(&tool_result),
            tool_result.error.as_deref(),
            Some(durable.updated_at),
            Some(reconciled_at),
        );
        audit.decision_source = Some(decision_source.to_string());
        match storage.commit_pending_agent_action_audited_result_trace_with_model_context(
            &audit,
            "executing",
            disposition.pending_target_status(),
            &recovered_trace,
            &recovered_model_context,
            reconciled_at,
        )? {
            AgentPendingActionResultCommitOutcome::Committed { .. }
            | AgentPendingActionResultCommitOutcome::Idempotent => {}
        }
        recovered = recovered.saturating_add(1);
    }
    Ok(recovered)
}

fn set_recovered_file_change_result_approval(
    snapshot: &mut ConversationTraceSnapshot,
    tool_result: &AgentToolResult,
    approval_status: AgentApprovalStatus,
) -> Result<(), String> {
    match snapshot.items.last_mut() {
        Some(ConversationTurnTraceItem::ToolResult {
            call_id,
            tool,
            approval_status: recovered_status,
            ..
        }) if call_id == &tool_result.call_id && tool == &tool_result.tool => {
            *recovered_status = approval_status;
            Ok(())
        }
        _ => Err("FileChange recovery did not append the exact terminal ToolResult".to_string()),
    }
}

#[allow(clippy::too_many_arguments)]
fn finalize_recovered_manual_direct_delete(
    storage: &StorageService,
    durable: &mut mycopilot_core::storage::models::AgentPendingActionRecord,
    action: &mut AgentProposedAction,
    target: &mycopilot_core::file_change::ResolvedFileChangeTarget,
    finalized_at: u64,
    updated_at: i64,
) -> Result<(), String> {
    use mycopilot_core::storage::service::AgentPendingActionJsonCommitOutcome;

    let Some(finalized_action) = finalized_direct_delete_action(action, target, finalized_at)?
    else {
        return Ok(());
    };
    let expected_action_json = serde_json::to_string(action)
        .map_err(|_| "Direct delete recovery action could not be encoded".to_string())?;
    if durable.action_json != expected_action_json {
        return Err("Direct delete recovery action changed before finalization".to_string());
    }
    let finalized_action_json = serde_json::to_string(&finalized_action)
        .map_err(|_| "finalized Direct delete action could not be encoded".to_string())?;
    let outcome = storage.commit_pending_direct_file_change_action_json(
        durable,
        &expected_action_json,
        &finalized_action_json,
        updated_at,
    )?;
    if !matches!(
        outcome,
        AgentPendingActionJsonCommitOutcome::Updated
            | AgentPendingActionJsonCommitOutcome::AlreadyCommitted
    ) {
        return Err(format!(
            "Direct delete recovery lost its finalization CAS: {outcome:?}"
        ));
    }
    *action = finalized_action;
    durable.action_json = finalized_action_json;
    durable.updated_at = updated_at;
    Ok(())
}

fn finalized_direct_delete_action(
    action: &AgentProposedAction,
    target: &mycopilot_core::file_change::ResolvedFileChangeTarget,
    finalized_at: u64,
) -> Result<Option<AgentProposedAction>, String> {
    use mycopilot_core::file_change::{FileChangeDeleteJournalState, FileChangeOperation};

    let AgentProposedAction::FileChange { file_change } = action else {
        return Ok(None);
    };
    if file_change.execution.transaction.operation != FileChangeOperation::Delete
        || file_change.execution.receipt.is_none()
    {
        return Ok(None);
    }
    let journal = file_change
        .execution
        .delete_journal
        .clone()
        .ok_or_else(|| "committed Direct delete is missing its recovery journal".to_string())?;
    match journal.state {
        FileChangeDeleteJournalState::Finalized => Ok(None),
        FileChangeDeleteJournalState::Tombstoned => {
            let finalization = DirectFileChangeFinalization {
                target: target.clone(),
                journal,
                finalized_at,
            };
            finalize_direct_file_change_action(action, finalization).map(Some)
        }
        FileChangeDeleteJournalState::Prepared | FileChangeDeleteJournalState::RolledBack => {
            Err("committed Direct delete has an invalid recovery journal state".to_string())
        }
    }
}

/// Recreates the original path-policy binding from the frozen display path and canonical target.
///
/// A normal execution resolves a workspace-relative `file_path`, so the committer deliberately
/// binds its receipt to that relative display path. Startup recovery only retains the canonical
/// target and must not resolve it as an absolute display path, because that would produce a
/// different transaction identity. For relative paths, derive the only workspace root that could
/// have produced both frozen values, resolve through the normal policy again, and require the
/// canonical path to match exactly.
fn resolve_direct_file_change_recovery_target(
    canonical_target: &str,
    file_path: &str,
) -> Option<mycopilot_core::file_change::ResolvedFileChangeTarget> {
    use std::path::Component;

    let frozen_path = Path::new(file_path);
    let target = if frozen_path.is_absolute() {
        mycopilot_core::file_change::FileChangePathPolicy::new(None, true)
            .resolve(file_path)
            .ok()?
    } else {
        let mut workspace_root = Path::new(canonical_target).to_path_buf();
        let mut normal_components = 0usize;
        for component in frozen_path.components() {
            match component {
                Component::Normal(_) => {
                    normal_components = normal_components.saturating_add(1);
                    if !workspace_root.pop() {
                        return None;
                    }
                }
                Component::CurDir => {}
                Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
            }
        }
        if normal_components == 0 {
            return None;
        }
        mycopilot_core::file_change::FileChangePathPolicy::new(Some(&workspace_root), false)
            .resolve(file_path)
            .ok()?
    };
    (target.absolute_path().to_string_lossy() == canonical_target).then_some(target)
}

fn retire_unsupported_or_malformed_pending_action(
    storage: &StorageService,
    record: &AgentPendingActionRecord,
    status: PendingActionStatus,
) -> Result<(), String> {
    match status {
        PendingActionStatus::Pending
        | PendingActionStatus::Approved
        | PendingActionStatus::Executing => {}
        PendingActionStatus::Rejected
        | PendingActionStatus::Cancelled
        | PendingActionStatus::Completed
        | PendingActionStatus::Failed => {
            return Err("unsupported_or_malformed_pending_action_has_terminal_status".to_string());
        }
    }
    let changed = storage
        .retire_unsupported_or_malformed_pending_agent_action_on_startup(
            &record.action_id,
            &record.status,
            now_ms(),
        )
        .map_err(|_| "unsupported_or_malformed_pending_action_could_not_be_retired".to_string())?;
    if !changed {
        return Err("unsupported_or_malformed_pending_action_lost_status_cas".to_string());
    }
    Ok(())
}
