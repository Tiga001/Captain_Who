impl AgentService {
    pub(in crate::application::agent) fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) -> std::pin::Pin<Box<impl std::future::Future<Output = ()> + Send + '_>> {
        // Approval execution awaits another model turn. Allocate its large future before
        // polling so execution and continuation stack frames do not both retain its state.
        Box::pin(self.run_action_continuation_inner(
            record,
            agent_input,
            notifications,
            final_pending_status,
            existing_cancellation_token,
        ))
    }

    async fn run_action_continuation_inner(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let mut steering_retirement =
            RuntimeSteeringRetirement::for_active_run(self.clone(), &run_id, notifications.clone());
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        let (completion, delivery_budget) = match self
            .blocking_work
            .completion
            .reserve_pair(runtime_work_estimated_bytes(&agent_input))
            .await
        {
            Ok(reservation) => reservation,
            Err(error) => {
                if agent_input
                    .context
                    .as_ref()
                    .is_some_and(|context| context.collaboration_identity.is_some())
                {
                    drop(steering_retirement);
                } else if let Err(cleanup_error) = steering_retirement.retire_on_control().await {
                    eprintln!("approval continuation steering retirement retained for recovery: {cleanup_error}");
                }
                self.report_runtime_completion_unavailable(
                    &run_id,
                    &cancellation_token,
                    &notifications,
                    &error.to_string(),
                );
                return;
            }
        };
        let preparation = self
            .blocking_work
            .preparation
            .reserve(runtime_work_estimated_bytes(&agent_input))
            .await;
        let prepared = match preparation {
            Ok(reservation) => {
                let service = self.clone();
                let record = record.clone();
                let notifications = notifications.clone();
                let cancellation_token = cancellation_token.clone();
                let run_id = run_id.clone();
                reservation.submit(move || {
                    // Pre-runtime refusal may use bounded durable retry; there is no model or
                    // tool network wait in this worker-owned preparation boundary.
                    tokio::runtime::Handle::current().block_on(async move {
                        match service.agent_tree_run_is_stopped(&run_id) {
                            Ok(true) => cancellation_token.cancel(),
                            Ok(false) => {}
                            Err(error) => {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    None,
                                    &cancellation_token,
                                    final_pending_status,
                                    "agent_tree_stop_state_unavailable",
                                    format!("审批续跑无法确认 Agent 树停止状态，已拒绝执行：{error}"),
                                );
                                return None;
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
                            service.finish_pre_runtime_action_continuation_failure(
                                &record,
                                &notifications,
                                None,
                                &cancellation_token,
                                final_pending_status,
                                "collaboration_identity_mismatch",
                                "审批续跑的 Collaboration identity 与冻结 checkpoint 不一致。".to_string(),
                            );
                            return None;
                        }
                        let active_child_wake = if let Some(identity) = resumed_identity.as_ref() {
                            match ChildAgentFactory::new(Arc::clone(&service.storage))
                                .resolve_trusted_active_wake_by_identity(identity)
                            {
                                Ok(bundle) => Some(bundle),
                                Err(_) => {
                                    service.preserve_claimed_action_after_pre_runtime_refusal(
                                        &record,
                                        &notifications,
                                        None,
                                        &cancellation_token,
                                        Some("collaboration_identity_revalidation_failed"),
                                        Some("子 Agent 审批续跑的 Host 协作身份已失效，已拒绝执行。".to_string()),
                                    );
                                    return None;
                                }
                            }
                        } else {
                            None
                        };
                        if cancellation_token.is_cancelled() {
                            service.finish_cancelled_action_continuation(
                                &record,
                                &notifications,
                                final_pending_status,
                                &cancellation_token,
                            )
                            .await;
                            return None;
                        }
                        if service.is_agent_input_scope_deleting(&record.agent_input) {
                            service.preserve_claimed_action_after_pre_runtime_refusal(
                                &record,
                                &notifications,
                                None,
                                &cancellation_token,
                                None,
                                None,
                            );
                            return None;
                        }
                        service.register_cancellation(&run_id, cancellation_token.clone());
                        match service.agent_tree_run_is_stopped(&run_id) {
                            Ok(true) => cancellation_token.cancel(),
                            Ok(false) => {}
                            Err(error) => {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    None,
                                    &cancellation_token,
                                    final_pending_status,
                                    "agent_tree_stop_state_unavailable",
                                    format!("审批续跑无法确认 Agent 树停止状态，已拒绝执行：{error}"),
                                );
                                return None;
                            }
                        }
                        if cancellation_token.is_cancelled() {
                            service.finish_cancelled_action_continuation(
                                &record,
                                &notifications,
                                final_pending_status,
                                &cancellation_token,
                            )
                            .await;
                            return None;
                        }
                        if service.is_agent_input_scope_deleting(&record.agent_input) {
                            cancellation_token.cancel();
                            service.preserve_claimed_action_after_pre_runtime_refusal(
                                &record,
                                &notifications,
                                None,
                                &cancellation_token,
                                None,
                                None,
                            );
                            return None;
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
                                service.preserve_claimed_action_after_pre_runtime_refusal(
                                    &record,
                                    &notifications,
                                    None,
                                    &cancellation_token,
                                    Some("conversation_turn_identity_missing"),
                                    Some("审批续跑缺少 Conversation Turn 持久化身份。".to_string()),
                                );
                                return None;
                            }
                        };
                        if service
                            .ensure_conversation_turn_owner(
                                &turn_conversation_id,
                                &run_id,
                                &turn_assistant_message_id,
                            )
                            .is_err()
                        {
                            service.preserve_claimed_action_after_pre_runtime_refusal(
                                &record,
                                &notifications,
                                None,
                                &cancellation_token,
                                Some("conversation_turn_ownership_conflict"),
                                Some("审批续跑无法安全取得当前会话执行权；已停止执行并保留恢复记录。".to_string()),
                            );
                            return None;
                        }
                        if service.ensure_turn_concurrency_permit(&run_id).is_err() {
                            service.finish_pre_runtime_action_continuation_failure(
                                &record,
                                &notifications,
                                None,
                                &cancellation_token,
                                final_pending_status,
                                "agent_turn_concurrency_limit",
                                "审批续跑当前无法取得执行许可；已安全停止。".to_string(),
                            );
                            return None;
                        }
                        let steer_input = service.resume_active_run_control(
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

                        let skill_resources = match service.restore_skill_resource_session(&agent_input) {
                            Ok(resources) => resources,
                            Err(error) => {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    Some(&steer_input),
                                    &cancellation_token,
                                    final_pending_status,
                                    "skill_resource_snapshot_unavailable",
                                    error.to_string(),
                                );
                                return None;
                            }
                        };
                        let mcp_tools = service.capture_mcp_tool_runtime(&record.agent_input);
                        let automation_report_sink = match service.automation_report_sink_for_agent_run_id(&run_id) {
                            Ok(sink) => sink,
                            Err(error) => {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    Some(&steer_input),
                                    &cancellation_token,
                                    final_pending_status,
                                    "automation_report_capability_unavailable",
                                    error,
                                );
                                return None;
                            }
                        };
                        let initial_context_window_tool_projection = match service
                            .context_window_tool_projection_with_mcp_and_automation_report(
                                &record.agent_input,
                                skill_resources.clone(),
                                mcp_tools.clone(),
                                automation_report_sink.clone(),
                                Some(&run_id),
                            ) {
                            Ok(projection) => projection,
                            Err(error) => {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    Some(&steer_input),
                                    &cancellation_token,
                                    final_pending_status,
                                    "context_window_tool_projection_unavailable",
                                    error,
                                );
                                return None;
                            }
                        };
                        let context_window_tool_projection =
                            RunContextToolProjection::new(initial_context_window_tool_projection);
                        if let Some(active) = active_child_wake.as_ref() {
                            if service
                                .storage
                                .transition_agent_wake(
                                    &active.spawn.initial_wake.wake_id,
                                    active.spawn.initial_wake.status,
                                    mycopilot_core::AgentWakeStatus::Running,
                                    Some(&active.claim_token),
                                )
                                .is_err()
                            {
                                service.finish_pre_runtime_action_continuation_failure(
                                    &record,
                                    &notifications,
                                    Some(&steer_input),
                                    &cancellation_token,
                                    final_pending_status,
                                    "collaboration_wake_resume_transition_failed",
                                    "子 Agent 审批续跑无法安全取得当前唤醒执行权。".to_string(),
                                );
                                return None;
                            }
                            service.notify_durable_turn_observers(&turn_assistant_message_id);
                        }
                        Some(PreparedRuntimeTurnSegment {
                    // Freeze the exact approval-resume claim before another capacity wait.
                    // A same-identity replacement Wake must not authorize this segment.
                    trusted_wake: active_child_wake.as_ref().map(|active| {
                        mycopilot_core::TrustedAgentWakeTurnAdmission {
                            agent_id: active.spawn.agent.agent_id.clone(),
                            wake_id: active.spawn.initial_wake.wake_id.clone(),
                            claim_token: active.claim_token.clone(),
                            source_agent_message_id: active.spawn.collaboration_identity.source_agent_message_id.clone(),
                        }
                    }),
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
                })
                    })
                }).wait().await
            }
            Err(error) => Err(error),
        };
        let segment = match prepared {
            Ok(Some(segment)) => segment,
            Ok(None) => {
                if let Err(error) = steering_retirement.retire_on_control().await {
                    self.report_runtime_completion_unavailable(
                        &run_id,
                        &cancellation_token,
                        &notifications,
                        &error.to_string(),
                    );
                }
                return;
            }
            Err(error) => {
                if let Err(cleanup_error) = steering_retirement.retire_on_control().await {
                    eprintln!("approval continuation steering retirement retained for recovery: {cleanup_error}");
                }
                self.report_runtime_completion_unavailable(
                    &run_id,
                    &cancellation_token,
                    &notifications,
                    &error.to_string(),
                );
                return;
            }
        };
        steering_retirement.adopt_segment(&segment);
        let _wake_owner_retirement = RuntimeWakeOwnerRetirement::new(self.clone(), &segment);
        let trusted_wake = segment.trusted_wake.clone();
        let turn_conversation_id = segment.conversation_id.clone();
        let turn_assistant_message_id = segment.assistant_message_id.clone();
        let resumed_identity = segment
            .agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.clone());
        let RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
            final_response_collaboration_boundary,
        } = self
            .run_prepared_turn_segment(segment, notifications.clone())
            .await;
        let service = self.clone();
        let failure_run_id = run_id.clone();
        let failure_token = cancellation_token.clone();
        let failure_notifications = notifications.clone();
        let settlement = completion.submit(move || {
            // This job owns the complete durable settlement, including bounded retry rollback
            // and publication. Dropping the awaiter cannot interrupt a started transaction.
            tokio::runtime::Handle::current().block_on(async move {
                if let Err(error) = &result {
                    if service.retire_fenced_runtime_segment(
                        error, trusted_wake.as_ref(), (&run_id, &turn_assistant_message_id), &cancellation_token,
                        &notifications, &terminal_event_gate,
                    ) {
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

                if service.is_agent_input_scope_deleting(&record.agent_input) {
                    terminal_event_gate.discard();
                    service.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
                    service.release_turn_concurrency_permit(&run_id);
                    service.discard_usage_context(&run_id);
                    service.discard_trace_snapshot(&run_id);
                    service.discard_exact_running_context_window_snapshot(&run_id);
                    service.unregister_cancellation_if_current(&run_id, &cancellation_token);
                    return None;
                }

                let mut deletion_cleanup = false;
                let mut committed_terminal_output = None;
                let mut committed_failure_events = Vec::new();
                let (durable_turn_terminal, settlement_committed) = match result {
                    Ok(mut agent_output) => {
                        let committed_durable_context = is_terminal_run_status(agent_output.status);
                        let owner_ids = (
                            record.snapshot.conversation_id.as_deref(),
                            record.snapshot.assistant_message_id.as_deref(),
                        );
                        let previous_usage_state = service
                            .usage_contexts
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .get(&run_id)
                            .cloned();
                        let persisted = persist_terminal_with_bounded_retry(
                            || {
                                let deletion_lifecycle = service.register_lifecycle_activity(
                                    &record.agent_input,
                                    &record.snapshot.run_id,
                                );
                                if deletion_lifecycle.is_err() {
                                    return Err("项目或会话正在移除，无法持久化审批续跑终态。".to_string());
                                }
                                match owner_ids {
                                    (Some(conversation_id), Some(assistant_message_id)) => service
                                        .persist_final_assistant_output_with_wake_fence(
                                            conversation_id,
                                            assistant_message_id,
                                            &mut agent_output,
                                            final_response_collaboration_boundary,
                                            trusted_wake.as_ref(),
                                        ),
                                    _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                                }
                            },
                            || {
                                if trusted_wake.is_none() {
                                    restore_run_usage_state(&service, &run_id, &previous_usage_state);
                                }
                            },
                        )
                        .await;
                        if let Err(error) = &persisted {
                            if service.retire_terminal_fenced_runtime_segment(
                                error, trusted_wake.as_ref(), (&run_id, &turn_assistant_message_id), &cancellation_token,
                                &notifications, &terminal_event_gate,
                            ) {
                                drop(steering_retirement);
                                return None;
                            }
                        }
                        let pending_transition = if persisted.is_ok() {
                            persist_terminal_with_bounded_retry(
                                || {
                                    let deletion_lifecycle = service.register_lifecycle_activity(
                                        &record.agent_input,
                                        &record.snapshot.run_id,
                                    );
                                    if deletion_lifecycle.is_err() {
                                        return Err(
                                            "项目或会话正在移除，无法完成审批状态迁移。".to_string()
                                        );
                                    }
                                    service.transition_pending_status(&record, final_pending_status)
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
                        let deletion_lifecycle = service.register_lifecycle_activity(
                            &record.agent_input,
                            &record.snapshot.run_id,
                        );
                        if deletion_lifecycle.is_err() {
                            deletion_cleanup = true;
                            terminal_event_gate.discard();
                            service.discard_usage_context(&run_id);
                        } else if let (Some(conversation_id), Some(assistant_message_id)) = owner_ids {
                            if terminal_commit_published {
                                service.notify_durable_turn_observers(assistant_message_id);
                                service.emit_terminal_context_window_snapshot(
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
                        let partial_response = error.partial_response().unwrap_or_default().to_string();
                        let usage = error.usage().cloned();
                        let code = error.code().map(ToString::to_string);
                        let details = error.details().cloned();
                        let message = error.to_string();
                        let runtime_terminal_trace = error.conversation_turn_trace().cloned();
                        let previous_usage_state = service
                            .usage_contexts
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner())
                            .get(&run_id)
                            .cloned();
                        let persisted = persist_terminal_with_bounded_retry(
                            || {
                                let deletion_lifecycle = service.register_lifecycle_activity(
                                    &record.agent_input,
                                    &record.snapshot.run_id,
                                );
                                if deletion_lifecycle.is_err() {
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
                                        None => service
                                            .storage
                                            .get_conversation_turn_trace(assistant_message_id)
                                            .and_then(|trace| match trace {
                                                Some(trace)
                                                    if trace.run_id == run_id
                                                        && trace.conversation_id == conversation_id
                                                        && trace.terminal_status
                                                            == ConversationTurnTraceTerminalStatus::InProgress =>
                                                {
                                                    let model_context_items = service
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
                                            if let Some(reason) = model_request_interruption {
                                                service.persist_assistant_model_request_interruption_with_model_context_and_wake_fence(
                                                    conversation_id,
                                                    assistant_message_id,
                                                    &message,
                                                    usage.clone(),
                                                    &conversation_turn_trace,
                                                    model_context_items.as_deref(),
                                                    reason,
                                                    &partial_response,
                                                    trusted_wake.as_ref(),
                                                )
                                            } else {
                                                service.persist_assistant_error_with_model_context_and_wake_fence(
                                                    conversation_id,
                                                    assistant_message_id,
                                                    &message,
                                                    usage.clone(),
                                                    &conversation_turn_trace,
                                                    model_context_items.as_deref(),
                                                    trusted_wake.as_ref(),
                                                )
                                            }
                                        },
                                    )
                                } else {
                                    Err("审批续跑缺少 assistant 持久化身份。".to_string())
                                }
                            },
                            || {
                                if trusted_wake.is_none() {
                                    restore_run_usage_state(&service, &run_id, &previous_usage_state);
                                }
                            },
                        )
                        .await;
                        let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                        if let Err(error) = &persisted {
                            if service.retire_terminal_fenced_runtime_segment(
                                error, trusted_wake.as_ref(), (&run_id, &turn_assistant_message_id), &cancellation_token,
                                &notifications, &terminal_event_gate,
                            ) {
                                drop(steering_retirement);
                                return None;
                            }
                        }
                        let pending_transition = if persisted.is_ok() {
                            persist_terminal_with_bounded_retry(
                                || {
                                    let deletion_lifecycle = service.register_lifecycle_activity(
                                        &record.agent_input,
                                        &record.snapshot.run_id,
                                    );
                                    if deletion_lifecycle.is_err() {
                                        return Err(
                                            "项目或会话正在移除，无法完成审批失败状态迁移。".to_string()
                                        );
                                    }
                                    service.transition_pending_status(&record, final_pending_status)
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
                        let deletion_lifecycle = service.register_lifecycle_activity(
                            &record.agent_input,
                            &record.snapshot.run_id,
                        );
                        if deletion_lifecycle.is_err() {
                            deletion_cleanup = true;
                            service.discard_usage_context(&run_id);
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
                                service.notify_durable_turn_observers(assistant_message_id);
                                service.emit_terminal_context_window_snapshot(
                                    &notifications,
                                    &record.agent_input,
                                    &run_id,
                                    conversation_id,
                                    assistant_message_id,
                                    if model_request_interruption.is_some() {
                                        &partial_response
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
                            committed_failure_events.push(terminal_error_event);
                            committed_failure_events.push(AgentEvent::Done {
                                run_id: run_id.clone(),
                                user_interrupted: None,
                                success: false,
                                status: Some(AgentRunStatus::Failed),
                                content: model_request_interruption.is_none().then_some(message),
                                usage: cumulative_usage,
                                finish_reason: None,
                                proposed_actions: Vec::new(),
                            });
                        }
                        (
                            terminal_commit_published && !deletion_cleanup,
                            settlement_committed,
                        )
                    }
                };

                if trusted_wake.is_some() && settlement_committed {
                    steering_retirement.finish_after_commit();
                }
                drop(steering_retirement);

                if durable_turn_terminal || deletion_cleanup {
                    service.release_conversation_turn_if_current(&turn_conversation_id, &run_id);
                    service.release_turn_concurrency_permit(&run_id);
                    service.workflow_readiness_changed(Some(&turn_conversation_id));
                }
                if deletion_cleanup || (durable_turn_terminal && !keep_trace_snapshot) {
                    service.discard_trace_snapshot(&run_id);
                    service.discard_exact_running_context_window_snapshot(&run_id);
                }
                if waiting_for_user_input && settlement_committed {
                    service.release_turn_concurrency_permit(&run_id);
                }
                // A failed durable settlement can retain its fence, but the retiring worker no longer
                // receives cancellation. Never let an interrupt acknowledge this stale token as live.
                service.unregister_cancellation_if_current(&run_id, &cancellation_token);
                if waiting_for_user_input && cancellation_token.is_cancelled() {
                    service.cancel_waiting_human_input_run(&run_id);
                }
                if durable_turn_terminal {
                    if let Err(error) = service
                        .storage
                        .settle_async_human_interaction_start_failure(&run_id)
                    {
                        eprintln!("failed to settle terminal async answer receipt: {error}");
                    }
                }
                if (waiting_for_user_input && settlement_committed) || durable_turn_terminal {
                    service.publish_human_delivery_changes(&turn_conversation_id, &notifications);

                }
                Some(RuntimeDeliveryContinuation {
                    budget: delivery_budget,
                    service: service.clone(), notifications: notifications.clone(),
                    should_deliver: (waiting_for_user_input && settlement_committed) || durable_turn_terminal,
                    publish: Box::new(move || {
                        if !service.is_agent_input_scope_deleting(&record.agent_input) {
                            for event in committed_failure_events {
                                let _ = notifications.send(agent_event_notification(event));
                            }
                        }
                // Match ordinary Turns: successful Done must not retain the completed
                // approval continuation's occupancy or permit when a receiver sends the next Turn.
                if let (Some(output), Some(assistant_message_id)) = (
                    committed_terminal_output.as_ref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    {
                        let deletion_lifecycle = service.register_lifecycle_activity(
                            &record.agent_input,
                            &record.snapshot.run_id,
                        );
                        if deletion_lifecycle.is_err() {
                            terminal_event_gate.discard();
                        } else {
                            emit_terminal_events_after_persistence_for_turn(
                                &service,
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
                    }),
                })
            })
        });
        let settled = match settlement.wait().await {
            Ok(Some(delivery)) => delivery.finish().await,
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        if let Err(error) = settled {
            self.report_runtime_completion_unavailable(
                &failure_run_id,
                &failure_token,
                &failure_notifications,
                &error.to_string(),
            );
        }
    }
}
