impl AgentService {
    pub fn reject_action(
        &self,
        run_id: &str,
        action_id: &str,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                return coordinator
                    .reject(run_id, action_id, message)?
                    .ok_or_else(|| {
                        "Browser risk approval changed while rejection was being committed."
                            .to_string()
                    });
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;
        self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Rejected,
            message,
            notifications,
        )
    }

    pub fn cancel_action(&self, run_id: &str, action_id: &str) -> Result<bool, String> {
        if let Some(coordinator) = self.browser_risk_coordinator.as_ref() {
            if let Some(conversation_id) = coordinator.pending_conversation_id(run_id, action_id) {
                self.collaboration_authorizer
                    .authorize_user_conversation_write(&conversation_id)
                    .map_err(|error| error.to_string())?;
                let cancelled = coordinator.cancel(run_id, action_id)?.ok_or_else(|| {
                    "Browser risk approval changed while cancellation was being committed."
                        .to_string()
                })?;
                if cancelled {
                    self.storage
                        .revoke_nonterminal_file_change_run_grants(run_id)
                        .map_err(|error| error.to_string())?;
                }
                return Ok(cancelled);
            }
        }
        self.authorize_user_pending_action(run_id, action_id)?;
        self.cancel_action_internal(run_id, action_id)
    }

    pub(super) fn cancel_action_internal(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<bool, String> {
        self.cancel_action_internal_with_outcome(run_id, action_id)
            .map(|outcome| outcome.affected)
    }

    fn cancel_action_internal_with_outcome(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<InternalActionCancellationOutcome, String> {
        if self
            .try_cancel_recovered_approved_mcp_action(run_id, action_id)?
            .is_some()
        {
            return Ok(InternalActionCancellationOutcome {
                affected: true,
                turn_termination_confirmed: true,
            });
        }
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(storage_id) =
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
        else {
            return Ok(InternalActionCancellationOutcome::default());
        };
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Ok(InternalActionCancellationOutcome::default());
        }
        let is_cancellable_process = matches!(
            record.snapshot.action,
            AgentProposedAction::Command { .. }
                | AgentProposedAction::SkillScript { .. }
                | AgentProposedAction::OfficeOperation { .. }
                | AgentProposedAction::McpToolCall { .. }
                | AgentProposedAction::BuiltinMcpToolApproval { .. }
        );
        let is_mcp_dispatching = record.snapshot.status == PendingActionStatus::Executing
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
                    | AgentProposedAction::BuiltinMcpToolApproval { .. }
            );
        if is_cancellable_process
            && (record.snapshot.status == PendingActionStatus::Approved || is_mcp_dispatching)
        {
            // Freeze the identity, then release approval/deletion locks before touching a process
            // Session. Session termination performs a bounded wait and must never hold the
            // pending-action or destructive-lifecycle mutexes while doing so.
            let record = record.clone();
            drop(pending_actions);
            drop(deletion_lifecycle);

            // A command which has already returned Running is no longer represented by the
            // legacy process guard. Arbitrate it through the same Session-state fence used by
            // `commit_handoff`; whichever transition wins is authoritative. The legacy flag
            // remains necessary for the approved-to-spawn gap and for non-command processes.
            let cancelled_sessions = self
                .command_sessions
                .cancel_pre_handoff_for_action(&record.snapshot.run_id, &record.snapshot.action_id);
            let cancelled_process = self.process_runs.cancel(&record.storage_id);
            let cancelled = cancelled_sessions > 0 || cancelled_process;
            let mut runtime_token_signalled = false;
            if cancelled {
                if let Some(token) = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&record.snapshot.run_id)
                {
                    token.cancel();
                    runtime_token_signalled = true;
                }
                self.fence_cancelled_run_steering(&record.snapshot.run_id);
                self.record_action_audit(
                    &record,
                    Some("cancelled"),
                    "cancellation_requested",
                    None,
                    None,
                    None,
                    Some("Process execution was cancelled by the user."),
                    Some(now_ms()),
                    None,
                );
                // This cancellation terminates the logical Run. Do not report success while a
                // remembered FileChange grant for that Run remains active; the runtime guard is
                // a second retry boundary, not the first authority fence.
                self.storage
                    .revoke_nonterminal_file_change_run_grants(&record.snapshot.run_id)
                    .map_err(|error| error.to_string())?;
            }
            return Ok(InternalActionCancellationOutcome {
                affected: cancelled,
                turn_termination_confirmed: runtime_token_signalled,
            });
        }
        if record.snapshot.status != PendingActionStatus::Pending {
            return Ok(InternalActionCancellationOutcome::default());
        }
        let call = tool_call_for_pending_record(record)?;
        self.persist_pending_status(
            record,
            PendingActionStatus::Pending,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        if let Err(error) = self.storage.set_pending_agent_action_target_status(
            &record.storage_id,
            pending_status_label(PendingActionStatus::Executing),
            pending_status_label(PendingActionStatus::Cancelled),
            now_ms(),
        ) {
            let rollback = self.persist_pending_status(
                record,
                PendingActionStatus::Executing,
                PendingActionStatus::Pending,
            );
            if rollback.is_ok() {
                record.snapshot.status = PendingActionStatus::Pending;
            }
            return match rollback {
                Ok(()) => Err(format!(
                    "待审批操作无法写入 cancelled 目标终态，已安全回滚为 pending：{error}"
                )),
                Err(rollback_error) => Err(format!(
                    "待审批操作无法写入 cancelled 目标终态：{error}；且无法回滚 executing 状态：{rollback_error}"
                )),
            };
        }
        let record = record.clone();
        drop(pending_actions);
        drop(deletion_lifecycle);
        if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action {
            let conversation_id = record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                "Skill installation approval has no conversation identity.".to_string()
            })?;
            // The user cancellation is authoritative even when the short-lived frozen package
            // was already pruned. Host cleanup is idempotent best effort and must not reopen or
            // strand the approval ticket.
            if let Some(service) = self.skill_installation.as_ref() {
                let _ =
                    service.reject_action(installation, conversation_id, &record.snapshot.run_id);
            }
        }
        if let Err(error) = self.finalize_cancelled_pending_action(&record, call) {
            if cancelled_staged_file_change_outcome_is_durable_or_unknown(&self.storage, &record) {
                return Err(format!(
                    "{error} 文件草稿的中止结果已经持久化或无法安全判定；待审批操作保持 executing 并保留 cancelled 目标终态，等待启动对账，禁止重新执行。"
                ));
            }
            if let Err(rollback_error) =
                self.transition_pending_status(&record, PendingActionStatus::Pending)
            {
                return Err(format!(
                    "{error} 此外，待审批操作无法回滚为 pending；已保持 executing 状态并保留 cancelled 目标终态，等待启动对账：{rollback_error}"
                ));
            }
            return Err(error);
        }
        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        self.transition_pending_status_to_cancelled_and_revoke_run_grants(&record)?;
        // `finalize_cancelled_pending_action` committed the assistant/trace terminal state and
        // the pending-action CAS above committed the matching action terminal state. Only after
        // both durable facts exist may the in-memory accelerator release this logical Turn.
        if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
            self.notify_durable_turn_observers(assistant_message_id);
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, &record.snapshot.run_id);
            self.release_turn_concurrency_permit(&record.snapshot.run_id);
        }
        Ok(InternalActionCancellationOutcome {
            affected: true,
            turn_termination_confirmed: true,
        })
    }

    /// Atomically cancels an MCP approval which was durable `approved` across a process restart
    /// but has not crossed the `executing` dispatch boundary.
    ///
    /// The pending-action mutex serializes decisions inside this Host, while the SQLite status CAS
    /// is authoritative across stale/restarted Host instances. Rejection takes the normal
    /// ToolResult continuation path; cancellation intentionally terminates the run here.
    fn try_cancel_recovered_approved_mcp_action(
        &self,
        run_id: &str,
        action_id: &str,
    ) -> Result<Option<AgentActionExecutionOutput>, String> {
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(storage_id) =
            resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
        else {
            return Ok(None);
        };
        let Some(record) = pending_actions.get(&storage_id) else {
            return Ok(None);
        };
        let is_recovered_approved = record.snapshot.status == PendingActionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&storage_id);
        if !is_recovered_approved {
            return Ok(None);
        }
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Err("项目或会话正在移除，无法处理恢复的 MCP 审批。".to_string());
        }
        let record = record.clone();
        let changed = self
            .storage
            .terminalize_mcp_agent_action_on_startup(
                &storage_id,
                pending_status_label(PendingActionStatus::Approved),
                McpStartupActionTerminalOutcome::Cancelled,
                self.mcp_approval_now_ms(),
            )
            .map_err(|_| "Recovered MCP approval could not be terminalized safely.".to_string())?;
        if !changed {
            return Err(
                "Recovered MCP approval changed while the decision was being committed."
                    .to_string(),
            );
        }
        pending_actions.remove(&storage_id);
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&storage_id);
        drop(pending_actions);
        drop(deletion_lifecycle);

        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
            self.notify_durable_turn_observers(assistant_message_id);
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, &record.snapshot.run_id);
            self.release_turn_concurrency_permit(&record.snapshot.run_id);
        }
        Ok(Some(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: pending_status_label(PendingActionStatus::Cancelled).to_string(),
            file_change_result: None,
            command_result: None,
            // Renderer learns the terminal MCP state from the typed lifecycle event above and
            // the approval status. A generic ToolResult would create a second result channel
            // whose payload contract is neither necessary nor safe for an external Server.
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Cancelled,
                run_id: record.snapshot.run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        }))
    }

    pub(super) fn finalize_cancelled_pending_action(
        &self,
        record: &PendingActionRecord,
        mut call: AgentToolCall,
    ) -> Result<(), String> {
        const REASON: &str = "Pending action was cancelled by the user.";
        call.approval_status = AgentApprovalStatus::Rejected;
        let execution = match &record.snapshot.action {
            AgentProposedAction::BuiltinMcpToolApproval { approval } => ActionExecutionDecision {
                status: "cancelled".to_string(),
                final_pending_status: PendingActionStatus::Cancelled,
                file_change_result: None,
                file_change: None,
                committed_file_change_action: None,
                direct_file_change_finalization: None,
                tool_result: mycopilot_core::builtin_mcp_tool_cancelled_result(approval),
            },
            AgentProposedAction::FileChange { .. } => {
                cancelled_file_change_execution(&self.storage, record)?
            }
            _ => action_execution_for_decision(
                &self.storage,
                record,
                &call,
                AgentApprovalDecisionStatus::Rejected,
                Some(REASON),
            ),
        };
        let completed_at = now_ms();
        if let (Some(conversation_id), Some(assistant_message_id), Some(checkpoint)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            record.agent_input.resume_checkpoint.as_ref(),
        ) {
            let model_observation = project_persisted_continuation_observation(
                &record.agent_input.model,
                &record.agent_input.api_url,
                record.agent_input.api_style,
                &execution.tool_result,
                &Default::default(),
            )
            .map_err(|error| error.to_string())?;
            let terminal = cancelled_conversation_trace_from_checkpoint(
                checkpoint,
                conversation_id,
                assistant_message_id,
                &call,
                &execution.tool_result,
                &model_observation,
            )?;
            let usage_record = self.prepare_run_usage_record(
                &record.snapshot.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(REASON.to_string()),
            );
            self.finalize_turn_with_human_root_notification(
                &record.snapshot.run_id,
                conversation_id,
                assistant_message_id,
                AgentRunStatus::Cancelled,
                "",
                status_for_run(AgentRunStatus::Cancelled),
                run_status_label(AgentRunStatus::Cancelled),
                &terminal.trace,
                Some(&terminal.model_context_items),
                completed_at,
                completed_at,
                usage_record.as_ref(),
                None,
            )?;
            self.finish_persisted_run_usage(&record.snapshot.run_id, AgentRunStatus::Cancelled);
            self.invalidate_conversation_context_state(conversation_id);
            self.discard_trace_snapshot(&record.snapshot.run_id);
        }
        self.record_action_audit(
            record,
            Some("cancelled"),
            "cancelled",
            execution.file_change_result.as_ref(),
            None,
            Some(&execution.tool_result),
            if matches!(
                record.snapshot.action,
                AgentProposedAction::FileChange { .. }
            ) {
                execution.tool_result.error.as_deref()
            } else {
                Some(REASON)
            },
            Some(completed_at),
            Some(completed_at),
        );
        Ok(())
    }

    /// Commits the exact pre-dispatch rejection receipt and resolves a commit-unknown response
    /// from durable state. Every retry and inspection uses the same timestamp so the audit and
    /// trace identity remain byte-for-byte stable.
    pub(super) fn commit_rejected_mcp_receipt(
        &self,
        record: &PendingActionRecord,
        persisted_agent_input: &AgentChatInput,
        completed_at: i64,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        for _ in 0..2 {
            if self
                .commit_rejected_mcp_result_trace_with_continuation(
                    record,
                    persisted_agent_input,
                    completed_at,
                    notifications,
                )
                .is_ok()
            {
                return Ok(());
            }
        }

        match self.inspect_rejected_mcp_result_trace_with_continuation(
            record,
            persisted_agent_input,
            completed_at,
        ) {
            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => Ok(()),
            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => Err(
                "MCP rejection receipt was already advanced by another continuation; duplicate continuation was stopped."
                    .to_string(),
            ),
            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => Err(
                "MCP rejection receipt was not committed; continuation was stopped.".to_string(),
            ),
            Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => Err(
                format!(
                    "MCP rejection receipt diverged at {component}; continuation was stopped: {reason}"
                ),
            ),
            Err(_) => Err(
                "MCP rejection receipt could not be inspected safely; continuation was stopped."
                    .to_string(),
            ),
        }
    }

    /// Elects the single Host continuation after the durable rejection receipt exists.
    ///
    /// The receipt transaction already owns `target_status=rejected`. This final status CAS is
    /// therefore both the same-process single-flight guard and the cross-Host arbitration point:
    /// only its winner may clear the payload, publish lifecycle, or spawn a model continuation.
    fn claim_rejected_mcp_continuation(&self, record: &PendingActionRecord) -> Result<(), String> {
        let mut pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let pending = pending_actions
            .get_mut(&record.storage_id)
            .ok_or_else(|| "MCP rejection no longer has a pending action.".to_string())?;
        let current = pending.snapshot.status;
        let is_recovered_approved = current == PendingActionStatus::Approved
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&record.storage_id);
        if current != PendingActionStatus::Pending && !is_recovered_approved {
            return Err(
                "MCP rejection was already settled by another approval decision.".to_string(),
            );
        }
        self.persist_pending_dispatch_status(pending, current, PendingActionStatus::Rejected)
            .map_err(|_| {
                "MCP rejection lost its durable status arbitration; continuation was stopped."
                    .to_string()
            })?;
        pending.snapshot.status = PendingActionStatus::Rejected;
        if is_recovered_approved {
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&record.storage_id);
        }
        Ok(())
    }
}
