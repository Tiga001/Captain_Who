impl AgentService {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::application::agent) async fn finish_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        tool_result: AgentToolResult,
        notifications: CoreServerNotificationSender,
        mut file_effect_guard: Option<super::super::run_lifecycle::FileEffectGuard>,
        cancellation_token: Option<AgentCancellationToken>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let mut steering_cleanup = self.active_run_steering_cleanup(&run_id, notifications.clone());
        let result_cancelled = cancellation_token
            .as_ref()
            .is_some_and(|token| token.is_cancelled())
            || tool_result
                .result
                .as_ref()
                .and_then(|result| result.get("cancelled"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
        let mut tool_result = tool_result;
        if result_cancelled {
            tool_result.ok = false;
            let message = "Skill script execution was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("skill_script.cancelled".to_string()),
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
            "skill_script",
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
                if let Some(guard) = file_effect_guard.as_mut() {
                    guard.mark_durably_settled();
                }
                self.unregister_cancellation(&run_id);
                return;
            }
            ManualFileEffectSettlement::Unsettled => {
                self.unregister_cancellation(&run_id);
                return;
            }
        };
        if let Some(guard) = file_effect_guard.as_mut() {
            guard.mark_durably_settled();
        }
        drop(file_effect_guard);
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
        #[cfg(test)]
        maybe_panic_skill_script_worker_after_receipt(&self.storage, &record.snapshot.action_id);
        self.run_action_continuation(
            record,
            agent_input,
            notifications,
            final_pending_status,
            cancellation_token,
        )
        .await;
    }

    pub(in crate::application::agent) async fn run_skill_script_execution(
        &self,
        record: PendingActionRecord,
        call: AgentToolCall,
        guard: CommandRunGuard,
        notifications: CoreServerNotificationSender,
        effects_started: Arc<AtomicBool>,
    ) {
        let run_id = record.snapshot.run_id.clone();
        let action_id = record.snapshot.action_id.clone();
        self.seed_trace_snapshot_from_checkpoint(
            &run_id,
            record.agent_input.resume_checkpoint.as_ref(),
        );
        if self.is_agent_input_scope_deleting(&record.agent_input) {
            self.discard_usage_context(&run_id);
            return;
        }
        let AgentProposedAction::SkillScript { mut script } = record.snapshot.action.clone() else {
            drop(guard);
            self.finish_skill_script_execution(
                record,
                call,
                skill_script_setup_failure_result(
                    &action_id,
                    "invalidActionSnapshot",
                    "retry",
                    "The approved action is not a Skill script snapshot.",
                ),
                notifications,
                None,
                None,
            )
            .await;
            return;
        };
        script.approval_status = AgentApprovalStatus::Approved;

        let mut file_effect_guard =
            match self.register_file_effect(&record.agent_input, &run_id, &record.storage_id) {
                Ok(guard) => guard,
                Err(_) if self.is_agent_input_scope_deleting(&record.agent_input) => {
                    self.discard_usage_context(&run_id);
                    return;
                }
                Err(error) => {
                    drop(guard);
                    self.finish_skill_script_execution(
                        record,
                        call,
                        skill_script_setup_failure_result(
                            &action_id,
                            "executionSetupFailed",
                            "retry",
                            &format!("Skill script execution setup failed: {error}"),
                        ),
                        notifications,
                        None,
                        None,
                    )
                    .await;
                    return;
                }
            };

        #[cfg(test)]
        {
            let injected_action_id_matches =
                skill_script_worker_panic_is_pending(&self.storage, &action_id);
            if injected_action_id_matches {
                file_effect_guard.mark_effects_started();
                effects_started.store(true, Ordering::SeqCst);
                maybe_panic_skill_script_worker_after_effect_boundary(&self.storage, &action_id);
            }
        }

        let cancellation_token = AgentCancellationToken::new();
        self.register_cancellation(&run_id, cancellation_token.clone());
        let cancel_flag = guard.cancel_flag();
        let post_execution_cancel_flag = Arc::clone(&cancel_flag);
        let run_cancellation_token = cancellation_token.clone();
        let mut tool_result = match self.restore_skill_resource_session(&record.agent_input) {
            Err(error) => {
                drop(guard);
                skill_script_setup_failure_result(
                    &action_id,
                    "snapshotUnavailable",
                    "reactivateSkill",
                    &format!("Skill resource snapshot could not be restored: {error}"),
                )
            }
            Ok(resources) => {
                let service = self.clone();
                let agent_input = record.agent_input.clone();
                file_effect_guard.mark_effects_started();
                effects_started.store(true, Ordering::SeqCst);
                match tokio::task::spawn_blocking(move || {
                    let _guard = guard;
                    service.execute_skill_script(
                        &agent_input,
                        &script,
                        resources.as_deref(),
                        CommandAuthorizationSource::ExplicitUser,
                        cancellation_token,
                        Some(cancel_flag),
                    )
                })
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        skill_script_worker_failure_result(&action_id, true, error.is_cancelled())
                    }
                }
            }
        };
        if post_execution_cancel_flag.load(Ordering::SeqCst) {
            run_cancellation_token.cancel();
        }
        let result_cancelled = run_cancellation_token.is_cancelled();
        if result_cancelled {
            tool_result.ok = false;
            let message = "Skill script execution was cancelled.".to_string();
            tool_result.error = Some(message.clone());
            if let Some(result) = tool_result.result.as_mut().and_then(Value::as_object_mut) {
                result.insert("cancelled".to_string(), Value::Bool(true));
                result.insert(
                    "errorCode".to_string(),
                    Value::String("skill_script.cancelled".to_string()),
                );
                result.insert("error".to_string(), Value::String(message));
            }
        }
        self.finish_skill_script_execution(
            record,
            call,
            tool_result,
            notifications,
            Some(file_effect_guard),
            Some(run_cancellation_token),
        )
        .await;
    }
}
