impl StorageService {
    pub fn transition_pending_agent_action(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::transition_pending_action(
            &connection,
            action_id,
            expected_status,
            status,
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作状态迁移必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    /// Resolves the approval notification in the same transaction that makes the approval stop
    /// being actionable. A crash cannot therefore leave a stale system notification behind.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_pending_agent_action_and_resolve_notification(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        self.transition_pending_agent_action_and_resolve_notification_internal(
            action_id,
            expected_status,
            status,
            agent_input_json,
            run_id,
            renderer_action_id,
            updated_at,
            false,
        )
    }

    /// Claims action/continuation dispatch authority only when the owning Agent Run has not been
    /// durably stopped. The stop check and lifecycle CAS share one immediate transaction.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_pending_agent_action_for_dispatch_and_resolve_notification(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        self.transition_pending_agent_action_and_resolve_notification_internal(
            action_id,
            expected_status,
            status,
            agent_input_json,
            run_id,
            renderer_action_id,
            updated_at,
            true,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn transition_pending_agent_action_and_resolve_notification_internal(
        &self,
        action_id: &str,
        expected_status: &str,
        status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
        reject_stopped_run: bool,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let affected = if reject_stopped_run {
            pending_action_repository::transition_pending_action_for_dispatch(
                &transaction,
                action_id,
                expected_status,
                status,
                agent_input_json,
                updated_at,
            )
        } else {
            pending_action_repository::transition_pending_action(
                &transaction,
                action_id,
                expected_status,
                status,
                agent_input_json,
                updated_at,
            )
        }
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作状态迁移必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        if status != "pending" {
            notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
                &transaction,
                run_id,
                renderer_action_id,
                updated_at,
            )
            .map_err(storage_error)?;
        }
        transaction.commit().map_err(storage_error)
    }

    /// Atomically makes a cancelled approval non-actionable and revokes every nonterminal
    /// FileChange Run grant owned by that logical Run.
    ///
    /// Cancellation terminates the Run rather than continuing the model. Keeping the pending
    /// status CAS, notification settlement, and grant revocation in one immediate transaction
    /// prevents a terminal UI result from racing still-active remembered write authority.
    #[allow(clippy::too_many_arguments)]
    pub fn transition_cancelled_pending_agent_action_and_revoke_file_change_run_grants(
        &self,
        action_id: &str,
        expected_status: &str,
        agent_input_json: &str,
        run_id: &str,
        renderer_action_id: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let affected = pending_action_repository::transition_pending_action(
            &transaction,
            action_id,
            expected_status,
            "cancelled",
            agent_input_json,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作取消必须且只能更新一条记录，actionId={action_id}，实际更新 {affected} 条。"
            ));
        }
        notification_repository::resolve_notification_events_by_run_and_approval_action_id_in_transaction(
            &transaction,
            run_id,
            renderer_action_id,
            updated_at,
        )
        .map_err(storage_error)?;
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &transaction,
            run_id,
            updated_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }

    /// Marks one exact child Wake as approval-paused only while its owning action is still
    /// pending in the same SQLite write transaction.
    ///
    /// A dispatcher observation can be older than a concurrent user decision. Serializing the
    /// pending-action recheck with the Wake CAS prevents that stale observation from moving an
    /// already resumed Wake back to `waiting_for_approval`.
    #[allow(clippy::too_many_arguments)]
    pub fn mark_agent_wake_waiting_for_pending_approval_at(
        &self,
        wake_id: &str,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        claim_token: &str,
        marked_at: i64,
    ) -> Result<AgentWakeApprovalWaitOutcome, String> {
        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let running = agent_graph_repository::transition_agent_wake_in_connection(
            &transaction,
            wake_id,
            crate::AgentWakeStatus::Running,
            crate::AgentWakeStatus::Running,
            Some(claim_token),
            marked_at,
        )
        .map_err(|error| error.to_string())?;
        if running.run_id.as_deref() != Some(run_id)
            || running.assistant_message_id.as_deref() != Some(assistant_message_id)
        {
            return Err(
                "approval wait transition does not own the dispatched child Turn".to_string(),
            );
        }
        let tree_stopped = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM agent_tree_run_stops WHERE run_id = ?1
                 )",
                [run_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(storage_error)?;
        let outcome = if tree_stopped {
            AgentWakeApprovalWaitOutcome::TreeStopped(running)
        } else {
            let pending_count: i64 = transaction
                .query_row(
                    "SELECT COUNT(*)
                     FROM agent_pending_actions
                     WHERE run_id = ?1
                       AND conversation_id = ?2
                       AND assistant_message_id = ?3
                       AND status = 'pending'
                       AND target_status IS NULL",
                    rusqlite::params![run_id, conversation_id, assistant_message_id],
                    |row| row.get(0),
                )
                .map_err(storage_error)?;
            match pending_count {
                0 => AgentWakeApprovalWaitOutcome::RunningAfterApproval(running),
                1 => {
                    let waiting = agent_graph_repository::transition_agent_wake_in_connection(
                        &transaction,
                        wake_id,
                        crate::AgentWakeStatus::Running,
                        crate::AgentWakeStatus::WaitingForApproval,
                        Some(claim_token),
                        marked_at,
                    )
                    .map_err(|error| error.to_string())?;
                    AgentWakeApprovalWaitOutcome::Waiting(waiting)
                }
                count => {
                    return Err(format!(
                        "approval wait transition found {count} pending actions for one child Turn"
                    ));
                }
            }
        };
        transaction.commit().map_err(storage_error)?;
        Ok(outcome)
    }

    /// Atomically accepts one Skill-script approval and, for a delegated Turn, resumes its exact
    /// Wake.
    ///
    /// The side effect may only be started after this transaction commits. Keeping the approved
    /// audit, the pending-action execution claim, and the child Wake in one CAS boundary prevents
    /// the durable `approved + waiting_for_approval` split-brain that otherwise has no executor.
    #[allow(clippy::too_many_arguments)]
    pub fn commit_pending_skill_script_approval_execution(
        &self,
        action_id: &str,
        executing_agent_input_json: &str,
        approved_audit: &AgentActionAuditRecord,
        child_wake: Option<(&str, crate::AgentWakeStatus, &str)>,
        committed_at: i64,
    ) -> Result<(), String> {
        validate_manual_approval_execution_audit(action_id, approved_audit, committed_at)?;
        serde_json::from_str::<serde_json::Value>(executing_agent_input_json)
            .map_err(|_| "manual approval execution checkpoint is not valid JSON".to_string())?;

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let pending = pending_action_repository::load_pending_action(&transaction, action_id)
            .map_err(storage_error)?
            .ok_or_else(|| format!("manual approval has no pending action: {action_id}"))?;
        validate_manual_approval_execution_identity(&pending, approved_audit)?;
        if pending.action_type != "skill_script" || pending.tool_name != "skills_run_script" {
            return Err(
                "manual Skill-script approval does not own a Skill-script pending action"
                    .to_string(),
            );
        }
        if pending.status != "pending" || pending.target_status.is_some() {
            return Err(format!(
                "manual approval execution lost its pending CAS: actionId={action_id}, status={}",
                pending.status
            ));
        }
        if let Some(existing) =
            agent_action_audit_repository::load_action_audit_record(&transaction, action_id)
                .map_err(storage_error)?
        {
            if !agent_action_audit_repository::matches_manual_preterminal_action_audit(
                &existing,
                approved_audit,
            ) {
                return Err(format!(
                    "manual approval audit identity changed before execution: actionId={action_id}"
                ));
            }
        }

        let changed = pending_action_repository::transition_pending_action_for_dispatch(
            &transaction,
            action_id,
            "pending",
            "executing",
            executing_agent_input_json,
            committed_at,
        )
        .map_err(storage_error)?;
        if changed != 1 {
            return Err(format!(
                "manual approval execution must claim exactly one pending action: actionId={action_id}, changed={changed}"
            ));
        }
        agent_action_audit_repository::upsert_action_audit_record(&transaction, approved_audit)
            .map_err(storage_error)?;

        if let Some((wake_id, expected_wake_status, claim_token)) = child_wake {
            if !matches!(
                expected_wake_status,
                crate::AgentWakeStatus::Running | crate::AgentWakeStatus::WaitingForApproval
            ) {
                return Err(
                    "manual approval child Wake must already own an active Turn".to_string()
                );
            }
            let resumed = agent_graph_repository::transition_agent_wake_in_connection(
                &transaction,
                wake_id,
                expected_wake_status,
                crate::AgentWakeStatus::Running,
                Some(claim_token),
                committed_at,
            )
            .map_err(|error| error.to_string())?;
            if resumed.run_id.as_deref() != Some(pending.run_id.as_str())
                || resumed.assistant_message_id.as_deref()
                    != pending.assistant_message_id.as_deref()
            {
                return Err(
                    "manual approval child Wake does not own the pending action Turn".to_string(),
                );
            }

            let conversation_id = pending.conversation_id.as_deref().ok_or_else(|| {
                "manual approval child Skill script has no conversation owner".to_string()
            })?;
            let assistant_message_id =
                pending.assistant_message_id.as_deref().ok_or_else(|| {
                    "manual approval child Skill script has no Assistant owner".to_string()
                })?;
            let tool_call_id = pending.tool_call_id.as_deref().ok_or_else(|| {
                "manual approval child Skill script has no ToolCall owner".to_string()
            })?;
            chat_repository::update_message_run_running_state(
                &transaction,
                conversation_id,
                assistant_message_id,
                &pending.run_id,
                tool_call_id,
                committed_at,
            )
            .map_err(storage_error)?;
            let usage_changed = transaction
                .execute(
                    "UPDATE agent_usage_records
                     SET status = 'running', error = NULL, completed_at = NULL
                     WHERE run_id = ?1
                       AND conversation_id = ?2
                       AND message_id = ?3
                       AND status = 'waiting_for_approval'",
                    rusqlite::params![&pending.run_id, conversation_id, assistant_message_id,],
                )
                .map_err(storage_error)?;
            if usage_changed != 1 {
                return Err(format!(
                    "manual approval child Skill script must resume exactly one waiting Usage row: actionId={action_id}, changed={usage_changed}"
                ));
            }
        }

        transaction.commit().map_err(storage_error)?;
        Ok(())
    }

    pub fn set_pending_agent_action_target_status(
        &self,
        action_id: &str,
        expected_status: &str,
        target_status: &str,
        updated_at: i64,
    ) -> Result<(), String> {
        let connection = self.state.connection()?;
        let affected = pending_action_repository::set_pending_action_target_status(
            &connection,
            action_id,
            expected_status,
            target_status,
            updated_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "待审批操作目标终态写入必须且只能更新一条记录，actionId={action_id}，expectedStatus={expected_status}，实际更新 {affected} 条。"
            ));
        }
        Ok(())
    }

    /// Atomically retires a claimed action when its model continuation cannot reach Runtime.
    ///
    /// The action executor has already persisted `expected_target_status` before this boundary.
    /// The exact target is therefore part of the CAS: a stale continuation cannot overwrite a
    /// newer decision. Pending lifecycle, assistant/run terminal state, trace/model context and
    /// usage become visible together or remain entirely unchanged for recovery.
    #[allow(clippy::too_many_arguments)]
    pub fn fail_claimed_agent_action_continuation(
        &self,
        action_id: &str,
        expected_status: &str,
        expected_target_status: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        failure_message: &str,
        trace: &ConversationTurnTrace,
        model_context_items: &[ConversationModelContextItem],
        completed_at: i64,
        usage: Option<&AgentUsageRecordInsert>,
    ) -> Result<(), String> {
        if !matches!(expected_status, "approved" | "executing" | "rejected") {
            return Err(format!(
                "pre-Runtime continuation failure requires approved/executing/rejected status, got {expected_status}"
            ));
        }
        if !matches!(
            expected_target_status,
            "rejected" | "cancelled" | "completed" | "failed"
        ) {
            return Err(format!(
                "pre-Runtime continuation failure requires a terminal target, got {expected_target_status}"
            ));
        }
        if trace.conversation_id != conversation_id
            || trace.assistant_message_id != assistant_message_id
            || trace.terminal_status != crate::ConversationTurnTraceTerminalStatus::Failed
            || trace.terminal_error.as_deref() != Some(failure_message)
        {
            return Err(
                "pre-Runtime continuation failure trace identity, status or error is invalid"
                    .to_string(),
            );
        }
        trace
            .validate_complete_model_context(model_context_items)
            .map_err(|error| {
                format!("pre-Runtime continuation model context is invalid: {error}")
            })?;
        if let Some(usage) = usage {
            if usage.run_id != trace.run_id
                || usage.conversation_id != conversation_id
                || usage.message_id != assistant_message_id
                || usage.status.as_deref() != Some("failed")
                || usage.error.as_deref() != Some(failure_message)
            {
                return Err(
                    "pre-Runtime continuation usage does not match the terminal Turn failure"
                        .to_string(),
                );
            }
        }

        let mut connection = self.state.connection()?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(storage_error)?;
        let pending_record =
            pending_action_repository::load_pending_action(&transaction, action_id)
                .map_err(storage_error)?
                .ok_or_else(|| {
                    "pre-Runtime continuation pending action no longer exists".to_string()
                })?;
        if pending_record.run_id != trace.run_id
            || pending_record.conversation_id.as_deref() != Some(conversation_id)
            || pending_record.assistant_message_id.as_deref() != Some(assistant_message_id)
        {
            return Err("pre-Runtime continuation pending identity changed".to_string());
        }
        let affected = pending_action_repository::fail_claimed_action_continuation(
            &transaction,
            action_id,
            &trace.run_id,
            conversation_id,
            assistant_message_id,
            expected_status,
            expected_target_status,
            completed_at,
        )
        .map_err(storage_error)?;
        if affected != 1 {
            return Err(format!(
                "pre-Runtime continuation failure lost its pending-action CAS: actionId={action_id}, expectedStatus={expected_status}, expectedTargetStatus={expected_target_status}"
            ));
        }
        resolve_pending_approval_notification_in_transaction(
            &transaction,
            &pending_record,
            completed_at,
        )?;
        chat_repository::update_message_status_and_content(
            &transaction,
            conversation_id,
            assistant_message_id,
            failure_message,
            Some("error"),
            completed_at,
        )
        .map_err(storage_error)?;
        chat_repository::update_message_run_terminal_state(
            &transaction,
            conversation_id,
            assistant_message_id,
            &trace.run_id,
            Some("error"),
            "failed",
            completed_at,
            None,
        )
        .map_err(storage_error)?;
        conversation_trace_repository::commit_trace_in_connection(
            &transaction,
            trace,
            completed_at,
            completed_at,
        )
        .map_err(storage_error)?;
        conversation_model_context_repository::commit_items_in_connection(
            &transaction,
            conversation_id,
            assistant_message_id,
            model_context_items,
        )
        .map_err(storage_error)?;
        let durable_model_context_items =
            conversation_model_context_repository::get_log_for_message(
                &transaction,
                assistant_message_id,
            )
            .map_err(storage_error)?
            .map(|log| log.items)
            .unwrap_or_default();
        trace
            .validate_complete_model_context(&durable_model_context_items)
            .map_err(|error| format!("terminal Assistant model context is incomplete: {error}"))?;
        if let Some(usage) = usage {
            usage_repository::upsert_usage_record(&transaction, usage).map_err(storage_error)?;
        }
        super::trace_reconciliation::enqueue_reconciled_human_root_notification(
            &transaction,
            &trace.run_id,
            conversation_id,
            assistant_message_id,
            "task_failed",
            completed_at,
        )?;
        file_change_run_grant_repository::revoke_nonterminal_run_grants(
            &transaction,
            &trace.run_id,
            completed_at,
        )
        .map_err(storage_error)?;
        transaction.commit().map_err(storage_error)
    }
}
