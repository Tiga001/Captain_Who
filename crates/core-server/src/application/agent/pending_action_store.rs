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

impl AgentService {
    /// Persists the hidden pre-dispatch journal for one automatically authorized MCP call.
    ///
    /// The row starts at `approved`, so it is never published through the pending-approval API.
    /// Only a subsequent durable `approved -> executing` CAS permits the transport invocation.
    pub(super) fn prepare_auto_mcp_action_journal(
        &self,
        run_id: &str,
        conversation_id: Option<&str>,
        assistant_message_id: Option<&str>,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<PendingActionRecord, String> {
        let (action_id, call_id) = match &action {
            AgentProposedAction::McpToolCall { approval }
                if approval.approval_mode == mycopilot_core::AgentMcpApprovalMode::Auto
                    && approval.call.approval_status == AgentApprovalStatus::Approved =>
            {
                (
                    approval.identity.action_id.clone(),
                    approval.identity.call_id.clone(),
                )
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval }
                if approval.approval_status == AgentApprovalStatus::Approved =>
            {
                (
                    approval.identity.action_id.clone(),
                    approval.identity.call_id.clone(),
                )
            }
            _ => {
                return Err(
                    "automatic MCP journal requires a Host-authorized typed MCP action".to_string(),
                )
            }
        };
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法启动 MCP 操作。".to_string());
        }
        if !pending_action_binding_matches_for_auto_journal(run_id, None, &action, &agent_input) {
            return Err("automatic MCP journal frozen identity is inconsistent".to_string());
        }

        let storage_id = pending_action_storage_id(run_id, &action_id);
        let record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id,
                run_id: run_id.to_string(),
                conversation_id: normalized_optional(conversation_id),
                assistant_message_id: normalized_optional(assistant_message_id),
                action_type: action_type_for_action(&action).to_string(),
                tool_name: tool_name_for_action(&action),
                tool_call_id: Some(call_id),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Approved,
            },
            agent_input,
        };

        {
            let pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if pending_actions.contains_key(&storage_id) {
                return Err("automatic MCP invocation journal already exists".to_string());
            }
        }
        let outcome = self.persist_pending_action(&record)?;
        if outcome != PendingActionStoreOutcome::Inserted {
            return Err(
                "automatic MCP invocation was already journaled and will not be replayed"
                    .to_string(),
            );
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match pending_actions.entry(storage_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record.clone());
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err("automatic MCP invocation journal raced another dispatch".to_string());
            }
        }
        drop(pending_actions);
        if self
            .persist_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(record.snapshot.created_at),
                None,
            )
            .is_err()
        {
            let _ = self.settle_auto_mcp_action_journal(
                &record,
                McpAutoActionJournalTerminalOutcome::Failed,
                None,
            );
            return Err(
                "automatic MCP invocation journal audit could not be persisted".to_string(),
            );
        }
        drop(deletion_lifecycle);
        Ok(record)
    }

    /// Claims the durable possibly-dispatched boundary immediately before transport invocation.
    pub(super) fn claim_auto_mcp_dispatch(
        &self,
        record: &mut PendingActionRecord,
    ) -> Result<(), String> {
        if record.snapshot.status != PendingActionStatus::Approved {
            return Err("automatic MCP journal is not ready for dispatch".to_string());
        }
        self.transition_pending_status(record, PendingActionStatus::Executing)?;
        record.snapshot.status = PendingActionStatus::Executing;
        Ok(())
    }

    /// Persists the hidden, resumable FileChange journal for an automatically approved action.
    ///
    /// This is the same current Pending Action shape used by manual approval. It is intentionally
    /// not published to the approval UI, but still owns the exact ToolCall, frozen checkpoint,
    /// normalized runtime binding, and canonical pending id before any filesystem side effect.
    #[cfg(test)]
    pub(super) fn prepare_auto_file_change_action_journal(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<PendingActionRecord, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        self.prepare_auto_file_change_action_journal_under_deletion_guard(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            &deletion_lifecycle,
        )
    }

    /// Variant for callers that already hold the deletion lifecycle mutex across the complete
    /// FileChange execution boundary. Passing the guarded state keeps the ownership check explicit
    /// and avoids recursively locking the non-reentrant mutex.
    pub(super) fn prepare_auto_file_change_action_journal_under_deletion_guard(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        deletion_lifecycle: &DeletionLifecycleState,
    ) -> Result<PendingActionRecord, String> {
        let AgentProposedAction::FileChange { file_change } = &action else {
            return Err("automatic FileChange journal requires a current FileChange action".into());
        };
        if file_change.approval_status != AgentApprovalStatus::Approved
            || file_change.schema_version != mycopilot_core::file_change::FILE_CHANGE_SCHEMA_VERSION
            || file_change.execution.source_tool_name != "apply_patch"
            || file_change.execution.source_call_id != file_change.id
            || file_change.execution.run_id != run_id
            || file_change.execution.conversation_id != conversation_id
            || file_change.execution.validate().is_err()
        {
            return Err("automatic FileChange journal frozen identity is invalid".into());
        }
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法启动文件修改。".to_string());
        }
        if !pending_action_binding_matches_for_auto_journal(run_id, None, &action, &agent_input) {
            return Err("automatic FileChange journal checkpoint identity is inconsistent".into());
        }

        let action_id = file_change.id.clone();
        let storage_id = pending_action_storage_id(run_id, &action_id);
        let created_at = now_ms();
        let record = PendingActionRecord {
            storage_id: storage_id.clone(),
            snapshot: PendingAgentActionSnapshot {
                action_id: action_id.clone(),
                run_id: run_id.to_string(),
                conversation_id: Some(conversation_id.to_string()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                tool_call_id: Some(action_id),
                action,
                created_at,
                status: PendingActionStatus::Approved,
            },
            agent_input,
        };
        tool_call_for_pending_record(&record)
            .map_err(|_| "automatic FileChange journal exact ToolCall is invalid".to_string())?;
        let pending = pending_storage_record(&record, created_at)?;
        let audit = auto_action_audit_record(
            run_id,
            Some(conversation_id),
            Some(assistant_message_id),
            &record.agent_input,
            &record.snapshot.action,
            "approved",
            None,
            None,
            None,
            None,
            created_at,
            None,
        );
        let outcome = self
            .storage
            .store_auto_file_change_pending_action_with_audit(pending, audit)?;
        if !matches!(
            outcome,
            PendingActionStoreOutcome::Inserted | PendingActionStoreOutcome::Idempotent
        ) {
            return Err("automatic FileChange journal identity conflict".to_string());
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match pending_actions.entry(storage_id) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(record.clone());
            }
            std::collections::hash_map::Entry::Occupied(entry)
                if same_pending_action_identity(entry.get(), &record) => {}
            std::collections::hash_map::Entry::Occupied(_) => {
                return Err("automatic FileChange in-memory journal identity conflict".into());
            }
        }
        drop(pending_actions);
        Ok(record)
    }

    /// Atomically claims both durable automatic FileChange journals immediately before commit.
    /// `Ok(true)` is the only result that permits crossing the filesystem effect boundary.
    pub(super) fn claim_auto_file_change_dispatch(
        &self,
        record: &mut PendingActionRecord,
    ) -> Result<bool, String> {
        if record.snapshot.status != PendingActionStatus::Approved {
            return Ok(false);
        }
        let approved_pending = pending_storage_record(record, now_ms())?;
        let approved_audit = auto_action_audit_record(
            &record.snapshot.run_id,
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            &record.agent_input,
            &record.snapshot.action,
            "approved",
            None,
            None,
            None,
            None,
            record.snapshot.created_at,
            None,
        );
        let executing_agent_input_json = persisted_pending_agent_input_json(
            &record.agent_input,
            PendingActionStatus::Executing,
        )?;
        let expected_run_grant_ref = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref());
        let expected_file_change = match (&record.snapshot.action, expected_run_grant_ref) {
            (AgentProposedAction::FileChange { file_change }, Some(_)) => Some(file_change),
            (_, Some(_)) => {
                return Err(
                    "automatic FileChange grant reference is attached to another action"
                        .to_string(),
                )
            }
            (_, None) => None,
        };
        let expected_run_context = if expected_run_grant_ref.is_some() {
            Some(record.agent_input.context.as_ref().ok_or_else(|| {
                "automatic FileChange grant authority has no frozen Run context".to_string()
            })?)
        } else {
            None
        };
        let claimed = self.storage.claim_auto_file_change_pending_execution(
            &approved_pending,
            &approved_audit,
            expected_run_grant_ref,
            expected_file_change,
            expected_run_context,
            &executing_agent_input_json,
            now_ms(),
        )?;
        if !claimed {
            return Ok(false);
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = pending_actions
            .get_mut(&record.storage_id)
            .ok_or_else(|| "automatic FileChange in-memory journal is missing".to_string())?;
        if !same_pending_action_identity_except_status(current, record)
            || current.snapshot.status != PendingActionStatus::Approved
        {
            return Err(
                "automatic FileChange in-memory journal changed after durable claim".into(),
            );
        }
        current.snapshot.status = PendingActionStatus::Executing;
        record.snapshot.status = PendingActionStatus::Executing;
        Ok(true)
    }

    /// Persists the authoritative automatic FileChange receipt before the ToolResult is returned
    /// to the model loop. A crash between any of these monotonic writes leaves the executing
    /// Pending Action recoverable; no caller may manufacture success from an incomplete receipt.
    pub(super) fn finalize_auto_file_change_action_journal(
        &self,
        record: &mut PendingActionRecord,
        execution: &ActionExecutionDecision,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        if record.snapshot.status != PendingActionStatus::Executing
            || !matches!(
                execution.final_pending_status,
                PendingActionStatus::Completed | PendingActionStatus::Failed
            )
        {
            return Err("automatic FileChange terminal journal state is invalid".to_string());
        }
        let completed_at = now_ms();
        let call = tool_call_for_pending_record(record)
            .map_err(|_| "automatic FileChange exact ToolCall is invalid".to_string())?;
        let mut settled_input = record.agent_input.clone();
        settled_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.storage_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        settled_input.tool_continuation = Some(AgentToolContinuation {
            call,
            result: execution.tool_result.clone(),
        });
        let mut commit_errors = Vec::new();
        let mut committed = false;
        for _ in 0..2 {
            match self.commit_auto_file_change_audited_result_trace_with_continuation(
                record,
                &settled_input,
                execution.final_pending_status,
                completed_at,
                notifications,
            ) {
                Ok(()) => {
                    committed = true;
                    break;
                }
                Err(error) => commit_errors.push(error),
            }
        }
        if !committed {
            return Err(commit_errors.join("; retry: "));
        }
        self.transition_pending_status(record, execution.final_pending_status)?;
        record.snapshot.status = execution.final_pending_status;
        Ok(())
    }

    /// Terminalizes and removes one hidden automatic MCP journal without persisting Tool output.
    pub(super) fn settle_auto_mcp_action_journal(
        &self,
        record: &PendingActionRecord,
        outcome: McpAutoActionJournalTerminalOutcome,
        invocation: Option<&mycopilot_core::AgentMcpToolInvocationEvent>,
    ) -> Result<(), String> {
        let expected_status = pending_status_label(record.snapshot.status);
        let settled = self.storage.settle_auto_mcp_action_journal(
            &record.storage_id,
            expected_status,
            outcome,
            invocation,
            now_ms(),
        )?;
        if !settled {
            return Err("automatic MCP journal terminal CAS was lost".to_string());
        }
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending_actions
            .get(&record.storage_id)
            .is_some_and(|current| same_pending_action_identity_except_status(current, record))
        {
            pending_actions.remove(&record.storage_id);
        }
        Ok(())
    }

    pub(super) fn store_pending_action(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
    ) -> Result<bool, String> {
        self.store_pending_action_internal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn store_pending_action_with_predecessor_settlement(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        predecessor: &PendingActionRecord,
        predecessor_terminal_status: PendingActionStatus,
    ) -> Result<bool, String> {
        self.store_pending_action_internal(
            run_id,
            conversation_id,
            assistant_message_id,
            action,
            agent_input,
            Some((predecessor, predecessor_terminal_status)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn store_pending_action_internal(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        action: AgentProposedAction,
        agent_input: AgentChatInput,
        predecessor_settlement: Option<(&PendingActionRecord, PendingActionStatus)>,
    ) -> Result<bool, String> {
        let agent_input = bind_pending_provider_configuration(&self.storage, agent_input)?;
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&agent_input) {
            return Err("项目或会话正在移除，无法发布待审批操作。".to_string());
        }
        if !pending_action_binding_matches(run_id, None, &action, &agent_input) {
            self.invalidate_mcp_pending_payload(&action);
            return Err("Pending action frozen Tool Call identity is inconsistent.".to_string());
        }
        let action_id = action_id_for_action(&action);
        let tool_call_id = match &action {
            AgentProposedAction::McpToolCall { approval } => approval.identity.call_id.clone(),
            AgentProposedAction::BuiltinCapabilityActivation { approval } => {
                approval.call_id.clone()
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                approval.identity.call_id.clone()
            }
            _ => action_id.clone(),
        };
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
                tool_call_id: Some(tool_call_id),
                action,
                created_at: now_ms(),
                status: PendingActionStatus::Pending,
            },
            agent_input,
        };
        tool_call_for_pending_record(&pending_record)
            .map_err(|_| "Pending action frozen Tool Call identity is inconsistent.".to_string())?;
        let approval_notification = self.human_root_approval_notification(
            run_id,
            conversation_id,
            assistant_message_id,
            &action_id,
            pending_record.snapshot.created_at,
        )?;

        if let Some((predecessor, terminal_status)) = predecessor_settlement {
            let mut pending_actions = self
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
            let predecessor_current =
                pending_actions
                    .get(&predecessor.storage_id)
                    .ok_or_else(|| {
                        format!("前置待审批操作的内存状态不存在：{}", predecessor.storage_id)
                    })?;
            if !same_pending_action_identity_except_status(predecessor_current, predecessor) {
                return Err("前置待审批操作的冻结身份已经变化。".to_string());
            }
            let predecessor_expected_status = predecessor_current.snapshot.status;
            let predecessor_terminal_agent_input_json = persisted_pending_agent_input_json(
                &predecessor_current.agent_input,
                terminal_status,
            )?;
            let successor_storage_record = pending_storage_record(&pending_record, now_ms())?;
            let builtin_initial_audit = matches!(
                &pending_record.snapshot.action,
                AgentProposedAction::BuiltinCapabilityActivation { .. }
            )
            .then(|| {
                action_audit_record(
                    &pending_record,
                    None,
                    "pending",
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
            });
            let updated_at = now_ms();
            let storage_result = match (builtin_initial_audit, approval_notification.as_ref()) {
                (Some(audit), Some(notification)) => self
                    .storage
                    .store_builtin_capability_pending_action_with_predecessor_settlement_audit_and_notification(
                        successor_storage_record,
                        audit,
                        notification,
                        &predecessor.storage_id,
                        &predecessor.snapshot.action_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (Some(audit), None) => self.storage
                    .store_builtin_capability_pending_action_with_predecessor_settlement_and_audit(
                        successor_storage_record,
                        audit,
                        &predecessor.storage_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (None, Some(notification)) => self.storage
                    .store_pending_agent_action_with_predecessor_settlement_and_notification(
                        successor_storage_record,
                        notification,
                        &predecessor.storage_id,
                        &predecessor.snapshot.action_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
                (None, None) => self.storage
                    .store_pending_agent_action_with_predecessor_settlement(
                        successor_storage_record,
                        &predecessor.storage_id,
                        pending_status_label(predecessor_expected_status),
                        pending_status_label(terminal_status),
                        &predecessor_terminal_agent_input_json,
                        updated_at,
                    ),
            };
            let storage_outcome = match storage_result {
                Ok(outcome) => outcome,
                Err(error) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                    return Err(error);
                }
            };

            let should_publish = match pending_actions.get(&storage_id) {
                Some(existing) if same_pending_action_identity(existing, &pending_record) => false,
                Some(existing) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                    return Err(format!(
                        "待审批操作 actionId={action_id} 与内存中的冻结快照冲突（existingRunId={}，candidateRunId={}）。",
                        existing.snapshot.run_id, pending_record.snapshot.run_id
                    ));
                }
                None => {
                    pending_actions.insert(storage_id, pending_record.clone());
                    true
                }
            };
            pending_actions
                .get_mut(&predecessor.storage_id)
                .expect("predecessor was validated under the same pending-action lock")
                .snapshot
                .status = terminal_status;
            drop(pending_actions);
            if matches!(storage_outcome, PendingActionStoreOutcome::Inserted)
                && !matches!(
                    &pending_record.snapshot.action,
                    AgentProposedAction::BuiltinCapabilityActivation { .. }
                )
            {
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
            return Ok(should_publish);
        }

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
        let storage_result = if matches!(
            &pending_record.snapshot.action,
            AgentProposedAction::BuiltinCapabilityActivation { .. }
        ) {
            let pending = pending_storage_record(&pending_record, now_ms())?;
            let audit = action_audit_record(
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
            if let Some(notification) = approval_notification.as_ref() {
                self.storage
                    .store_builtin_capability_pending_action_with_audit_and_notification(
                        pending,
                        audit,
                        notification,
                    )
            } else {
                self.storage
                    .store_builtin_capability_pending_action_with_audit(pending, audit)
            }
        } else {
            let pending = pending_storage_record(&pending_record, now_ms())?;
            if let Some(notification) = approval_notification.as_ref() {
                self.storage
                    .store_pending_agent_action_with_notification(pending, notification)
            } else {
                self.storage.store_pending_agent_action(pending)
            }
        };
        let storage_outcome = match storage_result {
            Ok(outcome) => outcome,
            Err(error) => {
                self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
                return Err(error);
            }
        };
        let should_publish = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            match pending_actions.get(&storage_id) {
                Some(existing) if same_pending_action_identity(existing, &pending_record) => false,
                Some(existing) => {
                    self.invalidate_mcp_pending_payload(&pending_record.snapshot.action);
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
        if matches!(storage_outcome, PendingActionStoreOutcome::Inserted)
            && !matches!(
                &pending_record.snapshot.action,
                AgentProposedAction::BuiltinCapabilityActivation { .. }
            )
        {
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

    pub(super) fn invalidate_mcp_pending_payload(&self, action: &AgentProposedAction) {
        match action {
            AgentProposedAction::McpToolCall { approval } => {
                if let Some(invoker) = self.mcp_tool_invoker.as_ref() {
                    let _ = invoker.invalidate_prepared_approval(&approval.identity);
                }
            }
            AgentProposedAction::BuiltinMcpToolApproval { approval } => {
                if let Some(runtime) = self.builtin_capabilities.as_ref() {
                    // Idempotent invalidation is shared by cancellation, expiry, storage failure,
                    // deletion and shutdown. No path may leave process-sealed arguments or a
                    // single-use grant live after the durable pending action stops being usable.
                    let _ = runtime.dismiss_builtin_mcp_tool_approval(approval);
                }
            }
            _ => {}
        }
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

    /// Commits the cancellation status and retires every remembered FileChange grant for the Run
    /// in one SQLite transaction before process-local state can report a terminal cancellation.
    pub(super) fn transition_pending_status_to_cancelled_and_revoke_run_grants(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get_mut(&record.storage_id)
            .ok_or_else(|| format!("待审批操作的内存状态不存在：{}", record.storage_id))?;
        let current = pending.snapshot.status;
        if current == PendingActionStatus::Cancelled {
            return Ok(());
        }
        ensure_pending_status_transition(current, PendingActionStatus::Cancelled)?;
        self.storage
            .transition_cancelled_pending_agent_action_and_revoke_file_change_run_grants(
                &record.storage_id,
                pending_status_label(current),
                &persisted_pending_agent_input_json(
                    &pending.agent_input,
                    PendingActionStatus::Cancelled,
                )?,
                &record.snapshot.run_id,
                &record.snapshot.action_id,
                now_ms(),
            )?;
        pending.snapshot.status = PendingActionStatus::Cancelled;
        Ok(())
    }

    pub(super) fn persist_pending_action(
        &self,
        record: &PendingActionRecord,
    ) -> Result<PendingActionStoreOutcome, String> {
        self.storage
            .store_pending_agent_action(pending_storage_record(record, now_ms())?)
    }

    /// Advances one manual Direct FileChange from its prepared credential to the exact durable
    /// commit receipt. Only this monotonic private-action transition is permitted while the row
    /// remains `executing`; public approval fields and the original ToolCall stay immutable.
    pub(super) fn commit_pending_file_change_action(
        &self,
        record: &mut PendingActionRecord,
        committed_action: AgentProposedAction,
    ) -> Result<(), String> {
        use mycopilot_core::storage::service::AgentPendingActionJsonCommitOutcome;

        let prepared_action = record.snapshot.action.clone();
        let expected_action_json = serde_json::to_string(&prepared_action)
            .map_err(|_| "Direct FileChange prepared action could not be encoded".to_string())?;
        let committed_action_json = serde_json::to_string(&committed_action)
            .map_err(|_| "Direct FileChange commit receipt could not be encoded".to_string())?;
        let updated_at = now_ms();
        let identity = pending_storage_record(record, updated_at)?;
        let outcome = self.storage.commit_pending_direct_file_change_action_json(
            &identity,
            &expected_action_json,
            &committed_action_json,
            updated_at,
        )?;
        if !matches!(
            outcome,
            AgentPendingActionJsonCommitOutcome::Updated
                | AgentPendingActionJsonCommitOutcome::AlreadyCommitted
        ) {
            return Err(format!(
                "Direct FileChange commit receipt could not acquire its exact pending-action CAS: {outcome:?}"
            ));
        }

        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let current = pending_actions.get_mut(&record.storage_id).ok_or_else(|| {
            "Direct FileChange pending action disappeared after its commit receipt was persisted"
                .to_string()
        })?;
        let current_action_json = serde_json::to_string(&current.snapshot.action)
            .map_err(|_| "Direct FileChange in-memory action could not be encoded".to_string())?;
        if current.snapshot.status != PendingActionStatus::Executing
            || (current_action_json != expected_action_json
                && current_action_json != committed_action_json)
        {
            return Err(
                "Direct FileChange in-memory pending identity changed after durable commit"
                    .to_string(),
            );
        }
        current.snapshot.action = committed_action.clone();
        record.snapshot.action = committed_action;
        Ok(())
    }

    pub(super) fn persist_pending_status(
        &self,
        record: &PendingActionRecord,
        expected_status: PendingActionStatus,
        status: PendingActionStatus,
    ) -> Result<(), String> {
        #[cfg(test)]
        if let Some(error) =
            take_pending_status_transition_failure(&record.storage_id, pending_status_label(status))
        {
            return Err(error);
        }
        self.storage
            .transition_pending_agent_action_and_resolve_notification(
                &record.storage_id,
                pending_status_label(expected_status),
                pending_status_label(status),
                &persisted_pending_agent_input_json(&record.agent_input, status)?,
                &record.snapshot.run_id,
                &record.snapshot.action_id,
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
        file_change_result: Option<&AgentFileChangeResult>,
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
            file_change_result,
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
        file_change_result: Option<&AgentFileChangeResult>,
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
            file_change_result,
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
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<bool, String> {
        self.persist_manual_audited_result_trace_for_decision(
            record,
            "approved",
            "manual",
            target_status,
            command_result,
            tool_result,
            trace,
            model_context_items,
            completed_at,
        )
    }

    /// Commits an explicit pre-dispatch MCP rejection using the same immutable
    /// audit/target/ToolResult boundary as an approved command result.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_rejected_mcp_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<bool, String> {
        self.persist_manual_audited_result_trace_for_decision(
            record,
            "rejected",
            "manual",
            PendingActionStatus::Rejected,
            None,
            tool_result,
            trace,
            model_context_items,
            completed_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_manual_audited_result_trace_for_decision(
        &self,
        record: &PendingActionRecord,
        decision: &str,
        decision_source: &str,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<bool, String> {
        let status = pending_status_label(target_status);
        #[cfg(test)]
        let injected_failure = if matches!(decision_source, "auto" | "run_grant") {
            take_auto_action_audit_failure(
                &record.snapshot.run_id,
                &record.snapshot.action_id,
                status,
            )
        } else {
            take_manual_action_audit_failure(&record.storage_id, status)
        };
        #[cfg(test)]
        if let Some(error) = injected_failure {
            return Err(error);
        }
        // A rejection is itself the user's terminal decision, so its timestamp must describe
        // that click rather than fall back to the proposal's creation time. Approved actions
        // retain the original approval timestamp already stored by the lifecycle transaction.
        let decided_at = (decision == "rejected").then_some(completed_at);
        let file_change_result = file_change_result_for_audit(record, tool_result)?;
        let mut audit = action_audit_record(
            record,
            Some(decision),
            status,
            file_change_result.as_ref(),
            command_result,
            Some(tool_result),
            tool_result.error.as_deref(),
            decided_at,
            Some(completed_at),
        );
        audit.decision_source = Some(decision_source.to_string());
        let outcome = self
            .storage
            .commit_pending_agent_action_audited_result_trace_with_model_context(
                &audit,
                pending_status_label(record.snapshot.status),
                status,
                trace,
                model_context_items,
                completed_at,
            )?;
        #[cfg(test)]
        let injected_post_commit_failure = if matches!(decision_source, "auto" | "run_grant") {
            take_auto_action_audit_post_commit_failure(
                &record.snapshot.run_id,
                &record.snapshot.action_id,
                status,
            )
        } else {
            take_manual_action_audit_post_commit_failure(&record.storage_id, status)
        };
        #[cfg(test)]
        if let Some(error) = injected_post_commit_failure {
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

    /// Atomically commits an automatically approved FileChange's terminal receipt together with
    /// its exact ToolResult trace and model-context projection. This shares the same strict
    /// settlement transaction as manual approval while preserving `decision_source=auto`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn persist_auto_file_change_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        target_status: PendingActionStatus,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<bool, String> {
        if !matches!(
            record.snapshot.action,
            AgentProposedAction::FileChange { .. }
        ) {
            return Err("automatic FileChange settlement requires a FileChange action".into());
        }
        let decision_source = if record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref())
            .is_some()
        {
            "run_grant"
        } else {
            "auto"
        };
        self.persist_manual_audited_result_trace_for_decision(
            record,
            "approved",
            decision_source,
            target_status,
            None,
            tool_result,
            trace,
            model_context_items,
            completed_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn inspect_manual_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_manual_audited_result_trace_for_decision(
            record,
            "approved",
            target_status,
            command_result,
            tool_result,
            trace,
            model_context_items,
            completed_at,
        )
    }

    /// Inspects whether an explicit pre-dispatch MCP rejection reached the durable
    /// audit/target/ToolResult boundary after the commit call returned an error.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn inspect_rejected_mcp_audited_result_trace(
        &self,
        record: &PendingActionRecord,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        self.inspect_manual_audited_result_trace_for_decision(
            record,
            "rejected",
            PendingActionStatus::Rejected,
            None,
            tool_result,
            trace,
            model_context_items,
            completed_at,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn inspect_manual_audited_result_trace_for_decision(
        &self,
        record: &PendingActionRecord,
        decision: &str,
        target_status: PendingActionStatus,
        command_result: Option<&AgentCommandExecutionResult>,
        tool_result: &AgentToolResult,
        trace: &mycopilot_core::ConversationTurnTrace,
        model_context_items: &[mycopilot_core::ConversationModelContextItem],
        completed_at: i64,
    ) -> Result<AgentPendingActionSettlementInspection, String> {
        let status = pending_status_label(target_status);
        let decided_at = (decision == "rejected").then_some(completed_at);
        let file_change_result = file_change_result_for_audit(record, tool_result)?;
        let audit = action_audit_record(
            record,
            Some(decision),
            status,
            file_change_result.as_ref(),
            command_result,
            Some(tool_result),
            tool_result.error.as_deref(),
            decided_at,
            Some(completed_at),
        );
        self.storage
            .inspect_pending_agent_action_audited_result_trace_with_model_context(
                &audit,
                pending_status_label(record.snapshot.status),
                status,
                trace,
                model_context_items,
                completed_at,
            )
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

pub(super) fn file_change_result_for_audit(
    record: &PendingActionRecord,
    tool_result: &AgentToolResult,
) -> Result<Option<AgentFileChangeResult>, String> {
    let AgentProposedAction::FileChange { file_change } = &record.snapshot.action else {
        return Ok(None);
    };
    if tool_result.call_id != file_change.id
        || tool_result.tool != "apply_patch"
        || record.snapshot.action_type != "file_change"
        || record.snapshot.tool_name != "apply_patch"
    {
        return Err("FileChange terminal ToolResult identity is invalid".to_string());
    }
    let result = serde_json::from_value::<AgentFileChangeResult>(
        tool_result
            .result
            .clone()
            .ok_or_else(|| "FileChange terminal ToolResult is missing its result".to_string())?,
    )
    .map_err(|_| "FileChange terminal ToolResult shape is invalid".to_string())?;
    if result.transaction_id != file_change.transaction_id
        || result.operation != file_change.operation
        || result.update_strategy != file_change.update_strategy
        || result.file_path != file_change.file_path
        || result.additions != file_change.additions
        || result.deletions != file_change.deletions
        || result.line_count != file_change.line_count
        || result.byte_count != file_change.byte_count
    {
        return Err("FileChange terminal ToolResult differs from its frozen proposal".to_string());
    }
    Ok(Some(result))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn action_audit_record(
    record: &PendingActionRecord,
    decision: Option<&str>,
    status: &str,
    file_change_result: Option<&AgentFileChangeResult>,
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
        file_change_result_json: file_change_result.map(serialize_json),
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
    file_change_result: Option<&AgentFileChangeResult>,
    command_result: Option<&AgentCommandExecutionResult>,
    tool_result: Option<&AgentToolResult>,
    error: Option<&str>,
    created_at: i64,
    completed_at: Option<i64>,
) -> AgentActionAuditRecord {
    let provider_action_id = action_id_for_action(action);
    let decision_source = if matches!(action, AgentProposedAction::FileChange { .. })
        && agent_input
            .resume_checkpoint
            .as_ref()
            .and_then(|checkpoint| checkpoint.file_change_run_grant_ref.as_ref())
            .is_some()
    {
        "run_grant"
    } else {
        "auto"
    };
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
        file_change_result_json: file_change_result.map(serialize_json),
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
        decision_source: Some(decision_source.to_string()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BuiltinMcpBindingApprovalStatus {
    Required,
    Approved,
}

pub(super) fn pending_action_binding_matches(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
) -> bool {
    pending_action_binding_matches_with_builtin_status(
        run_id,
        record,
        action,
        agent_input,
        BuiltinMcpBindingApprovalStatus::Required,
    )
}

fn pending_action_binding_matches_for_auto_journal(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
) -> bool {
    pending_action_binding_matches_with_builtin_status(
        run_id,
        record,
        action,
        agent_input,
        BuiltinMcpBindingApprovalStatus::Approved,
    )
}

fn pending_action_binding_matches_with_builtin_status(
    run_id: &str,
    record: Option<&AgentPendingActionRecord>,
    action: &AgentProposedAction,
    agent_input: &AgentChatInput,
    builtin_status: BuiltinMcpBindingApprovalStatus,
) -> bool {
    if matches!(
        action,
        AgentProposedAction::BuiltinCapabilityActivation { approval }
            if approval.run_id != run_id
    ) || matches!(
        action,
        AgentProposedAction::BuiltinMcpToolApproval { approval }
            if approval.identity.run_id != run_id
    ) {
        return false;
    }
    let action_id = action_id_for_action(action);
    let tool_name = tool_name_for_action(action);
    let (tool_call_id, pending_action_id) = match action {
        AgentProposedAction::McpToolCall { approval } => (
            approval.identity.call_id.as_str(),
            Some(approval.identity.action_id.clone()),
        ),
        AgentProposedAction::BuiltinCapabilityActivation { approval } => {
            (approval.call_id.as_str(), Some(approval.action_id.clone()))
        }
        AgentProposedAction::BuiltinMcpToolApproval { approval } => (
            approval.identity.call_id.as_str(),
            Some(approval.identity.action_id.clone()),
        ),
        AgentProposedAction::FileChange { file_change } => (
            file_change.id.as_str(),
            Some(pending_action_storage_id(run_id, &file_change.id)),
        ),
        _ => (action_id.as_str(), None),
    };
    let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() else {
        return false;
    };
    if checkpoint.run_id != run_id
        || checkpoint.pending_action_id.as_deref() != pending_action_id.as_deref()
        || checkpoint.pending_tool_call_id != tool_call_id
    {
        return false;
    }
    let mut checkpoint_calls = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .filter(|call| call.id == tool_call_id);
    let Some(checkpoint_call) = checkpoint_calls.next() else {
        return false;
    };
    if checkpoint_calls.next().is_some()
        || checkpoint_call.name != tool_name
        || checkpoint_call.provider_identity.runtime_call_id != tool_call_id
    {
        return false;
    }
    if !record.is_none_or(|record| {
        record.action_type == action_type_for_action(action)
            && record.run_id == run_id
            && record.action_id == pending_action_storage_id(run_id, &action_id)
            && record.tool_call_id.as_deref() == Some(tool_call_id)
            && record.tool_name == tool_name
    }) {
        return false;
    }

    if let AgentProposedAction::BuiltinCapabilityActivation { approval } = action {
        return mycopilot_core::validate_frozen_builtin_capability_activation_args(
            approval,
            &checkpoint_call.args,
        )
        .is_ok();
    }

    if let AgentProposedAction::BuiltinMcpToolApproval { approval } = action {
        return mycopilot_core::validate_builtin_mcp_tool_approval_shape(approval).is_ok()
            && approval.approval_status
                == match builtin_status {
                    BuiltinMcpBindingApprovalStatus::Required => AgentApprovalStatus::Required,
                    BuiltinMcpBindingApprovalStatus::Approved => AgentApprovalStatus::Approved,
                }
            && approval.identity.run_id == run_id
            && approval.identity.call_id == checkpoint_call.id
            && approval.identity.model_name == checkpoint_call.name
            && checkpoint_call.args == serde_json::json!({});
    }

    if let AgentProposedAction::FileChange { file_change } = action {
        let Some(context) = agent_input.context.as_ref() else {
            return false;
        };
        let expected_approval_status = match builtin_status {
            BuiltinMcpBindingApprovalStatus::Required => AgentApprovalStatus::Required,
            BuiltinMcpBindingApprovalStatus::Approved => AgentApprovalStatus::Approved,
        };
        let args_digest = match mycopilot_core::file_change::proposal_digest(&checkpoint_call.args)
        {
            Ok(digest) => digest,
            Err(_) => return false,
        };
        let run_grant_shape_valid = match (
            builtin_status,
            checkpoint.file_change_run_grant_ref.as_ref(),
        ) {
            (BuiltinMcpBindingApprovalStatus::Required, None) => true,
            (BuiltinMcpBindingApprovalStatus::Required, Some(_)) => false,
            (BuiltinMcpBindingApprovalStatus::Approved, None) => true,
            (BuiltinMcpBindingApprovalStatus::Approved, Some(grant_ref)) => {
                grant_ref.validate().is_ok()
                    && matches!(
                        file_change.operation,
                        mycopilot_core::AgentFileChangeOperation::Create
                            | mycopilot_core::AgentFileChangeOperation::Update
                    )
            }
        };
        return file_change.validate().is_ok()
            && run_grant_shape_valid
            && file_change.approval_status == expected_approval_status
            && file_change.execution.source_tool_name == "apply_patch"
            && file_change.execution.source_call_id == checkpoint_call.id
            && file_change.execution.source_args_digest == args_digest
            && file_change.execution.run_id == run_id
            && context.conversation_id.as_deref()
                == Some(file_change.execution.conversation_id.as_str())
            && context.project_id.as_deref() == file_change.execution.project_id.as_deref();
    }

    let AgentProposedAction::McpToolCall { approval } = action else {
        return true;
    };
    let lifecycle_state = match approval.approval_mode {
        mycopilot_core::AgentMcpApprovalMode::Prompt => {
            AgentMcpToolInvocationState::PendingApproval
        }
        mycopilot_core::AgentMcpApprovalMode::Auto => AgentMcpToolInvocationState::Approved,
        mycopilot_core::AgentMcpApprovalMode::Deny => return false,
    };
    if mcp_tool_invocation_event(
        approval,
        McpToolInvocationEventUpdate {
            state: lifecycle_state,
            dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
            outcome: None,
            is_error: None,
            error_code: None,
            duration_ms: None,
            output_truncated: false,
            result_size: None,
            failure_stage: None,
        },
    )
    .is_err()
    {
        return false;
    }
    let identity = &approval.identity;
    if identity.run_id != run_id
        || approval.call.id != identity.call_id
        || approval.call.tool != identity.provenance.model_tool_name
    {
        return false;
    }
    identity.run_id == checkpoint.run_id
}

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
        if !receipt_is_authoritative || !run_is_recoverable {
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
            return Err("Staged delete is not supported".to_string())
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

pub(super) fn resolve_pending_action_storage_id(
    pending_actions: &HashMap<String, PendingActionRecord>,
    run_id: &str,
    action_id: &str,
) -> Option<String> {
    let canonical = pending_action_storage_id(run_id, action_id);
    pending_actions
        .contains_key(&canonical)
        .then_some(canonical)
}

pub(super) fn pending_storage_record(
    record: &PendingActionRecord,
    updated_at: i64,
) -> Result<AgentPendingActionRecord, String> {
    Ok(AgentPendingActionRecord {
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
        )?,
        created_at: record.snapshot.created_at,
        updated_at,
    })
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
        && same_persisted_pending_agent_input(existing, candidate)
}

fn same_persisted_pending_agent_input(
    existing: &PendingActionRecord,
    candidate: &PendingActionRecord,
) -> bool {
    match (
        persisted_pending_agent_input_json(&existing.agent_input, existing.snapshot.status),
        persisted_pending_agent_input_json(&candidate.agent_input, candidate.snapshot.status),
    ) {
        (Ok(existing), Ok(candidate)) => existing == candidate,
        _ => false,
    }
}

fn same_pending_action_identity_except_status(
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
        && serialize_json(&existing.snapshot.action) == serialize_json(&candidate.snapshot.action)
}

pub(super) fn persisted_pending_agent_input_json(
    agent_input: &AgentChatInput,
    status: PendingActionStatus,
) -> Result<String, String> {
    let mut persisted_agent_input = agent_input.clone();
    // The explicit allowlist DTO below, rather than mutation of a full AgentChatInput
    // serialization, is the security boundary. Secret-bearing fields may remain in this
    // short-lived clone because `PersistedAgentResumeInput::from_agent_input` records only
    // non-secret identities and credential-required booleans.
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
    Ok(PersistedAgentResumeInput::from_agent_input(&persisted_agent_input)?.encode())
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

fn validate_current_effective_provider_protocol(
    frozen_revision: &str,
    current_protocol_revision: &str,
    model: &mycopilot_core::storage::models::ModelConfigRecord,
    connection: &mycopilot_core::storage::models::ModelConnectionConfig,
    frozen_profile: &mycopilot_core::ProviderProfileConfig,
    frozen_key: &ProviderProtocolKey,
) -> Result<(), String> {
    if !mycopilot_core::storage::config_repository::is_provider_protocol_revision(frozen_revision) {
        return Err("frozen pending-action Provider Protocol revision is invalid".to_string());
    }
    if frozen_revision != current_protocol_revision {
        return Err("frozen pending-action Provider Protocol no longer matches".to_string());
    }

    let current_dialect = ProviderProtocolDialect::detect_from_api_url(&connection.api_url);
    let current_profile = model
        .resolved_provider_profile_config(current_dialect)
        .map_err(|_| "frozen pending-action Provider Profile is unavailable".to_string())?;
    if &current_profile != frozen_profile
        || frozen_key.dialect != current_dialect
        || frozen_key.model_id != model.provider_model_id
    {
        return Err("frozen pending-action Provider Protocol no longer matches".to_string());
    }
    Ok(())
}

pub(super) fn restore_agent_input_secrets(
    storage: &Arc<StorageService>,
    persisted: DecodedPersistedAgentResumeInput,
) -> Result<AgentChatInput, String> {
    let mut agent_input = persisted.agent_input;
    let settings_snapshot = storage
        .load_model_settings_snapshot()
        .map_err(|_| "failed to resolve frozen pending-action provider settings".to_string())?
        .ok_or_else(|| "frozen pending-action provider settings are unavailable".to_string())?;
    let model_config_id = agent_input
        .model_config_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            "frozen pending-action local model configuration is unavailable".to_string()
        })?;
    let current_provider_connection_revision = settings_snapshot
        .provider_connection_revisions
        .get(model_config_id)
        .ok_or_else(|| "frozen pending-action provider connection is unavailable".to_string())?;
    if current_provider_connection_revision != &persisted.provider_connection_revision {
        return Err("frozen pending-action provider connection no longer matches".to_string());
    }
    if settings_snapshot.search_connection_revision != persisted.search_connection_revision {
        return Err("frozen pending-action search connection no longer matches".to_string());
    }
    let settings = settings_snapshot.settings;

    let conversation_model_id = match agent_input
        .context
        .as_ref()
        .and_then(|context| context.conversation_id.as_deref())
    {
        Some(conversation_id) => storage
            .load_conversation(conversation_id)
            .map_err(|_| "failed to verify frozen pending-action conversation".to_string())?
            .and_then(|conversation| conversation.model_id),
        None => None,
    };
    if conversation_model_id
        .as_deref()
        .is_some_and(|model_id| model_id != model_config_id)
    {
        return Err(
            "frozen pending-action model identity no longer matches its conversation".to_string(),
        );
    }
    let model = settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or_else(|| "frozen pending-action model configuration is unavailable".to_string())?;

    // Pending actions deliberately persist without API tokens. On restoration, resolve the
    // exact model-specific-or-global pair used by the frozen run. Endpoint changes fail closed;
    // credentials are rehydrated only after the endpoint digest has matched.
    let connection = settings.effective_connection_for(model).map_err(|_| {
        let credential_missing = match (
            model.api_url_override.as_deref(),
            model.api_token_override.as_deref(),
        ) {
            (Some(url), Some(token)) => !url.trim().is_empty() && token.trim().is_empty(),
            (None, None) => {
                !settings.api_url.trim().is_empty() && settings.api_token.trim().is_empty()
            }
            _ => false,
        };
        if persisted.provider_credential_required && credential_missing {
            "frozen pending-action provider credential is unavailable".to_string()
        } else {
            "frozen pending-action provider connection is unavailable".to_string()
        }
    })?;
    if persisted_endpoint_digest(&connection.api_url) != persisted.provider_endpoint_digest {
        return Err("frozen pending-action provider endpoint no longer matches".to_string());
    }
    let provider_profile_config = agent_input
        .provider_profile_config
        .as_ref()
        .ok_or_else(|| "frozen pending-action Provider Profile is unavailable".to_string())?;
    let provider_protocol_key = agent_input
        .provider_protocol_key
        .as_ref()
        .ok_or_else(|| "frozen pending-action Provider Protocol is unavailable".to_string())?;
    provider_protocol_key
        .validate_against_config(provider_profile_config)
        .map_err(|_| "frozen pending-action Provider Protocol is invalid".to_string())?;
    if provider_protocol_key.model_id != agent_input.model
        || provider_protocol_key
            .provider_configuration_revision
            .as_deref()
            != Some(persisted.provider_configuration_revision.as_str())
    {
        return Err("frozen pending-action Provider Protocol provenance diverged".to_string());
    }
    // The profile/key/capabilities belong to the already-running approval checkpoint. A settings
    // edit may rotate the current model's Provider Protocol while this action is pending; that new
    // identity is authoritative only for the next Run. The connection revision and endpoint digest
    // above still fail closed on endpoint/token drift before any credential is rehydrated.
    if let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() {
        if &checkpoint.provider_profile_config != provider_profile_config
            || &checkpoint.provider_protocol_key != provider_protocol_key
        {
            return Err("frozen pending-action checkpoint Provider Protocol diverged".to_string());
        }
    }
    let provider_credential_present = !connection.api_token.trim().is_empty();
    if provider_credential_present != persisted.provider_credential_required {
        return Err(
            "frozen pending-action provider credential presence no longer matches".to_string(),
        );
    }
    agent_input.api_url = connection.api_url;
    agent_input.api_token = connection.api_token;

    if let Some(search) = agent_input.search_config.as_mut() {
        if search.mode != search_mode_from_storage(&settings.search_mode) {
            return Err("frozen pending-action search mode no longer matches".to_string());
        }
        let search_credential_present = !settings.tavily_api_key.trim().is_empty();
        if search_credential_present != persisted.search_credential_required {
            return Err(
                "frozen pending-action search credential presence no longer matches".to_string(),
            );
        }
        if persisted.search_credential_required {
            search.tavily_api_key = Some(settings.tavily_api_key.trim().to_string());
        }
    }
    Ok(agent_input)
}

fn bind_pending_provider_configuration(
    storage: &Arc<StorageService>,
    agent_input: AgentChatInput,
) -> Result<AgentChatInput, String> {
    let snapshot = storage
        .load_model_settings_snapshot()
        .map_err(|_| "failed to freeze pending-action provider configuration".to_string())?
        .ok_or_else(|| "pending-action provider configuration is unavailable".to_string())?;
    let provider_configuration_revision = agent_input
        .provider_configuration_revision
        .as_deref()
        .filter(|revision| {
            mycopilot_core::storage::config_repository::is_provider_protocol_revision(revision)
        })
        .ok_or_else(|| "pending-action Provider Protocol revision is unavailable".to_string())?;
    let model_config_id = agent_input
        .model_config_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "pending-action local model configuration is unavailable".to_string())?;
    let model = snapshot
        .settings
        .models
        .iter()
        .find(|model| model.id == model_config_id)
        .ok_or_else(|| "pending-action model configuration is unavailable".to_string())?;
    let connection = snapshot
        .settings
        .effective_connection_for(model)
        .map_err(|_| "pending-action provider connection is unavailable".to_string())?;
    let current_provider_connection_revision = snapshot
        .provider_connection_revisions
        .get(&model.id)
        .ok_or_else(|| "pending-action provider connection identity is unavailable".to_string())?;
    let current_provider_protocol_revision = snapshot
        .provider_protocol_revisions
        .get(&model.id)
        .ok_or_else(|| "pending-action Provider Protocol identity is unavailable".to_string())?;
    if agent_input.provider_connection_revision.as_ref()
        != Some(current_provider_connection_revision)
    {
        return Err("pending-action provider connection changed before persistence".to_string());
    }
    if agent_input.search_connection_revision.as_ref() != Some(&snapshot.search_connection_revision)
    {
        return Err("pending-action search connection changed before persistence".to_string());
    }
    let provider_profile_config = agent_input
        .provider_profile_config
        .as_ref()
        .ok_or_else(|| "pending-action Provider Profile is unavailable".to_string())?;
    provider_profile_config
        .validate()
        .map_err(|_| "pending-action Provider Profile is invalid".to_string())?;
    let provider_protocol_key = agent_input
        .provider_protocol_key
        .as_ref()
        .ok_or_else(|| "pending-action Provider Protocol is unavailable".to_string())?;
    provider_protocol_key
        .validate_against_config(provider_profile_config)
        .map_err(|_| "pending-action Provider Protocol is invalid".to_string())?;
    if provider_protocol_key.model_id != agent_input.model
        || provider_protocol_key
            .provider_configuration_revision
            .as_deref()
            != Some(provider_configuration_revision)
    {
        return Err("pending-action Provider Protocol provenance is inconsistent".to_string());
    }
    validate_current_effective_provider_protocol(
        provider_configuration_revision,
        current_provider_protocol_revision,
        model,
        &connection,
        provider_profile_config,
        provider_protocol_key,
    )?;
    if connection.api_url != agent_input.api_url
        || connection.api_token != agent_input.api_token
        || model.supports_image != agent_input.model_capabilities.image_input
        || agent_input
            .context_window_tokens
            .is_some_and(|tokens| tokens != model.effective_context_window_tokens())
    {
        return Err(
            "pending-action provider configuration does not match the active run".to_string(),
        );
    }
    if let Some(checkpoint) = agent_input.resume_checkpoint.as_ref() {
        if &checkpoint.provider_profile_config != provider_profile_config
            || &checkpoint.provider_protocol_key != provider_protocol_key
        {
            return Err("pending-action checkpoint Provider Protocol is inconsistent".to_string());
        }
    }
    if let Some(search) = agent_input.search_config.as_ref() {
        let configured_key = (!snapshot.settings.tavily_api_key.trim().is_empty())
            .then_some(snapshot.settings.tavily_api_key.trim());
        if search.mode != search_mode_from_storage(&snapshot.settings.search_mode)
            || search.tavily_api_key.as_deref() != configured_key
        {
            return Err(
                "pending-action search configuration does not match the active run".to_string(),
            );
        }
    }
    Ok(agent_input)
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
            | (PendingActionStatus::Pending, PendingActionStatus::Rejected)
            | (PendingActionStatus::Pending, PendingActionStatus::Cancelled)
            | (PendingActionStatus::Approved, PendingActionStatus::Executing)
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
        AgentProposedAction::FileChange { file_change } => Some(file_change.file_path.as_str()),
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
