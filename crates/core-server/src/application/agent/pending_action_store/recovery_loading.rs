pub(super) fn load_persisted_pending_actions(
    storage: &Arc<StorageService>,
) -> Result<HashMap<String, PendingActionRecord>, String> {
    let records = storage
        .list_recoverable_agent_actions_after_reconciliation()
        .map_err(|error| format!("failed to load persisted pending actions: {error}"))?;

    let mut pending_actions = HashMap::with_capacity(records.len());
    for record in records {
        let status = pending_status_from_label(&record.status).ok_or_else(|| {
            format!(
                "persisted pending action {} has unknown status {}",
                record.action_id, record.status
            )
        })?;
        let decoded_input = match PersistedAgentResumeInput::decode(&record.agent_input_json) {
            Ok(decoded) => decoded,
            Err(_) => {
                retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
                continue;
            }
        };
        let action = match serde_json::from_str::<AgentProposedAction>(&record.action_json) {
            Ok(action) => action,
            Err(_) => {
                retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
                continue;
            }
        };
        if !pending_action_binding_matches(
            &record.run_id,
            Some(&record),
            &action,
            &decoded_input.agent_input,
        ) {
            retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
            continue;
        }
        let agent_input = match restore_agent_input_secrets(storage, decoded_input) {
            Ok(input) => input,
            Err(_) => {
                retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
                continue;
            }
        };
        let action_id = action_id_for_action(&action);
        let storage_id = pending_action_storage_id(&record.run_id, &action_id);
        if record.action_id != storage_id {
            retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
            continue;
        }
        let snapshot = PendingAgentActionSnapshot {
            action_id,
            action_type: record.action_type.clone(),
            tool_name: record.tool_name.clone(),
            tool_call_id: record.tool_call_id.clone(),
            run_id: record.run_id.clone(),
            conversation_id: record.conversation_id.clone(),
            assistant_message_id: record.assistant_message_id.clone(),
            action,
            created_at: record.created_at,
            status,
        };
        let pending_record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot,
            agent_input,
        };
        if tool_call_for_pending_record(&pending_record).is_err() {
            retire_unsupported_or_malformed_pending_action(storage, &record, status)?;
            continue;
        }
        pending_actions.insert(storage_id.clone(), pending_record);
    }
    Ok(pending_actions)
}

/// Retains Run-scoped FileChange authority only for an exact, currently recoverable logical Run.
///
/// A pending remember intent remains non-authority, but is retained when its exact explicit
/// granting action is itself a strict recoverable approval. This lets a pre-effect crash settle
/// that action once and activate only after an applied receipt. Other pending intents are retired.
/// An active grant must retain its authoritative granting receipt and a distinct strict current
/// Pending Action for the same run/conversation/project. A terminal, orphaned, malformed, forked,
/// or new Run therefore cannot inherit it. The startup constructor propagates any storage error,
/// so a failed revocation is retried on the next startup instead of exposing uncertain authority.
pub(super) fn reconcile_file_change_run_grants_on_startup(
    storage: &StorageService,
    pending_actions: &HashMap<String, PendingActionRecord>,
) -> Result<(), String> {
    for grant in storage
        .list_pending_file_change_run_grant_records()
        .map_err(|error| error.to_string())?
    {
        let granting_action_is_recoverable = match pending_actions
            .get(&grant.granting_pending_action_id)
        {
            Some(record) => {
                let current_action_has_no_grant_authority = record
                    .agent_input
                    .resume_checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        checkpoint.pending_action_id.as_deref() == Some(record.storage_id.as_str())
                            && checkpoint.file_change_run_grant_ref.is_none()
                    });
                let Some(context) = record.agent_input.context.as_ref() else {
                    storage
                        .retire_pending_file_change_run_grant(&grant.granting_pending_action_id)
                        .map_err(|error| error.to_string())?;
                    continue;
                };
                let current_file_change = match &record.snapshot.action {
                    AgentProposedAction::FileChange { file_change } => {
                        file_change.id == record.snapshot.action_id
                            && storage
                                .validate_pending_file_change_run_grant_binding(
                                    &grant,
                                    file_change,
                                    context,
                                )
                                .map_err(|error| error.to_string())?
                    }
                    _ => false,
                };
                current_file_change
                    && current_action_has_no_grant_authority
                    && record.snapshot.run_id == grant.run_id
                    && record.snapshot.conversation_id.as_deref()
                        == Some(grant.conversation_id.as_str())
                    && matches!(
                        record.snapshot.status,
                        PendingActionStatus::Pending | PendingActionStatus::Approved
                    )
            }
            None => false,
        };
        if !granting_action_is_recoverable {
            storage
                .retire_pending_file_change_run_grant(&grant.granting_pending_action_id)
                .map_err(|error| error.to_string())?;
        }
    }
    let human_waits = storage
        .list_sync_human_interaction_waits()
        .map_err(|e| e.to_string())?;
    let human_inputs = human_waits
        .iter()
        .filter_map(|(request, envelope)| {
            let encoded = serde_json::to_string(envelope).ok()?;
            let decoded = PersistedAgentResumeInput::decode(&encoded).ok()?;
            let checkpoint = decoded.agent_input.resume_checkpoint.as_ref()?;
            (checkpoint.run_id == request.run_id
                && checkpoint.pending_tool_call_id == request.tool_call_id
                && checkpoint.pause_reason
                    == mycopilot_core::AgentRunCheckpointPauseReason::UserInput)
                .then_some(decoded.agent_input)
        })
        .collect::<Vec<_>>();
    for grant in storage
        .list_active_file_change_run_grant_records()
        .map_err(|error| error.to_string())?
    {
        let receipt_is_authoritative = storage
            .get_active_file_change_run_grant(&grant.run_id)
            .map_err(|error| error.to_string())?
            .is_some_and(|active| active.grant_id == grant.grant_id);
        let run_is_recoverable = pending_actions.values().any(|record| {
            if record.snapshot.run_id != grant.run_id
                || record.snapshot.conversation_id.as_deref()
                    != Some(grant.conversation_id.as_str())
                || !matches!(
                    record.snapshot.status,
                    PendingActionStatus::Pending | PendingActionStatus::Approved
                )
                || record.storage_id == grant.granting_pending_action_id
            {
                return false;
            }
            let Some(context) = record.agent_input.context.as_ref() else {
                return false;
            };
            let checkpoint_matches =
                record
                    .agent_input
                    .resume_checkpoint
                    .as_ref()
                    .is_some_and(|checkpoint| {
                        checkpoint.pending_action_id.as_deref() == Some(record.storage_id.as_str())
                    });
            checkpoint_matches
                && context.conversation_id.as_deref() == Some(grant.conversation_id.as_str())
                && context.project_id.as_deref() == grant.project_id.as_deref()
        });
        let human_run_is_recoverable = human_inputs.iter().any(|input| {
            input
                .resume_checkpoint
                .as_ref()
                .is_some_and(|checkpoint| checkpoint.run_id == grant.run_id)
                && input.context.as_ref().is_some_and(|context| {
                    context.collaboration_identity.is_none()
                        && context.conversation_id.as_deref()
                            == Some(grant.conversation_id.as_str())
                        && context.project_id.as_deref() == grant.project_id.as_deref()
                })
        });
        if !receipt_is_authoritative || !(run_is_recoverable || human_run_is_recoverable) {
            storage
                .revoke_active_file_change_run_grant(&grant.run_id)
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn recovery_file_change_binding(
    action: &AgentProposedAction,
) -> Result<&mycopilot_core::file_change::FileChangeDirectBinding, String> {
    use mycopilot_core::file_change::{FileChangeOperation, FileChangePlan};

    let binding = match action {
        AgentProposedAction::FileChange { file_change } => {
            let execution = &file_change.execution;
            let operation_matches = matches!(
                (file_change.operation, execution.transaction.operation),
                (
                    mycopilot_core::AgentFileChangeOperation::Create,
                    FileChangeOperation::Create
                ) | (
                    mycopilot_core::AgentFileChangeOperation::Update,
                    FileChangeOperation::Update
                ) | (
                    mycopilot_core::AgentFileChangeOperation::Delete,
                    FileChangeOperation::Delete
                )
            );
            let is_staged = execution.staged_transaction_id.is_some();
            let target_content = execution.target_content.as_deref();
            if is_staged && target_content.is_none() {
                return Err("Staged FileChange recovery target content is missing".to_string());
            }
            let byte_count = target_content.map_or(0, |content| content.len() as u64);
            let line_count = target_content.map_or(0, |content| {
                if content.is_empty() {
                    0
                } else {
                    content.lines().count() as u64
                }
            });
            let strategy_matches = match file_change.operation {
                mycopilot_core::AgentFileChangeOperation::Update if is_staged => matches!(
                    file_change.update_strategy,
                    Some(
                        mycopilot_core::AgentFileChangeUpdateStrategy::Modify
                            | mycopilot_core::AgentFileChangeUpdateStrategy::Rewrite
                    )
                ),
                _ => file_change.update_strategy.is_none(),
            };
            if file_change.schema_version != mycopilot_core::file_change::FILE_CHANGE_SCHEMA_VERSION
                || file_change.id != execution.source_call_id
                || execution.source_tool_name != "apply_patch"
                || file_change.transaction_id != execution.transaction.id
                || execution.staged_transaction_id.as_deref()
                    != is_staged.then_some(file_change.transaction_id.as_str())
                || is_staged != execution.staged_transaction_revision.is_some()
                || file_change.file_path != execution.transaction.file_path
                || file_change.base_revision.as_deref() != execution.transaction.base.revision()
                || file_change.additions != execution.proposal.additions
                || file_change.deletions != execution.proposal.deletions
                || file_change.byte_count != byte_count
                || file_change.line_count != line_count
                || !operation_matches
                || !strategy_matches
            {
                return Err("FileChange recovery identity is invalid".to_string());
            }
            if is_staged {
                FileChangePlan::from_binding(execution)
                    .map_err(|_| "Staged FileChange recovery plan is invalid".to_string())?;
            } else {
                let inline_diff = file_change
                    .inline_diff
                    .as_ref()
                    .ok_or_else(|| "Direct FileChange recovery diff is missing".to_string())?;
                if inline_diff.truncated {
                    return Err("Direct FileChange recovery diff is truncated".to_string());
                }
                FileChangePlan::from_direct_binding(execution, inline_diff.patch.clone())
                    .map_err(|_| "Direct FileChange recovery plan is invalid".to_string())?;
            }
            execution
        }
        _ => return Err("action is not a current FileChange".to_string()),
    };
    binding
        .validate()
        .map_err(|_| "FileChange recovery binding is invalid".to_string())?;
    Ok(binding)
}

fn recovery_file_change_plan(
    action: &AgentProposedAction,
) -> Result<mycopilot_core::file_change::FileChangePlan, String> {
    match action {
        AgentProposedAction::FileChange { file_change }
            if file_change.execution.staged_transaction_id.is_some() =>
        {
            mycopilot_core::file_change::FileChangePlan::from_binding(&file_change.execution)
        }
        AgentProposedAction::FileChange { file_change } => {
            let inline_diff = file_change
                .inline_diff
                .as_ref()
                .ok_or_else(|| "Direct FileChange recovery diff is missing".to_string())?;
            if inline_diff.truncated {
                return Err("Direct FileChange recovery diff is truncated".to_string());
            }
            mycopilot_core::file_change::FileChangePlan::from_direct_binding(
                &file_change.execution,
                inline_diff.patch.clone(),
            )
        }
        _ => return Err("action is not a current FileChange".to_string()),
    }
    .map_err(|_| "FileChange recovery plan is invalid".to_string())
}

fn file_change_action_with_binding(
    action: &AgentProposedAction,
    binding: mycopilot_core::file_change::FileChangeDirectBinding,
) -> Result<AgentProposedAction, String> {
    let mut committed = action.clone();
    match &mut committed {
        AgentProposedAction::FileChange { file_change } => *file_change.execution = binding,
        _ => return Err("action is not a current FileChange".to_string()),
    }
    recovery_file_change_binding(&committed)?;
    Ok(committed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecoveredFileChangeDisposition {
    AlreadyApplied,
    DefinitelyNotExecuted,
    OutcomeUnknown,
}

impl RecoveredFileChangeDisposition {
    fn pending_target_status(self) -> &'static str {
        match self {
            Self::AlreadyApplied => "completed",
            Self::DefinitelyNotExecuted | Self::OutcomeUnknown => "failed",
        }
    }

    fn staged_status(self) -> &'static str {
        match self {
            Self::AlreadyApplied => "applied",
            Self::DefinitelyNotExecuted => "failed",
            Self::OutcomeUnknown => "outcome_unknown",
        }
    }
}

fn recovered_file_change_tool_result(
    action: &AgentProposedAction,
    disposition: RecoveredFileChangeDisposition,
) -> Result<(AgentFileChangeResult, AgentToolResult), String> {
    match action {
        AgentProposedAction::FileChange { file_change } => {
            let (status, outcome, revision, error_code, error, message, ok) = match disposition {
                RecoveredFileChangeDisposition::AlreadyApplied => (
                    mycopilot_core::AgentFileChangeResultStatus::AlreadyApplied,
                    mycopilot_core::AgentFileChangeOutcome::Applied,
                    file_change
                        .execution
                        .transaction
                        .target
                        .revision()
                        .map(str::to_string),
                    None,
                    None,
                    Some("文件变更已由目标摘要确认，无需重复执行。".to_string()),
                    true,
                ),
                RecoveredFileChangeDisposition::DefinitelyNotExecuted => (
                    mycopilot_core::AgentFileChangeResultStatus::Failed,
                    mycopilot_core::AgentFileChangeOutcome::DefinitelyNotExecuted,
                    None,
                    Some("failed".to_string()),
                    Some("文件修改未执行。".to_string()),
                    Some(
                        "应用重启后确认目标仍是冻结的修改前版本；为避免意外覆盖，未自动重放。"
                            .to_string(),
                    ),
                    false,
                ),
                RecoveredFileChangeDisposition::OutcomeUnknown => (
                    mycopilot_core::AgentFileChangeResultStatus::OutcomeUnknown,
                    mycopilot_core::AgentFileChangeOutcome::OutcomeUnknown,
                    None,
                    Some("outcome_unknown".to_string()),
                    Some("无法确认文件修改结果，请先检查文件当前状态。".to_string()),
                    Some(
                        "恢复检查无法证明目标等于冻结的修改前或修改后版本；未自动重试或覆盖。"
                            .to_string(),
                    ),
                    false,
                ),
            };
            let result = AgentFileChangeResult {
                schema_version: mycopilot_core::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
                status,
                outcome,
                transaction_id: file_change.transaction_id.clone(),
                operation: file_change.operation,
                update_strategy: file_change.update_strategy,
                file_path: file_change.file_path.clone(),
                additions: file_change.additions,
                deletions: file_change.deletions,
                line_count: file_change.line_count,
                byte_count: file_change.byte_count,
                revision,
                error_code,
                error,
                message,
            };
            result
                .validate()
                .map_err(|_| "FileChange recovery result is invalid".to_string())?;
            let tool_result = file_change_tool_result(&file_change.id, ok, &result);
            Ok((result, tool_result))
        }
        _ => Err("action is not a current FileChange".to_string()),
    }
}

fn staged_file_change_recovery_record(
    storage: &StorageService,
    action: &AgentProposedAction,
    agent_input: Option<&AgentChatInput>,
) -> Result<Option<mycopilot_core::storage::models::AgentFileChangeRecord>, String> {
    let AgentProposedAction::FileChange { file_change } = action else {
        return Ok(None);
    };
    let binding = &file_change.execution;
    if binding.staged_transaction_id.is_none() {
        return Ok(None);
    }
    let (conversation_id, project_id) = if let Some(input) = agent_input {
        let context = input
            .context
            .as_ref()
            .ok_or_else(|| "Staged FileChange recovery owner is missing".to_string())?;
        let conversation_id = context
            .conversation_id
            .as_deref()
            .ok_or_else(|| "Staged FileChange recovery conversation is missing".to_string())?;
        if conversation_id != binding.conversation_id
            || context.project_id.as_deref() != binding.project_id.as_deref()
        {
            return Err("Staged FileChange recovery owner is invalid".to_string());
        }
        (conversation_id, context.project_id.as_deref())
    } else {
        (
            binding.conversation_id.as_str(),
            binding.project_id.as_deref(),
        )
    };
    let mut record = storage
        .get_agent_file_change_for_owner(
            &file_change.transaction_id,
            conversation_id,
            project_id,
            &binding.run_id,
            &binding.source_tool_name,
        )?
        .ok_or_else(|| "Staged FileChange recovery transaction is missing".to_string())?;
    let expected_revision = binding
        .staged_transaction_revision
        .ok_or_else(|| "Staged FileChange recovery revision is missing".to_string())?;
    let expected_operation = match binding.transaction.operation {
        mycopilot_core::file_change::FileChangeOperation::Create => "create",
        mycopilot_core::file_change::FileChangeOperation::Update => "update",
        mycopilot_core::file_change::FileChangeOperation::Delete => {
            return Err("Staged delete is not supported".to_string());
        }
    };
    let strategy_matches = matches!(
        (
            file_change.operation,
            file_change.update_strategy,
            record.strategy.as_deref()
        ),
        (mycopilot_core::AgentFileChangeOperation::Create, None, None)
            | (
                mycopilot_core::AgentFileChangeOperation::Update,
                Some(mycopilot_core::AgentFileChangeUpdateStrategy::Modify),
                Some("modify")
            )
            | (
                mycopilot_core::AgentFileChangeOperation::Update,
                Some(mycopilot_core::AgentFileChangeUpdateStrategy::Rewrite),
                Some("rewrite")
            )
    );
    let observation_matches = serde_json::from_str::<
        mycopilot_core::file_change::FileObservationCheckpoint,
    >(&record.observation_json)
    .ok()
    .as_ref()
        == Some(&binding.observation);
    if record.schema_version
        != mycopilot_core::storage::file_change_repository::AGENT_FILE_CHANGE_SCHEMA_VERSION
        || !matches!(record.status.as_str(), "applying" | "applied")
        || record.final_action_id.as_deref() != Some(file_change.id.as_str())
        || record.final_action_arguments_digest.as_deref()
            != Some(binding.source_args_digest.as_str())
        || record.draft_revision != expected_revision
        || record.next_mutation_index != expected_revision
        || record.operation != expected_operation
        || !strategy_matches
        || record.file_path != file_change.file_path
        || record.base_revision.as_deref() != file_change.base_revision.as_deref()
        || record.base_content != binding.base_content.as_deref().unwrap_or_default()
        || binding.target_content.as_deref() != Some(record.content.as_str())
        || record.additions != file_change.additions
        || record.deletions != file_change.deletions
        || record.line_count != file_change.line_count
        || record.byte_count != file_change.byte_count
        || record.final_permission_revision.as_deref() != Some(binding.permission_revision.as_str())
        || record.final_tool_set_revision.as_deref() != Some(binding.tool_set_revision.as_str())
        || record.final_provider_wire_revision.as_deref()
            != Some(binding.provider_wire_revision.as_str())
        || record.observation_id != binding.observation_id
        || !observation_matches
    {
        return Err("Staged FileChange recovery transaction is invalid".to_string());
    }
    record.stats_final = true;
    Ok(Some(record))
}

fn settle_recovered_staged_file_change(
    storage: &StorageService,
    record: Option<&mut mycopilot_core::storage::models::AgentFileChangeRecord>,
    disposition: RecoveredFileChangeDisposition,
    updated_at: i64,
) -> Result<(), String> {
    let Some(record) = record else {
        return Ok(());
    };
    let terminal_status = disposition.staged_status();
    if record.status == terminal_status {
        return Ok(());
    }
    let expected_revision = record.draft_revision;
    let expected_index = record.next_mutation_index;
    record.status = terminal_status.to_string();
    record.stats_final = true;
    record.updated_at = updated_at;
    if storage.transition_agent_file_change(
        "applying",
        expected_revision,
        expected_index,
        record,
    )? {
        Ok(())
    } else {
        Err("Staged FileChange recovery lost its exact transaction CAS".to_string())
    }
}
