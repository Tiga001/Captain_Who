impl AgentService {
    async fn finish_cancelled_action_continuation(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        cancellation_token: &AgentCancellationToken,
    ) {
        const REASON: &str =
            "Agent run was cancelled after the approved tool result was durably recorded.";
        const PERSISTENCE_ERROR: &str = "The cancelled Agent run could not be durably finalized. It will be reconciled on restart.";
        let run_id = &record.snapshot.run_id;

        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                None,
                cancellation_token,
                None,
                None,
            );
            return;
        }

        let snapshot = self
            .trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let terminal = match (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
            snapshot,
        ) {
            (Some(conversation_id), Some(assistant_message_id), Some(snapshot)) => {
                cancelled_conversation_trace_from_snapshot(
                    snapshot,
                    run_id,
                    conversation_id,
                    assistant_message_id,
                    REASON,
                )
                .map(|terminal| {
                    (
                        conversation_id.to_string(),
                        assistant_message_id.to_string(),
                        terminal,
                    )
                })
            }
            _ => Err(
                "cancelled action continuation is missing its durable conversation trace snapshot"
                    .to_string(),
            ),
        };
        let mut output = AgentChatOutput {
            content: String::new(),
            status: AgentRunStatus::Cancelled,
            run_id: run_id.clone(),
            events: Vec::new(),
            tool_definitions: Vec::new(),
            todo: None,
            usage: None,
            finish_reason: Some(REASON.to_string()),
            proposed_actions: Vec::new(),
            conversation_turn_trace: terminal
                .as_ref()
                .ok()
                .map(|(_, _, terminal)| terminal.trace.clone()),
        };
        let previous_usage_state = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let persisted = persist_terminal_with_bounded_retry(
            || {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    return Err("项目或会话正在移除，无法持久化已取消审批续跑终态。".to_string());
                }
                terminal.as_ref().map_err(Clone::clone).and_then(
                    |(conversation_id, assistant_message_id, terminal)| {
                        self.persist_final_assistant_output_with_model_context(
                            conversation_id,
                            assistant_message_id,
                            &mut output,
                            &terminal.model_context_items,
                        )
                    },
                )
            },
            || restore_run_usage_state(self, run_id, &previous_usage_state),
        )
        .await;
        let pending_transition = if persisted.is_ok() {
            persist_terminal_with_bounded_retry(
                || {
                    let deletion_lifecycle = self
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if deletion_lifecycle.contains_input(&record.agent_input) {
                        return Err(
                            "项目或会话正在移除，无法完成已取消审批续跑的状态迁移。".to_string()
                        );
                    }
                    self.transition_pending_status(record, final_pending_status)
                },
                || {},
            )
            .await
        } else {
            Err(
                "已取消审批续跑的 assistant 终态未持久化，已保留非终态 pending 记录供启动对账。"
                    .to_string(),
            )
        };
        let deletion_lifecycle = self
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if deletion_lifecycle.contains_input(&record.agent_input) {
            drop(deletion_lifecycle);
            self.discard_usage_context(run_id);
            if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(conversation_id, run_id);
            }
            self.release_turn_concurrency_permit(run_id);
            self.discard_trace_snapshot(run_id);
            self.discard_exact_running_context_window_snapshot(run_id);
            self.unregister_cancellation_if_current(run_id, cancellation_token);
            return;
        }
        if persisted.is_err() {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                None,
                cancellation_token,
                Some("cancelled_run_persistence_failed"),
                Some(PERSISTENCE_ERROR.to_string()),
            );
            return;
        }
        if let Err(error) = pending_transition {
            emit_pending_transition_error(notifications, run_id, final_pending_status, &error);
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                None,
                cancellation_token,
                None,
                None,
            );
            return;
        }
        if self
            .storage
            .revoke_nonterminal_file_change_run_grants(run_id)
            .is_err()
        {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                None,
                cancellation_token,
                Some("file_change_run_grant_revocation_failed"),
                Some(
                    "已取消的审批续跑无法确认本轮文件修改授权已安全撤销；将在启动恢复时重新对账。"
                        .to_string(),
                ),
            );
            return;
        }

        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.invalidate_conversation_context_state(conversation_id);
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, run_id);
        }
        self.release_turn_concurrency_permit(run_id);
        if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
            self.notify_durable_turn_observers(assistant_message_id);
        }
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
            run_id: run_id.clone(),
            success: false,
            status: Some(AgentRunStatus::Cancelled),
            content: None,
            usage: output.usage,
            finish_reason: output.finish_reason,
            proposed_actions: Vec::new(),
        }));
        drop(deletion_lifecycle);
        if let Err(error) = self
            .storage
            .settle_async_human_interaction_start_failure(run_id)
        {
            eprintln!("failed to settle refused async answer continuation: {error}");
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.publish_human_delivery_changes(conversation_id, notifications);
        }
        self.schedule_human_input_deliveries(notifications.clone());
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_pre_runtime_action_continuation_failure(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        steer_input: Option<&AgentSteerInputQueue>,
        cancellation_token: &AgentCancellationToken,
        expected_target_status: PendingActionStatus,
        failure_code: &str,
        failure_message: String,
    ) {
        let run_id = &record.snapshot.run_id;
        let (Some(conversation_id), Some(assistant_message_id)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) else {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                steer_input,
                cancellation_token,
                Some("conversation_turn_identity_missing"),
                Some("审批续跑缺少 Conversation Turn 持久化身份。".to_string()),
            );
            return;
        };

        let steering_close_error = steer_input.and_then(|steer_input| {
            self.unregister_active_run_control(
                run_id,
                steer_input,
                AgentSteerRunRejectionCode::RunNotSteerable,
                "The agent run has finished and no longer accepts guidance.",
                notifications,
            )
            .err()
        });
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        let terminal_message = match steering_close_error {
            Some(error) => {
                format!("{failure_message}；同时无法关闭审批续跑的用户引导通道：{error}")
            }
            None => failure_message,
        };

        let terminal_projection = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)
            .and_then(|trace| match trace {
                Some(trace)
                    if trace.run_id == *run_id
                        && trace.conversation_id == conversation_id
                        && trace.terminal_status
                            == ConversationTurnTraceTerminalStatus::InProgress =>
                {
                    let model_context_items = self
                        .storage
                        .get_conversation_model_context_log(assistant_message_id)?
                        .map(|log| log.items)
                        .unwrap_or_default();
                    let next_sequence = trace
                        .items
                        .last()
                        .map(ConversationTurnTraceItem::sequence)
                        .unwrap_or(0)
                        .saturating_add(1);
                    terminal_conversation_trace_from_snapshot(
                        ConversationTraceSnapshot {
                            items: trace.items,
                            model_context_items,
                            next_sequence,
                            truncated: trace.truncated,
                        },
                        run_id,
                        conversation_id,
                        assistant_message_id,
                        ConversationTurnTraceTerminalStatus::Failed,
                        &terminal_message,
                    )
                }
                Some(_) => {
                    Err("审批续跑的 durable ConversationTurnTrace 身份或状态不一致。".to_string())
                }
                None => Err("审批续跑的 durable ConversationTurnTrace 不存在。".to_string()),
            });
        let terminal = match terminal_projection {
            Ok(terminal) => terminal,
            Err(_) => {
                self.preserve_claimed_action_after_pre_runtime_refusal(
                    record,
                    notifications,
                    None,
                    cancellation_token,
                    Some("conversation_trace_persistence_failed"),
                    Some("审批续跑无法安全持久化失败终态；已停止执行并保留恢复记录。".to_string()),
                );
                return;
            }
        };
        let previous_usage_state = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .cloned();
        let usage_record = self.prepare_run_usage_record(
            run_id,
            AgentRunStatus::Failed,
            None,
            Some(terminal_message.clone()),
        );
        let cumulative_usage = self.preview_cumulative_run_usage(run_id, None);
        let completed_at = now_ms();
        let persisted = {
            let mut pending_actions = self
                .pending_actions
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let pending = pending_actions
                .get_mut(&record.storage_id)
                .ok_or_else(|| format!("审批续跑的内存 pending 状态不存在：{}", record.storage_id));
            pending.and_then(|pending| {
                if pending.snapshot.run_id != *run_id
                    || pending.snapshot.conversation_id.as_deref() != Some(conversation_id)
                    || pending.snapshot.assistant_message_id.as_deref()
                        != Some(assistant_message_id)
                {
                    return Err("审批续跑的内存 pending 身份不一致。".to_string());
                }
                let expected_status = pending.snapshot.status;
                self.storage.fail_claimed_agent_action_continuation(
                    &record.storage_id,
                    pending_status_label(expected_status),
                    pending_status_label(expected_target_status),
                    conversation_id,
                    assistant_message_id,
                    &terminal_message,
                    &terminal.trace,
                    &terminal.model_context_items,
                    completed_at,
                    usage_record.as_ref(),
                )?;
                pending.snapshot.status = PendingActionStatus::Failed;
                Ok(())
            })
        };
        if persisted.is_err() {
            let mut usage_contexts = self
                .usage_contexts
                .lock()
                .unwrap_or_else(|lock_error| lock_error.into_inner());
            match previous_usage_state {
                Some(previous) => {
                    usage_contexts.insert(run_id.clone(), previous);
                }
                None => {
                    usage_contexts.remove(run_id);
                }
            }
            drop(usage_contexts);
            self.preserve_claimed_action_after_pre_runtime_refusal(
                record,
                notifications,
                None,
                cancellation_token,
                Some("conversation_trace_persistence_failed"),
                Some("审批续跑无法安全持久化失败终态；已停止执行并保留恢复记录。".to_string()),
            );
            return;
        }
        self.finish_persisted_run_usage(run_id, AgentRunStatus::Failed);
        self.notify_durable_turn_observers(assistant_message_id);

        self.emit_terminal_context_window_snapshot(
            notifications,
            &record.agent_input,
            run_id,
            conversation_id,
            assistant_message_id,
            &terminal_message,
        );
        self.release_conversation_turn_if_current(conversation_id, run_id);
        self.release_turn_concurrency_permit(run_id);
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);
        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
            run_id: Some(run_id.clone()),
            trace_sequence: None,
            message: terminal_message.clone(),
            recoverable: false,
            code: Some(failure_code.to_string()),
            details: None,
        }));
        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
            run_id: run_id.clone(),
            success: false,
            status: Some(AgentRunStatus::Failed),
            content: Some(terminal_message),
            usage: cumulative_usage,
            finish_reason: None,
            proposed_actions: Vec::new(),
        }));
        if let Err(error) = self
            .storage
            .settle_async_human_interaction_start_failure(run_id)
        {
            eprintln!("failed to settle refused async answer continuation: {error}");
        }
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.publish_human_delivery_changes(conversation_id, notifications);
        }
        self.schedule_human_input_deliveries(notifications.clone());
    }

    /// Stops an approval continuation before Runtime without inventing a terminal outcome.
    ///
    /// Some refusal paths cannot safely own or mutate the durable Turn (for example, a deletion
    /// fence or a foreign process-local Turn owner). The Pending Action therefore remains exact
    /// and recoverable, but remembered FileChange authority is durably revoked before this worker
    /// releases any process-owned resources or reports the refusal.
    #[allow(clippy::too_many_arguments)]
    fn preserve_claimed_action_after_pre_runtime_refusal(
        &self,
        record: &PendingActionRecord,
        notifications: &CoreServerNotificationSender,
        steer_input: Option<&AgentSteerInputQueue>,
        cancellation_token: &AgentCancellationToken,
        failure_code: Option<&str>,
        failure_message: Option<String>,
    ) {
        let run_id = &record.snapshot.run_id;
        let revocation_error = self
            .storage
            .revoke_nonterminal_file_change_run_grants(run_id)
            .err();

        if let Some(steer_input) = steer_input {
            let _ = self.unregister_active_run_control(
                run_id,
                steer_input,
                AgentSteerRunRejectionCode::RunNotSteerable,
                "The agent run did not enter Runtime and no longer accepts guidance.",
                notifications,
            );
        }
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        if revocation_error.is_some() {
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.clone()),
                trace_sequence: None,
                message: "审批续跑无法确认本轮文件修改授权已安全撤销；已停止执行，并将在启动恢复时重新对账。"
                    .to_string(),
                recoverable: true,
                code: Some("file_change_run_grant_revocation_failed".to_string()),
                details: None,
            }));
            return;
        }

        self.release_turn_concurrency_permit(run_id);
        if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
            self.release_conversation_turn_if_current(conversation_id, run_id);
        }
        self.discard_usage_context(run_id);
        self.discard_trace_snapshot(run_id);
        self.discard_exact_running_context_window_snapshot(run_id);

        if let (Some(code), Some(message)) = (failure_code, failure_message) {
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id.clone()),
                trace_sequence: None,
                message,
                recoverable: true,
                code: Some(code.to_string()),
                details: None,
            }));
        }
    }
}
