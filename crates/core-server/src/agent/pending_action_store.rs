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

impl AgentService {
    pub(super) fn store_pending_action(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<bool, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法发布待审批操作。".to_string());
        }
        let action_id = action_id_for_action(&action);
        let storage_id = pending_action_storage_id(run_id, &action_id);
        let pending_record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id: action_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: normalized_optional(Some(conversation_id)),
                assistant_message_id: normalized_optional(Some(assistant_message_id)),
                action_type: action_type_for_action(&action).to_string(),
                tool_name: tool_name_for_action(&action),
                tool_call_id: Some(action_id_for_action(&action)),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Pending,
            },
            agent_input,
        };

        {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = pending_actions.get(&storage_id) {
                if !same_pending_action_identity(existing, &pending_record) {
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
            }
        }
        let storage_outcome = self.persist_pending_action(&pending_record)?;
        let should_publish = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match pending_actions.get(&storage_id) {
                Some(existing) if same_pending_action_identity(existing, &pending_record) => false,
                Some(existing) => {
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
                None => {
                    pending_actions.insert(storage_id, pending_record.clone());
                    true
                }
            }
        };
        if matches!(storage_outcome, PendingActionStoreOutcome::Inserted) {
            self.record_action_audit(
                &pending_record,
                None,
                "pending",
                None,
                None,
                None,
                None,
                None,
                None,
            );
        }
        drop(deletion_lifecycle);
        Ok(should_publish)
    }

    pub(super) fn transition_pending_status(
        &self,
        record: &PendingActionRecord,
        status: PendingActionStatus,
    ) -> Result<(), String> {
        let action_id = &record.storage_id;
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get_mut(action_id)
            .ok_or_else(|| format!("待审批操作的内存状态不存在：{action_id}"))?;
        let current = pending.snapshot.status;
        if current == status {
            return Ok(());
        }
        ensure_pending_status_transition(current, status)?;
        self.persist_pending_status(pending, current, status)?;
        pending.snapshot.status = status;
        Ok(())
    }

    pub(super) fn persist_pending_action(
        &self,
        record: &PendingActionRecord,
    ) -> Result<PendingActionStoreOutcome, String> {
        self.storage
            .store_pending_agent_action(pending_storage_record(record, now_ms()))
    }

    pub(super) fn persist_pending_status(
        &self,
        record: &PendingActionRecord,
        expected_status: PendingActionStatus,
        status: PendingActionStatus,
    ) -> Result<(), String> {
        self.storage.transition_pending_agent_action(
            &record.storage_id,
            pending_status_label(expected_status),
            pending_status_label(status),
            &persisted_pending_agent_input_json(&record.agent_input, status),
            now_ms(),
        )
    }

    pub(super) fn persist_pending_target_status(
        &self,
        record: &PendingActionRecord,
        target_status: PendingActionStatus,
    ) -> Result<(), String> {
        if !matches!(
            target_status,
            PendingActionStatus::Rejected
                | PendingActionStatus::Cancelled
                | PendingActionStatus::Completed
                | PendingActionStatus::Failed
        ) {
            return Err(format!(
                "待审批操作目标状态必须是终态：{}",
                pending_status_label(target_status)
            ));
        }
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get(&record.storage_id)
            .ok_or_else(|| format!("待审批操作的内存状态不存在：{}", record.storage_id))?;
        self.storage.set_pending_agent_action_target_status(
            &record.storage_id,
            pending_status_label(pending.snapshot.status),
            pending_status_label(target_status),
            now_ms(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_action_audit(
        &self,
        record: &PendingActionRecord,
        decision: Option<&str>,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        decided_at: Option<i64>,
        completed_at: Option<i64>,
    ) {
        if let Err(error) = self.persist_action_audit(
            record,
            decision,
            status,
            patch_result,
            command_result,
            tool_result,
            error,
            decided_at,
            completed_at,
        ) {
            eprintln!("failed to write agent action audit log: {error}");
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_action_audit(
        &self,
        record: &PendingActionRecord,
        decision: Option<&str>,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        decided_at: Option<i64>,
        completed_at: Option<i64>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(error) = take_manual_action_audit_failure(&record.storage_id, status) {
            return Err(error);
        }
        self.storage.upsert_agent_action_audit(action_audit_record(
            record,
            decision,
            status,
            patch_result,
            command_result,
            tool_result,
            error,
            decided_at,
            completed_at,
        ))
    }

    /// Atomically commits the terminal audit, pending target and paired ToolResult trace for any
    /// manually approved file-producing action. `command_result` is populated only for
    /// `run_command`; Office and Skill actions preserve their full execution evidence inside the
    /// typed ToolResult.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_manual_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        completed_at: i64,
    ) -> Result<bool, String> {
        let status = pending_status_label(target_status);
        #[cfg(test)]
        if let Some(error) = take_manual_action_audit_failure(&record.storage_id, status) {
            return Err(error);
        }
        let audit = action_audit_record(
            record,
            Some("approved"),
            status,
            None,
            command_result,
            Some(tool_result),
            tool_result.error.as_deref(),
            None,
            Some(completed_at),
        );
        let outcome = self
            .storage
            .commit_pending_agent_action_audited_result_trace(
                &audit,
                pending_status_label(record.snapshot.status),
                status,
                trace,
                completed_at,
            )?;
        #[cfg(test)]
        if let Some(error) =
            take_manual_action_audit_post_commit_failure(&record.storage_id, status)
        {
            return Err(error);
        }
        match outcome {
            mycopilot_core::storage::service::AgentPendingActionResultCommitOutcome::Committed {
                trace_changed,
            } => Ok(trace_changed),
            mycopilot_core::storage::service::AgentPendingActionResultCommitOutcome::Idempotent => {
                Ok(false)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn inspect_manual_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        let status = pending_status_label(target_status);
        let audit = action_audit_record(
            record,
            Some("approved"),
            status,
            None,
            command_result,
            Some(tool_result),
            tool_result.error.as_deref(),
            None,
            Some(completed_at),
        );
        self.storage
            .inspect_pending_agent_action_audited_result_trace(
                &audit,
                pending_status_label(record.snapshot.status),
                status,
                trace,
                completed_at,
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn record_auto_action_audit(
        &self,
        run_id: &str,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        agent_input: &AgentChatInput,
        action: AgentProposedAction,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        created_at: i64,
        completed_at: i64,
    ) {
        if let Err(error) = self.persist_auto_action_audit(
            run_id,
            conversation_id.as_deref(),
            assistant_message_id.as_deref(),
            agent_input,
            &action,
            status,
            patch_result,
            command_result,
            tool_result,
            error,
            created_at,
            Some(completed_at),
        ) {
            eprintln!("failed to write auto agent action audit log: {error}");
        }
    }

    /// Durably records an automatically approved action audit transition.
    ///
    /// Most legacy auto-approved actions treat audit persistence as best effort through
    /// [`Self::record_auto_action_audit`]. File-producing Office operations use this fallible
    /// boundary directly for non-claim transitions. Office operations and commands use the
    /// stricter claim/finalize methods below so retries cannot overwrite an execution receipt or
    /// repeat side effects after a process restart.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_auto_action_audit(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        agent_input: &AgentChatInput,
        action: &AgentProposedAction,
        status: &str,
        patch_result: Option<&AgentPatchResult>,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: Option<&AgentToolResult>,
        error: Option<&str>,
        created_at: i64,
        completed_at: Option<i64>,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(error) =
            take_auto_action_audit_failure(run_id, &action_id_for_action(action), status)
        {
            return Err(error);
        }
        let audit = auto_action_audit_record(
            run_id,
            conversation_id,
            assistant_message_id,
            agent_input,
            action,
            status,
            patch_result,
            command_result,
            tool_result,
            error,
            created_at,
            completed_at,
        );
        self.storage.upsert_agent_action_audit(audit)
    }

    /// Persists a terminal pre-execution decision without replacing a prior execution receipt.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_auto_action_audit_if_absent(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        agent_input: &AgentChatInput,
        action: &AgentProposedAction,
        status: &str,
        tool_result: &AgentToolResult,
        error: Option<&str>,
        created_at: i64,
        completed_at: i64,
    ) -> Result<bool, String> {
        #[cfg(test)]
        if let Some(error) =
            take_auto_action_audit_failure(run_id, &action_id_for_action(action), status)
        {
            return Err(error);
        }
        let audit = auto_action_audit_record(
            run_id,
            conversation_id,
            assistant_message_id,
            agent_input,
            action,
            status,
            None,
            None,
            Some(tool_result),
            error,
            created_at,
            Some(completed_at),
        );
        self.storage.insert_agent_action_audit_if_absent(audit)
    }

    /// Reads an existing automatic action receipt without acquiring execution rights.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inspect_auto_action_execution_audit(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        agent_input: &AgentChatInput,
        action: &AgentProposedAction,
        created_at: i64,
    ) -> Result<Option<AgentActionAuditExecutionClaimOutcome>, String> {
        let audit = auto_action_audit_record(
            run_id,
            conversation_id,
            assistant_message_id,
            agent_input,
            action,
            "executing",
            None,
            None,
            None,
            None,
            created_at,
            None,
        );
        self.storage.inspect_agent_action_audit_execution(audit)
    }

    /// Atomically acquires the sole durable execution right for an automatically approved action.
    ///
    /// The receipt is committed before this method returns. A matching existing receipt is never
    /// treated as a lease and therefore never permits automatic replay after a crash.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn claim_auto_action_execution_audit(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        agent_input: &AgentChatInput,
        action: &AgentProposedAction,
        created_at: i64,
    ) -> Result<AgentActionAuditExecutionClaimOutcome, String> {
        #[cfg(test)]
        if let Some(error) =
            take_auto_action_audit_failure(run_id, &action_id_for_action(action), "executing")
        {
            return Err(error);
        }
        let audit = auto_action_audit_record(
            run_id,
            conversation_id,
            assistant_message_id,
            agent_input,
            action,
            "executing",
            None,
            None,
            None,
            None,
            created_at,
            None,
        );
        self.storage.claim_agent_action_audit_execution(audit)
    }

    /// Commits the result only if the exact frozen receipt still owns the `executing` state.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn finalize_auto_action_execution_audit(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        agent_input: &AgentChatInput,
        action: &AgentProposedAction,
        status: &str,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        error: Option<&str>,
        created_at: i64,
        completed_at: i64,
    ) -> Result<(), String> {
        let provider_action_id = action_id_for_action(action);
        #[cfg(test)]
        if let Some(error) = take_auto_action_audit_failure(run_id, &provider_action_id, status) {
            return Err(error);
        }
        let audit = auto_action_audit_record(
            run_id,
            conversation_id,
            assistant_message_id,
            agent_input,
            action,
            status,
            None,
            command_result,
            Some(tool_result),
            error,
            created_at,
            Some(completed_at),
        );
        let outcome = self.storage.finalize_agent_action_audit_execution(audit)?;
        #[cfg(test)]
        if outcome == AgentActionAuditFinalizationOutcome::Finalized {
            if let Some(error) =
                take_auto_action_audit_post_commit_failure(run_id, &provider_action_id, status)
            {
                return Err(error);
            }
        }
        match outcome {
            AgentActionAuditFinalizationOutcome::Finalized => Ok(()),
            AgentActionAuditFinalizationOutcome::ClaimMissingOrChanged => Err(format!(
                "automatic action audit claim was missing, terminal, or changed for run={run_id}, action={provider_action_id}"
            )),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn action_audit_record(
    record: &PendingActionRecord,
    decision: Option<&str>,
    status: &str,
    patch_result: Option<&AgentPatchResult>,
    command_result: Option<&AgentCommandExecutionResult>,
    tool_result: Option<&AgentToolResult>,
    error: Option<&str>,
    decided_at: Option<i64>,
    completed_at: Option<i64>,
) -> AgentActionAuditRecord {
    AgentActionAuditRecord {
        action_id: record.storage_id.clone(),
        run_id: record.snapshot.run_id.clone(),
        conversation_id: record.snapshot.conversation_id.clone(),
        assistant_message_id: record.snapshot.assistant_message_id.clone(),
        action_type: record.snapshot.action_type.clone(),
        tool_name: record.snapshot.tool_name.clone(),
        decision: decision.map(ToString::to_string),
        status: status.to_string(),
        action_json: serialize_json(&record.snapshot.action),
        patch_result_json: patch_result.map(serialize_json),
        command_result_json: command_result.map(serialize_json),
        tool_result_json: tool_result.map(serialize_json),
        error: error.map(ToString::to_string),
        created_at: record.snapshot.created_at,
        decided_at,
        completed_at,
        effective_permissions_json: Some(serialize_json(&permissions_from_input(
            &record.agent_input,
        ))),
        path_scope: path_scope_for_action(&record.agent_input, &record.snapshot.action),
        command_cwd_scope: command_cwd_scope_for_action(
            &record.agent_input,
            &record.snapshot.action,
        ),
        blocked_reason: error.map(ToString::to_string),
        decision_source: Some(
            if decision.is_none() && status == "pending" {
                "manual_pending"
            } else {
                "manual"
            }
            .to_string(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn auto_action_audit_record(
    run_id: &str,
    conversation_id: Option<&str>,
    assistant_message_id: Option<&str>,
    agent_input: &AgentChatInput,
    action: &AgentProposedAction,
    status: &str,
    patch_result: Option<&AgentPatchResult>,
    command_result: Option<&AgentCommandExecutionResult>,
    tool_result: Option<&AgentToolResult>,
    error: Option<&str>,
    created_at: i64,
    completed_at: Option<i64>,
) -> AgentActionAuditRecord {
    let provider_action_id = action_id_for_action(action);
    AgentActionAuditRecord {
        action_id: pending_action_storage_id(run_id, &provider_action_id),
        run_id: run_id.to_string(),
        conversation_id: conversation_id.map(ToString::to_string),
        assistant_message_id: assistant_message_id.map(ToString::to_string),
        action_type: action_type_for_action(action).to_string(),
        tool_name: tool_name_for_action(action),
        decision: Some("approved".to_string()),
        status: status.to_string(),
        action_json: serialize_json(action),
        patch_result_json: patch_result.map(serialize_json),
        command_result_json: command_result.map(serialize_json),
        tool_result_json: tool_result.map(serialize_json),
        error: error.map(ToString::to_string),
        created_at,
        decided_at: Some(created_at),
        completed_at,
        effective_permissions_json: Some(serialize_json(&permissions_from_input(agent_input))),
        path_scope: path_scope_for_action(agent_input, action),
        command_cwd_scope: command_cwd_scope_for_action(agent_input, action),
        blocked_reason: error.map(ToString::to_string),
        decision_source: Some("auto".to_string()),
    }
}

pub(super) fn load_persisted_pending_actions(
    storage: &Arc<StorageService>,
) -> Result<HashMap<String, PendingActionRecord>, String> {
    let records = storage
        .list_pending_agent_actions()
        .map_err(|error| format!("failed to load persisted pending actions: {error}"))?;

    records
        .into_iter()
        .map(|record| {
            let action = serde_json::from_str::<AgentProposedAction>(&record.action_json).map_err(
                |error| {
                    format!(
                        "failed to parse persisted pending action {}: {error}",
                        record.action_id
                    )
                },
            )?;
            let agent_input = serde_json::from_str::<AgentChatInput>(&record.agent_input_json)
                .map_err(|error| {
                    format!(
                        "failed to parse persisted pending action input {}: {error}",
                        record.action_id
                    )
                })?;
            let agent_input = restore_agent_input_secrets(storage, agent_input);
            let status = pending_status_from_label(&record.status).ok_or_else(|| {
                format!(
                    "persisted pending action {} has unknown status {}",
                    record.action_id, record.status
                )
            })?;
            let action_id = action_id_for_action(&action);
            let storage_id = record.action_id.clone();
            let snapshot = PendingAgentActionSnapshot {
                action_id,
                action_type: record.action_type,
                tool_name: record.tool_name,
                tool_call_id: record.tool_call_id,
                run_id: record.run_id,
                conversation_id: record.conversation_id,
                assistant_message_id: record.assistant_message_id,
                action,
                created_at: record.created_at,
                status,
            };
            Ok((
                storage_id.clone(),
                PendingActionRecord {
                    storage_id,
                    snapshot,
                    agent_input,
                },
            ))
        })
        .collect()
}

pub(super) fn resolve_pending_action_storage_id(
    pending_actions: &HashMap<String, PendingActionRecord>,
    run_id: &str,
    action_id: &str,
) -> Option<String> {
    let canonical = pending_action_storage_id(run_id, action_id);
    if pending_actions.contains_key(&canonical) {
        return Some(canonical);
    }
    pending_actions.iter().find_map(|(storage_id, record)| {
        (record.snapshot.run_id == run_id && record.snapshot.action_id == action_id)
            .then(|| storage_id.clone())
    })
}

pub(super) fn pending_storage_record(
    record: &PendingActionRecord,
    updated_at: i64,
) -> AgentPendingActionRecord {
    AgentPendingActionRecord {
        action_id: record.storage_id.clone(),
        run_id: record.snapshot.run_id.clone(),
        conversation_id: record.snapshot.conversation_id.clone(),
        assistant_message_id: record.snapshot.assistant_message_id.clone(),
        action_type: record.snapshot.action_type.clone(),
        tool_name: record.snapshot.tool_name.clone(),
        tool_call_id: record.snapshot.tool_call_id.clone(),
        status: pending_status_label(record.snapshot.status).to_string(),
        target_status: None,
        action_json: serialize_json(&record.snapshot.action),
        agent_input_json: persisted_pending_agent_input_json(
            &record.agent_input,
            record.snapshot.status,
        ),
        created_at: record.snapshot.created_at,
        updated_at,
    }
}

pub(super) fn same_pending_action_identity(
    existing: &PendingActionRecord,
    candidate: &PendingActionRecord,
) -> bool {
    existing.storage_id == candidate.storage_id
        && existing.snapshot.action_id == candidate.snapshot.action_id
        && existing.snapshot.run_id == candidate.snapshot.run_id
        && existing.snapshot.conversation_id == candidate.snapshot.conversation_id
        && existing.snapshot.assistant_message_id == candidate.snapshot.assistant_message_id
        && existing.snapshot.action_type == candidate.snapshot.action_type
        && existing.snapshot.tool_name == candidate.snapshot.tool_name
        && existing.snapshot.tool_call_id == candidate.snapshot.tool_call_id
        && existing.snapshot.status == candidate.snapshot.status
        && serialize_json(&existing.snapshot.action) == serialize_json(&candidate.snapshot.action)
        && persisted_pending_agent_input_json(&existing.agent_input, existing.snapshot.status)
            == persisted_pending_agent_input_json(&candidate.agent_input, candidate.snapshot.status)
}

pub(super) fn persisted_pending_agent_input_json(
    agent_input: &AgentChatInput,
    status: PendingActionStatus,
) -> String {
    let mut persisted_agent_input = agent_input.clone();
    persisted_agent_input.api_token.clear();
    // Pending actions must survive restart, while an approved action may still be executing and
    // need its continuation input. Once the action is terminal, the live continuation owns any
    // remaining in-memory copy; the durable row only retains Skill identity and revision metadata.
    if pending_status_redacts_run_scoped_input(status) {
        persisted_agent_input.skill_discovery = None;
        if let Some(activation) = persisted_agent_input.skill_activation.as_mut() {
            for skill in &mut activation.skills {
                skill.instructions.clear();
            }
        }
        if let Some(checkpoint) = persisted_agent_input.resume_checkpoint.as_mut() {
            for item in &mut checkpoint.context_items {
                let is_skill_context = item.sources.iter().any(|source| {
                    matches!(source.as_str(), "skill_instructions" | "skill_catalog")
                }) || item
                    .origin
                    .as_ref()
                    .is_some_and(|origin| origin.kind == "skill");
                if is_skill_context {
                    item.content.clear();
                }
            }
            mycopilot_core::redact_terminal_skill_discovery(checkpoint);
        }
    }
    serialize_json(&persisted_agent_input)
}

pub(super) fn pending_status_redacts_run_scoped_input(status: PendingActionStatus) -> bool {
    match status {
        PendingActionStatus::Pending
        | PendingActionStatus::Approved
        | PendingActionStatus::Executing => false,
        PendingActionStatus::Rejected
        | PendingActionStatus::Cancelled
        | PendingActionStatus::Completed
        | PendingActionStatus::Failed => true,
    }
}

pub(super) fn restore_agent_input_secrets(
    storage: &Arc<StorageService>,
    mut agent_input: AgentChatInput,
) -> AgentChatInput {
    if !agent_input.api_token.trim().is_empty() {
        return agent_input;
    }

    let Ok(Some(settings)) = storage.load_model_settings() else {
        return agent_input;
    };

    let conversation_model_id = agent_input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
        .and_then(|conversation_id| storage.load_conversation(conversation_id).ok().flatten())
        .and_then(|conversation| conversation.model_id);
    let model = conversation_model_id
        .as_deref()
        .and_then(|model_id| settings.models.iter().find(|model| model.id == model_id))
        .or_else(|| {
            settings
                .models
                .iter()
                .find(|model| model.id == agent_input.model)
        });

    // Pending actions deliberately persist without API tokens. On restoration, resolve the
    // same model-specific-or-global pair used by a fresh run so approval continuation cannot
    // silently switch providers after an app restart.
    if let Some(model) = model {
        if let Ok(connection) = settings.effective_connection_for(model) {
            agent_input.api_url = connection.api_url;
            agent_input.api_token = connection.api_token;
        }
    }
    agent_input
}

pub(super) fn agent_input_with_run_checkpoint(
    agent_input: &AgentChatInput,
    checkpoint: &AgentRunCheckpoint,
) -> AgentChatInput {
    let mut resume_input = agent_input.clone();
    resume_input.messages.clear();
    resume_input.attachments.clear();
    resume_input.approval_decision = None;
    resume_input.tool_continuation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input
}

pub(super) fn pending_status_from_label(value: &str) -> Option<PendingActionStatus> {
    match value {
        "pending" => Some(PendingActionStatus::Pending),
        "approved" => Some(PendingActionStatus::Approved),
        "executing" => Some(PendingActionStatus::Executing),
        "rejected" => Some(PendingActionStatus::Rejected),
        "cancelled" => Some(PendingActionStatus::Cancelled),
        "completed" => Some(PendingActionStatus::Completed),
        "failed" => Some(PendingActionStatus::Failed),
        _ => None,
    }
}

pub(super) fn ensure_pending_status_transition(
    current: PendingActionStatus,
    next: PendingActionStatus,
) -> Result<(), String> {
    let allowed = matches!(
        (current, next),
        (PendingActionStatus::Pending, PendingActionStatus::Approved)
            | (PendingActionStatus::Pending, PendingActionStatus::Executing)
            | (PendingActionStatus::Pending, PendingActionStatus::Cancelled)
            | (PendingActionStatus::Approved, PendingActionStatus::Completed)
            | (PendingActionStatus::Approved, PendingActionStatus::Failed)
            | (PendingActionStatus::Approved, PendingActionStatus::Cancelled)
            | (PendingActionStatus::Executing, PendingActionStatus::Completed)
            | (PendingActionStatus::Executing, PendingActionStatus::Failed)
            | (PendingActionStatus::Executing, PendingActionStatus::Rejected)
            | (PendingActionStatus::Executing, PendingActionStatus::Cancelled)
            // Compensating rollback: cancellation is not made visible unless its paired
            // assistant/trace commit succeeds.
            | (PendingActionStatus::Executing, PendingActionStatus::Pending)
    );
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "非法待审批状态迁移：{} -> {}",
            pending_status_label(current),
            pending_status_label(next)
        ))
    }
}

pub(super) fn path_scope_for_action(
    input: &AgentChatInput,
    action: &AgentProposedAction,
) -> Option<String> {
    if let AgentProposedAction::OfficeOperation { office_operation } = action {
        if !office_operation.prepared.paths.is_empty() {
            // A valid Office request is bounded by the provider argument limit. Keep a second
            // defensive bound for persisted/corrupt snapshots so audit generation itself cannot
            // amplify untrusted data. Valid plans retain every slot's purpose and scope.
            const MAX_AUDITED_OFFICE_PATHS: usize = 260;
            let paths = office_operation
                .prepared
                .paths
                .iter()
                .take(MAX_AUDITED_OFFICE_PATHS)
                .map(|path| {
                    serde_json::json!({
                        "slot": path.slot,
                        "purpose": path.purpose,
                        "scope": path.scope,
                    })
                })
                .collect::<Vec<_>>();
            return Some(serialize_json(&serde_json::json!({
                "schemaVersion": 1,
                "totalPathCount": office_operation.prepared.paths.len(),
                "truncated": office_operation.prepared.paths.len() > MAX_AUDITED_OFFICE_PATHS,
                "paths": paths,
            })));
        }
    }

    let path = match action {
        AgentProposedAction::Diff { diff } => Some(diff.file_path.as_str()),
        AgentProposedAction::FileWrite { file_write } => Some(file_write.file_path.as_str()),
        AgentProposedAction::SkillMaterialization { materialization } => {
            Some(materialization.destination.as_str())
        }
        AgentProposedAction::SkillScript { .. } => None,
        AgentProposedAction::OfficeOperation { office_operation } => office_operation
            .prepared
            .request
            .destination_path
            .as_deref()
            .or(office_operation
                .prepared
                .request
                .output_path
                .as_deref()
                .or(office_operation.prepared.request.document_path.as_deref())),
        AgentProposedAction::ToolCall { call } if call.tool == "apply_patch" => call
            .args
            .get("filePath")
            .and_then(serde_json::Value::as_str),
        _ => None,
    }?;
    Some(scope_for_path(input, path))
}

pub(super) fn command_cwd_scope_for_action(
    input: &AgentChatInput,
    action: &AgentProposedAction,
) -> Option<String> {
    let cwd = match action {
        AgentProposedAction::Command { command } => command.cwd.as_deref().unwrap_or("."),
        AgentProposedAction::SkillScript { .. } => ".",
        AgentProposedAction::OfficeOperation { .. } => ".",
        AgentProposedAction::ToolCall { call } if call.tool == "run_command" => call
            .args
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("."),
        _ => return None,
    };
    Some(scope_for_path(input, cwd))
}

pub(super) fn scope_for_path(input: &AgentChatInput, path: &str) -> String {
    let Some(root) = workspace_root_optional(input) else {
        return "no_workspace".to_string();
    };
    let trimmed = path.trim();
    if trimmed.is_empty() || trimmed == "." {
        return "workspace".to_string();
    }
    let path = Path::new(trimmed);
    if path.is_absolute() {
        if path.starts_with(root) {
            "workspace".to_string()
        } else {
            "outside_workspace".to_string()
        }
    } else if trimmed.starts_with('@') {
        "system_alias".to_string()
    } else {
        "workspace".to_string()
    }
}

pub(super) fn agent_input_project_id(input: &AgentChatInput) -> Option<&str> {
    input
        .context
        .as_ref()
        .and_then(|context| context.project_id.as_deref())
}

pub(super) fn agent_input_conversation_id(input: &AgentChatInput) -> Option<&str> {
    input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
}
