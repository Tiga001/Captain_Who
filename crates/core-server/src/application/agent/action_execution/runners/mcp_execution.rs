impl AgentService {
    pub(in crate::application::agent) async fn run_mcp_tool_execution(
        &self,
        mut record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
        dispatch_already_claimed: bool,
    ) {
        let run_id = record.snapshot.run_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        let AgentProposedAction::McpToolCall { approval } = record.snapshot.action.clone() else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        let invoker = self.mcp_tool_invoker.clone();
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Cancelled);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Cancelled);
            return;
        }

        let cancellation = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation.clone());
        let guard_cancelled = guard
            .cancel_flag()
            .load(std::sync::atomic::Ordering::SeqCst);
        if guard_cancelled {
            cancellation.cancel();
        }

        let cancelled_before_dispatch = cancellation.is_cancelled();
        let preflight_result = if cancelled_before_dispatch {
            Err(AgentError::cancelled())
        } else if let Some(invoker) = invoker.as_ref() {
            invoker.revalidate_approved(&approval)
        } else {
            Err(AgentError::new("MCP invocation Host is unavailable."))
        };
        if preflight_result.is_err() {
            self.invalidate_mcp_pending_payload(&record.snapshot.action);
        }

        // This durable CAS is the conservative dispatch boundary. Startup recovery treats an
        // interrupted `executing` MCP action as outcome-unknown and never replays it. Catalog,
        // policy, expiry and authenticated-payload validation have already completed while the
        // call was definitely not dispatched; the bridge repeats them after this CAS as its
        // TOCTOU defense.
        if preflight_result.is_ok() && !dispatch_already_claimed {
            if self
                .transition_pending_status_for_dispatch(&record, PendingActionStatus::Executing)
                .is_err()
            {
                self.invalidate_mcp_pending_payload(&record.snapshot.action);
                self.unregister_cancellation_if_current(&run_id, &cancellation);
                return;
            }
            record.snapshot.status = PendingActionStatus::Executing;
        }

        let _guard = guard;
        let started_at = Instant::now();
        if preflight_result.is_ok() {
            emit_mcp_lifecycle_event(
                &notifications,
                &run_id,
                &approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Dispatching,
                    dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
                    outcome: None,
                    is_error: None,
                    error_code: None,
                    duration_ms: None,
                    output_truncated: false,
                    result_size: None,
                    failure_stage: None,
                },
            );
        }

        let failed_before_dispatch = preflight_result.is_err();
        let invocation_result = match preflight_result {
            Err(error) => Err(error),
            Ok(()) => {
                invoker
                    .expect("successful MCP preflight requires a live invocation Host")
                    .invoke_approved(
                        McpApprovedToolInvocation {
                            approval: approval.as_ref().clone(),
                        },
                        cancellation.clone(),
                    )
                    .await
            }
        };
        // `invoke_approved` normally consumes the one-time payload before dispatch. A second
        // TOCTOU revalidation can fail before that consume, so every terminal return performs an
        // idempotent delete as a final cleanup boundary.
        self.invalidate_mcp_pending_payload(&record.snapshot.action);
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

        let settlement =
            settle_mcp_invocation(&approval, invocation_result, failed_before_dispatch);
        let tool_result = settlement.tool_result;
        let invocation_state = settlement.state;
        let invocation_outcome = settlement.outcome;
        let event_error_code = settlement.error_code;
        let dispatch_certainty = settlement.dispatch_certainty;
        let output_truncated = settlement.output_truncated;
        let result_size = settlement.result_size;
        let failure_stage = settlement.failure_stage;

        // The Provider ToolCall frozen in the checkpoint is immutable. Approval is represented by
        // the typed decision, invocation lifecycle and paired ToolResult; rewriting the committed
        // call from `required` to `approved` would violate the append-only trace contract.
        let mut agent_input = record.agent_input.clone();
        agent_input.approval_decision = Some(AgentApprovalDecision {
            action_id: record.snapshot.action_id.clone(),
            status: AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        agent_input.tool_continuation = Some(AgentToolContinuation {
            call: call.clone(),
            result: tool_result.clone(),
        });
        let mut persisted_agent_input = agent_input.clone();
        if let Some(continuation) = persisted_agent_input.tool_continuation.as_mut() {
            continuation.result = persisted_mcp_tool_result(&continuation.result);
        }
        let final_pending_status = if invocation_state == AgentMcpToolInvocationState::Completed {
            PendingActionStatus::Completed
        } else if invocation_state == AgentMcpToolInvocationState::Cancelled {
            PendingActionStatus::Cancelled
        } else {
            PendingActionStatus::Failed
        };
        let completed_at = now_ms();
        if self
            .commit_audited_result_trace_with_continuation(
                &record,
                &persisted_agent_input,
                final_pending_status,
                None,
                completed_at,
                &notifications,
            )
            .is_err()
        {
            // The invocation is never retried. Leaving the durable row in `executing` makes a
            // restart conservatively reconcile it as outcome-unknown.
            self.unregister_cancellation_if_current(&run_id, &cancellation);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                trace_sequence: None,
                message: "The MCP Tool result could not be durably recorded. Its external outcome must be treated as unknown until restart reconciliation completes.".to_string(),
                recoverable: false,
                code: Some("mcp_result_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }

        emit_mcp_lifecycle_event(
            &notifications,
            &run_id,
            &approval,
            McpToolInvocationEventUpdate {
                state: invocation_state,
                dispatch_certainty,
                outcome: Some(invocation_outcome),
                is_error: mcp_lifecycle_is_error(invocation_outcome),
                error_code: event_error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated,
                result_size,
                failure_stage,
            },
        );
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
}
