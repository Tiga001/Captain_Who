use super::*;

pub(super) fn publish_inline_file_write_tool_result(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    action: &AgentProposedAction,
    decision_status: AgentApprovalDecisionStatus,
    tool_result: &AgentToolResult,
) -> bool {
    let should_publish = matches!(action, AgentProposedAction::FileWrite { .. })
        || (decision_status == AgentApprovalDecisionStatus::Rejected
            && matches!(action, AgentProposedAction::OfficeOperation { .. }));
    if !should_publish {
        return false;
    }
    let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
        run_id: run_id.to_string(),
        result: tool_result.clone(),
    }));
    true
}

impl AgentService {
    pub(super) fn validate_provider_continuations_before_dispatch(
        &self,
        record: &PendingActionRecord,
    ) -> Result<(), String> {
        let Some(checkpoint) = record.agent_input.resume_checkpoint.as_ref() else {
            return Ok(());
        };
        let has_provider_tool_calls = !checkpoint
            .assistant_turn_identity
            .tool_call_identities
            .is_empty();
        checkpoint
            .provider_protocol_key
            .validate_against_config(&checkpoint.provider_profile_config)
            .map_err(|error| {
                format!(
                    "provider_context_boundary_required: frozen Provider profile/key mismatch before approved action dispatch: {error}"
                )
            })?;
        let capabilities = mycopilot_core::resolve_provider_runtime_capabilities(
            &checkpoint.provider_protocol_key,
        )
        .map_err(|error| {
            format!(
                "provider_context_boundary_required: Provider runtime capability unavailable before approved action dispatch: {error}"
            )
        })?;
        let requires_provider_continuation = capabilities
            .classify_turn(
                has_provider_tool_calls,
                !checkpoint.provider_continuation_refs.is_empty(),
                checkpoint.provider_profile_config.reasoning.mode,
            )
            .requires_exact_approval_refs();
        if checkpoint.provider_continuation_refs.is_empty() {
            return if requires_provider_continuation {
                Err("provider_continuation_missing: approved action was not dispatched".to_string())
            } else {
                Ok(())
            };
        }
        let conversation_id = record.snapshot.conversation_id.as_deref().ok_or_else(|| {
            "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                .to_string()
        })?;
        let assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .filter(|assistant_message_id| !assistant_message_id.trim().is_empty())
            .ok_or_else(|| {
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string()
            })?;
        let run_id = record.snapshot.run_id.trim();
        if run_id.is_empty() {
            return Err(
                "provider_continuation.invalid_approval_scope: approved action was not dispatched"
                    .to_string(),
            );
        }
        self.provider_continuation_vault
            .as_deref()
            .ok_or_else(|| {
                "provider_continuation.credential_unavailable: approved action was not dispatched"
                    .to_string()
            })?
            .validate_approval_checkpoint_refs(
                conversation_id,
                assistant_message_id,
                run_id,
                &checkpoint.provider_protocol_key,
                &checkpoint.provider_continuation_refs,
                &checkpoint.assistant_turn_identity,
            )
            .map_err(|error| format!("{}: approved action was not dispatched", error.code()))
    }

    pub fn list_pending_actions(&self) -> Vec<PendingAgentActionSnapshot> {
        let pending_actions = self
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let startup_recoverable_mcp_approvals = self
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut snapshots = pending_actions
            .iter()
            .filter(|(storage_id, record)| {
                record.snapshot.status == PendingActionStatus::Pending
                    || (record.snapshot.status == PendingActionStatus::Approved
                        && startup_recoverable_mcp_approvals.contains(*storage_id))
            })
            .map(|(_, record)| record.snapshot.clone())
            .collect::<Vec<_>>();
        snapshots.sort_by_key(|snapshot| snapshot.created_at);
        snapshots
    }

    pub fn read_file_draft(
        &self,
        draft_id: &str,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileDraftContentPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
        let snapshot = file_draft_snapshot(&draft)?;
        let (content, offset, next_offset, truncated) =
            paginate_chars(&draft.content, offset, max_chars);
        Ok(AgentFileDraftContentPage {
            draft: snapshot,
            content,
            offset,
            next_offset,
            truncated,
        })
    }

    pub fn get_file_write_diff(
        &self,
        draft_id: &str,
        offset: Option<usize>,
        max_chars: Option<usize>,
    ) -> Result<AgentFileWriteDiffPage, String> {
        let draft = self
            .storage
            .get_agent_file_draft(draft_id)?
            .ok_or_else(|| format!("未找到文件草稿：{draft_id}"))?;
        let diff = file_write_diff(&draft);
        let (patch, offset, next_offset, truncated) = paginate_chars(&diff, offset, max_chars);
        Ok(AgentFileWriteDiffPage {
            draft_id: draft.id,
            patch,
            offset,
            next_offset,
            truncated,
        })
    }

    pub fn approve_action(
        &self,
        run_id: &str,
        action_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Approved,
            None,
            notifications,
        )
    }

    pub fn reject_action(
        &self,
        run_id: &str,
        action_id: &str,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        self.queue_action_continuation(
            run_id,
            action_id,
            AgentApprovalDecisionStatus::Rejected,
            message,
            notifications,
        )
    }

    pub fn cancel_action(&self, run_id: &str, action_id: &str) -> Result<bool, String> {
        if self
            .try_cancel_recovered_approved_mcp_action(run_id, action_id)?
            .is_some()
        {
            return Ok(true);
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
            return Ok(false);
        };
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Ok(false);
        }
        let is_cancellable_process = matches!(
            record.snapshot.action,
            AgentProposedAction::Command { .. }
                | AgentProposedAction::SkillScript { .. }
                | AgentProposedAction::OfficeOperation { .. }
                | AgentProposedAction::McpToolCall { .. }
        );
        let is_mcp_dispatching = record.snapshot.status == PendingActionStatus::Executing
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
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
            if cancelled {
                if let Some(token) = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&record.snapshot.run_id)
                {
                    token.cancel();
                }
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
            }
            return Ok(cancelled);
        }
        if record.snapshot.status != PendingActionStatus::Pending {
            return Ok(false);
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
            let service = self
                .skill_installation
                .as_ref()
                .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
            service
                .reject_action(installation, conversation_id, &record.snapshot.run_id)
                .map_err(|error| error.to_string())?;
        }
        if let Err(error) = self.finalize_cancelled_pending_action(&record, call) {
            if cancelled_file_write_outcome_is_durable_or_unknown(&self.storage, &record) {
                return Err(format!(
                    "{error} 文件草稿的拒绝结果已经持久化或无法安全判定；待审批操作保持 executing 并保留 cancelled 目标终态，等待启动对账，禁止重新执行。"
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
        self.transition_pending_status(&record, PendingActionStatus::Cancelled)?;
        Ok(true)
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
        Ok(Some(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: pending_status_label(PendingActionStatus::Cancelled).to_string(),
            patch_result: None,
            file_write_result: None,
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
        let execution = action_execution_for_decision(
            &self.storage,
            record,
            &call,
            AgentApprovalDecisionStatus::Rejected,
            Some(REASON),
        );
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
            let trace = cancelled_conversation_trace_from_checkpoint(
                checkpoint,
                conversation_id,
                assistant_message_id,
                &call,
                &execution.tool_result,
                &model_observation,
                REASON,
            );
            let usage_record = self.prepare_run_usage_record(
                &record.snapshot.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(REASON.to_string()),
            );
            self.storage
                .finalize_chat_message_with_conversation_trace_and_usage(
                    conversation_id,
                    assistant_message_id,
                    "",
                    status_for_run(AgentRunStatus::Cancelled),
                    run_status_label(AgentRunStatus::Cancelled),
                    &trace,
                    completed_at,
                    completed_at,
                    usage_record.as_ref(),
                )?;
            self.finish_persisted_run_usage(&record.snapshot.run_id, AgentRunStatus::Cancelled);
            self.invalidate_conversation_context_state(conversation_id);
            self.discard_trace_snapshot(&record.snapshot.run_id);
        }
        self.record_action_audit(
            record,
            Some("cancelled"),
            "cancelled",
            execution.patch_result.as_ref(),
            None,
            Some(&execution.tool_result),
            Some(REASON),
            Some(completed_at),
            Some(completed_at),
        );
        Ok(())
    }

    /// Commits the exact pre-dispatch rejection receipt and resolves a commit-unknown response
    /// from durable state. Every retry and inspection uses the same timestamp so the audit and
    /// trace identity remain byte-for-byte stable.
    fn commit_rejected_mcp_receipt(
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
        self.persist_pending_status(pending, current, PendingActionStatus::Rejected)
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

    pub(super) fn queue_action_continuation(
        &self,
        run_id: &str,
        action_id: &str,
        decision_status: AgentApprovalDecisionStatus,
        message: Option<String>,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        if decision_status == AgentApprovalDecisionStatus::Approved {
            if let Some(output) =
                self.try_resume_approved_mcp_action(run_id, action_id, notifications.clone())?
            {
                return Ok(output);
            }
        }
        let mut deletion_lifecycle = Some(
            self.deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
        );
        let (
            record,
            call,
            approved_process_guard,
            approved_materialization_guard,
            inline_continuation_guard,
        ) = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(storage_id) =
                resolve_pending_action_storage_id(&pending_actions, run_id, action_id)
            else {
                return Err(format!(
                    "未找到待审批操作：runId={run_id}, actionId={action_id}"
                ));
            };
            let record = pending_actions
                .get_mut(&storage_id)
                .expect("resolved pending action exists");
            let is_rejected_mcp = decision_status == AgentApprovalDecisionStatus::Rejected
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::McpToolCall { .. }
                );
            let is_recovered_approved_mcp_rejection = is_rejected_mcp
                && record.snapshot.status == PendingActionStatus::Approved
                && self
                    .startup_recoverable_mcp_approvals
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .contains(&storage_id);
            if record.snapshot.status != PendingActionStatus::Pending
                && !is_recovered_approved_mcp_rejection
            {
                return Err(format!("待审批操作已经处理：{action_id}"));
            }
            if deletion_lifecycle
                .as_ref()
                .expect("deletion lifecycle guard is held while preparing approval")
                .contains_input(&record.agent_input)
            {
                return Err("项目或会话正在移除，无法处理待审批操作。".to_string());
            }
            if decision_status == AgentApprovalDecisionStatus::Approved {
                if let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action {
                    let now = self.mcp_approval_now_ms();
                    if approval.expires_at <= now {
                        let retired = self
                            .storage
                            .terminalize_mcp_agent_action_on_startup(
                                &record.storage_id,
                                "pending",
                                McpStartupActionTerminalOutcome::Expired,
                                now,
                            )
                            .map_err(|_| {
                                "MCP approval expiry could not be persisted safely.".to_string()
                            })?;
                        if !retired {
                            return Err("MCP approval changed while its expiry was being settled."
                                .to_string());
                        }
                        record.snapshot.status = PendingActionStatus::Failed;
                        self.invalidate_mcp_pending_payload(&record.snapshot.action);
                        return Err(
                            "MCP approval expired before dispatch; the tool was not invoked."
                                .to_string(),
                        );
                    }
                }
                if let Err(error) = self.validate_provider_continuations_before_dispatch(record) {
                    // Publish the durable terminal intent before replacing the resume payload.
                    // A crash between these writes leaves a non-dispatchable pending row that
                    // startup reconciliation can finish; it can never fall back to approval.
                    self.storage.set_pending_agent_action_target_status(
                        &record.storage_id,
                        pending_status_label(PendingActionStatus::Pending),
                        pending_status_label(PendingActionStatus::Cancelled),
                        now_ms(),
                    )?;
                    self.persist_pending_status(
                        record,
                        PendingActionStatus::Pending,
                        PendingActionStatus::Cancelled,
                    )?;
                    record.snapshot.status = PendingActionStatus::Cancelled;
                    self.invalidate_mcp_pending_payload(&record.snapshot.action);
                    self.record_action_audit(
                        record,
                        Some("blocked"),
                        "cancelled",
                        None,
                        None,
                        None,
                        Some("Provider continuation validation failed before dispatch."),
                        Some(now_ms()),
                        Some(now_ms()),
                    );
                    return Err(error);
                }
            }
            let call = tool_call_for_pending_record(record)?;
            if decision_status == AgentApprovalDecisionStatus::Approved {
                authorize_structured_file_write(
                    &record.agent_input,
                    &record.snapshot.action,
                    FileWriteAuthorizationSource::ExplicitUser,
                )
                .map_err(|error| error.to_string())?;
            }
            let is_approved_process = decision_status == AgentApprovalDecisionStatus::Approved
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::Command { .. }
                        | AgentProposedAction::SkillScript { .. }
                        | AgentProposedAction::OfficeOperation { .. }
                        | AgentProposedAction::McpToolCall { .. }
                );
            let is_approved_materialization = decision_status
                == AgentApprovalDecisionStatus::Approved
                && matches!(
                    record.snapshot.action,
                    AgentProposedAction::SkillMaterialization { .. }
                );
            let approved_process_guard = is_approved_process.then(|| {
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let approved_materialization_guard = is_approved_materialization.then(|| {
                // The shared lifecycle lock is held, so direct registration is atomic with the
                // project/conversation tombstone check above and cannot race deletion.
                self.file_effects.register(
                    agent_input_project_id(&record.agent_input),
                    agent_input_conversation_id(&record.agent_input),
                    &record.snapshot.run_id,
                    &record.storage_id,
                )
            });
            let inline_continuation_guard = (!is_approved_process && !is_rejected_mcp).then(|| {
                // Synchronous decisions still queue an async model continuation. Register its
                // pre-spawn lease under the lifecycle lock so message deletion cannot pass
                // between durable action settlement and worker startup.
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id)
            });
            let execution_status = if is_approved_process || is_approved_materialization {
                PendingActionStatus::Approved
            } else if is_rejected_mcp {
                // Rejection is definitely pre-dispatch. Keep either the original `pending`
                // state or a startup-recovered `approved` state until the paired audit,
                // target and ToolResult receipt commit atomically. `executing` remains reserved
                // for the conservative external-dispatch boundary.
                record.snapshot.status
            } else {
                PendingActionStatus::Executing
            };
            if execution_status != record.snapshot.status {
                self.persist_pending_status(
                    record,
                    PendingActionStatus::Pending,
                    execution_status,
                )?;
                record.snapshot.status = execution_status;
            }
            (
                record.clone(),
                call,
                approved_process_guard,
                approved_materialization_guard,
                inline_continuation_guard,
            )
        };
        let mut approved_materialization_guard = approved_materialization_guard;
        let mut inline_continuation_guard = inline_continuation_guard;

        let mut call = call;
        let is_mcp_action = matches!(
            record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        );
        if !is_mcp_action {
            call.approval_status = match decision_status {
                AgentApprovalDecisionStatus::Approved => AgentApprovalStatus::Approved,
                AgentApprovalDecisionStatus::Rejected => AgentApprovalStatus::Rejected,
            };
        }
        let decided_at = now_ms();
        if decision_status == AgentApprovalDecisionStatus::Rejected && !is_mcp_action {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(record.snapshot.action, AgentProposedAction::Command { .. })
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_command_execution(
                record,
                call,
                approved_process_guard.expect("approved command registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillScript { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_skill_script_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Skill script registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::OfficeOperation { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_office_operation_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved Office operation registered under pending lock"),
                notifications,
            );
        }
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
        {
            self.record_action_audit(
                &record,
                Some("approved"),
                "approved",
                None,
                None,
                None,
                None,
                Some(decided_at),
                None,
            );
            drop(deletion_lifecycle);
            return self.queue_mcp_tool_execution(
                record,
                call,
                approved_process_guard
                    .expect("approved MCP invocation registered under pending lock"),
                notifications,
            );
        }

        let is_approved_materialization = decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillMaterialization { .. }
            );
        if is_approved_materialization {
            // The lease was registered atomically with the deletion-marker check above. Release
            // the global lifecycle mutex before resource restoration and filesystem I/O so a
            // deletion can install its marker and use the bounded FileEffectTracker drain.
            drop(deletion_lifecycle.take());
        }

        let mut continuation_message = message.clone();
        if decision_status == AgentApprovalDecisionStatus::Rejected {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let conversation_id =
                    record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                        "Skill installation approval has no conversation identity.".to_string()
                    })?;
                let service = self
                    .skill_installation
                    .as_ref()
                    .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
                service
                    .reject_action(installation, conversation_id, &record.snapshot.run_id)
                    .map_err(|error| error.to_string())?;
            }
        }
        let execution = if decision_status == AgentApprovalDecisionStatus::Rejected && is_mcp_action
        {
            let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                unreachable!("typed MCP action was checked above");
            };
            let tool_result = mcp_tool_result_from_rejected_approval(approval, message.as_deref())
                .map_err(|error| error.to_string())?;
            continuation_message = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("userFeedback"))
                .and_then(serde_json::Value::as_str)
                .map(ToString::to_string);
            ActionExecutionDecision {
                status: "rejected".to_string(),
                final_pending_status: PendingActionStatus::Rejected,
                patch_result: None,
                file_write_result: None,
                file_change: None,
                tool_result,
            }
        } else if decision_status == AgentApprovalDecisionStatus::Approved {
            if let AgentProposedAction::SkillInstallation { installation } = &record.snapshot.action
            {
                let mut installation = (**installation).clone();
                installation.approval_status = AgentApprovalStatus::Approved;
                let conversation_id =
                    record.snapshot.conversation_id.as_deref().ok_or_else(|| {
                        "Skill installation approval has no conversation identity.".to_string()
                    })?;
                let service = self
                    .skill_installation
                    .as_ref()
                    .ok_or_else(|| "Skill installation Host is unavailable.".to_string())?;
                let tool_result = service.commit_approved(
                    &installation,
                    conversation_id,
                    &record.snapshot.run_id,
                );
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "installed".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    patch_result: None,
                    file_write_result: None,
                    file_change: None,
                    tool_result,
                }
            } else if let AgentProposedAction::SkillMaterialization { materialization } =
                &record.snapshot.action
            {
                let mut materialization = materialization.clone();
                materialization.approval_status = AgentApprovalStatus::Approved;
                let tool_result = match self.restore_skill_resource_session(&record.agent_input) {
                    Ok(resources) => {
                        approved_materialization_guard
                            .as_mut()
                            .expect(
                                "approved Skill materialization registered under lifecycle lock",
                            )
                            .mark_effects_started();
                        self.execute_skill_materialization(
                            &record.agent_input,
                            &materialization,
                            resources.as_deref(),
                        )
                    }
                    Err(error) => AgentToolResult {
                        exact_archive_file: None,
                        call_id: materialization.id.clone(),
                        tool: "skills_materialize_resource".to_string(),
                        ok: false,
                        result: Some(serde_json::json!({
                            "type": "skill_materialization",
                            "code": "snapshotUnavailable",
                            "recovery": "reactivateSkill",
                        })),
                        error: Some(error.to_string()),
                    },
                };
                ActionExecutionDecision {
                    status: if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    },
                    final_pending_status: if tool_result.ok {
                        PendingActionStatus::Completed
                    } else {
                        PendingActionStatus::Failed
                    },
                    patch_result: None,
                    file_write_result: None,
                    file_change: None,
                    tool_result,
                }
            } else {
                action_execution_for_decision(
                    &self.storage,
                    &record,
                    &call,
                    decision_status,
                    message.as_deref(),
                )
            }
        } else {
            action_execution_for_decision(
                &self.storage,
                &record,
                &call,
                decision_status,
                message.as_deref(),
            )
        };
        let mut final_pending_status = execution.final_pending_status;
        let mut tool_result = execution.tool_result.clone();
        let mut execution_status = execution.status.clone();
        if decision_status == AgentApprovalDecisionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::SkillInstallation { .. }
            )
        {
            let commit_may_have_succeeded = tool_result
                .result
                .as_ref()
                .and_then(|value| value.get("commitMayHaveSucceeded"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if tool_result.ok || commit_may_have_succeeded {
                if let (Some(installations), Some(workflow)) = (
                    self.skill_installation_service.as_deref(),
                    self.skill_installation_workflow.as_deref(),
                ) {
                    crate::transport::notify_skills_changed(
                        &self.storage,
                        &self.skills,
                        installations,
                        Some(workflow),
                        Some(&notifications),
                        if tool_result.ok {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::Installed
                        } else {
                            mycopilot_protocol_rs::SkillsChangedReasonDto::CatalogChanged
                        },
                        None,
                    );
                }
            }
        }
        let is_rejected_mcp =
            decision_status == AgentApprovalDecisionStatus::Rejected && is_mcp_action;
        let agent_input = if is_rejected_mcp {
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: record.snapshot.action_id.clone(),
                status: decision_status,
                message: continuation_message.clone(),
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            let mut persisted_agent_input = agent_input.clone();
            if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
                continuation.result =
                    mycopilot_core::mcp_tool_result_persistence_projection(&continuation.result);
            }
            let completed_at = now_ms();
            self.commit_rejected_mcp_receipt(
                &record,
                &persisted_agent_input,
                completed_at,
                &notifications,
            )?;
            self.claim_rejected_mcp_continuation(&record)?;
            // Only the durable status-CAS winner owns the pre-spawn lease. Registering before
            // arbitration would let a duplicate reject overwrite the winner's action-keyed guard
            // and unregister it when the losing guard drops.
            inline_continuation_guard = Some(
                self.process_runs
                    .register(&record.storage_id, &record.snapshot.run_id),
            );
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
                unreachable!("typed MCP rejection was checked before settlement");
            };
            if let Ok(invocation) = mcp_tool_invocation_event(
                approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Rejected,
                    dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                    outcome: Some(AgentMcpToolInvocationOutcome::Rejected),
                    is_error: None,
                    error_code: Some("mcp.approval_rejected"),
                    duration_ms: None,
                    output_truncated: false,
                    result_size: None,
                    failure_stage: None,
                },
            ) {
                let _ = notifications.send(agent_event_notification(
                    AgentEvent::McpToolInvocationStateChanged {
                        run_id: record.snapshot.run_id.clone(),
                        invocation,
                    },
                ));
            }
            agent_input
        } else if is_approved_materialization {
            match self.settle_manual_file_effect(
                &record,
                &call,
                final_pending_status,
                tool_result,
                "skill_materialization",
                &notifications,
            ) {
                ManualFileEffectSettlement::Committed {
                    agent_input,
                    tool_result: settled_result,
                    pending_status,
                } => {
                    final_pending_status = pending_status;
                    tool_result = settled_result;
                    execution_status = if tool_result.ok {
                        "applied".to_string()
                    } else {
                        "failed".to_string()
                    };
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    *agent_input
                }
                ManualFileEffectSettlement::CommittedAndAdvanced => {
                    approved_materialization_guard
                        .as_mut()
                        .expect("approved Skill materialization has an effect guard")
                        .mark_durably_settled();
                    return Err(
                        "Skill materialization receipt was already advanced by another continuation; duplicate continuation was stopped."
                            .to_string(),
                    );
                }
                ManualFileEffectSettlement::Unsettled => {
                    return Err(
                        "Skill materialization finished without a confirmed durable terminal receipt; inspect state before retrying."
                            .to_string(),
                    );
                }
            }
        } else {
            self.record_action_audit(
                &record,
                Some(match decision_status {
                    AgentApprovalDecisionStatus::Approved => "approved",
                    AgentApprovalDecisionStatus::Rejected => "rejected",
                }),
                &execution.status,
                execution.patch_result.as_ref(),
                None,
                Some(&tool_result),
                tool_result.error.as_deref(),
                Some(decided_at),
                Some(now_ms()),
            );
            self.persist_pending_target_status(&record, final_pending_status)?;

            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: record.snapshot.action_id.clone(),
                status: decision_status,
                message: continuation_message.clone(),
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });
            self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)?;
            agent_input
        };
        let run_id = record.snapshot.run_id.clone();
        let inline_continuation_guard = inline_continuation_guard
            .expect("every synchronous approval decision owns a pre-spawn continuation lease");
        let continuation_cancel_flag = inline_continuation_guard.cancel_flag();
        let continuation_cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, continuation_cancellation.clone());
        // Inline diff/file-write actions keep the shared lifecycle lock until their target status
        // and paired ToolResult trace are durable. Materialization and asynchronous process paths
        // use a FileEffectGuard, so deletion can observe and drain them without an unbounded mutex
        // wait.
        drop(approved_materialization_guard);
        drop(deletion_lifecycle);
        let publish_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !publish_lifecycle.contains_input(&record.agent_input)
            && publish_inline_file_write_tool_result(
                &notifications,
                &record.snapshot.run_id,
                &record.snapshot.action,
                decision_status,
                &tool_result,
            )
        {
            // Approved Office operations return earlier and publish exactly one ToolResult from
            // their asynchronous executor. Rejected Office actions reach this synchronous path,
            // so publish the paired rejection result here just like write_file.
            if matches!(
                record.snapshot.action,
                AgentProposedAction::FileWrite { .. }
            ) {
                if let Some(file_write_result) = execution.file_write_result.as_ref() {
                    if let Ok(Some(draft)) = self
                        .storage
                        .get_agent_file_draft(&file_write_result.draft_id)
                    {
                        if let Ok(snapshot) = file_draft_snapshot(&draft) {
                            let _ = notifications.send(agent_event_notification(
                                AgentEvent::FileDraftUpdated {
                                    run_id: record.snapshot.run_id.clone(),
                                    draft: snapshot,
                                },
                            ));
                        }
                    }
                }
            }
        }
        drop(publish_lifecycle);

        let service = self.clone();
        let continuation_record = record.clone();
        tokio::spawn(async move {
            // Deletion may have cancelled the pre-spawn lease before this task was first polled.
            // Translate that signal without consulting the lifecycle mutex so the worker can
            // release its lease while deletion holds the marker and waits for a bounded drain.
            if continuation_cancel_flag.load(Ordering::SeqCst) {
                continuation_cancellation.cancel();
            }
            service
                .run_action_continuation(
                    continuation_record,
                    agent_input,
                    notifications,
                    final_pending_status,
                    Some(continuation_cancellation),
                )
                .await;
            drop(inline_continuation_guard);
        });

        let renderer_tool_result = if matches!(
            record.snapshot.action,
            AgentProposedAction::McpToolCall { .. }
        ) {
            None
        } else {
            Some(tool_result)
        };
        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: execution_status,
            patch_result: execution.patch_result,
            file_write_result: execution.file_write_result,
            command_result: None,
            // MCP has a dedicated approval/lifecycle contract. Keep the generic result response
            // for built-ins and runtime extensions only.
            tool_result: renderer_tool_result,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    /// Explicitly resumes an MCP approval which was durably `approved` but had not crossed the
    /// dispatch boundary before restart.
    ///
    /// The same public approve API is intentionally reused: a second explicit user action performs
    /// complete current Host/catalog/payload revalidation, claims the durable dispatch boundary,
    /// and only then queues execution. Concurrent attempts lose the `approved -> executing` CAS.
    fn try_resume_approved_mcp_action(
        &self,
        run_id: &str,
        action_id: &str,
        notifications: CoreServerNotificationSender,
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
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
        if record.snapshot.status != PendingActionStatus::Approved
            || !matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
            || !self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&storage_id)
        {
            return Ok(None);
        }
        if deletion_lifecycle.contains_input(&record.agent_input) {
            return Err("项目或会话正在移除，无法恢复 MCP 操作。".to_string());
        }
        if let Err(error) = self.validate_provider_continuations_before_dispatch(record) {
            let retired = self
                .storage
                .terminalize_mcp_agent_action_on_startup(
                    &record.storage_id,
                    "approved",
                    McpStartupActionTerminalOutcome::PayloadUnavailable,
                    self.mcp_approval_now_ms(),
                )
                .map_err(|_| {
                    "Recovered MCP approval could not be retired before dispatch.".to_string()
                })?;
            if !retired {
                return Err(
                    "Recovered MCP approval changed while Provider continuation validation was being settled."
                        .to_string(),
                );
            }
            record.snapshot.status = PendingActionStatus::Failed;
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner())
                .remove(&storage_id);
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            return Err(error);
        }
        let AgentProposedAction::McpToolCall { approval } = &record.snapshot.action else {
            unreachable!("MCP action was checked above");
        };
        let now = self.mcp_approval_now_ms();
        if approval.expires_at <= now {
            let retired = self
                .storage
                .terminalize_mcp_agent_action_on_startup(
                    &record.storage_id,
                    "approved",
                    McpStartupActionTerminalOutcome::Expired,
                    now,
                )
                .map_err(|_| "MCP approval expiry could not be persisted safely.".to_string())?;
            if !retired {
                return Err("MCP approval changed while its expiry was being settled.".to_string());
            }
            record.snapshot.status = PendingActionStatus::Failed;
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&storage_id);
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            return Err(
                "MCP approval expired before dispatch; the tool was not invoked.".to_string(),
            );
        }
        let invoker = self
            .mcp_tool_invoker
            .as_ref()
            .ok_or_else(|| "MCP invocation Host is unavailable.".to_string())?;
        invoker
            .revalidate_approved(approval)
            .map_err(|error| error.to_string())?;
        // Preserve the frozen Provider ToolCall exactly as it appeared in the checkpoint.
        // Approval is carried by the typed action journal and lifecycle, not by rewriting the
        // append-only call from `required` to `approved` after restart.
        let call = tool_call_for_pending_record(record)?;
        self.persist_pending_status(
            record,
            PendingActionStatus::Approved,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&storage_id);
        let guard = self
            .process_runs
            .register(&record.storage_id, &record.snapshot.run_id);
        let record = record.clone();
        drop(pending_actions);
        drop(deletion_lifecycle);
        self.queue_claimed_mcp_tool_execution(record, call, guard, notifications)
            .map(Some)
    }
}

pub(super) fn paginate_chars(
    value: &str,
    offset: Option<usize>,
    max_chars: Option<usize>,
) -> (String, usize, Option<usize>, bool) {
    let offset = offset.unwrap_or(0);
    let limit = max_chars.unwrap_or(50_000).clamp(1_000, 100_000);
    let total = value.chars().count();
    let start = offset.min(total);
    let end = start.saturating_add(limit).min(total);
    let content = value.chars().skip(start).take(end - start).collect();
    let truncated = end < total;
    (content, start, truncated.then_some(end), truncated)
}
