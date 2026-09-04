impl AgentService {
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
            .get(&storage_id)
            .expect("resolved pending action exists");
        let is_recoverable_approved_mcp = record.snapshot.status == PendingActionStatus::Approved
            && matches!(
                record.snapshot.action,
                AgentProposedAction::McpToolCall { .. }
            )
            && self
                .startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .contains(&storage_id);
        if !is_recoverable_approved_mcp {
            return Ok(None);
        }
        self.ensure_approval_predecessors_settled(record)?;
        let record = pending_actions
            .get_mut(&storage_id)
            .expect("resolved pending action exists");
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
            let failed_record = record.clone();
            pending_actions.remove(&storage_id);
            self.startup_recoverable_mcp_approvals
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner())
                .remove(&storage_id);
            drop(pending_actions);
            drop(deletion_lifecycle);
            self.invalidate_mcp_pending_payload(&failed_record.snapshot.action);
            if let Some(conversation_id) = failed_record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(
                    conversation_id,
                    &failed_record.snapshot.run_id,
                );
                self.release_turn_concurrency_permit(&failed_record.snapshot.run_id);
            }
            return Ok(Some(AgentActionExecutionOutput {
                action_id: failed_record.snapshot.action_id,
                action_type: failed_record.snapshot.action_type,
                tool_name: failed_record.snapshot.tool_name,
                status: "failed".to_string(),
                file_change_result: None,
                command_result: None,
                tool_result: None,
                agent_output: AgentChatOutput {
                    content: error,
                    status: AgentRunStatus::Failed,
                    run_id: failed_record.snapshot.run_id,
                    events: Vec::new(),
                    tool_definitions: Vec::new(),
                    todo: None,
                    usage: None,
                    finish_reason: None,
                    proposed_actions: Vec::new(),
                    conversation_turn_trace: None,
                },
            }));
        }
        // Preserve the frozen Provider ToolCall exactly as it appeared in the checkpoint.
        // Approval is carried by the typed action journal and lifecycle, not by rewriting the
        // append-only call from `required` to `approved` after restart.
        let call = tool_call_for_pending_record(record)?;
        let guard = self
            .process_runs
            .register(&record.storage_id, &record.snapshot.run_id);
        self.persist_pending_dispatch_status(
            record,
            PendingActionStatus::Approved,
            PendingActionStatus::Executing,
        )?;
        record.snapshot.status = PendingActionStatus::Executing;
        self.startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&storage_id);
        let record = record.clone();
        drop(pending_actions);
        drop(deletion_lifecycle);
        self.queue_claimed_mcp_tool_execution(record, call, guard, notifications)
            .map(Some)
    }
}
