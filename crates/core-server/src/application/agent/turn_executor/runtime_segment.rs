impl AgentService {
    /// Runs exactly one Runtime segment. Initial root/wake execution and approval continuation
    /// share this body; their distinct durable completion policies remain outside it.
    pub(super) async fn run_prepared_turn_segment(
        &self,
        segment: PreparedRuntimeTurnSegment,
        notifications: CoreServerNotificationSender,
    ) -> RuntimeTurnSegmentOutcome {
        let PreparedRuntimeTurnSegment {
            run_id,
            conversation_id,
            assistant_message_id,
            assistant_created_at,
            agent_input,
            human_input_resume,
            skill_resources,
            mcp_tools,
            automation_report_sink,
            context_window_tool_projection,
            cancellation_token,
            steer_input,
            pending_action_predecessor_settlement,
            invalidate_mcp_payload_on_pending_store_failure,
            steering_close_error_context,
        } = segment;

        let human_binding = human_input_resume
            .as_ref()
            .map(|(_, binding)| binding.clone());
        let emitter_human_binding = human_binding.clone();
        let human_approval_predecessor = pending_action_predecessor_settlement.clone();
        let emitter_notifications = notifications.clone();
        let emitter_service = self.clone();
        let emitter_conversation_id = conversation_id.clone();
        let emitter_assistant_message_id = assistant_message_id.clone();
        let emitter_agent_input = agent_input.clone();
        let emitter_cancellation_token = cancellation_token.clone();
        let emitter_collaboration_identity = agent_input
            .context
            .as_ref()
            .and_then(|context| context.collaboration_identity.clone());
        let emitter_run_id = run_id.clone();
        let terminal_event_gate = Arc::new(AgentTerminalEventGate::default());
        let emitter_terminal_event_gate = terminal_event_gate.clone();
        let pending_store_failure = Arc::new(Mutex::new(None::<String>));
        let emitter_pending_store_failure = Arc::clone(&pending_store_failure);
        let waiting_pending_action_storage_id = Arc::new(Mutex::new(None::<String>));
        let emitter_waiting_pending_action_storage_id =
            Arc::clone(&waiting_pending_action_storage_id);
        let collaboration_stream_cutoffs = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let emitter_collaboration_stream_cutoffs = Arc::clone(&collaboration_stream_cutoffs);
        let final_response_collaboration_cutoff = Arc::new(Mutex::new(None::<u64>));
        let emitter_final_response_collaboration_cutoff =
            Arc::clone(&final_response_collaboration_cutoff);
        let emitter: AgentEventEmitter = Arc::new(move |event| {
            if let AgentEvent::ApprovalRequired {
                run_id,
                action,
                checkpoint,
                segment_usage,
            } = &event
            {
                // The approval becomes externally actionable as soon as its pending row and
                // notification commit. Account for the producing Runtime segment first so a fast
                // approval continuation or interrupt cannot terminalize the shared logical Run
                // without these tokens.
                match emitter_service.stage_waiting_segment_usage(run_id, segment_usage.clone()) {
                    Ok(AgentWaitingSegmentUsagePersistenceOutcome::Persisted) => {}
                    Ok(AgentWaitingSegmentUsagePersistenceOutcome::TurnTerminal) => {
                        if invalidate_mcp_payload_on_pending_store_failure {
                            emitter_service.invalidate_mcp_pending_payload(action);
                        }
                        emitter_terminal_event_gate.discard();
                        return;
                    }
                    Err(error) => {
                        if invalidate_mcp_payload_on_pending_store_failure {
                            emitter_service.invalidate_mcp_pending_payload(action);
                        }
                        emitter_terminal_event_gate.discard();
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(format!(
                            "无法在发布待审批操作前持久化当前模型调用用量：{error}"
                        ));
                        return;
                    }
                }
                if let Err(error) = emitter_service.close_active_run_steering(
                    run_id,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    "The agent run is waiting for approval and no longer accepts guidance.",
                    &emitter_notifications,
                ) {
                    if invalidate_mcp_payload_on_pending_store_failure {
                        emitter_service.invalidate_mcp_pending_payload(action);
                    }
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                let mut checkpoint_input =
                    agent_input_with_run_checkpoint(&emitter_agent_input, checkpoint);
                if let Err(error) =
                    emitter_service.refresh_agent_input_attachment_library(&mut checkpoint_input)
                {
                    if invalidate_mcp_payload_on_pending_store_failure {
                        emitter_service.invalidate_mcp_pending_payload(action);
                    }
                    emitter_terminal_event_gate.discard();
                    *emitter_pending_store_failure
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                    return;
                }
                #[cfg(test)]
                run_before_pending_action_store_hook(&action_id_for_action(action.as_ref()));
                let pending_store = if let Some((predecessor, terminal_status)) =
                    pending_action_predecessor_settlement.as_ref()
                {
                    emitter_service.store_pending_action_with_predecessor_settlement(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.as_ref().clone(),
                        checkpoint_input,
                        predecessor,
                        *terminal_status,
                    )
                } else {
                    emitter_service.store_pending_action(
                        run_id,
                        &emitter_conversation_id,
                        &emitter_assistant_message_id,
                        action.as_ref().clone(),
                        checkpoint_input,
                    )
                };
                let should_publish = match pending_store {
                    Ok(should_publish) => should_publish,
                    Err(error) => {
                        if invalidate_mcp_payload_on_pending_store_failure {
                            emitter_service.invalidate_mcp_pending_payload(action);
                        }
                        emitter_terminal_event_gate.discard();
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) = Some(error);
                        return;
                    }
                };
                if let Some(binding) = emitter_human_binding.as_ref() {
                    if let Err(error) = emitter_service
                        .storage
                        .mark_sync_human_interaction_applied(binding)
                    {
                        eprintln!("failed to acknowledge question/approval handoff: {error}");
                    }
                }
                let action_id = action_id_for_action(action.as_ref());
                *emitter_waiting_pending_action_storage_id
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) =
                    Some(pending_action_storage_id(run_id, &action_id));
                if !should_publish {
                    return;
                }
                #[cfg(test)]
                run_before_approval_publication_arbitration_hook(&action_id);
                // Cancellation and approval publication form a two-sided handoff. An interrupt
                // may signal the Runtime after its last model-loop check but before this callback
                // makes the pending action visible in memory. The cancellation registry mutex is
                // also the publication linearization boundary: either this event is enqueued
                // before an interrupt can return, or the publisher observes the cancelled token
                // and terminalizes the newly visible action without emitting a stale approval.
                let cancellation_publication_guard = emitter_service
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let owns_current_generation = cancellation_publication_guard
                    .get(run_id)
                    .is_some_and(|current| current.shares_state_with(&emitter_cancellation_token));
                if !owns_current_generation {
                    // A newer continuation now owns this logical run. The retiring emitter may
                    // no longer publish, but it must never cancel the newer generation's action.
                    drop(cancellation_publication_guard);
                    emitter_terminal_event_gate.discard();
                    return;
                }
                if emitter_cancellation_token.is_cancelled() {
                    drop(cancellation_publication_guard);
                    emitter_terminal_event_gate.discard();
                    if let Err(error) = emitter_service.cancel_action_internal(run_id, &action_id) {
                        *emitter_pending_store_failure
                            .lock()
                            .unwrap_or_else(|lock_error| lock_error.into_inner()) =
                            Some(format!("无法结算取消期间刚发布的待审批操作：{error}"));
                    }
                    return;
                }
                let event = emitter_service.project_cumulative_usage_onto_event(event);
                if let Some(event) = emitter_terminal_event_gate.route(event) {
                    emit_agent_event_notifications(
                        &emitter_notifications,
                        emitter_collaboration_identity.as_ref(),
                        &emitter_run_id,
                        &emitter_assistant_message_id,
                        event,
                    );
                }
                drop(cancellation_publication_guard);
                return;
            }
            if emitter_pending_store_failure
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .is_some()
            {
                return;
            }
            let human_waiting = matches!(&event,
                AgentEvent::State { state, .. } if state.status == AgentRunStatus::WaitingForUserInput)
                || matches!(
                    &event,
                    AgentEvent::Done {
                        status: Some(AgentRunStatus::WaitingForUserInput),
                        ..
                    }
                );
            if human_waiting {
                let cancellations = emitter_service
                    .cancellations
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                if emitter_cancellation_token.is_cancelled()
                    || !cancellations
                        .get(&emitter_run_id)
                        .is_some_and(|token| token.shares_state_with(&emitter_cancellation_token))
                {
                    return;
                }
                let mut event = event;
                if let AgentEvent::Done { usage, .. } = &mut event {
                    *usage = emitter_service.preview_cumulative_run_usage(&emitter_run_id, None);
                }
                if let Some(event) = emitter_terminal_event_gate.route(event) {
                    emit_agent_event_notifications(
                        &emitter_notifications,
                        emitter_collaboration_identity.as_ref(),
                        &emitter_run_id,
                        &emitter_assistant_message_id,
                        event,
                    );
                }
                return;
            }
            // The Runtime emits Waiting state immediately after ApprovalRequired. Route these
            // cancellation-sensitive events under the same mutex as token signalling so an
            // interrupt cannot return and then be followed by a stale Waiting notification.
            let is_waiting_event = match &event {
                AgentEvent::State { state, .. } => {
                    state.status == AgentRunStatus::WaitingForApproval
                }
                AgentEvent::Done { status, .. } => {
                    *status == Some(AgentRunStatus::WaitingForApproval)
                }
                _ => false,
            };
            if is_waiting_event {
                #[cfg(test)]
                run_before_waiting_publication_arbitration_hook(&emitter_run_id);
                // ApprovalRequired already staged this Runtime segment exactly once. Waiting Done
                // therefore projects the accumulator without merging the same provider usage a
                // second time.
                let mut event = event;
                if let AgentEvent::Done { usage, .. } = &mut event {
                    *usage = emitter_service.preview_cumulative_run_usage(&emitter_run_id, None);
                }
                let pending_action_storage_id = emitter_waiting_pending_action_storage_id
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                let Some(pending_action_storage_id) = pending_action_storage_id else {
                    emitter_terminal_event_gate.discard();
                    return;
                };
                // Fixed lock order: pending_actions -> cancellations. Approval advances the
                // pending status while holding the first lock; cancellation token publication
                // uses the second. Holding both through the non-blocking notification send makes
                // Waiting linearizable with both operations, including the approved-to-spawn gap
                // before an asynchronous command/MCP/Skill/Office worker registers its new token.
                let pending_actions_guard = emitter_service
                    .pending_actions
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let pending_is_current = pending_actions_guard
                    .get(&pending_action_storage_id)
                    .is_some_and(|record| {
                        record.snapshot.run_id == emitter_run_id
                            && record.snapshot.status == PendingActionStatus::Pending
                    });
                if !pending_is_current {
                    drop(pending_actions_guard);
                    emitter_terminal_event_gate.discard();
                    return;
                }
                let cancellation_publication_guard = emitter_service
                    .cancellations
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                let owns_current_generation = cancellation_publication_guard
                    .get(&emitter_run_id)
                    .is_some_and(|current| current.shares_state_with(&emitter_cancellation_token));
                if !owns_current_generation || emitter_cancellation_token.is_cancelled() {
                    drop(cancellation_publication_guard);
                    drop(pending_actions_guard);
                    emitter_terminal_event_gate.discard();
                    return;
                }
                if let Some(event) = emitter_terminal_event_gate.route(event) {
                    emit_agent_event_notifications(
                        &emitter_notifications,
                        emitter_collaboration_identity.as_ref(),
                        &emitter_run_id,
                        &emitter_assistant_message_id,
                        event,
                    );
                }
                drop(cancellation_publication_guard);
                drop(pending_actions_guard);
                return;
            }
            match &event {
                AgentEvent::MessageStreamStarted { stream_id, .. } => {
                    let root_sequence = emitter_service
                        .storage
                        .get_agent_node_by_conversation(&emitter_conversation_id)
                        .ok()
                        .flatten()
                        .and_then(|node| {
                            emitter_service
                                .storage
                                .latest_agent_collaboration_event_sequence(&node.root_agent_id)
                                .ok()
                        })
                        .unwrap_or(0);
                    emitter_collaboration_stream_cutoffs
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(stream_id.clone(), root_sequence);
                }
                AgentEvent::MessageStreamReset { stream_id, .. } => {
                    emitter_collaboration_stream_cutoffs
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(stream_id);
                }
                AgentEvent::MessageStreamCommitted {
                    stream_id,
                    trace_sequence,
                    ..
                } => {
                    let cutoff = emitter_collaboration_stream_cutoffs
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(stream_id)
                        .unwrap_or(0);
                    if trace_sequence.is_none() {
                        *emitter_final_response_collaboration_cutoff
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(cutoff);
                    }
                }
                _ => {}
            }
            let event = emitter_service.project_cumulative_usage_onto_event(event);
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                emit_agent_event_notifications(
                    &emitter_notifications,
                    emitter_collaboration_identity.as_ref(),
                    &emitter_run_id,
                    &emitter_assistant_message_id,
                    event,
                );
            }
        });

        let host_executor = self.host_action_executor(
            agent_input.clone(),
            run_id.clone(),
            Some(conversation_id.clone()),
            Some(assistant_message_id.clone()),
            skill_resources.clone(),
            notifications.clone(),
        );
        let trace_observer = self.trace_observer(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            assistant_created_at,
            agent_input.clone(),
            context_window_tool_projection.clone(),
            notifications.clone(),
        );
        let context_compaction_services = self.context_compaction_services(
            &run_id,
            &conversation_id,
            &assistant_message_id,
            agent_input.clone(),
            context_window_tool_projection,
            notifications.clone(),
        );
        let model_request_observer =
            self.model_request_observer(&run_id, &conversation_id, &assistant_message_id);
        let context_window_observer = agent_input
            .context_window_indicator_enabled
            .then(|| {
                agent_input
                    .model_config_id
                    .as_deref()
                    .map(|model_config_id| {
                        self.context_window_observer(
                            &run_id,
                            &conversation_id,
                            model_config_id,
                            &agent_input.model,
                            notifications.clone(),
                        )
                    })
            })
            .flatten();
        let mut host_services = AgentRuntimeHostServices::new()
            .with_web_search_policy(self.web_search_policy_source())
            .with_host_actions(host_executor, self.storage.clone())
            .with_command_session_executor(Arc::new(self.command_sessions.clone()))
            .with_office_engine(self.office_engine.clone())
            .with_trace_observer(trace_observer)
            .with_model_request_observer(model_request_observer)
            .with_context_compaction(context_compaction_services);
        if agent_input.context.as_ref().is_some_and(|context| {
            context.collaboration_identity.is_none()
                && context.conversation_id.as_deref() == Some(conversation_id.as_str())
        }) && agent_input
            .prompt_preferences
            .as_ref()
            .is_none_or(|preferences| preferences.automation_execution_context.is_none())
        {
            host_services =
                host_services.with_human_interaction_runtime(Arc::new(StoredBlockingHumanInput {
                    service: self.clone(),
                    input: agent_input.clone(),
                    token: cancellation_token.clone(),
                    notifications: notifications.clone(),
                    predecessor: human_binding.clone(),
                    approval_predecessor: human_approval_predecessor,
                }));
            host_services = host_services.with_human_interaction_policy(Arc::new(
                crate::application::human_interaction::StoredHumanInteractionPolicy(
                    self.storage.clone(),
                ),
            ));
        }
        if let Some(sink) = automation_report_sink {
            host_services = host_services.with_automation_report_sink(sink);
        }
        if let Some(builtin_capabilities) = self.builtin_capabilities.clone() {
            host_services = host_services.with_builtin_capabilities(builtin_capabilities);
        }
        if let Some(provider_continuation_vault) = self.provider_continuation_vault.as_ref() {
            host_services = host_services
                .with_provider_continuation_vault(Arc::clone(provider_continuation_vault));
        }
        if let Some(context_window_observer) = context_window_observer {
            host_services = host_services.with_context_window_observer(context_window_observer);
        }
        if let Some(image_generation_execution) = self.image_generation_execution.clone() {
            host_services =
                host_services.with_image_generation_execution(image_generation_execution);
        }
        if let Some(skill_installation_prepare) = self.skill_installation_prepare.clone() {
            host_services =
                host_services.with_skill_installation_prepare(skill_installation_prepare);
        }
        if let Some(skill_installation) = self.skill_installation.clone() {
            host_services = host_services.with_skill_installation_commit(skill_installation);
        }
        let skill_workspace = agent_input
            .context
            .as_ref()
            .and_then(|context| context.workspace.as_ref())
            .and_then(|workspace| {
                workspace
                    .project_id
                    .clone()
                    .zip(workspace.root_path.as_deref().map(std::path::PathBuf::from))
            });
        host_services =
            host_services.with_skill_activation_resolver(model_skill_activation_resolver(
                self.storage.clone(),
                self.skills.clone(),
                skill_workspace,
            ));
        host_services = host_services.with_steer_input(steer_input.clone());
        host_services = host_services.with_collaboration_inbox(Arc::new(
            PersistentAgentSamplingBoundaryInbox::new(Arc::clone(&self.storage)),
        ));
        let collaboration_harness =
            crate::application::agent_harness::AgentCollaborationHarnessAdapter::new(
                Arc::clone(&self.storage),
                self.clone(),
                self.collaboration_authorizer(),
                Arc::clone(&self.collaboration_dispatcher),
                notifications.clone(),
            );
        host_services =
            match collaboration_harness.attach_to_host_services(host_services, &conversation_id) {
                Ok(services) => services,
                Err(error) => {
                    let final_response_collaboration_cutoff = *final_response_collaboration_cutoff
                        .lock()
                        .unwrap_or_else(|lock_error| lock_error.into_inner());
                    return RuntimeTurnSegmentOutcome {
                        result: Err(error),
                        terminal_event_gate,
                        final_response_collaboration_cutoff,
                    };
                }
            };
        if let Some(resources) = skill_resources {
            host_services = host_services.with_skill_resources(resources);
        }
        if let Some(resolver) = self.artifact_runtime.clone() {
            host_services = host_services.with_command_runtime_profile_resolver(resolver);
        }
        if let Some(mcp_tools) = mcp_tools {
            host_services = host_services.with_mcp_tools(mcp_tools);
        }

        self.schedule_async_human_input_deliveries(notifications.clone());
        let mut runtime_input = agent_input;
        if let Some((resume, _)) = human_input_resume {
            host_services = host_services.with_user_input_resume(resume);
            runtime_input.resume_checkpoint = None;
            runtime_input.tool_continuation = None;
            runtime_input.approval_decision = None;
        }
        let result = send_chat_with_host_services(
            runtime_input,
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
        let close_message = match &result {
            Ok(output) if output.status == AgentRunStatus::WaitingForApproval => {
                "The agent run is waiting for approval and no longer accepts guidance."
            }
            _ => "The agent run has finished and no longer accepts guidance.",
        };
        let result = match self.unregister_active_run_control(
            &run_id,
            &steer_input,
            AgentSteerRunRejectionCode::RunNotSteerable,
            close_message,
            &notifications,
        ) {
            Ok(()) => result,
            Err(error) => {
                terminal_event_gate.discard();
                Err(AgentError::new(format!(
                    "{steering_close_error_context}：{error}"
                )))
            }
        };

        let final_response_collaboration_cutoff = *final_response_collaboration_cutoff
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        RuntimeTurnSegmentOutcome {
            result,
            terminal_event_gate,
            final_response_collaboration_cutoff,
        }
    }
}
