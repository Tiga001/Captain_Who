impl AgentService {
    pub(in crate::application::agent) async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let _steering_cleanup = self.active_run_steering_cleanup(&run_id, notifications.clone());
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        match self.agent_tree_run_is_stopped(&run_id) {
            Ok(true) => cancellation_token.cancel(),
            Ok(false) => {}
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    None,
                    &cancellation_token,
                    final_pending_status,
                    "agent_tree_stop_state_unavailable",
                    format!("审批续跑无法确认 Agent 树停止状态，已拒绝执行：{error}"),
                );
                return;
            }
        }
        let frozen_identity = record
            .agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref());
        let resumed_identity = agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.clone());
        if frozen_identity != resumed_identity.as_ref() {
            self.finish_pre_runtime_action_continuation_failure(
                &record,
                &notifications,
                None,
                &cancellation_token,
                final_pending_status,
                "collaboration_identity_mismatch",
                "审批续跑的 Collaboration identity 与冻结 checkpoint 不一致。".to_string(),
            );
            return;
        }
        let active_child_wake = if let Some(identity) = resumed_identity.as_ref() {
            match ChildAgentFactory::new(Arc::clone(&self.storage))
                .resolve_trusted_active_wake_by_identity(identity)
            {
                Ok(bundle) => Some(bundle),
                Err(_) => {
                    self.preserve_claimed_action_after_pre_runtime_refusal(
                        &record,
                        &notifications,
                        None,
                        &cancellation_token,
                        Some("collaboration_identity_revalidation_failed"),
                        Some("子 Agent 审批续跑的 Host 协作身份已失效，已拒绝执行。".to_string()),
                    );
                    return;
                }
            }
        } else {
            None
        };
        if cancellation_token.is_cancelled() {
            self.finish_cancelled_action_continuation(
                &record,
                &notifications,
                final_pending_status,
                &cancellation_token,
            )
            .await;
            return;
        }
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                &record,
                &notifications,
                None,
                &cancellation_token,
                None,
                None,
            );
            return;
        }
        self.register_cancellation(&run_id, cancellation_token.clone());
        match self.agent_tree_run_is_stopped(&run_id) {
            Ok(true) => cancellation_token.cancel(),
            Ok(false) => {}
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    None,
                    &cancellation_token,
                    final_pending_status,
                    "agent_tree_stop_state_unavailable",
                    format!("审批续跑无法确认 Agent 树停止状态，已拒绝执行：{error}"),
                );
                return;
            }
        }
        if cancellation_token.is_cancelled() {
            self.finish_cancelled_action_continuation(
                &record,
                &notifications,
                final_pending_status,
                &cancellation_token,
            )
            .await;
            return;
        }
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.preserve_claimed_action_after_pre_runtime_refusal(
                &record,
                &notifications,
                None,
                &cancellation_token,
                None,
                None,
            );
            return;
        }
        let (turn_conversation_id, turn_assistant_message_id) = match (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) {
            (Some(conversation_id), Some(assistant_message_id)) => (
                conversation_id.to_string(),
                assistant_message_id.to_string(),
            ),
            _ => {
                self.preserve_claimed_action_after_pre_runtime_refusal(
                    &record,
                    &notifications,
                    None,
                    &cancellation_token,
                    Some("conversation_turn_identity_missing"),
                    Some("审批续跑缺少 Conversation Turn 持久化身份。".to_string()),
                );
                return;
            }
        };
        if self
            .ensure_conversation_turn_owner(
                &turn_conversation_id,
                &run_id,
                &turn_assistant_message_id,
            )
            .is_err()
        {
            self.preserve_claimed_action_after_pre_runtime_refusal(
                &record,
                &notifications,
                None,
                &cancellation_token,
                Some("conversation_turn_ownership_conflict"),
                Some("审批续跑无法安全取得当前会话执行权；已停止执行并保留恢复记录。".to_string()),
            );
            return;
        }
        if self.ensure_turn_concurrency_permit(&run_id).is_err() {
            self.finish_pre_runtime_action_continuation_failure(
                &record,
                &notifications,
                None,
                &cancellation_token,
                final_pending_status,
                "agent_turn_concurrency_limit",
                "审批续跑当前无法取得执行许可；已安全停止。".to_string(),
            );
            return;
        }
        let steer_input = self.resume_active_run_control(
            &run_id,
            &turn_conversation_id,
            &turn_assistant_message_id,
            record
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.as_deref()),
            record.agent_input.model_capabilities,
            permissions_from_input(&record.agent_input),
        );

        let skill_resources = match self.restore_skill_resource_session(&agent_input) {
            Ok(resources) => resources,
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    Some(&steer_input),
                    &cancellation_token,
                    final_pending_status,
                    "skill_resource_snapshot_unavailable",
                    error.to_string(),
                );
                return;
            }
        };
        let mcp_tools = self.capture_mcp_tool_runtime(&record.agent_input);
        let automation_report_sink = match self.automation_report_sink_for_agent_run_id(&run_id) {
            Ok(sink) => sink,
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    Some(&steer_input),
                    &cancellation_token,
                    final_pending_status,
                    "automation_report_capability_unavailable",
                    error,
                );
                return;
            }
        };
        let initial_context_window_tool_projection = match self
            .context_window_tool_projection_with_mcp_and_automation_report(
                &record.agent_input,
                skill_resources.clone(),
                mcp_tools.clone(),
                automation_report_sink.clone(),
                Some(&run_id),
            ) {
            Ok(projection) => projection,
            Err(error) => {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    Some(&steer_input),
                    &cancellation_token,
                    final_pending_status,
                    "context_window_tool_projection_unavailable",
                    error,
                );
                return;
            }
        };
        let context_window_tool_projection =
            RunContextToolProjection::new(initial_context_window_tool_projection);
        if let Some(active) = active_child_wake.as_ref() {
            if self
                .storage
                .transition_agent_wake(
                    &active.spawn.initial_wake.wake_id,
                    active.spawn.initial_wake.status,
                    mycopilot_core::AgentWakeStatus::Running,
                    Some(&active.claim_token),
                )
                .is_err()
            {
                self.finish_pre_runtime_action_continuation_failure(
                    &record,
                    &notifications,
                    Some(&steer_input),
                    &cancellation_token,
                    final_pending_status,
                    "collaboration_wake_resume_transition_failed",
                    "子 Agent 审批续跑无法安全取得当前唤醒执行权。".to_string(),
                );
                return;
            }
            self.notify_durable_turn_observers(&turn_assistant_message_id);
        }
        let RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
            final_response_collaboration_cutoff,
        } = self
            .run_prepared_turn_segment(
                PreparedRuntimeTurnSegment {
                    run_id: run_id.clone(),
                    conversation_id: turn_conversation_id.clone(),
                    assistant_message_id: turn_assistant_message_id.clone(),
                    assistant_created_at: record.snapshot.created_at,
                    agent_input,
                    human_input_resume: None,
                    skill_resources,
                    mcp_tools,
                    automation_report_sink,
                    context_window_tool_projection,
                    cancellation_token: cancellation_token.clone(),
                    steer_input,
                    pending_action_predecessor_settlement: Some((
                        record.clone(),
                        final_pending_status,
                    )),
                    invalidate_mcp_payload_on_pending_store_failure: false,
                    steering_close_error_context: "无法关闭审批续跑的用户引导通道并持久化剩余引导",
                },
                notifications.clone(),
            )
            .await;
        let waiting_for_user_input =
            matches!(&result, Ok(output) if output.status == AgentRunStatus::WaitingForUserInput);
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if matches!(output.status, AgentRunStatus::WaitingForApproval | AgentRunStatus::WaitingForUserInput)
        );

        if self.is_agent_input_scope_deleting(&record.agent_input) {
            terminal_event_gate.discard();
            self.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
            self.discard_usage_context(&run_id);
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return;
        }

        let mut deletion_cleanup = false;
        let mut committed_terminal_output = None;
        let (durable_turn_terminal, settlement_committed) = match result {
            Ok(mut agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                let owner_ids = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                );
                let previous_usage_state = self
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = self
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if deletion_lifecycle.contains_input(&record.agent_input) {
                            return Err("项目或会话正在移除，无法持久化审批续跑终态。".to_string());
                        }
                        match owner_ids {
                            (Some(conversation_id), Some(assistant_message_id)) => self
                                .persist_final_assistant_output(
                                    conversation_id,
                                    assistant_message_id,
                                    &mut agent_output,
                                    final_response_collaboration_cutoff,
                                ),
                            _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                        }
                    },
                    || restore_run_usage_state(self, &run_id, &previous_usage_state),
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
                                    "项目或会话正在移除，无法完成审批状态迁移。".to_string()
                                );
                            }
                            self.transition_pending_status(&record, final_pending_status)
                        },
                        || {},
                    )
                    .await
                } else {
                    Err("assistant 终态未持久化，已保留非终态 pending 记录供启动对账。".to_string())
                };
                let settlement_committed = persisted.is_ok() && pending_transition.is_ok();
                if let Err(error) = &pending_transition {
                    terminal_event_gate.discard();
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        error,
                    );
                }
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    committed_durable_context,
                );
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    deletion_cleanup = true;
                    terminal_event_gate.discard();
                    self.discard_usage_context(&run_id);
                } else if let (Some(conversation_id), Some(assistant_message_id)) = owner_ids {
                    if terminal_commit_published {
                        self.notify_durable_turn_observers(assistant_message_id);
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            trace_sequence: None,
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if terminal_commit_published {
                        committed_terminal_output = Some(agent_output);
                    }
                }
                (
                    terminal_commit_published && !deletion_cleanup,
                    settlement_committed,
                )
            }
            Err(error) => {
                terminal_event_gate.discard();
                let model_request_interruption = error.model_request_interruption();
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let runtime_terminal_trace = error.conversation_turn_trace().cloned();
                let previous_usage_state = self
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner())
                    .get(&run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = self
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner());
                        if deletion_lifecycle.contains_input(&record.agent_input) {
                            return Err(
                                "项目或会话正在移除，无法持久化审批续跑失败终态。".to_string()
                            );
                        }
                        if let (Some(conversation_id), Some(assistant_message_id)) = (
                            record.snapshot.conversation_id.as_deref(),
                            record.snapshot.assistant_message_id.as_deref(),
                        ) {
                            let terminal_projection = match runtime_terminal_trace.clone() {
                                Some(trace) => Ok((trace, None)),
                                None => self
                                    .storage
                                    .get_conversation_turn_trace(assistant_message_id)
                                    .and_then(|trace| match trace {
                                        Some(trace)
                                            if trace.run_id == run_id
                                                && trace.conversation_id == conversation_id
                                                && trace.terminal_status
                                                    == ConversationTurnTraceTerminalStatus::InProgress =>
                                        {
                                            let model_context_items = self
                                                .storage
                                                .get_conversation_model_context_log(
                                                    assistant_message_id,
                                                )?
                                                .map(|log| log.items)
                                                .unwrap_or_default();
                                            let next_sequence = trace
                                                .items
                                                .last()
                                                .map(ConversationTurnTraceItem::sequence)
                                                .unwrap_or(0)
                                                .saturating_add(1);
                                            let terminal =
                                                terminal_conversation_trace_from_snapshot(
                                                    ConversationTraceSnapshot {
                                                        items: trace.items,
                                                        model_context_items,
                                                        next_sequence,
                                                        truncated: trace.truncated,
                                                    },
                                                    &run_id,
                                                    conversation_id,
                                                    assistant_message_id,
                                                    ConversationTurnTraceTerminalStatus::Failed,
                                                    &message,
                                                )?;
                                            Ok((terminal.trace, Some(terminal.model_context_items)))
                                        }
                                        Some(_) => Err(
                                            "审批续跑的 durable ConversationTurnTrace 身份或状态不一致。"
                                                .to_string(),
                                        ),
                                        None => Ok((
                                            failed_conversation_trace_without_items(
                                                &run_id,
                                                conversation_id,
                                                assistant_message_id,
                                                &message,
                                            ),
                                            Some(Vec::new()),
                                        )),
                                    }),
                            };
                            terminal_projection.and_then(
                                |(conversation_turn_trace, model_context_items)| {
                                    if model_request_interruption.is_some() {
                                        self.persist_assistant_model_request_interruption_with_model_context(
                                            conversation_id,
                                            assistant_message_id,
                                            &message,
                                            usage.clone(),
                                            &conversation_turn_trace,
                                            model_context_items.as_deref(),
                                        )
                                    } else {
                                        self.persist_assistant_error_with_model_context(
                                            conversation_id,
                                            assistant_message_id,
                                            &message,
                                            usage.clone(),
                                            &conversation_turn_trace,
                                            model_context_items.as_deref(),
                                        )
                                    }
                                },
                            )
                        } else {
                            Err("审批续跑缺少 assistant 持久化身份。".to_string())
                        }
                    },
                    || restore_run_usage_state(self, &run_id, &previous_usage_state),
                )
                .await;
                let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                let pending_transition = if persisted.is_ok() {
                    persist_terminal_with_bounded_retry(
                        || {
                            let deletion_lifecycle = self
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|lock_error| lock_error.into_inner());
                            if deletion_lifecycle.contains_input(&record.agent_input) {
                                return Err(
                                    "项目或会话正在移除，无法完成审批失败状态迁移。".to_string()
                                );
                            }
                            self.transition_pending_status(&record, final_pending_status)
                        },
                        || {},
                    )
                    .await
                } else {
                    Err(
                        "assistant 失败终态未持久化，已保留非终态 pending 记录供启动对账。"
                            .to_string(),
                    )
                };
                let settlement_committed = persisted.is_ok() && pending_transition.is_ok();
                if let Err(transition_error) = &pending_transition {
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        transition_error,
                    );
                }
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    true,
                );
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    deletion_cleanup = true;
                    self.discard_usage_context(&run_id);
                } else if let Err(error) = &persisted {
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        trace_sequence: None,
                        message: format!("无法原子持久化 assistant 失败终态与会话轨迹：{error}"),
                        recoverable: true,
                        code: Some("conversation_trace_persistence_failed".to_string()),
                        details: None,
                    }));
                } else if terminal_commit_published {
                    if let (Some(conversation_id), Some(assistant_message_id)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.snapshot.assistant_message_id.as_deref(),
                    ) {
                        self.notify_durable_turn_observers(assistant_message_id);
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            if model_request_interruption.is_some() {
                                ""
                            } else {
                                &message
                            },
                        );
                    }
                    let terminal_error_event = if let Some(reason) = model_request_interruption {
                        model_request_interruption_event(&run_id, reason)
                    } else {
                        AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            trace_sequence: None,
                            message: message.clone(),
                            recoverable: false,
                            code,
                            details,
                        }
                    };
                    let _ = notifications.send(agent_event_notification(terminal_error_event));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: model_request_interruption.is_none().then_some(message),
                        usage: cumulative_usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
                (
                    terminal_commit_published && !deletion_cleanup,
                    settlement_committed,
                )
            }
        };

        if durable_turn_terminal || deletion_cleanup {
            self.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
        }
        if deletion_cleanup || (durable_turn_terminal && !keep_trace_snapshot) {
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
        }
        if waiting_for_user_input && settlement_committed {
            self.release_turn_concurrency_permit(&run_id);
        }
        if durable_turn_terminal
            || deletion_cleanup
            || (keep_trace_snapshot && settlement_committed)
        {
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
        }
        // Match ordinary Turns: successful Done must not retain the completed
        // approval continuation's occupancy or permit when a receiver sends the next Turn.
        if let (Some(output), Some(assistant_message_id)) = (
            committed_terminal_output.as_ref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) {
            {
                let deletion_lifecycle = self
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if deletion_lifecycle.contains_input(&record.agent_input) {
                    terminal_event_gate.discard();
                } else {
                    emit_terminal_events_after_persistence_for_turn(
                        &notifications,
                        &terminal_event_gate,
                        output,
                        resumed_identity.as_ref(),
                        assistant_message_id,
                    );
                }
            }
            #[cfg(test)]
            run_after_terminal_publication_hook(&run_id);
        }
        if waiting_for_user_input && cancellation_token.is_cancelled() {
            self.cancel_waiting_human_input_run(&run_id);
        }
        if durable_turn_terminal {
            if let Err(error) = self
                .storage
                .settle_async_human_interaction_start_failure(&run_id)
            {
                eprintln!("failed to settle terminal async answer receipt: {error}");
            }
        }
        if (waiting_for_user_input && settlement_committed) || durable_turn_terminal {
            self.publish_human_delivery_changes(&turn_conversation_id, &notifications);
            self.schedule_human_input_deliveries(notifications);
        }
    }
}
