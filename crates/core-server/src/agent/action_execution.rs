use super::*;

impl AgentService {
    pub(super) fn host_action_executor(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
    ) -> AgentHostActionExecutor {
        let service = self.clone();
        Arc::new(move |action, cancellation_token| {
            service.execute_auto_approved_action(
                agent_input.clone(),
                run_id.clone(),
                conversation_id.clone(),
                assistant_message_id.clone(),
                action,
                cancellation_token,
            )
        })
    }

    pub(super) fn execute_auto_approved_action(
        &self,
        agent_input: AgentChatInput,
        run_id: String,
        conversation_id: Option<String>,
        assistant_message_id: Option<String>,
        action: AgentProposedAction,
        cancellation_token: AgentCancellationToken,
    ) -> AgentResult<AgentToolResult> {
        cancellation_token.check()?;
        if self.is_agent_input_project_deleting(&agent_input) {
            return Err(AgentError::cancelled());
        }
        let created_at = now_ms();
        match action {
            AgentProposedAction::Diff { diff } => {
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action_id = diff.id.clone();
                let execution = approved_patch_execution_for_input(&agent_input, &action_id, &diff);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Diff { diff },
                    &execution.status,
                    execution.patch_result.as_ref(),
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(execution.tool_result)
            }
            AgentProposedAction::Command { command } => {
                let workspace_root = workspace_root_optional(&agent_input);
                let permissions = permissions_from_input(&agent_input);
                let command_for_error = command.clone();
                let command_result = run_authorized_command(
                    workspace_root.as_deref(),
                    &command,
                    permissions,
                    CommandAuthorizationSource::Automatic,
                    cancellation_token.clone(),
                    None,
                )
                .unwrap_or_else(|error| {
                    let policy_evaluation = error.policy_evaluation().cloned();
                    failed_command_result(&command_for_error, error.to_string(), policy_evaluation)
                });
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                let command_succeeded = command_result.error.is_none()
                    && !command_result.timed_out
                    && !command_result.cancelled
                    && command_result.exit_code == Some(0);
                let tool_result =
                    command_tool_result(&command.id, command_succeeded, &command_result);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    AgentProposedAction::Command { command },
                    if command_succeeded {
                        "completed"
                    } else {
                        "failed"
                    },
                    None,
                    Some(&command_result),
                    Some(&tool_result),
                    tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(tool_result)
            }
            AgentProposedAction::FileWrite { file_write } => {
                let deleting_projects = self
                    .deleting_projects
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                if agent_input_project_id(&agent_input)
                    .is_some_and(|project_id| deleting_projects.contains(project_id))
                {
                    return Err(AgentError::cancelled());
                }
                cancellation_token.check()?;
                let action = AgentProposedAction::FileWrite {
                    file_write: file_write.clone(),
                };
                let execution =
                    approved_file_write_execution(&self.storage, &agent_input, &file_write);
                self.record_auto_action_audit(
                    &run_id,
                    conversation_id,
                    assistant_message_id,
                    &agent_input,
                    action,
                    &execution.status,
                    None,
                    None,
                    Some(&execution.tool_result),
                    execution.tool_result.error.as_deref(),
                    created_at,
                    now_ms(),
                );
                drop(deleting_projects);
                Ok(execution.tool_result)
            }
            AgentProposedAction::ToolCall { call } => Ok(AgentToolResult {
                call_id: call.id,
                tool: call.tool,
                ok: false,
                result: None,
                error: Some(
                    "自动批准执行器只支持结构化 apply_patch diff 和 run_command。".to_string(),
                ),
            }),
        }
    }

    pub(super) fn queue_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let command_record = record.clone();
        tokio::spawn(async move {
            service
                .run_command_execution(command_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            patch_result: None,
            file_write_result: None,
            command_result: None,
            tool_result: None,
            agent_output: AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Running,
                run_id,
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: None,
            },
        })
    }

    pub(super) async fn run_command_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_project_deleting(&record.agent_input) {
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

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let command_for_error = command.clone();
        let command_for_execution = command.clone();
        let execution_record = record.clone();
        let mut command_result = match tokio::task::spawn_blocking(move || {
            let _guard = guard;
            if cancel_flag.load(Ordering::SeqCst) || cancellation_token.is_cancelled() {
                Ok(cancelled_command_result(&command_for_execution))
            } else {
                run_explicitly_approved_command_from_snapshot(
                    &execution_record,
                    cancellation_token,
                    Some(cancel_flag),
                )
            }
        })
        .await
        {
            Ok(Ok(result)) => result,
            Ok(Err(error)) => {
                let policy_evaluation = error.policy_evaluation().cloned();
                failed_command_result(&command_for_error, error.to_string(), policy_evaluation)
            }
            Err(error) => failed_command_result(
                &command_for_error,
                format!("命令执行任务失败：{error}"),
                None,
            ),
        };
        let execution_was_cancelled = run_cancellation_token.is_cancelled()
            || post_execution_cancel_flag.load(Ordering::SeqCst)
            || command_result.cancelled;
        if execution_was_cancelled {
            command_result.cancelled = true;
        }

        let (agent_input, final_pending_status) = {
            let deleting_projects = self
                .deleting_projects
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if agent_input_project_id(&record.agent_input)
                .is_some_and(|project_id| deleting_projects.contains(project_id))
            {
                self.discard_usage_context(&run_id);
                self.unregister_cancellation(&run_id);
                return;
            }

            let command_succeeded = !execution_was_cancelled
                && command_result.error.is_none()
                && !command_result.timed_out
                && !command_result.cancelled
                && command_result.exit_code == Some(0);
            let final_pending_status = if execution_was_cancelled {
                PendingActionStatus::Cancelled
            } else if command_succeeded {
                PendingActionStatus::Completed
            } else {
                PendingActionStatus::Failed
            };
            let tool_result = command_tool_result(&action_id, command_succeeded, &command_result);
            self.record_action_audit(
                &record,
                Some("approved"),
                pending_status_label(final_pending_status),
                None,
                Some(&command_result),
                Some(&tool_result),
                tool_result.error.as_deref(),
                None,
                Some(now_ms()),
            );

            let mut agent_input = record.agent_input.clone();
            agent_input.approval_decision = Some(AgentApprovalDecision {
                action_id: action_id.clone(),
                status: AgentApprovalDecisionStatus::Approved,
                message: None,
            });
            agent_input.tool_continuation = Some(AgentToolContinuation {
                call,
                result: tool_result,
            });
            (agent_input, final_pending_status)
        };
        if let Err(error) = self.persist_pending_target_status(&record, final_pending_status) {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("命令目标终态无法持久化，已停止续跑：{error}"),
                recoverable: true,
                code: Some("pending_action_target_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Err(error) =
            self.commit_trace_snapshot_with_continuation(&record, &agent_input, &notifications)
        {
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                run_id: Some(run_id),
                message: format!("命令结果无法写入会话轨迹：{error}"),
                recoverable: true,
                code: Some("conversation_trace_persistence_failed".to_string()),
                details: None,
            }));
            return;
        }
        if let Some(continuation) = agent_input.tool_continuation.as_ref() {
            let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                run_id: run_id.clone(),
                result: continuation.result.clone(),
            }));
        }

        if execution_was_cancelled {
            const REASON: &str =
                "Agent run was cancelled while the approved command was executing.";
            let persisted = if let (
                Some(conversation_id),
                Some(assistant_message_id),
                Some(checkpoint),
                Some(continuation),
            ) = (
                record.snapshot.conversation_id.as_deref(),
                record.snapshot.assistant_message_id.as_deref(),
                record.agent_input.resume_checkpoint.as_ref(),
                agent_input.tool_continuation.as_ref(),
            ) {
                let trace = cancelled_conversation_trace_from_checkpoint(
                    checkpoint,
                    conversation_id,
                    assistant_message_id,
                    &continuation.call,
                    &continuation.result,
                    REASON,
                );
                let output = AgentChatOutput {
                    content: String::new(),
                    status: AgentRunStatus::Cancelled,
                    run_id: run_id.clone(),
                    events: Vec::new(),
                    tool_definitions: Vec::new(),
                    todo: None,
                    usage: None,
                    finish_reason: Some(REASON.to_string()),
                    proposed_actions: Vec::new(),
                    conversation_turn_trace: Some(trace),
                };
                self.persist_final_assistant_output(conversation_id, assistant_message_id, &output)
            } else {
                Err("cancelled command is missing its conversation trace checkpoint".to_string())
            };
            if let Err(error) = persisted {
                self.unregister_cancellation(&run_id);
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    message: format!(
                        "无法原子持久化已取消命令的 assistant 终态与会话轨迹：{error}"
                    ),
                    recoverable: true,
                    code: Some("conversation_trace_persistence_failed".to_string()),
                    details: None,
                }));
                return;
            }
            if let Err(error) =
                self.transition_pending_status(&record, PendingActionStatus::Cancelled)
            {
                emit_pending_transition_error(
                    &notifications,
                    &run_id,
                    PendingActionStatus::Cancelled,
                    &error,
                );
                self.unregister_cancellation(&run_id);
                return;
            }
            self.discard_trace_snapshot(&run_id);
            self.unregister_cancellation(&run_id);
            let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                run_id,
                success: false,
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                usage: None,
                finish_reason: Some(REASON.to_string()),
                proposed_actions: Vec::new(),
            }));
            return;
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

    pub(super) async fn run_action_continuation(
        &self,
        record: PendingActionRecord,
        agent_input: AgentChatInput,
        notifications: CoreServerNotificationSender,
        final_pending_status: PendingActionStatus,
        existing_cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        if self.is_agent_input_project_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }
        let cancellation_token = existing_cancellation_token.unwrap_or_default();
        self.register_cancellation(&run_id, cancellation_token.clone());
        if self.is_agent_input_project_deleting(&record.agent_input) {
            cancellation_token.cancel();
            self.unregister_cancellation(&run_id);
            self.discard_usage_context(&run_id);
            return;
        }

        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = record.snapshot.conversation_id.clone();
        let emitter_assistant_message_id = record.snapshot.assistant_message_id.clone();
        let emitter_agent_input = record.agent_input.clone();
        let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
        let emitter_terminal_event_gate = terminal_event_gate.clone();
        let pending_store_failure = Arc::new(Mutex::new(None::<String>));
        let emitter_pending_store_failure = Arc::clone(&pending_store_failure);
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired {
                run_id,
                action,
                checkpoint,
            } = &event
            {
                let agent_input = agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                match emitter_service.store_pending_action(
                    run_id,
                    emitter_conversation_id.as_deref().unwrap_or_default(),
                    emitter_assistant_message_id.as_deref().unwrap_or_default(),
                    action.clone(),
                    agent_input,
                ) {
                    Ok(true) => {}
                    Ok(false) => return,
                    Err(error) => {
                        emitter_terminal_event_gate.discard();
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                        return;
                    }
                }
            }
            if emitter_pending_store_failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some()
            {
                return;
            }
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                let _ = emitter_notifications.send(agent_event_notification(event));
            }
        });

        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            record.snapshot.conversation_id.clone(),
            record.snapshot.assistant_message_id.clone(),
        );
        let trace_conversation_id = record
            .snapshot
            .conversation_id
            .as_deref()
            .unwrap_or_default();
        let trace_assistant_message_id = record
            .snapshot
            .assistant_message_id
            .as_deref()
            .unwrap_or_default();
        let trace_observer = self.trace_observer(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.snapshot.created_at,
            record.agent_input.clone(),
            notifications.clone(),
        );
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            trace_conversation_id,
            trace_assistant_message_id,
            record.agent_input.clone(),
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, trace_conversation_id, trace_assistant_message_id);
        let host_services = AgentRuntimeHostServices::new()
            .with_host_actions(host_executor, self.storage.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        let result = send_chat_with_host_services(
            agent_input,
            run_id.clone(),
            emitter,
            cancellation_token,
            host_services,
        )
        .await;
        let result = match pending_store_failure
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            Some(error) => {
                terminal_event_gate.discard();
                Err(pending_action_persistence_error(error))
            }
            None => result,
        };
        let keep_trace_snapshot = matches!(
            &result,
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval
        );

        let deleting_projects = self
            .deleting_projects
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if agent_input_project_id(&record.agent_input)
            .is_some_and(|project_id| deleting_projects.contains(project_id))
        {
            drop(deleting_projects);
            self.discard_usage_context(&run_id);
            self.unregister_cancellation(&run_id);
            return;
        }

        match result {
            Ok(agent_output) => {
                let committed_durable_context = is_terminal_run_status(agent_output.status);
                let owner_ids = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                );
                let persisted = match owner_ids {
                    (Some(conversation_id), Some(assistant_message_id)) => self
                        .persist_final_assistant_output(
                            conversation_id,
                            assistant_message_id,
                            &agent_output,
                        ),
                    _ => Err("审批续跑缺少 assistant 持久化身份。".to_string()),
                };
                let pending_transition = if persisted.is_ok() {
                    self.transition_pending_status(&record, final_pending_status)
                } else {
                    Err("assistant 终态未持久化，已保留非终态 pending 记录供启动对账。".to_string())
                }
                .inspect_err(|error| {
                    terminal_event_gate.discard();
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        error,
                    );
                });
                if let (Some(conversation_id), Some(assistant_message_id)) = owner_ids {
                    if pending_terminal_commit_is_publishable(
                        persisted.is_ok(),
                        pending_transition.is_ok(),
                        committed_durable_context,
                    ) {
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
                        self.discard_usage_context(&run_id);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if pending_terminal_commit_is_publishable(
                        persisted.is_ok(),
                        pending_transition.is_ok(),
                        committed_durable_context,
                    ) {
                        emit_terminal_events_after_persistence(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                        );
                    }
                }
            }
            Err(error) => {
                terminal_event_gate.discard();
                let usage = error.usage().cloned();
                let code = error.code().map(ToString::to_string);
                let details = error.details().cloned();
                let message = error.to_string();
                let conversation_turn_trace = match (
                    error.conversation_turn_trace().cloned(),
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    (Some(trace), _, _) => Some(trace),
                    (None, Some(conversation_id), Some(assistant_message_id)) => {
                        Some(failed_conversation_trace_without_items(
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        ))
                    }
                    _ => None,
                };
                let persisted = if let (Some(conversation_id), Some(assistant_message_id)) = (
                    record.snapshot.conversation_id.as_deref(),
                    record.snapshot.assistant_message_id.as_deref(),
                ) {
                    let persisted = self.persist_assistant_error(
                        conversation_id,
                        assistant_message_id,
                        &message,
                        usage.clone(),
                        conversation_turn_trace
                            .as_ref()
                            .expect("trace exists when conversation and assistant ids exist"),
                    );
                    if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(run_id.clone()),
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    persisted
                } else {
                    Err("审批续跑缺少 assistant 持久化身份。".to_string())
                };
                let pending_transition = if persisted.is_ok() {
                    self.transition_pending_status(&record, final_pending_status)
                } else {
                    Err(
                        "assistant 失败终态未持久化，已保留非终态 pending 记录供启动对账。"
                            .to_string(),
                    )
                }
                .inspect_err(|transition_error| {
                    emit_pending_transition_error(
                        &notifications,
                        &run_id,
                        final_pending_status,
                        transition_error,
                    );
                });
                let terminal_commit_published = pending_terminal_commit_is_publishable(
                    persisted.is_ok(),
                    pending_transition.is_ok(),
                    true,
                );
                if terminal_commit_published {
                    if let (Some(conversation_id), Some(assistant_message_id)) = (
                        record.snapshot.conversation_id.as_deref(),
                        record.snapshot.assistant_message_id.as_deref(),
                    ) {
                        self.emit_terminal_context_window_snapshot(
                            &notifications,
                            &record.agent_input,
                            &run_id,
                            conversation_id,
                            assistant_message_id,
                            &message,
                        );
                    }
                    let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                        run_id: Some(run_id.clone()),
                        message: message.clone(),
                        recoverable: false,
                        code,
                        details,
                    }));
                    let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                        run_id: run_id.clone(),
                        success: false,
                        status: Some(AgentRunStatus::Failed),
                        content: Some(message),
                        usage,
                        finish_reason: None,
                        proposed_actions: Vec::new(),
                    }));
                }
            }
        }

        drop(deleting_projects);
        if !keep_trace_snapshot {
            self.discard_trace_snapshot(&run_id);
        }
        self.unregister_cancellation(&run_id);
    }
}

/// Executes only the command frozen in the backend-owned pending-action snapshot.
///
/// The approval endpoint accepts an action id rather than a replacement command. Keeping snapshot
/// selection and the `ExplicitUser` authorization source together at this boundary prevents a
/// caller from turning approval of one command into execution of another.
pub(super) fn run_explicitly_approved_command_from_snapshot(
    record: &PendingActionRecord,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    let AgentProposedAction::Command { command } = &record.snapshot.action else {
        return Err("待审批操作不包含可执行命令。".to_string().into());
    };
    let workspace_root = workspace_root_optional(&record.agent_input);
    run_authorized_command(
        workspace_root.as_deref(),
        command,
        permissions_from_input(&record.agent_input),
        CommandAuthorizationSource::ExplicitUser,
        cancellation_token,
        action_cancel_flag,
    )
}

pub(super) fn cancelled_file_write_outcome_is_durable_or_unknown(
    storage: &StorageService,
    record: &PendingActionRecord,
) -> bool {
    let AgentProposedAction::FileWrite { file_write } = &record.snapshot.action else {
        return false;
    };
    match storage.get_agent_file_draft(&file_write.draft_id) {
        Ok(Some(draft)) => draft.status == "rejected",
        Ok(None) => false,
        // A storage read failure makes rollback unsafe: the rejection write may have committed.
        Err(_) => true,
    }
}
