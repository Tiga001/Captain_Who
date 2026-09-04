impl AgentService {
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

    /// Claims dispatch authority while keeping the process-local pending projection in lockstep
    /// with the guarded durable CAS. Callers must not update only their cloned `record`: terminal
    /// settlement reads the shared map to derive its expected durable status.
    pub(super) fn transition_pending_status_for_dispatch(
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
        self.persist_pending_dispatch_status(pending, current, status)?;
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

    /// Persists a status transition which grants process, filesystem, transport, or model
    /// continuation authority. Unlike terminal/cancellation bookkeeping, this CAS fails when the
    /// exact Run is already covered by a durable Agent-tree stop.
    pub(super) fn persist_pending_dispatch_status(
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
            .transition_pending_agent_action_for_dispatch_and_resolve_notification(
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

    /// Persists the pre-dispatch receipt for the current automatically authorized external MCP
    /// path. Manual approvals continue to use `persist_action_audit`; this narrow producer must
    /// retain `decision_source=auto` so startup reconciliation can distinguish its hidden journal
    /// from a user-approved action without guessing from later state.
    fn persist_auto_external_mcp_journal_audit(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        if !matches!(
            &record.snapshot.action,
            AgentProposedAction::McpToolCall { approval }
                if approval.approval_mode == mycopilot_core::AgentMcpApprovalMode::Auto
                    && approval.call.approval_status == AgentApprovalStatus::Approved
        ) {
            return Err(
                "automatic external MCP audit requires a Host-authorized MCP action".to_string(),
            );
        }
        #[cfg(test)]
        if let Some(error) = take_auto_action_audit_failure(
            &record.snapshot.run_id,
            &record.snapshot.action_id,
            "approved",
        ) {
            return Err(error);
        }
        self.storage
            .upsert_agent_action_audit(auto_action_audit_record(
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
        let outcome = if decision == "rejected" {
            self.storage
                .commit_pending_agent_action_audited_result_trace_for_continuation(
                    &audit,
                    pending_status_label(record.snapshot.status),
                    status,
                    trace,
                    model_context_items,
                    completed_at,
                )
        } else {
            self.storage
                .commit_pending_agent_action_audited_result_trace_with_model_context(
                    &audit,
                    pending_status_label(record.snapshot.status),
                    status,
                    trace,
                    model_context_items,
                    completed_at,
                )
        }?;
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
