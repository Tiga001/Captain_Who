impl AgentService {
    pub(in crate::application::agent) fn queue_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .supervise_skill_script_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
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

    pub(in crate::application::agent) fn queue_office_operation_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentActionExecutionOutput, String> {
        let run_id = record.snapshot.run_id.clone();
        let service = self.clone();
        let execution_record = record.clone();
        tokio::spawn(async move {
            service
                .run_office_operation_execution(execution_record, call, guard, notifications)
                .await;
        });

        Ok(AgentActionExecutionOutput {
            action_id: record.snapshot.action_id,
            action_type: record.snapshot.action_type,
            tool_name: record.snapshot.tool_name,
            status: "approved".to_string(),
            file_change_result: None,
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

    pub(in crate::application::agent) async fn run_office_operation_execution(
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
        let AgentProposedAction::OfficeOperation {
            mut office_operation,
        } = record.snapshot.action.clone()
        else {
            let _ = self.persist_pending_target_status(&record, PendingActionStatus::Failed);
            let _ = self.transition_pending_status(&record, PendingActionStatus::Failed);
            return;
        };
        office_operation.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
            };

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let run_cancellation_token = cancellation_token.clone();
        let cancel_flag = guard.cancel_flag();
        let service = self.clone();
        let agent_input = record.agent_input.clone();
        file_effect_guard.mark_effects_started();
        let task_result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            match service.restore_skill_resource_session(&agent_input) {
                Ok(skill_resources) => service.execute_office_operation(
                    &agent_input,
                    &office_operation,
                    skill_resources,
                    cancellation_token,
                    Some(cancel_flag),
                ),
                Err(error) => office_skill_resource_restore_failure(
                    &office_operation,
                    office_tool_name(office_operation.prepared.request.document_kind),
                    &error.to_string(),
                ),
            }
        })
        .await;
        let mut tool_result = match task_result {
            Ok(result) => result,
            Err(error) => AgentToolResult {
                exact_archive_file: None,
                call_id: action_id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(serde_json::json!({
                    "type": "office_operation",
                    "code": "executionTaskFailed",
                    "recovery": "retry",
                })),
                error: Some(format!("Office operation task failed: {error}")),
            },
        };
        // The Office engine owns the commit boundary. A cancellation observed before its
        // commit-ready point produces a cancelled result and discards staging; once the atomic
        // publish begins, a later cancellation must not rewrite a completed commit as cancelled.
        let result_cancelled = tool_result
            .result
            .as_ref()
            .and_then(|result| result.get("cancelled"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if result_cancelled {
            tool_result.ok = false;
            let message = "Office operation was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("office.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        let desired_pending_status = if result_cancelled {
            PendingActionStatus::Cancelled
        } else if tool_result.ok {
            PendingActionStatus::Completed
        } else {
            PendingActionStatus::Failed
        };
        let settlement = self.settle_manual_file_effect(
            &record,
            &call,
            desired_pending_status,
            tool_result,
            "office_operation",
            &notifications,
        );
        let (agent_input, tool_result, final_pending_status) = match settlement {
            ManualFileEffectSettlement::Committed {
                agent_input,
                tool_result,
                pending_status,
            } => (*agent_input, tool_result, pending_status),
            ManualFileEffectSettlement::CommittedAndAdvanced => {
                steering_cleanup.disarm();
                file_effect_guard.mark_durably_settled();
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        file_effect_guard.mark_durably_settled();
        drop(file_effect_guard);
        // The receipt is already durable, but a concurrent message/conversation deletion may now
        // own the scope. Do not emit a transient result for an owner that is being removed; the
        // continuation below observes the same marker and terminates without recreating state.
        {
            let deletion_lifecycle = self
                .deletion_lifecycle
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if !deletion_lifecycle.contains_input(&record.agent_input) {
                let _ = notifications.send(agent_event_notification(AgentEvent::ToolResult {
                    run_id: run_id.clone(),
                    result: tool_result,
                }));
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
