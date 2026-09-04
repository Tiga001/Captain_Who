impl AgentService {
    pub(in crate::application::agent) async fn execute_auto_builtin_mcp_tool_action(
        &self,
        context: &AutoApprovedActionContext,
        approval: Box<mycopilot_core::AgentBuiltinMcpToolApproval>,
        cancellation: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        let action = AgentProposedAction::BuiltinMcpToolApproval {
            approval: approval.clone(),
        };
        let permissions = permissions_from_input(&context.agent_input);
        if permissions.builtin_execution
            != mycopilot_core::AgentBuiltinExecutionPermission::AutoApprove
            || approval.approval_status != AgentApprovalStatus::Approved
            || approval.identity.run_id != context.run_id
        {
            self.invalidate_mcp_pending_payload(&action);
            return Err(AgentError::structured(
                "builtin_mcp.auto_invocation_not_authorized",
                "The built-in MCP invocation is not authorized for automatic execution.",
                serde_json::json!({
                    "type": "builtin_mcp_tool_approval",
                    "code": "autoInvocationNotAuthorized",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }
        let Some(runtime) = self.builtin_capabilities.clone() else {
            self.invalidate_mcp_pending_payload(&action);
            return Err(AgentError::structured(
                "builtin_mcp.runtime_unavailable",
                "The built-in MCP invocation runtime is unavailable.",
                serde_json::json!({
                    "type": "builtin_mcp_tool_approval",
                    "code": "runtimeUnavailable",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        };
        if self.is_agent_input_scope_deleting(&context.agent_input) {
            self.invalidate_mcp_pending_payload(&action);
            return Err(AgentError::cancelled());
        }

        // The hidden approved row is the durable definitely-not-dispatched state. It is never
        // projected as a user approval; startup retires it as payload-unavailable rather than
        // replaying the process-only arguments or grant.
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
                    "builtin_mcp.auto_dispatch_journal_unavailable",
                    "The built-in MCP invocation could not establish its durable dispatch boundary.",
                    serde_json::json!({
                        "type": "builtin_mcp_tool_approval",
                        "code": "autoDispatchJournalUnavailable",
                        "retryable": false,
                        "dispatchCertainty": "definitely_not_dispatched",
                    }),
                ));
            }
        };

        let grant = match runtime.approve_builtin_mcp_tool(&approval) {
            Ok(grant) => grant,
            Err(error) => {
                let _ = runtime.dismiss_builtin_mcp_tool_approval(&approval);
                let _ = self.settle_auto_mcp_action_journal(
                    &journal,
                    McpAutoActionJournalTerminalOutcome::Failed,
                    None,
                );
                return Ok(AgentToolResult {
                    exact_archive_file: None,
                    call_id: approval.identity.call_id.clone(),
                    tool: approval.identity.model_name.clone(),
                    ok: false,
                    result: Some(serde_json::json!({
                        "schemaVersion": 1,
                        "type": "builtin_mcp_tool_approval",
                        "status": "failed",
                        "dispatchCertainty": "definitely_not_dispatched",
                        "contentOmitted": true,
                    })),
                    error: Some(error.to_string()),
                });
            }
        };

        if cancellation.is_cancelled() {
            let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
            let result = mycopilot_core::builtin_mcp_tool_cancelled_result(&approval);
            let _ = self.settle_auto_mcp_action_journal(
                &journal,
                McpAutoActionJournalTerminalOutcome::Cancelled,
                None,
            );
            return Ok(result);
        }
        if self.claim_auto_mcp_dispatch(&mut journal).is_err() {
            let _ = runtime.revoke_builtin_mcp_tool_grant(&grant.grant_id, &grant.approval_id);
            // A failed CAS may be a post-commit observation failure. Leave any executing row for
            // startup reconciliation and never dispatch the process-only payload.
            let _ = self.settle_auto_mcp_action_journal(
                &journal,
                McpAutoActionJournalTerminalOutcome::Failed,
                None,
            );
            return Err(AgentError::structured(
                "builtin_mcp.auto_dispatch_claim_failed",
                "The built-in MCP invocation lost its durable dispatch claim and was not sent.",
                serde_json::json!({
                    "type": "builtin_mcp_tool_approval",
                    "code": "autoDispatchClaimFailed",
                    "retryable": false,
                    "dispatchCertainty": "definitely_not_dispatched",
                }),
            ));
        }

        let invocation_runtime = runtime.clone();
        let invocation_approval = (*approval).clone();
        let invocation_cancellation = cancellation.clone();
        let result = supervised_builtin_mcp_tool_result(
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
        .await;
        let status = result
            .result
            .as_ref()
            .and_then(|value| value.get("status"))
            .and_then(Value::as_str);
        let journal_outcome = if result.ok {
            McpAutoActionJournalTerminalOutcome::Completed
        } else if status == Some("cancelled") {
            McpAutoActionJournalTerminalOutcome::Cancelled
        } else if status == Some("outcome_unknown") {
            McpAutoActionJournalTerminalOutcome::OutcomeUnknown
        } else {
            McpAutoActionJournalTerminalOutcome::Failed
        };
        if self
            .settle_auto_mcp_action_journal(&journal, journal_outcome, None)
            .is_err()
        {
            // The call crossed the durable dispatch fence. If its terminal journal cannot be
            // proven, suppress the transient response and force an outcome-unknown observation.
            return Ok(mycopilot_core::builtin_mcp_tool_outcome_unknown_result(
                &approval,
            ));
        }
        Ok(result)
    }
}
