impl AgentService {
    pub async fn shutdown_active_runs(&self, timeout: Duration) -> (usize, bool) {
        let manual_tokens = self
            .manual_context_compaction_cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for token in &manual_tokens {
            token.cancel();
        }
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
            self.command_sessions.cancel_pre_handoff_for_run(run_id);
            self.process_runs.cancel_run(run_id);
        }

        // Process shutdown and Agent Run retirement advance independently under one deadline.
        // The process lane closes start admission, terminates every group, and waits for durable
        // Session settlement even when there are no active model runs.
        let command_sessions = self.command_sessions.clone();
        let (session_result_tx, session_result_rx) = std::sync::mpsc::sync_channel(1);
        let _session_worker = std::thread::Builder::new()
            .name("agent-command-session-shutdown".to_string())
            .spawn(move || {
                let _ = session_result_tx.send(command_sessions.shutdown(timeout));
            });

        let deadline = tokio::time::Instant::now() + timeout;
        let mut sessions_settled = false;
        let mut active_runs_settled = active_runs.is_empty();
        loop {
            let active_run_count = self
                .cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len();
            active_runs_settled |= active_run_count == 0;
            if let Ok(settled) = session_result_rx.try_recv() {
                sessions_settled = settled;
            }
            let manual_settled = self
                .manual_context_compaction_cancellations
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_empty();
            if active_runs_settled && sessions_settled && manual_settled {
                return (active_runs.len() + manual_tokens.len(), false);
            }
            if tokio::time::Instant::now() >= deadline {
                if !active_runs_settled {
                    let remaining_run_ids = self
                        .cancellations
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .keys()
                        .cloned()
                        .collect::<Vec<_>>();
                    self.persist_forced_cancelled_runs(&remaining_run_ids);
                }
                return (active_runs.len() + manual_tokens.len(), true);
            }

            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn cancelled_terminal_projection_from_latest_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        unresolved_tool_error: &str,
        terminal_error: Option<&str>,
    ) -> Result<TerminalConversationTraceProjection, String> {
        let snapshot = self
            .terminal_source_snapshot(run_id, conversation_id, assistant_message_id)?
            .unwrap_or_default();
        match terminal_error {
            Some(terminal_error) => cancelled_conversation_trace_from_snapshot_with_terminal_error(
                snapshot,
                run_id,
                conversation_id,
                assistant_message_id,
                unresolved_tool_error,
                terminal_error,
            ),
            None => cancelled_conversation_trace_from_snapshot(
                snapshot,
                run_id,
                conversation_id,
                assistant_message_id,
                unresolved_tool_error,
            ),
        }
    }

    fn failed_terminal_projection_from_latest_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        terminal_error: &str,
    ) -> Result<Option<TerminalConversationTraceProjection>, String> {
        let snapshot =
            self.terminal_source_snapshot(run_id, conversation_id, assistant_message_id)?;
        snapshot
            .map(|snapshot| {
                terminal_conversation_trace_from_snapshot(
                    snapshot,
                    run_id,
                    conversation_id,
                    assistant_message_id,
                    ConversationTurnTraceTerminalStatus::Failed,
                    terminal_error,
                )
            })
            .transpose()
    }

    /// Preserve Host-precommitted tool results when a later Runtime projection fails. The
    /// append-only journal remains authoritative; an unpublished conflicting snapshot must not
    /// replace a successful result with a synthetic failure or cancellation during cleanup.
    fn terminal_source_snapshot(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
    ) -> Result<Option<ConversationTraceSnapshot>, String> {
        let snapshot = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let Some(trace) = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)?
        else {
            return Ok(snapshot);
        };
        if trace.run_id != run_id || trace.conversation_id != conversation_id {
            return Err("Terminal trace does not belong to the retiring run.".to_string());
        }
        let model_context_items = self
            .storage
            .get_conversation_model_context_log(assistant_message_id)?
            .map(|log| log.items)
            .unwrap_or_default();
        if snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.items.starts_with(&trace.items)
                && snapshot
                    .model_context_items
                    .starts_with(&model_context_items)
                && (!trace.truncated || snapshot.truncated)
        }) {
            return Ok(snapshot);
        }
        let next_sequence = trace
            .items
            .last()
            .map(|item| item.sequence().saturating_add(1))
            .unwrap_or(0);
        Ok(Some(ConversationTraceSnapshot {
            items: trace.items,
            model_context_items,
            next_sequence,
            truncated: trace.truncated,
        }))
    }

    pub(super) fn persist_forced_cancelled_runs(&self, run_ids: &[String]) {
        self.persist_cancelled_runs_with_reason(
            run_ids,
            "Core shutdown timed out while cancelling the active run.",
        );
    }

    pub(super) fn persist_cancelled_runs_with_reason(&self, run_ids: &[String], reason: &str) {
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
        let completed_at = now_ms();
        for context in contexts {
            let terminal = match self.cancelled_terminal_projection_from_latest_snapshot(
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                reason,
                Some(reason),
            ) {
                Ok(terminal) => terminal,
                Err(error) => {
                    // Shutdown must not manufacture model history from a rejected in-memory
                    // projection. Leave the already durable in-progress trace untouched; the
                    // current-schema startup reconciler closes it with its exact durable model
                    // context before any new run is admitted.
                    eprintln!("failed to close forced cancelled conversation trace: {error}");
                    continue;
                }
            };
            let usage_record = self.prepare_run_usage_record(
                &context.run_id,
                AgentRunStatus::Cancelled,
                None,
                Some(reason.to_string()),
            );
            let persisted = self.finalize_turn_with_human_root_notification(
                &context.run_id,
                &context.conversation_id,
                &context.assistant_message_id,
                AgentRunStatus::Cancelled,
                "",
                status_for_run(AgentRunStatus::Cancelled),
                run_status_label(AgentRunStatus::Cancelled),
                &terminal.trace,
                Some(&terminal.model_context_items),
                context.started_at,
                completed_at,
                usage_record.as_ref(),
                None,
            );
            if persisted.is_ok() {
                self.finish_persisted_run_usage(&context.run_id, AgentRunStatus::Cancelled);
                self.retire_builtin_capability_run(&context.run_id);
                self.invalidate_conversation_context_state(&context.conversation_id);
            } else if let Err(error) = persisted {
                // The transaction is all-or-nothing. A later startup retires the unchanged
                // in-progress trace through the same strict trace/model-context boundary.
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

    pub(super) fn unregister_cancellation_if_current(
        &self,
        run_id: &str,
        token: &AgentCancellationToken,
    ) {
        let mut cancellations = self
            .cancellations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if cancellations
            .get(run_id)
            .is_some_and(|current| current.shares_state_with(token))
        {
            cancellations.remove(run_id);
        }
    }

    pub(super) fn is_agent_input_scope_deleting(&self, agent_input: &AgentChatInput) -> bool {
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_input(agent_input)
    }

    pub(super) fn is_project_deleting(&self, project_id: Option<&str>) -> bool {
        let Some(project_id) = project_id else {
            return false;
        };
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .projects
            .contains(project_id)
    }

    pub(super) fn is_conversation_deleting(&self, conversation_id: Option<&str>) -> bool {
        let Some(conversation_id) = conversation_id else {
            return false;
        };
        self.deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .conversations
            .contains(conversation_id)
    }
}
