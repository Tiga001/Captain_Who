use super::*;

impl AgentService {
    pub fn cancel_run(&self, run_id: &str) -> bool {
        let cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let cancelled_run = if let Some(token) = cancellations.get(run_id) {
            token.cancel();
            true
        } else {
            false
        };
        let cancelled_commands = self.command_runs.cancel_run(run_id);
        cancelled_run || cancelled_commands > 0
    }

    pub fn delete_project(&self, project_id: &str) -> Result<(), String> {
        {
            let mut deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            deleting_projects.insert(project_id.to_string());
        }

        let run_ids = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .iter()
                .filter(|(_, state)| state.context.project_id.as_deref() == Some(project_id))
                .map(|(run_id, _)| run_id.clone())
                .collect::<Vec<_>>()
        };
        for run_id in run_ids {
            self.cancel_run(&run_id);
        }

        let result = self.storage.delete_project(project_id);
        if result.is_ok() {
            self.invalidate_all_conversation_context_states();
            {
                let mut pending_actions = self
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                pending_actions.retain(|_, record| {
                    !agent_input_belongs_to_project(&record.agent_input, project_id)
                });
            }
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            usage_contexts
                .retain(|_, state| state.context.project_id.as_deref() != Some(project_id));
        } else {
            let mut deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            deleting_projects.remove(project_id);
        }
        result
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        self.storage.delete_conversation(conversation_id)?;
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    pub fn delete_chat_messages(
        &self,
        conversation_id: &str,
        message_ids: &[String],
    ) -> Result<(), String> {
        self.storage
            .delete_chat_messages(conversation_id, message_ids)?;
        self.invalidate_conversation_context_state(conversation_id);
        Ok(())
    }

    pub async fn shutdown_active_runs(&self, timeout: Duration) -> (usize, bool) {
        let active_runs = {
            let cancellations = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            cancellations
                .iter()
                .map(|(run_id, token)| (run_id.clone(), token.clone()))
                .collect::<Vec<_>>()
        };

        for (run_id, token) in &active_runs {
            token.cancel();
            self.command_runs.cancel_run(run_id);
        }

        if active_runs.is_empty() {
            return (0, false);
        }

        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let active_run_count = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len();
            if active_run_count == 0 {
                return (active_runs.len(), false);
            }
            if tokio::time::Instant::now() >= deadline {
                let remaining_run_ids = self
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>();
                self.persist_forced_cancelled_runs(&remaining_run_ids);
                return (active_runs.len(), true);
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    pub(super) fn persist_forced_cancelled_runs(&self, run_ids: &[String]) {
        const REASON: &str = "Core shutdown timed out while cancelling the active run.";
        let contexts = {
            let usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            run_ids
                .iter()
                .filter_map(|run_id| {
                    usage_contexts
                        .get(run_id)
                        .map(|state| state.context.clone())
                })
                .collect::<Vec<_>>()
        };
        let snapshots = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        let completed_at = now_ms();
        for context in contexts {
            let trace = cancelled_conversation_trace_from_snapshot(
                snapshots.get(&context.run_id).cloned().unwrap_or_default(),
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                REASON,
            );
            let usage_record = self.prepare_run_usage_record(
                &context.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(REASON.to_string()),
            );
            let persisted = self
                .storage
                .finalize_chat_message_with_conversation_trace_and_usage(
                    &context.conversation_id,
                    &context.assistant_message_id,
                    "",
                    status_for_run(AgentRunStatus::Cancelled),
                    run_status_label(AgentRunStatus::Cancelled),
                    &trace,
                    context.started_at,
                    completed_at,
                    usage_record.as_ref(),
                );
            if persisted.is_ok() {
                self.finish_persisted_run_usage(&context.run_id, AgentRunStatus::Cancelled);
                self.invalidate_conversation_context_state(&context.conversation_id);
            } else if let Err(error) = persisted {
                eprintln!("failed to persist forced cancelled conversation trace: {error}");
            }
        }
    }

    pub(super) fn register_cancellation(&self, run_id: &str, token: AgentCancellationToken) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.insert(run_id.to_string(), token);
    }

    pub(super) fn unregister_cancellation(&self, run_id: &str) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cancellations.remove(run_id);
    }

    pub(super) fn is_agent_input_project_deleting(&self, agent_input: &AgentChatInput) -> bool {
        self.is_project_deleting(agent_input_project_id(agent_input))
    }

    pub(super) fn is_project_deleting(&self, project_id: Option<&str>) -> bool {
        let Some(project_id) = project_id else {
            return false;
        };
        self.deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains(project_id)
    }

    pub(super) fn persist_final_assistant_output(
        &self,
        conversation_id: &str,
        assistant_message_id: &str,
        output: &AgentChatOutput,
    ) -> Result<(), String> {
        let completed_at = now_ms();
        if matches!(
            output.status,
            AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
        ) {
            let trace =
                output
                    .conversation_turn_trace
                    .clone()
                    .unwrap_or_else(|| match output.status {
                        AgentRunStatus::Completed => completed_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                        ),
                        AgentRunStatus::Cancelled => cancelled_conversation_trace_without_items(
                            &output.run_id,
                            conversation_id,
                            assistant_message_id,
                            output
                                .finish_reason
                                .as_deref()
                                .unwrap_or("Agent run was cancelled."),
                        ),
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
            let usage_record = self.prepare_run_usage_record(
                &output.run_id,
                output.status,
                output.usage.clone(),
                output.finish_reason.clone(),
            );
            self.storage
                .finalize_chat_message_with_conversation_trace_and_usage(
                    conversation_id,
                    assistant_message_id,
                    if output.status == AgentRunStatus::Cancelled {
                        ""
                    } else {
                        &output.content
                    },
                    status_for_run(output.status),
                    run_status_label(output.status),
                    &trace,
                    completed_at,
                    completed_at,
                    usage_record.as_ref(),
                )?;
            self.finish_persisted_run_usage(&output.run_id, output.status);
            return Ok(());
        }
        self.persist_run_usage(
            &output.run_id,
            output.status,
            output.usage.clone(),
            output.finish_reason.clone(),
        )?;
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
    ) -> Result<(), String> {
        let run_id = self.find_usage_run_id(conversation_id, assistant_message_id);
        let completed_at = now_ms();
        let usage_record = run_id.as_deref().and_then(|run_id| {
            self.prepare_run_usage_record(
                run_id,
                AgentRunStatus::Failed,
                usage,
                Some(message.to_string()),
            )
        });
        self.storage
            .finalize_chat_message_with_conversation_trace_and_usage(
                conversation_id,
                assistant_message_id,
                message,
                Some("error"),
                run_status_label(AgentRunStatus::Failed),
                conversation_turn_trace,
                completed_at,
                completed_at,
                usage_record.as_ref(),
            )?;
        if let Some(run_id) = run_id {
            self.finish_persisted_run_usage(&run_id, AgentRunStatus::Failed);
        }
        Ok(())
    }
}
