impl AgentService {
    pub(in crate::application::agent) async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let mut steering_cleanup = self.active_run_steering_cleanup(&run_id, notifications.clone());
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::Command { command } = record.snapshot.action.clone() else {
            if let Err(error) =
                self.persist_pending_target_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
                return;
            }
            if let Err(error) = self.transition_pending_status(&record, PendingActionStatus::Failed)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Failed,
                    &error,
                );
            }
            return;
        };

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => Some(guard),
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let command_for_error = command.clone();
        let artifact_runtime = self.artifact_runtime.clone();
        let office_engine = self.office_engine.clone();
        let skill_resources = self
            .restore_skill_resource_session(&record.agent_input)
            .ok()
            .flatten();
        let file_input_context = agent_file_input_execution_context(
            &record.agent_input,
            skill_resources,
            Arc::clone(&self.storage),
        );
        file_effect_guard
            .as_mut()
            .expect("approved command retains its file-effect lease before Session start")
            .mark_effects_started();
        let command_sessions = self.command_sessions.clone();
        let session_owner = CommandSessionOwner {
            conversation_id: record.snapshot.conversation_id.clone().unwrap_or_default(),
            assistant_message_id: record
                .snapshot
                .assistant_message_id
                .clone()
                .unwrap_or_default(),
            origin_run_id: run_id.clone(),
            call_id: command.id.clone(),
            project_id: agent_input_project_id(&record.agent_input).map(ToString::to_string),
        };
        let workspace_root = workspace_root_optional(&record.agent_input);
        let permissions = permissions_from_input(&record.agent_input);
        let session_notifications = notifications.clone();
        let cancellation_probe_token = cancellation_token.clone();
        let session_command = command.clone();
        let session_action_id = action_id.clone();
        let command_launch = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            let mut file_effect_guard = file_effect_guard;
            let launch = command_sessions.start(StartAgentCommandSession {
                owner: session_owner,
                workspace_root: workspace_root.as_deref(),
                command: &session_command,
                permissions,
                authorization_source: CommandAuthorizationSource::ExplicitUser,
                approval_provenance: serde_json::json!({
                    "source": "explicit_user",
                    "actionId": session_action_id,
                    "approvalStatus": "approved",
                }),
                artifact_runtime,
                office_engine: Some(office_engine),
                file_inputs: Some(&file_input_context),
                notifications: Some(session_notifications),
                cancellation_token,
                cancel_probe: Some(Arc::new(move || {
                    cancellation_probe_token.is_cancelled() || cancel_flag.load(Ordering::SeqCst)
                })),
                file_effect_guard: &mut file_effect_guard,
            });
            (launch, file_effect_guard)
        })
        .await;

        let (command_launch, mut file_effect_guard) = match command_launch {
            Ok((launch, file_effect_guard)) => (launch, file_effect_guard),
            Err(error) => (Err(format!("命令执行任务失败：{error}")), None),
        };

        let mut command_result = match command_launch {
            Ok(AgentCommandSessionLaunch::Exited(terminal)) => terminal.execution,
            Ok(AgentCommandSessionLaunch::Running {
                snapshot,
                tool_result,
                mut handoff_guard,
            }) => {
                let mut continuation_input = record.agent_input.clone();
                continuation_input.approval_decision = Some(AgentApprovalDecision {
                    action_id: record.snapshot.action_id.clone(),
                    status: AgentApprovalDecisionStatus::Approved,
                    message: None,
                });
                continuation_input.tool_continuation = Some(AgentToolContinuation {
                    call: call.clone(),
                    result: tool_result.clone(),
                });
                let completed_at = now_ms();
                let mut settlement_errors = Vec::new();
                let mut handoff_already_advanced = false;
                let mut audit_definitely_uncommitted = false;
                let handoff = handoff_guard.commit(
                    || {
                        run_cancellation_token.is_cancelled()
                            || post_execution_cancel_flag.load(Ordering::SeqCst)
                    },
                    || {
                        for _ in 0..2 {
                            match self.commit_audited_result_trace_with_continuation(
                                &record,
                                &continuation_input,
                                PendingActionStatus::Completed,
                                None,
                                completed_at,
                                &notifications,
                            ) {
                                Ok(()) => return Ok(()),
                                Err(error) => settlement_errors.push(error),
                            }
                        }
                        match self.inspect_audited_result_trace_with_continuation(
                            &record,
                            &continuation_input,
                            PendingActionStatus::Completed,
                            None,
                            completed_at,
                        ) {
                            Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                                Ok(())
                            }
                            Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                                handoff_already_advanced = true;
                                Ok(())
                            }
                            Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                                audit_definitely_uncommitted = true;
                                Err(format!(
                                    "running receipt was definitely not committed: {}",
                                    settlement_errors.join("; retry: ")
                                ))
                            }
                            Ok(AgentPendingActionSettlementInspection::Diverged {
                                component,
                                reason,
                            }) => Err(format!(
                                "running receipt is divergent at {component}: {reason}; attempts: {}",
                                settlement_errors.join("; retry: ")
                            )),
                            Err(inspection_error) => Err(format!(
                                "running receipt could not be inspected: {inspection_error}; attempts: {}",
                                settlement_errors.join("; retry: ")
                            )),
                        }
                    },
                );

                match handoff {
                    Ok(AgentCommandHandoffOutcome::Adopted) => {
                        if handoff_already_advanced {
                            steering_cleanup.disarm();
                            self.unregister_cancellation_if_current(
                                &run_id,
                                &run_cancellation_token,
                            );
                            return;
                        }
                        {
                            let deletion_lifecycle = self
                                .deletion_lifecycle
                                .lock()
                                .unwrap_or_else(|error| error.into_inner());
                            if !deletion_lifecycle.contains_input(&record.agent_input) {
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::ToolResult {
                                        run_id: run_id.clone(),
                                        result: tool_result,
                                    },
                                ));
                            }
                        }
                        self.run_action_continuation(
                            record,
                            continuation_input,
                            notifications,
                            PendingActionStatus::Completed,
                            Some(run_cancellation_token),
                        )
                        .await;
                        return;
                    }
                    Ok(AgentCommandHandoffOutcome::CancelledBeforeCommit) => {
                        match handoff_guard.abort_before_handoff() {
                            Ok(terminal) => {
                                let mut execution = terminal.execution;
                                execution.cancelled = true;
                                execution
                            }
                            Err(error) => {
                                self.unregister_cancellation_if_current(
                                    &run_id,
                                    &run_cancellation_token,
                                );
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::Error {
                                        run_id: Some(run_id),
                                        trace_sequence: None,
                                        message: "命令已在持久交接前取消，但无法确认进程已经终止；不会伪造终态回执。".to_string(),
                                        recoverable: true,
                                        code: Some(
                                            "command_session_pre_handoff_abort_failed".to_string(),
                                        ),
                                        details: Some(serde_json::json!({
                                            "type": "command_session",
                                            "code": "preHandoffTerminationUnconfirmed",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": false,
                                            "terminationError": bounded_audit_error(&error),
                                        })),
                                    },
                                ));
                                return;
                            }
                        }
                    }
                    Ok(AgentCommandHandoffOutcome::PersistenceFailed(error))
                        if audit_definitely_uncommitted =>
                    {
                        match handoff_guard.abort_before_handoff() {
                            Ok(terminal) => {
                                let mut execution = terminal.execution;
                                execution.error.get_or_insert_with(|| {
                                    format!(
                                        "Command Session was terminated before handoff because its running receipt was not durable: {error}"
                                    )
                                });
                                execution
                            }
                            Err(abort_error) => {
                                self.unregister_cancellation_if_current(
                                    &run_id,
                                    &run_cancellation_token,
                                );
                                let _ = notifications.send(agent_event_notification(
                                    AgentEvent::Error {
                                        run_id: Some(run_id),
                                        trace_sequence: None,
                                        message: "命令的持久交接回执确认未提交，且无法确认进程已经终止；不会伪造已结算状态。".to_string(),
                                        recoverable: true,
                                        code: Some(
                                            "command_session_pre_handoff_abort_failed".to_string(),
                                        ),
                                        details: Some(serde_json::json!({
                                            "type": "command_session",
                                            "code": "preHandoffTerminationUnconfirmed",
                                            "sessionId": snapshot.session_id,
                                            "effectsMayHaveOccurred": true,
                                            "terminationConfirmed": false,
                                            "auditError": bounded_audit_error(&error),
                                            "terminationError": bounded_audit_error(&abort_error),
                                        })),
                                    },
                                ));
                                return;
                            }
                        }
                    }
                    Ok(AgentCommandHandoffOutcome::PersistenceFailed(error)) => {
                        let termination = handoff_guard.abort_before_handoff();
                        self.unregister_cancellation_if_current(&run_id, &run_cancellation_token);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id),
                            trace_sequence: None,
                            message: "命令 Session 的持久交接结果无法权威确认；已停止续跑，且不会伪造已结算状态。".to_string(),
                            recoverable: true,
                            code: Some("command_session_handoff_indeterminate".to_string()),
                            details: Some(serde_json::json!({
                                "type": "command_session",
                                "code": "handoffIndeterminate",
                                "sessionId": snapshot.session_id,
                                "effectsMayHaveOccurred": true,
                                "terminationConfirmed": termination.is_ok(),
                                "auditError": bounded_audit_error(&error),
                                "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                                "execution": termination.as_ref().ok().map(|value| &value.execution),
                            })),
                        }));
                        return;
                    }
                    Err(error) => {
                        let termination = handoff_guard.abort_before_handoff();
                        self.unregister_cancellation_if_current(&run_id, &run_cancellation_token);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id),
                            trace_sequence: None,
                            message: "命令 Session 所有权交接失败；已停止续跑。".to_string(),
                            recoverable: true,
                            code: Some("command_session_handoff_failed".to_string()),
                            details: Some(serde_json::json!({
                                "type": "command_session",
                                "code": "handoffFailed",
                                "sessionId": snapshot.session_id,
                                "effectsMayHaveOccurred": true,
                                "terminationConfirmed": termination.is_ok(),
                                "handoffError": bounded_audit_error(&error),
                                "terminationError": termination.as_ref().err().map(|value| bounded_audit_error(value)),
                            })),
                        }));
                        return;
                    }
                }
            }
            Err(error) => failed_command_result(&command_for_error, error, None),
        };
        let execution_was_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || command_result.cancelled;
        if execution_was_cancelled {
            command_result.cancelled = true;
        }

        let (agent_input, final_pending_status) = {
            let command_succeeded = !execution_was_cancelled
                && command_result.error.is_none()
                && !command_result.timed_out
                && !command_result.cancelled
                && command_result.exit_code == Some(0);
            let desired_pending_status = if execution_was_cancelled {
                PendingActionStatus::Cancelled
            } else if command_succeeded {
                PendingActionStatus::Completed
            } else {
                PendingActionStatus::Failed
            };
            let mut final_pending_status = desired_pending_status;
            let mut tool_result = command_tool_result(&action_id, &command_result);
            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call: call.clone(),
                result: tool_result.clone(),
            });

            // Capture one immutable terminal timestamp and retry the exact settlement. The
            // storage transaction is idempotent, so a SQLite commit that succeeded but returned
            // an error is recovered without replacing the real result with a synthetic failure.
            let completed_at = now_ms();
            let mut settlement_errors = Vec::new();
            let mut settled = false;
            for _ in 0..2 {
                match self.commit_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                    &notifications,
                ) {
                    Ok(()) => {
                        settled = true;
                        break;
                    }
                    Err(error) => settlement_errors.push(error),
                }
            }

            if !settled {
                match self.inspect_audited_command_result_trace_with_continuation(
                    &record,
                    &agent_input,
                    desired_pending_status,
                    &command_result,
                    completed_at,
                ) {
                    Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                        settled = true;
                    }
                    Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                        steering_cleanup.disarm();
                        if let Some(guard) = file_effect_guard.as_mut() {
                            guard.mark_durably_settled();
                        }
                        self.unregister_cancellation(&run_id);
                        return;
                    }
                    Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {}
                    Ok(AgentPendingActionSettlementInspection::Diverged { component, reason }) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但审计事务处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&reason),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                    Err(inspection_error) => {
                        self.unregister_cancellation(&run_id);
                        emit_manual_command_settlement_error(
                            &notifications,
                            &run_id,
                            ManualCommandSettlementError {
                                code: "approval_result_commit_indeterminate",
                                message: format!(
                                    "命令已经结束，但无法权威核对审计事务是否提交；已停止续跑：{inspection_error}"
                                ),
                                attempt_error: &settlement_errors.join("; retry: "),
                                inspection_error: Some(&inspection_error),
                                command_result: &command_result,
                                tool_result: &tool_result,
                            },
                        );
                        return;
                    }
                }
            }

            if !settled {
                let audit_error = settlement_errors.join("; retry: ");
                tool_result = command_audit_persistence_failure(
                    &command_for_error,
                    "afterExecution",
                    &audit_error,
                    Some(&command_result),
                );
                final_pending_status = PendingActionStatus::Failed;
                agent_input.tool_continuation = Some(AgentToolContinuation {
                    call: call.clone(),
                    result: tool_result.clone(),
                });

                let mut failure_errors = Vec::new();
                for _ in 0..2 {
                    match self.commit_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                        &notifications,
                    ) {
                        Ok(()) => {
                            settled = true;
                            break;
                        }
                        Err(error) => failure_errors.push(error),
                    }
                }

                if !settled {
                    let persistence_error = failure_errors.join("; retry: ");
                    match self.inspect_audited_command_result_trace_with_continuation(
                        &record,
                        &agent_input,
                        final_pending_status,
                        &command_result,
                        completed_at,
                    ) {
                        Ok(AgentPendingActionSettlementInspection::CommittedAtBoundary) => {
                            settled = true;
                        }
                        Ok(AgentPendingActionSettlementInspection::CommittedAndAdvanced) => {
                            steering_cleanup.disarm();
                            if let Some(guard) = file_effect_guard.as_mut() {
                                guard.mark_durably_settled();
                            }
                            self.unregister_cancellation(&run_id);
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::DefinitelyUncommitted) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_persistence_failed",
                                    message: format!(
                                        "命令已经结束，但审计、目标终态和工具结果轨迹确认未提交；已停止续跑：{persistence_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: None,
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Ok(AgentPendingActionSettlementInspection::Diverged {
                            component,
                            reason,
                        }) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但失败回执处于冲突或部分提交状态；已停止续跑：{component}: {reason}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&reason),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                        Err(inspection_error) => {
                            self.unregister_cancellation(&run_id);
                            emit_manual_command_settlement_error(
                                &notifications,
                                &run_id,
                                ManualCommandSettlementError {
                                    code: "approval_result_commit_indeterminate",
                                    message: format!(
                                        "命令已经结束，但无法权威核对失败回执是否提交；已停止续跑：{inspection_error}"
                                    ),
                                    attempt_error: &persistence_error,
                                    inspection_error: Some(&inspection_error),
                                    command_result: &command_result,
                                    tool_result: &tool_result,
                                },
                            );
                            return;
                        }
                    }
                }
            }
            debug_assert!(settled);
            if let Some(guard) = file_effect_guard.as_mut() {
                guard.mark_durably_settled();
            }
            (agent_input, final_pending_status)
        };
        // The file-producing boundary is now durably paired with its ToolResult. Release the
        // project effect lease before model continuation; a concurrent deletion may proceed and
        // the continuation's deletion marker check will then stop any new action.
        drop(file_effect_guard);

        if execution_was_cancelled && final_pending_status == PendingActionStatus::Cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
            if self.is_agent_input_scope_deleting(&record.agent_input) {
                self.discard_usage_context(&run_id);
                if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                    self.release_conversation_turn_if_current(conversation_id, &run_id);
                }
                self.release_turn_concurrency_permit(&run_id);
                self.discard_trace_snapshot(&run_id);
                self.discard_exact_running_context_window_snapshot(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }
            if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: continuation.result.clone(),
                }));
            }
            let continuation_snapshot = self
                .trace_snapshots
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .get(&run_id)
                .cloned();
            let terminal =
                if let (Some(conversation_id), Some(assistant_message_id), Some(snapshot)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                    continuation_snapshot,
                ) {
                    // Settlement already committed the exact Archive pointer and the central-gated
                    // model observation into this authoritative snapshot.
                    let terminal = cancelled_conversation_trace_from_snapshot(
                        snapshot,
                        &run_id,
                        conversation_id,
                        assistant_message_id,
                        REASON,
                    );
                    terminal.map(|terminal| {
                        (
                            conversation_id.to_string(),
                            assistant_message_id.to_string(),
                            terminal,
                        )
                    })
                } else {
                    Err(
                        "cancelled command is missing its settled conversation trace snapshot"
                            .to_string(),
                    )
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
                .get(&run_id)
                .cloned();
            let persisted = persist_terminal_with_bounded_retry(
                || {
                    let deletion_lifecycle = self
                        .deletion_lifecycle
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if deletion_lifecycle.contains_input(&record.agent_input) {
                        return Err("项目或会话正在移除，无法持久化已取消命令终态。".to_string());
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
                                "项目或会话正在移除，无法完成已取消命令状态迁移。".to_string()
                            );
                        }
                        self.transition_pending_status(&record, PendingActionStatus::Cancelled)
                    },
                    || {},
                )
                .await
            } else {
                Err("已取消命令终态未持久化，已保留 pending 记录供启动对账。".to_string())
            };
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if deletion_lifecycle.contains_input(&record.agent_input) {
                drop(deletion_lifecycle);
                self.discard_usage_context(&run_id);
                if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                    self.release_conversation_turn_if_current(conversation_id, &run_id);
                }
                self.release_turn_concurrency_permit(&run_id);
                self.discard_trace_snapshot(&run_id);
                self.discard_exact_running_context_window_snapshot(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }
            if let Err(error) = persisted {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    trace_sequence: None,
                    message: format!(
                        "无法原子持久化已取消命令的 assistant 终态与会话轨迹：{error}"
                    ),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
            if let Err(error) = pending_transition {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Cancelled,
                    &error,
                );
                return;
            }
            if let Some(conversation_id) = record.snapshot.conversation_id.as_deref() {
                self.release_conversation_turn_if_current(conversation_id, &run_id);
            }
            self.release_turn_concurrency_permit(&run_id);
            if let Some(assistant_message_id) = record.snapshot.assistant_message_id.as_deref() {
                self.notify_durable_turn_observers(assistant_message_id);
            }
            self.discard_trace_snapshot(&run_id);
            self.discard_exact_running_context_window_snapshot(&run_id);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                run_id,
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: output.usage,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
        }

        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                if let Some(continuation) = agent_input.tool_continuation.as_ref() {
                    let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                        run_id: run_id.clone(),
                        result: continuation.result.clone(),
                    }));
                }
            }
        }

        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            Some(run_cancellation_token),
        )
        .await;
    }
}
