use super::*;

impl AgentService {
    pub fn start_conversation_turn(
        &self,
        input: AgentConversationTurnInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentConversationTurnOutput, AgentServiceError> {
        if self.is_project_deleting(input.project_id.as_deref())
            || self.is_conversation_deleting(input.conversation_id.as_deref())
        {
            return Err("项目或会话正在移除，无法开始新的 agent 运行。"
                .to_string()
                .into());
        }
        let run_id = next_run_id();
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());

        let prepared = match prepare_conversation_turn(&self.storage, &self.skills, input, &run_id)
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.unregister_cancellation_if_current(&run_id, &cancellation_token);
                return Err(error);
            }
        };

        self.register_usage_context(&run_id, prepared.usage_context.clone());
        if self.is_agent_input_scope_deleting(&prepared.agent_input) {
            self.discard_usage_context(&run_id);
            self.unregister_cancellation_if_current(&run_id, &cancellation_token);
            return Err("项目或会话正在移除，无法开始新的 agent 运行。"
                .to_string()
                .into());
        }

        let output = prepared.output.clone();
        let service = self.clone();
        let worker_run_id = run_id.clone();
        let worker_conversation_id = output.conversation_id.clone();
        let worker_assistant_message_id = output.assistant_message_id.clone();
        let worker_assistant_created_at = output.assistant_message.created_at;
        let pending_agent_input = prepared.agent_input.clone();
        let skill_resources = prepared.skill_resources.clone();

        tokio::spawn(async move {
            let emitter_notifications = notifications.clone();
            let emitter_service = service.clone();
            let emitter_conversation_id = worker_conversation_id.clone();
            let emitter_assistant_message_id = worker_assistant_message_id.clone();
            let emitter_agent_input = pending_agent_input.clone();
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
                    let agent_input =
                        agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                    match emitter_service.store_pending_action(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
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
                let event = emitter_service.project_cumulative_usage_onto_event(event);
                if let Some(event) = emitter_terminal_event_gate.route(event) {
                    let _ = emitter_notifications.send(agent_event_notification(event));
                }
            });

            let host_executor = service.host_action_executor(
                pending_agent_input.clone(),
                worker_run_id.clone(),
                Some(worker_conversation_id.clone()),
                Some(worker_assistant_message_id.clone()),
                skill_resources.clone(),
            );
            let trace_observer = service.trace_observer(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
                worker_assistant_created_at,
                pending_agent_input.clone(),
                notifications.clone(),
            );
            let context_compaction_services = service.context_compaction_services(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
                pending_agent_input.clone(),
                notifications.clone(),
            );
            let model_request_observer = service.model_request_observer(
                &worker_run_id,
                &worker_conversation_id,
                &worker_assistant_message_id,
            );
            let mut host_services = AgentRuntimeHostServices::new()
                .with_host_actions(host_executor, service.storage.clone())
                .with_office_engine(service.office_engine.clone())
                .with_trace_observer(trace_observer)
                .with_model_request_observer(model_request_observer)
                .with_context_compaction(context_compaction_services);
            host_services = host_services.with_skill_activation_resolver(
                model_skill_activation_resolver(service.storage.clone(), service.skills.clone()),
            );
            if let Some(resources) = skill_resources {
                host_services = host_services.with_skill_resources(resources);
            }
            if let Some(resolver) = service.artifact_runtime.clone() {
                host_services = host_services.with_command_runtime_profile_resolver(resolver);
            }
            let result = send_chat_with_host_services(
                prepared.agent_input,
                worker_run_id.clone(),
                emitter,
                cancellation_token.clone(),
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

            let deletion_lifecycle = service
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if deletion_lifecycle.contains_input(&pending_agent_input) {
                drop(deletion_lifecycle);
                service.discard_usage_context(&worker_run_id);
                service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
                return;
            }

            match result {
                Ok(mut agent_output) => {
                    let committed_durable_context = is_terminal_run_status(agent_output.status);
                    let persisted = service.persist_final_assistant_output(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &mut agent_output,
                    );
                    if persisted.is_ok() && committed_durable_context {
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
                        service.discard_usage_context(&worker_run_id);
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: format!("无法原子持久化 assistant 终态与会话轨迹：{error}"),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if persisted.is_ok() && committed_durable_context {
                        emit_terminal_events_after_persistence(
                            &notifications,
                            &terminal_event_gate,
                            &agent_output,
                        );
                    }
                }
                Err(error) => {
                    terminal_event_gate.discard();
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
                    let persisted = service.persist_assistant_error(
                        &worker_conversation_id,
                        &worker_assistant_message_id,
                        &message,
                        usage.clone(),
                        &conversation_turn_trace,
                    );
                    let cumulative_usage = persisted.as_ref().ok().cloned().flatten();
                    if persisted.is_ok() {
                        service.emit_terminal_context_window_snapshot(
                            &notifications,
                            &pending_agent_input,
                            &worker_run_id,
                            &worker_conversation_id,
                            &worker_assistant_message_id,
                            &message,
                        );
                    } else if let Err(error) = &persisted {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: format!(
                                "无法原子持久化 assistant 失败终态与会话轨迹：{error}"
                            ),
                            recoverable: true,
                            code: Some("conversation_trace_persistence_failed".to_string()),
                            details: None,
                        }));
                    }
                    if persisted.is_ok() {
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(worker_run_id.clone()),
                            message: message.clone(),
                            recoverable: false,
                            code,
                            details,
                        }));
                        let _ = notifications.send(agent_event_notification(AgentEvent::Done {
                            run_id: worker_run_id.clone(),
                            success: false,
                            status: Some(AgentRunStatus::Failed),
                            content: Some(message),
                            usage: cumulative_usage,
                            finish_reason: None,
                            proposed_actions: Vec::new(),
                        }));
                    }
                }
            }

            drop(deletion_lifecycle);
            if !keep_trace_snapshot {
                service.discard_trace_snapshot(&worker_run_id);
            }
            service.unregister_cancellation_if_current(&worker_run_id, &cancellation_token);
        });

        Ok(output)
    }
}
