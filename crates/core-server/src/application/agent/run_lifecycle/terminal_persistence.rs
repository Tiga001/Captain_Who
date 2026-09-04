impl AgentService {
    pub(super) fn persist_final_assistant_output(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        self.persist_final_assistant_output_inner(
            conversation_id,
            assistant_message_id,
            output,
            None,
            collaboration_cutoff,
        )
    }

    pub(super) fn persist_final_assistant_output_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        model_context_items: &[ConversationModelContextItem],
    ) -> Result<(), String> {
        self.persist_final_assistant_output_inner(
            conversation_id,
            assistant_message_id,
            output,
            Some(model_context_items),
            None,
        )
    }

    fn persist_final_assistant_output_inner(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &mut AgentChatOutput,
        model_context_items: Option<&[ConversationModelContextItem]>,
        collaboration_cutoff: Option<u64>,
    ) -> Result<(), String> {
        let completed_at = now_ms();
        if matches!(
            output.status,
            AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
        ) {
            // Runtime cancellation may carry a terminal Trace produced from its private recorder,
            // but AgentChatOutput does not carry the paired model-context projection. When the
            // caller has not already supplied that pair, close the latest published Host snapshot
            // and commit its Trace and model context together. Mixing the Runtime terminal Trace
            // with the older durable context can leave an unresolved ToolCall and strand the Turn
            // InProgress. Bounded root shutdown cancellation uses the same projection.
            // Provider finish reasons (for example `tool_calls`) describe the previous model
            // response. They are not cancellation diagnostics and must never become a visible
            // terminal error or the error text of a synthetically settled ToolResult.
            const CANCELLATION_TOOL_RESULT_ERROR: &str = "Agent run was cancelled.";
            let cancelled_projection =
                if output.status == AgentRunStatus::Cancelled && model_context_items.is_none() {
                    Some(self.cancelled_terminal_projection_from_latest_snapshot(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                        CANCELLATION_TOOL_RESULT_ERROR,
                        None,
                    )?)
                } else {
                    None
                };
            let failed_projection =
                if output.status == AgentRunStatus::Failed && model_context_items.is_none() {
                    let terminal_error = output
                        .conversation_turn_trace
                        .as_ref()
                        .and_then(|trace| trace.terminal_error.as_deref())
                        .or(output.finish_reason.as_deref())
                        .unwrap_or("Agent run failed.");
                    self.failed_terminal_projection_from_latest_snapshot(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                        terminal_error,
                    )?
                } else {
                    None
                };
            let trace = cancelled_projection
                .as_ref()
                .map(|terminal| terminal.trace.clone())
                .or_else(|| {
                    failed_projection
                        .as_ref()
                        .map(|terminal| terminal.trace.clone())
                })
                .or_else(|| output.conversation_turn_trace.clone())
                .or_else(|| {
                    (output.status == AgentRunStatus::Cancelled).then(|| {
                        cancelled_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                        )
                    })
                })
                .unwrap_or_else(|| match output.status {
                    AgentRunStatus::Completed => completed_conversation_trace_without_items(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                    ),
                    AgentRunStatus::Cancelled => unreachable!(),
                    AgentRunStatus::Failed => failed_conversation_trace_without_items(
                        &output.run_id,
                        conversation_id,
                        assistant_message_id,
                        output
                            .finish_reason
                            .as_deref()
                            .unwrap_or("Agent run failed."),
                    ),
                    _ => unreachable!(),
                });
            let model_context_items = model_context_items
                .or_else(|| {
                    cancelled_projection
                        .as_ref()
                        .map(|terminal| terminal.model_context_items.as_slice())
                })
                .or_else(|| {
                    failed_projection
                        .as_ref()
                        .map(|terminal| terminal.model_context_items.as_slice())
                });
            let usage_error = match output.status {
                AgentRunStatus::Failed => trace.terminal_error.clone(),
                AgentRunStatus::Completed | AgentRunStatus::Cancelled => None,
                _ => unreachable!(),
            };
            let usage_record = self.prepare_run_usage_record(
                &output.run_id,
                output.status,
                output.usage.clone(),
                usage_error,
            );
            let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
            self.finalize_turn_with_human_root_notification(
                &output.run_id,
                conversation_id,
                assistant_message_id,
                output.status,
                if output.status == AgentRunStatus::Cancelled {
                    ""
                } else {
                    &output.content
                },
                status_for_run(output.status),
                run_status_label(output.status),
                &trace,
                model_context_items,
                completed_at,
                completed_at,
                usage_record.as_ref(),
                collaboration_cutoff,
            )?;
            replace_output_usage(output, cumulative_usage);
            self.finish_persisted_run_usage(&output.run_id, output.status);
            self.retire_builtin_capability_run(&output.run_id);
            return Ok(());
        }
        if output.status == AgentRunStatus::WaitingForApproval {
            let [pending_action] = output.proposed_actions.as_slice() else {
                return Err(
                    "WaitingForApproval output must identify exactly one pending action"
                        .to_string(),
                );
            };
            let pending_action_storage_id =
                pending_action_storage_id(&output.run_id, &action_id_for_action(pending_action));
            // ApprovalRequired stages this segment's Usage before publishing the pending action.
            // Reuse that cumulative state without adding the same provider response twice.
            let usage_record =
                self.prepare_run_usage_record(&output.run_id, output.status, None, None);
            let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
            let persisted = self
                .storage
                .persist_waiting_for_approval_if_run_in_progress(
                    conversation_id,
                    assistant_message_id,
                    &output.run_id,
                    &pending_action_storage_id,
                    &output.content,
                    status_for_run(output.status),
                    completed_at,
                    usage_record.as_ref(),
                )?;
            replace_output_usage(output, cumulative_usage);
            match persisted {
                AgentWaitingForApprovalPersistenceOutcome::Persisted
                | AgentWaitingForApprovalPersistenceOutcome::PendingActionAdvanced => {}
                AgentWaitingForApprovalPersistenceOutcome::TurnTerminal => {
                    // A cancellation terminalized the exact Turn while this Runtime segment was
                    // retiring. Its durable state is authoritative; discard the stale
                    // process-local Usage/Trace snapshot instead of recreating WaitingForApproval.
                    self.discard_usage_context(&output.run_id);
                }
            }
            return Ok(());
        }
        self.persist_run_usage(&output.run_id, output.status, output.usage.clone(), None)?;
        let cumulative_usage = self.preview_cumulative_run_usage(&output.run_id, None);
        replace_output_usage(output, cumulative_usage);
        self.storage.update_chat_message_status_and_content(
            conversation_id,
            assistant_message_id,
            &output.content,
            status_for_run(output.status),
            completed_at,
        )
    }

    pub(super) fn persist_assistant_error(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_error_with_model_context(
            conversation_id,
            assistant_message_id,
            message,
            usage,
            conversation_turn_trace,
            None,
        )
    }

    pub(super) fn persist_assistant_error_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_failure_projection(
            conversation_id,
            assistant_message_id,
            message,
            Some("error"),
            message,
            usage,
            conversation_turn_trace,
            model_context_items,
        )
    }

    pub(super) fn persist_assistant_model_request_interruption(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
    ) -> Result<Option<AgentUsage>, String> {
        self.persist_assistant_model_request_interruption_with_model_context(
            conversation_id,
            assistant_message_id,
            diagnostic_message,
            usage,
            conversation_turn_trace,
            None,
        )
    }

    pub(super) fn persist_assistant_model_request_interruption_with_model_context(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        // The failed sampling attempt never committed a model turn. Keep its diagnostic in usage
        // and trace audit data, while settling the visible assistant message as an empty prefix.
        self.persist_assistant_failure_projection(
            conversation_id,
            assistant_message_id,
            "",
            Some("sent"),
            diagnostic_message,
            usage,
            conversation_turn_trace,
            model_context_items,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn persist_assistant_failure_projection(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        persisted_content: &str,
        message_status: Option<&str>,
        diagnostic_message: &str,
        usage: Option<AgentUsage>,
        conversation_turn_trace: &ConversationTurnTrace,
        model_context_items: Option<&[ConversationModelContextItem]>,
    ) -> Result<Option<AgentUsage>, String> {
        let failed_projection = if model_context_items.is_none() {
            self.failed_terminal_projection_from_latest_snapshot(
                &conversation_turn_trace.run_id,
                conversation_id,
                assistant_message_id,
                diagnostic_message,
            )?
        } else {
            None
        };
        let conversation_turn_trace = failed_projection
            .as_ref()
            .map(|terminal| &terminal.trace)
            .unwrap_or(conversation_turn_trace);
        let model_context_items = model_context_items.or_else(|| {
            failed_projection
                .as_ref()
                .map(|terminal| terminal.model_context_items.as_slice())
        });
        let run_id = self.find_usage_run_id(conversation_id, assistant_message_id);
        let fallback_usage = usage.clone();
        let completed_at = now_ms();
        let usage_record = run_id.as_deref().and_then(|run_id| {
            self.prepare_run_usage_record(
                run_id,
                AgentRunStatus::Failed,
                usage,
                Some(diagnostic_message.to_string()),
            )
        });
        self.finalize_turn_with_human_root_notification(
            &conversation_turn_trace.run_id,
            conversation_id,
            assistant_message_id,
            AgentRunStatus::Failed,
            persisted_content,
            message_status,
            run_status_label(AgentRunStatus::Failed),
            conversation_turn_trace,
            model_context_items,
            completed_at,
            completed_at,
            usage_record.as_ref(),
            None,
        )?;
        let cumulative_usage = run_id
            .as_deref()
            .and_then(|run_id| self.preview_cumulative_run_usage(run_id, None))
            .or(fallback_usage);
        if let Some(run_id) = run_id {
            self.finish_persisted_run_usage(&run_id, AgentRunStatus::Failed);
            self.retire_builtin_capability_run(&run_id);
        }
        Ok(cumulative_usage)
    }
}
