impl AgentService {
    async fn supervise_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        let effects_started = Arc::new(AtomicBool::new(false));
        let worker_effects_started = Arc::clone(&effects_started);
        let worker_service = self.clone();
        let worker_record = record.clone();
        let worker_call = call.clone();
        let worker_notifications = notifications.clone();
        let worker = join_skill_script_worker(async move {
            worker_service
                .run_skill_script_execution(
                    worker_record,
                    worker_call,
                    guard,
                    worker_notifications,
                    worker_effects_started,
                )
                .await;
        })
        .await;
        let Err(error) = worker else {
            return;
        };

        // A worker panic/cancellation must never strand an approved action in this live process.
        // Runtime shutdown also cancels this supervisor, so startup reconciliation remains the
        // authority for that separate crash window and the Skill script is never replayed.
        self.unregister_cancellation(&run_id);
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let current = match self.storage.get_pending_agent_action(&record.storage_id) {
            Ok(Some(current)) => current,
            Ok(None) => return,
            Err(storage_error) => {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    trace_sequence: None,
                    message: "The Skill script worker stopped, but its durable action state could not be inspected; restart reconciliation will resolve it without replaying the script.".to_string(),
                    recoverable: true,
                    code: Some("skill_script_worker_state_unavailable".to_string()),
                    details: Some(serde_json::json!({
                        "type": "skill_script",
                        "code": "workerStateUnavailable",
                        "effectsMayHaveOccurred": effects_started.load(Ordering::SeqCst),
                        "inspectionError": bounded_audit_error(&storage_error),
                    })),
                }));
                return;
            }
        };
        if let Some(target_status) = current
            .target_status
            .as_deref()
            .and_then(pending_status_from_label)
            .filter(|status| {
                matches!(
                    status,
                    PendingActionStatus::Completed
                        | PendingActionStatus::Failed
                        | PendingActionStatus::Cancelled
                )
            })
        {
            self.finish_supervised_skill_script_continuation_failure(
                &record,
                &call,
                &notifications,
                target_status,
            );
            return;
        }
        if !matches!(current.status.as_str(), "approved" | "executing")
            || current.target_status.is_some()
        {
            return;
        }

        let effects_may_have_occurred = effects_started.load(Ordering::SeqCst);
        let tool_result = skill_script_worker_failure_result(
            &action_id,
            effects_may_have_occurred,
            error.is_cancelled(),
        );
        // If the panicking worker crossed the effect boundary, dropping its original guard
        // deliberately left an unsettled fence. Registering the same identity lets the common
        // terminal settlement clear that fence only after the outcome-unknown receipt is durable.
        let file_effect_guard = match self.register_file_effect(
            &record.agent_input,
            &run_id,
            &record.storage_id,
        ) {
            Ok(guard) => Some(guard),
            Err(_) if self.is_agent_input_scope_deleting(&record.agent_input) => {
                self.discard_usage_context(&run_id);
                return;
            }
            Err(register_error) => {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id),
                    trace_sequence: None,
                    message: "The Skill script worker stopped and its terminal receipt could not acquire the file-effect settlement guard; restart reconciliation will resolve it without replaying the script.".to_string(),
                    recoverable: true,
                    code: Some("skill_script_worker_settlement_unavailable".to_string()),
                    details: Some(serde_json::json!({
                        "type": "skill_script",
                        "code": "workerSettlementUnavailable",
                        "effectsMayHaveOccurred": effects_may_have_occurred,
                        "registrationError": bounded_audit_error(&register_error.to_string()),
                    })),
                }));
                return;
            }
        };
        self.finish_skill_script_execution(
            record,
            call,
            tool_result,
            notifications,
            file_effect_guard,
            None,
        )
        .await;
    }

    fn finish_supervised_skill_script_continuation_failure(
        &self,
        record: &PendingActionRecord,
        call: &AgentToolCall,
        notifications: &CoreServerNotificationSender,
        target_status: PendingActionStatus,
    ) {
        const FAILURE_CODE: &str = "skill_script_continuation_worker_failed";
        const FAILURE_MESSAGE: &str = "The Skill script result was durably recorded, but its model continuation stopped unexpectedly. The script and Provider request were not replayed.";
        let run_id = &record.snapshot.run_id;
        let (Some(conversation_id), Some(assistant_message_id)) = (
            record.snapshot.conversation_id.as_deref(),
            record.snapshot.assistant_message_id.as_deref(),
        ) else {
            return;
        };
        let exact_receipt_is_pending = self
            .storage
            .get_conversation_turn_trace(assistant_message_id)
            .ok()
            .flatten()
            .is_some_and(|trace| {
                trace.run_id == *run_id
                    && trace.conversation_id == conversation_id
                    && trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
                    && trace.items.iter().any(|item| {
                        matches!(
                            item,
                            ConversationTurnTraceItem::ToolResult {
                                call_id,
                                tool,
                                ..
                            } if call_id == &call.id && tool == &call.tool
                        )
                    })
            });
        if !exact_receipt_is_pending {
            return;
        }

        let active_child_wake = record
            .agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.as_ref())
            .and_then(|identity| {
                ChildAgentFactory::new(Arc::clone(&self.storage))
                    .resolve_trusted_active_wake_by_identity(identity)
                    .ok()
            });
        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(run_id, cancellation_token.clone());
        let steer_input = self.register_active_run_control(
            run_id,
            conversation_id,
            assistant_message_id,
            record
                .agent_input
                .context
                .as_ref()
                .and_then(|context| context.project_id.as_deref()),
            record.agent_input.model_capabilities,
            permissions_from_input(&record.agent_input),
        );
        self.finish_pre_runtime_action_continuation_failure(
            record,
            notifications,
            Some(&steer_input),
            &cancellation_token,
            target_status,
            FAILURE_CODE,
            FAILURE_MESSAGE.to_string(),
        );

        let durably_failed = self
            .storage
            .get_pending_agent_action(&record.storage_id)
            .ok()
            .flatten()
            .is_some_and(|pending| pending.status == "failed")
            && self
                .storage
                .get_conversation_turn_trace(assistant_message_id)
                .ok()
                .flatten()
                .is_some_and(|trace| {
                    trace.terminal_status == ConversationTurnTraceTerminalStatus::Failed
                });
        if !durably_failed {
            return;
        }

        if let Some(active) = active_child_wake {
            let finish = mycopilot_core::FinishAgentTurnResultInput {
                wake_id: active.spawn.initial_wake.wake_id,
                expected_status: active.spawn.initial_wake.status,
                claim_token: active.claim_token,
                terminal_status: mycopilot_core::AgentWakeStatus::Failed,
                run_id: Some(run_id.clone()),
                assistant_message_id: Some(assistant_message_id.to_string()),
                summary: "The child Agent stopped after recording the Skill script result."
                    .to_string(),
                terminal_error: Some(FAILURE_MESSAGE.to_string()),
            };
            let mut settled = false;
            for _ in 0..2 {
                if self.storage.finish_agent_turn_with_result(&finish).is_ok() {
                    settled = true;
                    break;
                }
            }
            if !settled {
                let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                    run_id: Some(run_id.clone()),
                    trace_sequence: None,
                    message: "The Skill continuation failed durably, but its child Wake result could not be settled; startup recovery will finish it without replay.".to_string(),
                    recoverable: true,
                    code: Some("skill_script_continuation_wake_settlement_failed".to_string()),
                    details: None,
                }));
            }
        }
    }
}
