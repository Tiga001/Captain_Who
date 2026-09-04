impl AgentService {
    pub(in crate::application::agent) fn queue_claimed_mcp_tool_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_mcp_tool_execution(execution_record, call, guard, notifications, true)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
            command_result: None,
            tool_result: None,
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

    pub(in crate::application::agent) async fn execute_auto_mcp_tool_action(
        &self,
        context: &AutoApprovedActionContext,
        approval: Box<mycopilot_core::AgentMcpToolApproval>,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        if approval.approval_mode != mycopilot_core::AgentMcpApprovalMode::Auto
            || approval.call.approval_status != AgentApprovalStatus::Approved
            || approval.identity.run_id != context.run_id
            || approval.identity.call_id != approval.call.id
            || approval.identity.provenance.model_tool_name != approval.call.tool
        {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::structured(
                "mcp.auto_invocation_not_authorized",
                "The MCP invocation is not authorized for automatic execution.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "autoInvocationNotAuthorized",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }
        let Some(invoker) = self.mcp_tool_invoker.clone() else {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::structured(
                "mcp.runtime_unavailable",
                "The MCP invocation runtime is unavailable.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "runtimeUnavailable",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        };
        if self.is_agent_input_scope_deleting(&context.agent_input) {
            self.invalidate_mcp_pending_payload(&AgentProposedAction::McpToolCall {
                approval: approval.clone(),
            });
            return Err(AgentError::cancelled());
        }

        let action = AgentProposedAction::McpToolCall {
            approval: approval.clone(),
        };
        let mut journal = match self.prepare_auto_mcp_action_journal(
            &context.run_id,
            context.conversation_id.as_deref(),
            context.assistant_message_id.as_deref(),
            action.clone(),
            context.agent_input.clone(),
        ) {
            Ok(record) => record,
            Err(_) => {
                self.invalidate_mcp_pending_payload(&action);
                return Err(AgentError::structured(
                    "mcp.auto_dispatch_journal_unavailable",
                    "The MCP invocation could not establish its durable dispatch boundary.",
                    serde_json::json!({
                        "type": "mcp_tool",
                        "code": "autoDispatchJournalUnavailable",
                        "retryable": false,
                        "dispatchCertainty": "definitely_not_dispatched",
                    }),
                ));
            }
        };
        let preflight_result = if cancellation.is_cancelled() {
            Err(AgentError::cancelled())
        } else {
            invoker.revalidate_approved(&approval)
        };
        if preflight_result.is_err() {
            self.invalidate_mcp_pending_payload(&action);
        }

        let started_at = Instant::now();
        let failed_before_dispatch = preflight_result.is_err();
        if failed_before_dispatch {
            let invocation_result = preflight_result.map(|()| unreachable!());
            let settlement =
                settle_mcp_invocation(&approval, invocation_result, failed_before_dispatch);
            let terminal_outcome = if settlement.state == AgentMcpToolInvocationState::Cancelled {
                McpAutoActionJournalTerminalOutcome::Cancelled
            } else {
                McpAutoActionJournalTerminalOutcome::Failed
            };
            let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            let lifecycle_update = McpToolInvocationEventUpdate {
                state: settlement.state,
                dispatch_certainty: settlement.dispatch_certainty,
                outcome: Some(settlement.outcome),
                is_error: mcp_lifecycle_is_error(settlement.outcome),
                error_code: settlement.error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated: settlement.output_truncated,
                result_size: settlement.result_size.clone(),
                failure_stage: settlement.failure_stage,
            };
            let lifecycle = mcp_tool_invocation_event(&approval, lifecycle_update.clone()).ok();
            let _ =
                self.settle_auto_mcp_action_journal(&journal, terminal_outcome, lifecycle.as_ref());
            if let Some(notifications) = context.notifications.as_ref() {
                emit_mcp_lifecycle_event(
                    notifications,
                    &context.run_id,
                    &approval,
                    lifecycle_update,
                );
            }
            return Ok(settlement.tool_result);
        }

        if self.claim_auto_mcp_dispatch(&mut journal).is_err() {
            self.invalidate_mcp_pending_payload(&action);
            let claim_failure = mcp_tool_invocation_event(
                &approval,
                McpToolInvocationEventUpdate {
                    state: AgentMcpToolInvocationState::Failed,
                    dispatch_certainty: AgentMcpDispatchCertainty::DefinitelyNotDispatched,
                    outcome: Some(AgentMcpToolInvocationOutcome::TransportError),
                    is_error: Some(true),
                    error_code: Some("mcp.auto_dispatch_claim_failed"),
                    duration_ms: Some(0),
                    output_truncated: false,
                    result_size: None,
                    failure_stage: Some(AgentMcpInvocationFailureStage::Dispatch),
                },
            )
            .ok();
            let _ = self.settle_auto_mcp_action_journal(
                &journal,
                McpAutoActionJournalTerminalOutcome::Failed,
                claim_failure.as_ref(),
            );
            return Err(AgentError::structured(
                "mcp.auto_dispatch_claim_failed",
                "The MCP invocation lost its durable dispatch claim and was not sent.",
                serde_json::json!({
                    "type": "mcp_tool",
                    "code": "autoDispatchClaimFailed",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }

        if let Some(notifications) = context.notifications.as_ref() {
            emit_mcp_lifecycle_event(
                notifications,
                &context.run_id,
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

        let invocation_result = invoker
            .invoke_approved(
                McpApprovedToolInvocation {
                    approval: approval.as_ref().clone(),
                },
                cancellation,
            )
            .await;
        // The payload is single-use. `invoke_approved` consumes it before dispatch; this delete
        // also closes every fail-before-consume path without creating a replayable payload.
        self.invalidate_mcp_pending_payload(&action);

        let mut settlement = settle_mcp_invocation(&approval, invocation_result, false);
        let journal_outcome = match settlement.state {
            AgentMcpToolInvocationState::Completed => {
                McpAutoActionJournalTerminalOutcome::Completed
            }
            AgentMcpToolInvocationState::OutcomeUnknown => {
                McpAutoActionJournalTerminalOutcome::OutcomeUnknown
            }
            _ => McpAutoActionJournalTerminalOutcome::Failed,
        };
        let elapsed_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        let lifecycle = mcp_tool_invocation_event(
            &approval,
            McpToolInvocationEventUpdate {
                state: settlement.state,
                dispatch_certainty: settlement.dispatch_certainty,
                outcome: Some(settlement.outcome),
                is_error: mcp_lifecycle_is_error(settlement.outcome),
                error_code: settlement.error_code.as_deref(),
                duration_ms: Some(elapsed_ms),
                output_truncated: settlement.output_truncated,
                result_size: settlement.result_size.clone(),
                failure_stage: settlement.failure_stage,
            },
        )
        .ok();
        if self
            .settle_auto_mcp_action_journal(&journal, journal_outcome, lifecycle.as_ref())
            .is_err()
        {
            // A durable terminal receipt could not be proven. Never expose the transient response
            // as authoritative: leave the `executing` row for startup reconciliation and report
            // outcome-unknown without retrying the external call.
            settlement = McpInvocationSettlement {
                tool_result: failed_mcp_tool_result(
                    &approval,
                    "mcp.tool_outcome_unknown",
                    "The MCP invocation may have completed, but its durable result boundary is unknown. Check the authoritative system before deciding whether to try again.",
                    AgentMcpDispatchCertainty::PossiblyDispatched,
                ),
                state: AgentMcpToolInvocationState::OutcomeUnknown,
                outcome: AgentMcpToolInvocationOutcome::OutcomeUnknown,
                error_code: Some("mcp.tool_outcome_unknown".to_string()),
                dispatch_certainty: AgentMcpDispatchCertainty::PossiblyDispatched,
                output_truncated: false,
                result_size: None,
                failure_stage: Some(AgentMcpInvocationFailureStage::Persistence),
            };
        }
        if let Some(notifications) = context.notifications.as_ref() {
            emit_mcp_lifecycle_event(
                notifications,
                &context.run_id,
                &approval,
                McpToolInvocationEventUpdate {
                    state: settlement.state,
                    dispatch_certainty: settlement.dispatch_certainty,
                    outcome: Some(settlement.outcome),
                    is_error: mcp_lifecycle_is_error(settlement.outcome),
                    error_code: settlement.error_code.as_deref(),
                    duration_ms: Some(elapsed_ms),
                    output_truncated: settlement.output_truncated,
                    result_size: settlement.result_size.clone(),
                    failure_stage: settlement.failure_stage,
                },
            );
        }
        Ok(settlement.tool_result)
    }
}
