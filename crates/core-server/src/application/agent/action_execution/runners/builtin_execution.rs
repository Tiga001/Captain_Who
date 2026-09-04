impl AgentService {
    async fn run_builtin_mcp_tool_execution(
        &self,
        mut record: PendingActionRecord,
        call: AgentToolCall,
        grant: mycopilot_core::BuiltinMcpToolGrant,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        let AgentProposedAction::BuiltinMcpToolApproval { mut approval } =
            record.snapshot.action.clone()
        else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            return;
        };
        approval.approval_status = AgentApprovalStatus::Approved;
        let Some(runtime) = self.builtin_capabilities.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            return;
        };
        let cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation.clone());
        if guard.cancel_flag().load(Ordering::SeqCst) {
            cancellation.cancel();
        }
        let _guard = guard;
        let result = if cancellation.is_cancelled() {
            let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
            mycopilot_core::builtin_mcp_tool_cancelled_result(&approval)
        } else {
            if self
                .transition_pending_status_for_dispatch(&record, PendingActionStatus::Executing)
                .is_err()
            {
                let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation);
                return;
            }
            record.snapshot.status = PendingActionStatus::Executing;
            let invocation_runtime = runtime.clone();
            let invocation_approval = (*approval).clone();
            let invocation_cancellation = cancellation.clone();
            supervised_builtin_mcp_tool_result(
                &approval,
                BUILTIN_MCP_APPROVED_INVOCATION_WATCHDOG,
                async move {
                    invocation_runtime
                        .invoke_approved_builtin_mcp_tool(
                            invocation_approval,
                            grant,
                            invocation_cancellation,
                        )
                        .await
                },
            )
            .await
        };
        let final_pending_status = if result.ok {
            PendingActionStatus::Completed
        } else if result
            .result
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str)
            == Some("cancelled")
        {
            PendingActionStatus::Cancelled
        } else {
            PendingActionStatus::Failed
        };
        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.snapshot.action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call: call.clone(),
            result: result.clone(),
        });
        let mut persisted_agent_input = agent_input.clone();
        if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
            continuation.result = persisted_builtin_mcp_tool_result(&continuation.result);
        }
        match self.commit_builtin_mcp_tool_result_or_terminalize(
            &record,
            &persisted_agent_input,
            final_pending_status,
            &notifications,
            &cancellation,
        ) {
            BuiltinMcpToolResultCommitDisposition::Committed => {}
            BuiltinMcpToolResultCommitDisposition::CommittedAndAdvanced
            | BuiltinMcpToolResultCommitDisposition::Terminalized => {
                self.unregister_cancellation_if_current(&run_id, &cancellation);
                return;
            }
        }
        self.emit_safe_builtin_mcp_tool_result(&notifications, &run_id, &result);
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(cancellation.clone()),
        )
        .await;
        self.unregister_cancellation_if_current(&run_id, &cancellation);
    }

    pub(in crate::application::agent) fn commit_builtin_mcp_tool_result_or_terminalize(
        &self,
        record: &PendingActionRecord,
        persisted_agent_input: &AgentChatInput,
        final_pending_status: PendingActionStatus,
        notifications: &CoreServerNotificationSender,
        cancellation: &AgentCancellationToken,
    ) -> BuiltinMcpToolResultCommitDisposition {
        let completed_at = now_ms();
        match self.commit_audited_result_trace_with_continuation(
            record,
            persisted_agent_input,
            final_pending_status,
            None,
            completed_at,
            notifications,
        ) {
            Ok(()) => BuiltinMcpToolResultCommitDisposition::Committed,
            Err(_) => {
                let inspection = self.inspect_audited_result_trace_with_continuation(
                    record,
                    persisted_agent_input,
                    final_pending_status,
                    None,
                    completed_at,
                );
                match inspection {
                    Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                        BuiltinMcpToolResultCommitDisposition::Committed
                    }
                    Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                        BuiltinMcpToolResultCommitDisposition::CommittedAndAdvanced
                    }
                    Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted)
                    | Ok(AgentPendingActionSettlementInspection::Diverged { .. })
                    | Err(_) => {
                        self.terminalize_builtin_mcp_tool_without_receipt(
                            record,
                            notifications,
                            cancellation,
                        );
                        BuiltinMcpToolResultCommitDisposition::Terminalized
                    }
                }
            }
        }
    }

    pub(in crate::application::agent) fn emit_safe_builtin_mcp_tool_result(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        result: &AgentToolResult,
    ) {
        let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
            run_id: run_id.to_string(),
            result: persisted_builtin_mcp_tool_result(result),
        }));
    }

    fn terminalize_builtin_mcp_tool_without_receipt(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        cancellation: &AgentCancellationToken,
    ) {
        const FAILURE_CODE: &str = "builtin_mcp_result_persistence_failed";
        const FAILURE_MESSAGE: &str = "The sensitive built-in MCP Tool did not produce a durable terminal receipt. It will not be replayed; its external outcome is unknown.";
        let run_id = &record.snapshot.run_id;
        let AgentProposedAction::BuiltinMcpToolApproval { approval } = &record.snapshot.action
        else {
            self.unregister_cancellation_if_current(run_id, cancellation);
            return;
        };
        let (outcome, result) = if record.snapshot.status == PendingActionStatus::Executing {
            (
                McpStartupActionTerminalOutcome::OutcomeUnknown,
                mycopilot_core::builtin_mcp_tool_outcome_unknown_result(approval),
            )
        } else {
            (
                McpStartupActionTerminalOutcome::PayloadUnavailable,
                mycopilot_core::builtin_mcp_tool_payload_unavailable_result(approval),
            )
        };
        let mut terminalized = false;
        let mut lost_terminal_fence = false;
        for _ in 0..2 {
            match self
                .storage
                .terminalize_builtin_mcp_tool_agent_action_on_startup(
                    &record.storage_id,
                    pending_status_label(record.snapshot.status),
                    outcome,
                    now_ms(),
                ) {
                Ok(true) => {
                    terminalized = true;
                    break;
                }
                Ok(false) => {
                    lost_terminal_fence = true;
                    break;
                }
                Err(_) => {}
            }
        }
        if lost_terminal_fence {
            self.unregister_cancellation_if_current(run_id, cancellation);
            return;
        }

        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        self.pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&record.storage_id);
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&record.storage_id);
        self.finish_persisted_run_usage(run_id, AgentRunStatus::Failed);
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.invalidate_conversation_context_state(conversation_id);
            self.release_conversation_turn_if_current(conversation_id, run_id);
        }
        self.release_turn_concurrency_permit(run_id);
        if terminalized {
            if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
                self.notify_durable_turn_observers(assistant_message_id);
            }
        }
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);
        self.unregister_cancellation_if_current(run_id, cancellation);

        self.emit_safe_builtin_mcp_tool_result(notifications, run_id, &result);
        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
            run_id: Some(run_id.clone()),
            trace_sequence: None,
            message: FAILURE_MESSAGE.to_string(),
            recoverable: false,
            code: Some(FAILURE_CODE.to_string()),
            details: None,
        }));
        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
            run_id: run_id.clone(),
            success: false,
            status: Some(AgentRunStatus::Failed),
            content: Some(FAILURE_MESSAGE.to_string()),
            usage: self.preview_cumulative_run_usage(run_id, None),
            finish_reason: None,
            proposed_actions: Vec::new(),
        }));
    }
}
