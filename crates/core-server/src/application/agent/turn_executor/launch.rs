impl AgentService {
    /// Commits the durable Turn lease and launches one initial Runtime segment. Human-root and
    /// trusted child-Wake adapters both end here; only their exact preparation rollback differs.
    pub(super) fn launch_prepared_initial_turn(
        &self,
        prepared: PreparedConversationTurn,
        cancellation_token: AgentCancellationToken,
        notifications: CoreServerNotificationSender,
        rollback: PreparedTurnRollback,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        let run_id = prepared.output.run_id.clone();
        let conversation_id = prepared.output.conversation_id.clone();
        let assistant_message_id = prepared.output.assistant_message_id.clone();
        let mcp_tools = self.capture_mcp_tool_runtime(&prepared.agent_input);
        self.invalidate_conversation_context_state(&conversation_id);
        let projected_runtime = self
            .automation_report_sink_for_agent_run_id(&run_id)
            .and_then(|automation_report_sink| {
                self.context_window_tool_projection_with_mcp_and_automation_report(
                    &prepared.agent_input,
                    prepared.skill_resources.as_ref().map(Arc::clone),
                    mcp_tools.clone(),
                    automation_report_sink.clone(),
                    Some(&run_id),
                )
                .map(|projection| (projection, automation_report_sink))
            });
        let (context_window_tool_projection, automation_report_sink) = match projected_runtime {
            Ok((projection, automation_report_sink)) => (
                RunContextToolProjection::new(projection),
                automation_report_sink,
            ),
            Err(error) => {
                if let PreparedTurnRollback::Rewrite { request_id } = &rollback {
                    let cause = error.to_string();
                    return match self.settle_prepared_rewrite_failure(
                        &conversation_id,
                        &assistant_message_id,
                        Some(&run_id),
                        request_id,
                        &cause,
                    ) {
                        Ok(output) => {
                            self.release_conversation_turn_if_current(&conversation_id, &run_id);
                            self.release_turn_concurrency_permit(&run_id);
                            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                            output.ok_or_else(|| AgentServiceError::from(cause))
                        }
                        Err(settlement_error) => Err(format!(
                            "{cause}；同时无法终态化已接受的编辑重发 Turn：{settlement_error}"
                        )
                        .into()),
                    };
                }
                self.release_conversation_turn_if_current(&conversation_id, &run_id);
                self.release_turn_concurrency_permit(&run_id);
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                return Err(match rollback {
                    PreparedTurnRollback::AgentWake => error.into(),
                    rollback => self.rollback_prepared_initial_turn(
                        &conversation_id,
                        &assistant_message_id,
                        Some(&run_id),
                        &rollback,
                        error,
                    ),
                });
            }
        };
        self.register_usage_context(&run_id, prepared.usage_context.clone());
        if self.is_agent_input_scope_deleting(&prepared.agent_input) {
            const CAUSE: &str = "项目或会话正在移除，无法开始新的 agent 运行。";
            if let PreparedTurnRollback::Rewrite { request_id } = &rollback {
                return match self.settle_prepared_rewrite_failure(
                    &conversation_id,
                    &assistant_message_id,
                    Some(&run_id),
                    request_id,
                    CAUSE,
                ) {
                    Ok(output) => {
                        self.release_conversation_turn_if_current(&conversation_id, &run_id);
                        self.release_turn_concurrency_permit(&run_id);
                        self.discard_usage_context(&run_id);
                        self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                        output.ok_or_else(|| AgentServiceError::from(CAUSE.to_string()))
                    }
                    Err(settlement_error) => Err(format!(
                        "{CAUSE}；同时无法终态化已接受的编辑重发 Turn：{settlement_error}"
                    )
                    .into()),
                };
            }
            self.release_conversation_turn_if_current(&conversation_id, &run_id);
            self.release_turn_concurrency_permit(&run_id);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return Err(match rollback {
                PreparedTurnRollback::AgentWake => CAUSE.to_string().into(),
                rollback => self.rollback_prepared_initial_turn(
                    &conversation_id,
                    &assistant_message_id,
                    Some(&run_id),
                    &rollback,
                    CAUSE,
                ),
            });
        }

        let steer_input = self.register_active_run_control(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            prepared
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.as_deref()),
            prepared.agent_input.model_capabilities,
            prepared
                .agent_input
                .context
                .as_ref()
                .map(|context| context.permissions)
                .unwrap_or_default(),
        );
        initialize_turn_diff_best_effort(
            &self.storage,
            &prepared.agent_input,
            &run_id,
            &conversation_id,
            &assistant_message_id,
        );

        let output = prepared.output.clone();
        let service = self.clone();
        let worker_run_id = run_id;
        let worker_conversation_id = conversation_id;
        let worker_assistant_message_id = assistant_message_id;
        let segment = PreparedRuntimeTurnSegment {
            run_id: worker_run_id.clone(),
            conversation_id: worker_conversation_id.clone(),
            assistant_message_id: worker_assistant_message_id.clone(),
            assistant_created_at: output.assistant_message.created_at,
            agent_input: prepared.agent_input,
            human_input_resume: None,
            skill_resources: prepared.skill_resources,
            mcp_tools,
            automation_report_sink,
            context_window_tool_projection,
            cancellation_token: cancellation_token.clone(),
            steer_input,
            pending_action_predecessor_settlement: None,
            invalidate_mcp_payload_on_pending_store_failure: true,
            steering_close_error_context: "无法关闭用户引导通道并持久化剩余引导",
        };

        if matches!(rollback, PreparedTurnRollback::HumanResponse) {
            let started = self
                .storage
                .load_async_human_interaction_binding_for_run(&worker_run_id)
                .and_then(|binding| match binding {
                    Some(binding) if !cancellation_token.is_cancelled() => self
                        .storage
                        .mark_async_human_interaction_turn_started(&binding),
                    _ => Ok(false),
                });
            if !matches!(started, Ok(true)) {
                self.release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
                self.release_turn_concurrency_permit(&worker_run_id);
                self.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
                self.discard_usage_context(&worker_run_id);
                let _ = self.unregister_active_run_control(
                    &worker_run_id,
                    &segment.steer_input,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    "Human answer startup was fenced.",
                    &notifications,
                );
                return Err(self.rollback_prepared_initial_turn(
                    &worker_conversation_id,
                    &worker_assistant_message_id,
                    Some(&worker_run_id),
                    &rollback,
                    started
                        .err()
                        .map(|e| e.to_string())
                        .unwrap_or_else(|| "human answer startup was cancelled".into()),
                ));
            }
        }

        tokio::spawn(async move {
            service
                .complete_runtime_segment(segment, notifications)
                .await;
        });

        Ok(output)
    }

    /// Shared durable completion for an initial segment and a trusted human-answer segment.
    pub(super) async fn complete_runtime_segment(
        &self,
        segment: PreparedRuntimeTurnSegment,
        notifications: CoreServerNotificationSender,
    ) {
        let service = self.clone();
        let worker_run_id = segment.run_id.clone();
        let worker_conversation_id = segment.conversation_id.clone();
        let worker_assistant_message_id = segment.assistant_message_id.clone();
        let pending_agent_input = segment.agent_input.clone();
        let cancellation_token = segment.cancellation_token.clone();
        let human_binding = segment
            .human_input_resume
            .as_ref()
            .map(|(_, binding)| binding.clone());

        let RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
            final_response_collaboration_cutoff,
        } = service
            .run_prepared_turn_segment(segment, notifications.clone())
            .await;
        let waiting_for_user_input =
            matches!(&result, Ok(output) if output.status == AgentRunStatus::WaitingForUserInput);
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if matches!(output.status, AgentRunStatus::WaitingForApproval | AgentRunStatus::WaitingForUserInput)
        );

        // Test-only scheduling point before any persistence lock is acquired. This lets the
        // approval handoff regression deterministically advance the newly published action
        // while the retiring Runtime segment has not yet attempted its Waiting projection.
        #[cfg(test)]
        if matches!(
            &result,
            Ok(output) if matches!(output.status, AgentRunStatus::WaitingForApproval | AgentRunStatus::WaitingForUserInput)
        ) {
            super::run_lifecycle::run_before_waiting_persistence_hook(&worker_run_id);
        }

        let input_is_deleting = service
            .deletion_lifecycle
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .contains_input(&pending_agent_input);
        if input_is_deleting {
            terminal_event_gate.discard();
            service.release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
            service.release_turn_concurrency_permit(&worker_run_id);
            service.discard_usage_context(&worker_run_id);
            service.discard_trace_snapshot(&worker_run_id);
            service.discard_exact_running_context_window_snapshot(&worker_run_id);
            service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
            return;
        }

        let mut deletion_cleanup = false;
        let (durable_terminal, persistence_committed) = match result {
            Ok(mut agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                let previous_usage_state = service
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .get(&worker_run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = service
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|error| error.into_inner());
                        if deletion_lifecycle.contains_input(&pending_agent_input) {
                            return Err("项目或会话正在移除，无法持久化 agent 终态。".to_string());
                        }
                        service.persist_final_assistant_output(
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            &mut agent_output,
                            final_response_collaboration_cutoff,
                        )
                    },
                    || restore_run_usage_state(&service, &worker_run_id, &previous_usage_state),
                )
                .await;
                let persistence_committed = persisted.is_ok();
                let deletion_lifecycle = service
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let durable_terminal = if deletion_lifecycle.contains_input(&pending_agent_input) {
                    deletion_cleanup = true;
                    terminal_event_gate.discard();
                    service.discard_usage_context(&worker_run_id);
                    false
                } else {
                    if persistence_committed {
                        service.notify_durable_turn_observers(&worker_assistant_message_id);
                    }
                    let durable_terminal = persistence_committed && committed_durable_context;
                    if durable_terminal {
                        service.emit_terminal_context_window_snapshot(
                            &notifications,
                            &pending_agent_input,
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            if agent_output.status == AgentRunStatus::Cancelled {
                                ""
                            } else {
                                &agent_output.content
                            },
                        );
                    } else if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            trace_sequence: None,
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if durable_terminal {
                        emit_terminal_events_after_persistence_for_turn(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                            pending_agent_input
                                .context
                                .as_ref()
                                .and_then(|context| context.collaboration_identity.as_ref()),
                            &worker_assistant_message_id,
                        );
                    }
                    durable_terminal
                };
                (durable_terminal, persistence_committed)
            }
            Err(error) => {
                let model_request_interruption = error.model_request_interruption();
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let conversation_turn_trace =
                    error.conversation_turn_trace().cloned().unwrap_or_else(|| {
                        failed_conversation_trace_without_items(
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            &message,
                        )
                    });
                let previous_usage_state = service
                    .usage_contexts
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner())
                    .get(&worker_run_id)
                    .cloned();
                let persisted = persist_terminal_with_bounded_retry(
                    || {
                        let deletion_lifecycle = service
                            .deletion_lifecycle
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner());
                        if deletion_lifecycle.contains_input(&pending_agent_input) {
                            return Err(
                                "项目或会话正在移除，无法持久化 agent 失败终态。".to_string()
                            );
                        }
                        if model_request_interruption.is_some() {
                            service.persist_assistant_model_request_interruption(
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                &message,
                                usage.clone(),
                                &conversation_turn_trace,
                            )
                        } else {
                            service.persist_assistant_error(
                                &worker_conversation_id,
                                &worker_assistant_message_id,
                                &message,
                                usage.clone(),
                                &conversation_turn_trace,
                            )
                        }
                    },
                    || restore_run_usage_state(&service, &worker_run_id, &previous_usage_state),
                )
                .await;
                let persistence_committed = persisted.is_ok();
                let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                let deletion_lifecycle = service
                    .deletion_lifecycle
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner());
                let durable_terminal = if deletion_lifecycle.contains_input(&pending_agent_input) {
                    deletion_cleanup = true;
                    terminal_event_gate.discard();
                    service.discard_usage_context(&worker_run_id);
                    false
                } else {
                    if persistence_committed {
                        service.notify_durable_turn_observers(&worker_assistant_message_id);
                        service.emit_terminal_context_window_snapshot(
                            &notifications,
                            &pending_agent_input,
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            if model_request_interruption.is_some() {
                                ""
                            } else {
                                &message
                            },
                        );
                    } else if let Err(error) = &persisted {
                        terminal_event_gate.discard();
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            trace_sequence: None,
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if persistence_committed {
                        let collaboration_identity = pending_agent_input
                            .context
                            .as_ref()
                            .and_then(|context| context.collaboration_identity.as_ref());
                        let terminal_error_event = if let Some(reason) = model_request_interruption
                        {
                            terminal_event_gate.discard();
                            model_request_interruption_event(&worker_run_id, reason)
                        } else {
                            terminal_event_gate
                                .take_error_after_persistence()
                                .unwrap_or_else(|| AgentEvent::Error {
                                    run_id: Some(worker_run_id.clone()),
                                    trace_sequence: None,
                                    message: message.clone(),
                                    recoverable: false,
                                    code,
                                    details,
                                })
                        };
                        emit_agent_event_notifications(
                            &notifications,
                            collaboration_identity,
                            &worker_run_id,
                            &worker_assistant_message_id,
                            terminal_error_event,
                        );
                        emit_agent_event_notifications(
                            &notifications,
                            collaboration_identity,
                            &worker_run_id,
                            &worker_assistant_message_id,
                            AgentEvent::Done {
                                run_id: worker_run_id.clone(),
                                success: false,
                                status: Some(AgentRunStatus::Failed),
                                content: model_request_interruption.is_none().then_some(message),
                                usage: cumulative_usage,
                                finish_reason: None,
                                proposed_actions: Vec::new(),
                            },
                        );
                    }
                    persistence_committed
                };
                (durable_terminal, persistence_committed)
            }
        };

        if durable_terminal || deletion_cleanup {
            service.release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
            service.release_turn_concurrency_permit(&worker_run_id);
        }
        if deletion_cleanup || (durable_terminal && !keep_trace_snapshot) {
            service.discard_trace_snapshot(&worker_run_id);
            service.discard_exact_running_context_window_snapshot(&worker_run_id);
        }
        if waiting_for_user_input && persistence_committed {
            service.release_turn_concurrency_permit(&worker_run_id);
        }
        if durable_terminal || deletion_cleanup || (keep_trace_snapshot && persistence_committed) {
            service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
        }

        if waiting_for_user_input && cancellation_token.is_cancelled() {
            service.cancel_waiting_human_input_run(&worker_run_id);
        }
        if durable_terminal {
            // A failed first sample has no completed model-consumption observation. Retire its
            // executing answer receipt now, so it cannot strand later submissions until restart.
            if let Err(error) = service
                .storage
                .settle_async_human_interaction_start_failure(&worker_run_id)
            {
                eprintln!("failed to settle terminal async answer receipt: {error}");
            }
        }
        if (waiting_for_user_input && persistence_committed) || durable_terminal {
            service.publish_human_delivery_changes(&worker_conversation_id, &notifications);
            service.schedule_human_input_deliveries(notifications.clone());
        }
        if durable_terminal {
            if let Some(binding) = human_binding.as_ref() {
                if let Err(error) = service.storage.mark_sync_human_interaction_applied(binding) {
                    eprintln!("failed to acknowledge committed human response: {error}");
                }
            }
            if let Err(error) = service
                .storage
                .cancel_sync_human_interactions_for_run(&worker_run_id)
            {
                eprintln!("failed to retire terminal human questions: {error}");
            }
        }
    }
}
