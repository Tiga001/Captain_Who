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
        if !self.install_prepared_history(&prepared).unwrap_or_else(|_| {
            #[cfg(debug_assertions)]
            eprintln!("[历史上下文缓存] 计量基线复用失败：会话={conversation_id}，原因=基线校验或恢复失败，转为正常准备");
            false
        }) {
            self.invalidate_conversation_context_state(&conversation_id);
        }
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
                    PreparedTurnRollback::AgentWake(_) => error.into(),
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
                PreparedTurnRollback::AgentWake(_) => CAUSE.to_string().into(),
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
            trusted_wake: match &rollback {
                PreparedTurnRollback::AgentWake(trusted) => Some(trusted.clone()),
                _ => None,
            },
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

        let runtime_scheduled =
            mycopilot_core::performance::enabled().then(std::time::Instant::now);
        tokio::spawn(async move {
            if let Some(scheduled) = runtime_scheduled {
                mycopilot_core::performance::record(
                    "turn",
                    "runtime_schedule_wait",
                    scheduled.elapsed(),
                    0,
                );
            }
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
        let trusted_wake = segment.trusted_wake.clone();
        let _wake_owner_retirement = RuntimeWakeOwnerRetirement::new(service.clone(), &segment);
        let cancellation_token = segment.cancellation_token.clone();
        let mut steering_retirement = RuntimeSteeringRetirement::for_segment(
            service.clone(),
            &segment,
            notifications.clone(),
        );
        let human_binding = segment
            .human_input_resume
            .as_ref()
            .map(|(_, binding)| binding.clone());

        // Every live segment reserves its durable completion before starting the model. The
        // existing logical Turn permit bounds waiters; this does not grant another logical run.
        // Shutdown closes this lane only after live segments have drained, so a terminal result
        // cannot be discarded merely because preparation or another completion is busy.
        let (completion, delivery_budget) = match service
            .blocking_work
            .completion
            .reserve_pair(runtime_work_estimated_bytes(&pending_agent_input))
            .await
        {
            Ok(reservation) => reservation,
            Err(error) => {
                if trusted_wake.is_some() {
                    // No trustworthy execution/commit fence was acquired. Retain durable
                    // guidance for recovery; only the exact in-memory entry may be closed.
                    drop(steering_retirement);
                } else if let Err(cleanup_error) = steering_retirement.retire_on_control().await {
                    eprintln!("runtime startup steering retirement retained for recovery: {cleanup_error}");
                }
                service.report_runtime_completion_unavailable(
                    &worker_run_id,
                    &cancellation_token,
                    &notifications,
                    &error.to_string(),
                );
                return;
            }
        };

        // The second reservation follows the owned settlement through answer routing and
        // Done, bounding post-owner-release handoff waiters without a physical worker. Both
        // reservations are granted atomically to avoid hold-and-wait at the byte limit.

        let RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
            final_response_collaboration_boundary,
        } = service
            .run_prepared_turn_segment(segment, notifications.clone())
            .await;
        let failure_service = service.clone();
        let failure_run_id = worker_run_id.clone();
        let failure_token = cancellation_token.clone();
        let failure_notifications = notifications.clone();
        let settlement = completion.submit(move || {
            #[cfg(test)]
            run_before_runtime_terminal_settlement_hook(&worker_run_id);
            // Only the existing bounded terminal-retry timer is awaited here. Provider/tool
            // futures never occupy a blocking worker. All guards are acquired inside this job,
            // so no caller waits for the executor while retaining an admission/storage guard.
            tokio::runtime::Handle::current().block_on(async move {
                if let Err(error) = &result {
                    if service.retire_fenced_runtime_segment(
                        error, trusted_wake.as_ref(), (&worker_run_id, &worker_assistant_message_id),
                        &cancellation_token, &notifications, &terminal_event_gate,
                    ) {
                        // Drop only closes the exact process-local queue. No assistant, receipt,
                        // usage, guidance or occupancy mutation belongs to an expired owner.
                        drop(steering_retirement);
                        return None;
                    }
                }
                let result = if trusted_wake.is_none() {
                    steering_retirement.finish_result(result, &terminal_event_gate)
                } else {
                    result
                };
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
                    return None;
                }

                let mut deletion_cleanup = false;
                let mut committed_terminal_output = None;
                let mut committed_failure_events = Vec::new();
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
                                service.persist_final_assistant_output_with_wake_fence(
                                    &worker_conversation_id,
                                    &worker_assistant_message_id,
                                    &mut agent_output,
                                    final_response_collaboration_boundary,
                                    trusted_wake.as_ref(),
                                )
                            },
                            || {
                                if trusted_wake.is_none() {
                                    restore_run_usage_state(&service, &worker_run_id, &previous_usage_state);
                                }
                            },
                        )
                        .await;
                        if let Err(error) = &persisted {
                            if service.retire_terminal_fenced_runtime_segment(
                                error, trusted_wake.as_ref(), (&worker_run_id, &worker_assistant_message_id), &cancellation_token,
                                &notifications, &terminal_event_gate,
                            ) {
                                drop(steering_retirement);
                                return None;
                            }
                        }
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
                                committed_terminal_output = Some(agent_output);
                            }
                            durable_terminal
                        };
                        (durable_terminal, persistence_committed)
                    }
                    Err(error) => {
                        let model_request_interruption = error.model_request_interruption();
                        let partial_response = error.partial_response().unwrap_or_default().to_string();
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
                                if let Some(reason) = model_request_interruption {
                                    service.persist_assistant_model_request_interruption_with_model_context_and_wake_fence(
                                        &worker_conversation_id,
                                        &worker_assistant_message_id,
                                        &message,
                                        usage.clone(),
                                        &conversation_turn_trace,
                                        None,
                                        reason,
                                        &partial_response,
                                        trusted_wake.as_ref(),
                                    )
                                } else {
                                    service.persist_assistant_error_with_model_context_and_wake_fence(
                                        &worker_conversation_id,
                                        &worker_assistant_message_id,
                                        &message,
                                        usage.clone(),
                                        &conversation_turn_trace,
                                        None,
                                        trusted_wake.as_ref(),
                                    )
                                }
                            },
                            || {
                                if trusted_wake.is_none() {
                                    restore_run_usage_state(&service, &worker_run_id, &previous_usage_state);
                                }
                            },
                        )
                        .await;
                        if let Err(error) = &persisted {
                            if service.retire_terminal_fenced_runtime_segment(
                                error, trusted_wake.as_ref(), (&worker_run_id, &worker_assistant_message_id), &cancellation_token,
                                &notifications, &terminal_event_gate,
                            ) {
                                drop(steering_retirement);
                                return None;
                            }
                        }
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
                                        &partial_response
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
                                committed_failure_events.push(terminal_error_event);
                                committed_failure_events.push(AgentEvent::Done {
                                        run_id: worker_run_id.clone(),
                                        user_interrupted: None,
                                        success: false,
                                        status: Some(AgentRunStatus::Failed),
                                        content: model_request_interruption.is_none().then_some(message),
                                        usage: cumulative_usage,
                                        finish_reason: None,
                                        proposed_actions: Vec::new(),
                                    });
                            }
                            persistence_committed
                        };
                        (durable_terminal, persistence_committed)
                    }
                };

                if trusted_wake.is_some() && persistence_committed {
                    steering_retirement.finish_after_commit();
                }
                drop(steering_retirement);

                if durable_terminal {
                    // The atomic terminal commit, not unrelated global receipt recovery, ends
                    // this logical Turn. Retire only its exact process-local owner now: a busy
                    // admission stream must not make preflight report a completed Turn active.
                    // Recovery below still retains admission exclusion before inspecting claims;
                    // receipts, workflow publication and Done retain their original order.
                    service.release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
                    service.release_turn_concurrency_permit(&worker_run_id);
                    if !keep_trace_snapshot {
                        service.discard_trace_snapshot(&worker_run_id);
                        service.discard_exact_running_context_window_snapshot(&worker_run_id);
                    }
                    service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
                }

                let needs_admission = durable_terminal && !deletion_cleanup;
                let recovery_service = service.clone();
                let recovery_input = pending_agent_input.clone();
                DeferredRuntimeSettlement {
                    service: recovery_service,
                    input: recovery_input,
                    needs_admission,
                    complete: Some(Box::new(move || {
                        if durable_terminal && !deletion_cleanup {
                            if let Err(error) = service
                                .storage
                                .workflow_execution_mark_run_unread(&worker_run_id)
                            {
                                eprintln!("failed to mark completed organization input unread: {error}");
                            }
                            if let Ok(inputs) = service
                                .storage
                                .workflow_execution_inputs_for_conversation(&worker_conversation_id)
                            {
                                let instances = inputs
                                    .into_iter()
                                    .filter(|input| input.run_id.as_deref() == Some(worker_run_id.as_str()))
                                    .map(|input| input.instance_id)
                                    .collect::<std::collections::HashSet<_>>();
                                for instance_id in instances {
                                    service.publish_workflow_runtime(&instance_id, &notifications);
                                }
                            }
                        }
                        if deletion_cleanup {
                            service.release_conversation_turn_if_current(&worker_conversation_id, &worker_run_id);
                            service.release_turn_concurrency_permit(&worker_run_id);
                            service.discard_trace_snapshot(&worker_run_id);
                            service.discard_exact_running_context_window_snapshot(&worker_run_id);
                        }
                        if durable_terminal || deletion_cleanup {
                            service.workflow_readiness_changed(Some(&worker_conversation_id));
                        }
                        if waiting_for_user_input && persistence_committed {
                            service.release_turn_concurrency_permit(&worker_run_id);
                        }
                        // This worker has exited even if SQLite could not commit its terminal state. Retain the
                        // durable fence for recovery, but never advertise an abandoned token as a live executor.
                        // The identity check preserves a newer approval/human-input continuation's token.
                        if !durable_terminal {
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
                        if durable_terminal && !deletion_cleanup {
                            // Persistence and owner release above are authoritative. This optional preparation
                            // also benefits organization recipients whose conversation has no renderer mounted.
                            service.enqueue_history_warmup(&worker_conversation_id);
                        }
                        RuntimeDeliveryContinuation {
                            budget: delivery_budget,
                            service: service.clone(),
                            notifications: notifications.clone(),
                            should_deliver: (waiting_for_user_input && persistence_committed) || durable_terminal,
                            publish: Box::new(move || {
                                if !service.is_agent_input_scope_deleting(&pending_agent_input) {
                                    for event in committed_failure_events {
                                        emit_agent_event_notifications(
                                            &service, &notifications,
                                            pending_agent_input.context.as_ref().and_then(|context| context.collaboration_identity.as_ref()),
                                            &worker_run_id, &worker_assistant_message_id, event,
                                        );
                                    }
                                }
                        // Successful Done is also the admission boundary for the next user Turn. Publish it
                        // only after the old owner and permit have been released and its context cleanup is
                        // complete; a receiver may synchronously start another Turn from this notification.
                        if let Some(output) = committed_terminal_output.as_ref() {
                            {
                                let deletion_lifecycle = service
                                    .deletion_lifecycle
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner());
                                if deletion_lifecycle.contains_input(&pending_agent_input) {
                                    terminal_event_gate.discard();
                                } else {
                                    emit_terminal_events_after_persistence_for_turn(
                                        &service,
                                        &notifications,
                                        &terminal_event_gate,
                                        output,
                                        pending_agent_input
                                            .context
                                            .as_ref()
                                            .and_then(|context| context.collaboration_identity.as_ref()),
                                        &worker_assistant_message_id,
                                    );
                                }
                            }
                            #[cfg(test)]
                            run_after_terminal_publication_hook(&worker_run_id);
                        }

                            }),
                        }
                    })),
                    delivery: None,
                }.try_finish()
            })
        });
        let settlement = match settlement.wait().await {
            Ok(Some(tail)) => tail.finish().await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = settlement {
            failure_service.report_runtime_completion_unavailable(
                &failure_run_id,
                &failure_token,
                &failure_notifications,
                &error.to_string(),
            );
        }
    }

    pub(in crate::application::agent) fn retire_fenced_runtime_segment(
        &self,
        error: &AgentError,
        trusted: Option<&mycopilot_core::TrustedAgentWakeTurnAdmission>,
        owner_ids: (&str, &str),
        cancellation_token: &AgentCancellationToken,
        notifications: &CoreServerNotificationSender,
        terminal_gate: &AgentTerminalEventGate,
    ) -> bool {
        let (run_id, assistant_message_id) = owner_ids;
        if error.code() != Some("agent_wake_execution_fenced") {
            return false;
        }
        terminal_gate.discard();
        let handoff = trusted
            .ok_or_else(|| "fenced Runtime has no captured claim".to_string())
            .and_then(|trusted| {
                self.record_fenced_agent_wake_execution(trusted, run_id, assistant_message_id)
            });
        let message = match handoff {
            Ok(()) => error.to_string(),
            Err(handoff_error) => {
                eprintln!("fenced Runtime observer handoff failed: {handoff_error}");
                format!("{error}; runtime recovery handoff failed: {handoff_error}")
            }
        };
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        // Lease/recovery coordination must re-read the persisted original claim. No stale
        // segment is allowed to publish Done, release a successor's slot, or write its trace.
        self.notify_durable_turn_observers(assistant_message_id);
        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
            run_id: Some(run_id.into()),
            trace_sequence: None,
            message,
            recoverable: true,
            code: Some("agent_wake_execution_fenced".into()),
            details: error.details().cloned(),
        }));
        true
    }

    pub(in crate::application::agent) fn retire_terminal_fenced_runtime_segment(
        &self,
        error: &str,
        trusted: Option<&mycopilot_core::TrustedAgentWakeTurnAdmission>,
        owner_ids: (&str, &str),
        cancellation_token: &AgentCancellationToken,
        notifications: &CoreServerNotificationSender,
        terminal_gate: &AgentTerminalEventGate,
    ) -> bool {
        let (run_id, assistant_message_id) = owner_ids;
        if !error.starts_with("agent_wake_terminal_fenced:") {
            return false;
        }
        let fenced = AgentError::structured(
            "agent_wake_execution_fenced",
            error,
            serde_json::json!({"wakeId": trusted.map(|trusted| &trusted.wake_id)}),
        );
        self.retire_fenced_runtime_segment(
            &fenced,
            trusted,
            (run_id, assistant_message_id),
            cancellation_token,
            notifications,
            terminal_gate,
        )
    }

    pub(in crate::application::agent) fn report_runtime_completion_unavailable(
        &self,
        run_id: &str,
        cancellation_token: &AgentCancellationToken,
        notifications: &CoreServerNotificationSender,
        error: &str,
    ) {
        // This worker has stopped; its durable in-progress fence and logical permit remain for
        // recovery, exactly as after terminal-persistence failure. Never report a false success.
        self.unregister_cancellation_if_current(run_id, cancellation_token);
        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
            run_id: Some(run_id.to_string()),
            trace_sequence: None,
            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
            recoverable: true,
            code: Some("conversation_trace_persistence_failed".to_string()),
            details: None,
        }));
    }
}

#[cfg(test)]
include!("runtime_isolation_tests.rs");

/// The old workflow recovery must keep admission exclusion, but waiting for that mutex must not
/// occupy either completion worker: deletion can own it while waiting for another Turn to retire.
struct DeferredRuntimeSettlement {
    service: AgentService,
    input: AgentChatInput,
    needs_admission: bool,
    complete: Option<Box<dyn FnOnce() -> RuntimeDeliveryContinuation + Send>>,
    delivery: Option<RuntimeDeliveryContinuation>,
}
impl DeferredRuntimeSettlement {
    fn try_finish(mut self) -> Option<Self> {
        if self.needs_admission && !self.service.is_agent_input_scope_deleting(&self.input) {
            // Its durable terminal and exact owner retirement already happened. A single
            // bounded recovery worker waits for admission; never repeatedly requeue this tail
            // on completion workers while an unrelated stream of admissions is busy.
            return Some(self);
        }
        // Waiting/failed-persistence segments have no global recovery work. Their cancellation
        // retirement must not queue behind an admission holder waiting for those same segments.
        self.delivery = Some((self.complete.take().expect("completion is consumed once"))());
        Some(self)
    }

    async fn finish(mut self) -> Result<(), crate::application::blocking_executor::BlockingError> {
        if let Some(delivery) = self.delivery.take() {
            return delivery.finish().await;
        }
        let executor = self.service.blocking_work.recovery.clone();
        let reservation = executor
            .reserve(runtime_work_estimated_bytes(&self.input))
            .await?;
        let delivery = reservation
            .submit(move || {
                if self.needs_admission && !self.service.is_agent_input_scope_deleting(&self.input)
                {
                    // Keep the original claim/preparation exclusion. This lane is independent of
                    // preparation, control, lease, observation and durable terminal settlement.
                    let _admission = self
                        .service
                        .conversation_admission
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if let Err(error) = self.service.storage.workflow_execution_recover_claims() {
                        eprintln!("failed to settle organization input receipts: {error}");
                    }
                }
                (self.complete.take().expect("completion is consumed once"))()
            })
            .wait()
            .await?;
        delivery.finish().await
    }
}

/// Carries exact steering ownership across async capacity/model waits. Ordinary retirement runs
/// on the reserved completion worker. Abandonment only closes the in-memory entry: queued rows
/// are already durable and must remain available to startup recovery when no worker is available.
pub(in crate::application::agent) struct RuntimeSteeringRetirement {
    service: AgentService,
    run_id: String,
    queue: Option<AgentSteerInputQueue>,
    notifications: CoreServerNotificationSender,
    error_context: &'static str,
}
impl RuntimeSteeringRetirement {
    fn for_segment(
        service: AgentService,
        segment: &PreparedRuntimeTurnSegment,
        notifications: CoreServerNotificationSender,
    ) -> Self {
        Self {
            service,
            run_id: segment.run_id.clone(),
            queue: Some(segment.steer_input.clone()),
            notifications,
            error_context: segment.steering_close_error_context,
        }
    }

    pub(in crate::application::agent) fn for_active_run(
        service: AgentService,
        run_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> Self {
        let queue = service
            .active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(run_id)
            .map(|control| control.steer_input.clone());
        Self {
            service,
            run_id: run_id.into(),
            queue,
            notifications,
            error_context: "无法关闭审批续跑的用户引导通道并持久化剩余引导",
        }
    }

    pub(in crate::application::agent) fn adopt_segment(
        &mut self,
        segment: &PreparedRuntimeTurnSegment,
    ) {
        self.queue = Some(segment.steer_input.clone());
        self.error_context = segment.steering_close_error_context;
    }

    fn finish(&mut self) -> Result<(), String> {
        let Some(queue) = self.queue.take() else {
            return Ok(());
        };
        self.service.unregister_active_run_control(
            &self.run_id,
            &queue,
            AgentSteerRunRejectionCode::RunNotSteerable,
            "The agent run has finished and no longer accepts guidance.",
            &self.notifications,
        )
    }

    pub(in crate::application::agent) fn finish_result(
        &mut self,
        result: AgentResult<AgentChatOutput>,
        terminal_gate: &AgentTerminalEventGate,
    ) -> AgentResult<AgentChatOutput> {
        match self.finish() {
            Ok(()) => result,
            Err(error) => {
                terminal_gate.discard();
                Err(AgentError::new(format!("{}：{error}", self.error_context)))
            }
        }
    }

    pub(in crate::application::agent) fn finish_after_commit(&mut self) {
        if let Err(error) = self.finish() {
            // The terminal transaction is authoritative. A cleanup error cannot rewrite it,
            // and the durable guidance rows remain recovery evidence.
            let _ = self
                .notifications
                .send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(self.run_id.clone()),
                    trace_sequence: None,
                    message: format!("{}：{error}", self.error_context),
                    recoverable: true,
                    code: Some("guidance_retirement_failed".into()),
                    details: None,
                }));
        }
    }

    pub(in crate::application::agent) async fn retire_on_control(
        mut self,
    ) -> Result<(), crate::application::blocking_executor::BlockingError> {
        let reservation = self.service.blocking_work.control.reserve(0).await?;
        reservation
            .submit(move || self.finish())
            .wait()
            .await?
            .map_err(crate::application::blocking_executor::BlockingError::WorkerUnavailable)
    }
}
impl Drop for RuntimeSteeringRetirement {
    fn drop(&mut self) {
        let Some(queue) = self.queue.take() else {
            return;
        };
        let mut active = self
            .service
            .active_runs
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let Some(control) = active.get_mut(&self.run_id) else {
            return;
        };
        if !control.steer_input.is_same_queue(&queue) {
            return;
        }
        control.steer_state = super::ActiveRunSteerState::Closed {
            code: AgentSteerRunRejectionCode::RunNotSteerable,
            message: "The agent execution stopped before the guidance could be applied.".into(),
        };
        // Each accepted guidance has a durable row before being enqueued. Closing this exact
        // process-local queue cannot erase those recovery records or close a successor queue.
        drop(control.steer_input.close_and_take_pending());
    }
}

/// This registry is only a process-local accelerator. Exact token identity prevents an old
/// segment from removing an approval successor, and refused claims remain for durable recovery.
pub(in crate::application::agent) struct RuntimeWakeOwnerRetirement {
    service: AgentService,
    trusted: Option<mycopilot_core::TrustedAgentWakeTurnAdmission>,
    run_id: String,
    assistant_message_id: String,
    cancellation: AgentCancellationToken,
}
impl RuntimeWakeOwnerRetirement {
    pub(in crate::application::agent) fn new(
        service: AgentService,
        segment: &PreparedRuntimeTurnSegment,
    ) -> Self {
        Self {
            service,
            trusted: segment.trusted_wake.clone(),
            run_id: segment.run_id.clone(),
            assistant_message_id: segment.assistant_message_id.clone(),
            cancellation: segment.cancellation_token.clone(),
        }
    }
}
impl Drop for RuntimeWakeOwnerRetirement {
    fn drop(&mut self) {
        if let Some(trusted) = &self.trusted {
            self.service.retire_agent_wake_runtime_owner(
                trusted,
                &self.run_id,
                &self.assistant_message_id,
                &self.cancellation,
            );
        }
    }
}

/// Finishes the original synchronous answer-routing contract before publishing Done. This may
/// acquire admission/start a root Turn, so it must not consume a physical completion worker.
/// The owned continuation is awaited by the existing Turn task, never detached after Done.
pub(in crate::application::agent) struct RuntimeDeliveryContinuation {
    pub(in crate::application::agent) budget:
        crate::application::blocking_executor::BlockingReservation,
    pub(in crate::application::agent) service: AgentService,
    pub(in crate::application::agent) notifications: CoreServerNotificationSender,
    pub(in crate::application::agent) should_deliver: bool,
    pub(in crate::application::agent) publish: Box<dyn FnOnce() + Send>,
}
impl RuntimeDeliveryContinuation {
    pub(in crate::application::agent) async fn finish(
        self,
    ) -> Result<(), crate::application::blocking_executor::BlockingError> {
        let reservation = self
            .service
            .blocking_work
            .preparation
            .reserve(64 * 1024)
            .await?;
        reservation
            .submit(move || {
                // Explicitly move the unused execution reservation into this owned job; closure
                // field capture must not drop its budget before routing/publication completes.
                let _budget = self.budget;
                if self.should_deliver {
                    self.service
                        .schedule_human_input_deliveries(self.notifications);
                }
                (self.publish)();
            })
            .wait()
            .await
    }
}
