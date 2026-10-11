impl AgentService {
    /// Runs exactly one Runtime segment. Initial root/wake execution and approval continuation
    /// share this body; their distinct durable completion policies remain outside it.
    pub(super) async fn run_prepared_turn_segment(
        &self,
        segment: PreparedRuntimeTurnSegment,
        notifications: CoreServerNotificationSender,
    ) -> RuntimeTurnSegmentOutcome {
        let PreparedRuntimeTurnSegment {
            trusted_wake,
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
            steering_close_error_context: _,
        } = segment;

        self.bind_active_run_steering_notifications(&run_id, &steer_input, notifications.clone());
        let emitter_steer_input = steer_input.clone();

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
        let collaboration_stream_boundaries = Arc::new(Mutex::new(HashMap::<String, u64>::new()));
        let emitter_collaboration_stream_boundaries = Arc::clone(&collaboration_stream_boundaries);
        let final_response_collaboration_boundary = Arc::new(Mutex::new(None::<u64>));
        let emitter_final_response_collaboration_boundary =
            Arc::clone(&final_response_collaboration_boundary);
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
                // Retire only this Runtime's queue before the pending action can become visible.
                // The successor keeps accepted guidance while approval/Host execution pauses
                // sampling. A fast continuation can adopt it even before this segment returns;
                // the old Runtime Drop and unregister still refer only to the retired queue.
                let successor_steer_input =
                    emitter_service.handoff_active_run_steering(run_id, &emitter_steer_input);
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
                        if let Some(successor) = successor_steer_input.as_ref() {
                            if let Err(close_error) = emitter_service.unregister_active_run_control(
                                run_id,
                                successor,
                                AgentSteerRunRejectionCode::RunNotSteerable,
                                "The approval could not be persisted and the agent run has stopped.",
                                &emitter_notifications,
                            ) {
                                eprintln!("failed to settle guidance after approval persistence failure: {close_error}");
                            }
                        }
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
                        &emitter_service,
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
                        &emitter_service,
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
                        &emitter_service,
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
                    emitter_collaboration_stream_boundaries
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .insert(stream_id.clone(), root_sequence);
                }
                AgentEvent::MessageStreamReset { stream_id, .. } => {
                    emitter_collaboration_stream_boundaries
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(stream_id);
                }
                AgentEvent::MessageStreamCommitted {
                    stream_id,
                    trace_sequence,
                    ..
                } => {
                    let boundary = emitter_collaboration_stream_boundaries
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .remove(stream_id)
                        .unwrap_or(0);
                    if trace_sequence.is_none() {
                        *emitter_final_response_collaboration_boundary
                            .lock()
                            .unwrap_or_else(|error| error.into_inner()) = Some(boundary);
                    }
                }
                _ => {}
            }
            let event = emitter_service.project_cumulative_usage_onto_event(event);
            if let Some(event) = emitter_terminal_event_gate.route(event) {
                emit_agent_event_notifications(
                    &emitter_service,
                    &emitter_notifications,
                    emitter_collaboration_identity.as_ref(),
                    &emitter_run_id,
                    &emitter_assistant_message_id,
                    event,
                );
            }
        });

        // Preparation performs synchronous SQLite, skill and collaboration host construction.
        // The model future below remains on Tokio; only this finite owned phase occupies a worker.
        let preparation = self
            .blocking_work
            .preparation
            .reserve(runtime_work_estimated_bytes(&agent_input))
            .await;
        let prepared_host = match preparation {
            Ok(reservation) => {
                let service = self.clone();
                let prepared_wake = trusted_wake.clone();
                let agent_input = agent_input.clone();
                let run_id = run_id.clone();
                let conversation_id = conversation_id.clone();
                let assistant_message_id = assistant_message_id.clone();
                let cancellation_token = cancellation_token.clone();
                let steer_input = steer_input.clone();
                let notifications = notifications.clone();
                reservation
                    .submit(move || -> AgentResult<AgentRuntimeHostServices> {
                        let prepared = (|| -> AgentResult<AgentRuntimeHostServices> {
                        let host_executor = service.host_action_executor(
                            agent_input.clone(),
                            run_id.clone(),
                            Some(conversation_id.clone()),
                            Some(assistant_message_id.clone()),
                            skill_resources.clone(),
                            notifications.clone(),
                        );
                        let trace_observer = service.trace_observer(
                            &run_id,
                            &conversation_id,
                            &assistant_message_id,
                            assistant_created_at,
                            agent_input.clone(),
                            context_window_tool_projection.clone(),
                            notifications.clone(),
                        );
                        let context_compaction_services = service.context_compaction_services(
                            &run_id,
                            &conversation_id,
                            &assistant_message_id,
                            agent_input.clone(),
                            context_window_tool_projection,
                            notifications.clone(),
                        );
                        let model_request_observer = service.model_request_observer(
                            &run_id,
                            &conversation_id,
                            &assistant_message_id,
                        );
                        let context_window_observer = agent_input
                            .context_window_indicator_enabled
                            .then(|| {
                                agent_input.model_config_id.as_deref().map(|_| {
                                    service.context_window_observer(
                                        &run_id,
                                        &conversation_id,
                                        &agent_input,
                                        notifications.clone(),
                                    )
                                })
                            })
                            .flatten();
                        let mut host_services = AgentRuntimeHostServices::new()
                            // Both callers complete a background segment: live notifications carry text,
                            // while persistence uses the canonical trace/content. Neither replays output.events.
                            // Keep terminal and control events for usage projection and completion handling.
                            .with_transient_message_deltas()
                            .with_conversation_world_state(service.conversation_world_state_host(
                                &agent_input,
                                &run_id,
                                &conversation_id,
                                &assistant_message_id,
                                &cancellation_token,
                            ))
                            .with_web_search_policy(service.web_search_policy_source())
                            .with_host_actions(host_executor, service.storage.clone())
                            .with_command_session_executor(Arc::new(
                                service.command_sessions.clone(),
                            ))
                            .with_office_engine(service.office_engine.clone())
                            .with_trace_observer(trace_observer)
                            .with_model_request_observer(model_request_observer)
                            .with_context_compaction(context_compaction_services);
                        if agent_input.context.as_ref().is_some_and(|context| {
                            context.collaboration_identity.is_none()
                                && context.conversation_id.as_deref()
                                    == Some(conversation_id.as_str())
                        }) && agent_input
                            .prompt_preferences
                            .as_ref()
                            .is_none_or(|preferences| {
                                preferences.automation_execution_context.is_none()
                            })
                        {
                            host_services = host_services.with_human_interaction_runtime(Arc::new(
                                StoredBlockingHumanInput {
                                    service: service.clone(),
                                    input: agent_input.clone(),
                                    token: cancellation_token.clone(),
                                    steer_input: steer_input.clone(),
                                    notifications: notifications.clone(),
                                    predecessor: human_binding.clone(),
                                    approval_predecessor: human_approval_predecessor,
                                },
                            ));
                            host_services = host_services.with_human_interaction_policy(Arc::new(
                                crate::application::human_interaction::StoredHumanInteractionPolicy(
                                    service.storage.clone(),
                                ),
                            ));
                        }
                        host_services = service.attach_workflow_runtime(
                            host_services,
                            &agent_input,
                            super::workflow_execution::WorkflowTurnOwner {
                                run_id: &run_id,
                                conversation_id: &conversation_id,
                                assistant_message_id: &assistant_message_id,
                                token: &cancellation_token,
                            },
                            notifications.clone(),
                        );
                        if let Some(sink) = automation_report_sink {
                            host_services = host_services.with_automation_report_sink(sink);
                        }
                        if let Some(builtin_capabilities) = service.builtin_capabilities.clone() {
                            host_services =
                                host_services.with_builtin_capabilities(builtin_capabilities);
                        }
                        if let Some(provider_continuation_vault) =
                            service.provider_continuation_vault.as_ref()
                        {
                            host_services = host_services.with_provider_continuation_vault(
                                Arc::clone(provider_continuation_vault),
                            );
                        }
                        if let Some(context_window_observer) = context_window_observer {
                            host_services =
                                host_services.with_context_window_observer(context_window_observer);
                        }
                        if let Some(image_generation_execution) =
                            service.image_generation_execution.clone()
                        {
                            host_services = host_services
                                .with_image_generation_execution(image_generation_execution);
                        }
                        if let Some(skill_installation_prepare) =
                            service.skill_installation_prepare.clone()
                        {
                            host_services = host_services
                                .with_skill_installation_prepare(skill_installation_prepare);
                        }
                        if let Some(skill_installation) = service.skill_installation.clone() {
                            host_services =
                                host_services.with_skill_installation_commit(skill_installation);
                        }
                        let skill_workspace = agent_input
                            .context
                            .as_ref()
                            .and_then(|context| context.workspace.clone());
                        host_services = host_services.with_skill_activation_resolver(
                            model_skill_activation_resolver(
                                service.storage.clone(),
                                service.skills.clone(),
                                skill_workspace,
                            ),
                        );
                        host_services = host_services.with_steer_input(steer_input.clone());
                        host_services = host_services.with_collaboration_inbox(Arc::new(
                            PersistentAgentSamplingBoundaryInbox::new(Arc::clone(&service.storage)),
                        ));
                        let collaboration_harness =
                        crate::application::agent_harness::AgentCollaborationHarnessAdapter::new(
                            Arc::clone(&service.storage),
                            service.clone(),
                            service.collaboration_authorizer(),
                            Arc::clone(&service.collaboration_dispatcher),
                            notifications.clone(),
                        );
                        host_services = match collaboration_harness.attach_to_host_services(
                            host_services,
                            &conversation_id,
                            &run_id,
                        ) {
                            Ok(services) => services,
                            Err(error) => return Err(error),
                        };
                        if let Some(resources) = skill_resources {
                            host_services = host_services.with_skill_resources(resources);
                        }
                        if let Some(resolver) = service.artifact_runtime.clone() {
                            host_services =
                                host_services.with_command_runtime_profile_resolver(resolver);
                        }
                        if let Some(mcp_tools) = mcp_tools {
                            host_services = host_services.with_mcp_tools(mcp_tools);
                        }

                        service.schedule_async_human_input_deliveries(notifications.clone());
                        Ok(host_services)
                        })();
                        // Capacity and host construction may outlive a durable claim. Validate
                        // the captured token at the last synchronous boundary before Runtime,
                        // rather than resolving whichever replacement token owns this identity.
                        if let Some(trusted) = prepared_wake.as_ref() {
                            service.register_agent_wake_runtime_owner(
                                trusted, &run_id, &assistant_message_id, &cancellation_token,
                            ).map_err(|error| runtime_wake_fence_error(trusted, error))?;
                            #[cfg(test)]
                            run_before_runtime_execution_fence_hook(&run_id);
                            if let Err(error) = service.storage.validate_agent_wake_execution_fence(
                                trusted, &run_id, &assistant_message_id,
                            ) {
                                // This segment no longer has authority even to write a failure
                                // trace. Its old token cannot terminalize a same-Turn successor.
                                // Durable tree-stop/recovery owners decide the eventual outcome.
                                return Err(runtime_wake_fence_error(trusted, error));
                            }
                            if cancellation_token.is_cancelled() {
                                return Err(AgentError::cancelled());
                            }
                        }
                        prepared
                    })
                    .wait()
                    .await
                    .map_err(|error| match trusted_wake.as_ref() {
                        Some(trusted) => runtime_wake_fence_error(trusted, error),
                        None => AgentError::new(error.to_string()),
                    })
                    .and_then(|result| result)
            }
            Err(error) => Err(match trusted_wake.as_ref() {
                Some(trusted) => runtime_wake_fence_error(trusted, error),
                None => AgentError::new(error.to_string()),
            }),
        };
        let mut host_services = match prepared_host {
            Ok(services) => services,
            Err(error) => {
                let final_response_collaboration_boundary = *final_response_collaboration_boundary
                    .lock()
                    .unwrap_or_else(|lock_error| lock_error.into_inner());
                return RuntimeTurnSegmentOutcome {
                    result: settle_segment_cancellation(Err(error), &run_id, &terminal_event_gate),
                    terminal_event_gate,
                    final_response_collaboration_boundary,
                };
            }
        };
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
        // Exact steering retirement and its synchronous durable guidance rejection run in
        // the caller's already-reserved completion job, before terminal persistence/publication.

        let final_response_collaboration_boundary = *final_response_collaboration_boundary
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        RuntimeTurnSegmentOutcome {
            result: settle_segment_cancellation(result, &run_id, &terminal_event_gate),
            terminal_event_gate,
            final_response_collaboration_boundary,
        }
    }
}

/// Host callbacks can return typed cancellation at any sampling/preflight boundary, before the
/// Runtime's ordinary cancellation branch emits a terminal output. Keep that control outcome on
/// the same durable path as other cancellations, for initial turns and approval continuations.
/// Never use the token alone here: a real failure racing Stop must remain a failure.
fn settle_segment_cancellation(
    result: AgentResult<AgentChatOutput>,
    run_id: &str,
    terminal_event_gate: &AgentTerminalEventGate,
) -> AgentResult<AgentChatOutput> {
    match result {
        Err(error) if error.is_cancelled() => {
            terminal_event_gate.discard();
            Ok(AgentChatOutput {
                content: String::new(),
                status: AgentRunStatus::Cancelled,
                run_id: run_id.to_string(),
                events: Vec::new(),
                tool_definitions: Vec::new(),
                todo: None,
                usage: error.usage().cloned(),
                finish_reason: None,
                proposed_actions: Vec::new(),
                conversation_turn_trace: error.conversation_turn_trace().cloned(),
            })
        }
        result => result,
    }
}

#[cfg(test)]
mod cancellation_result_tests {
    use super::*;

    #[test]
    fn cancellation_settlement_preserves_usage_and_does_not_reclassify_real_errors() {
        let gate = AgentTerminalEventGate::default();
        let real_error = settle_segment_cancellation(
            Err(AgentError::new("agent run 已取消。")),
            "typed-cancellation",
            &gate,
        )
        .unwrap_err();
        assert!(
            !real_error.is_cancelled(),
            "message text is not a cancellation signal"
        );

        let usage = AgentUsage {
            input_tokens: Some(24),
            output_tokens: Some(6),
            total_tokens: Some(30),
            output_thinking_tokens: None,
            cached_input_tokens: None,
            cache_creation_input_tokens: None,
            billable_request_count: None,
        };
        let output = settle_segment_cancellation(
            Err(AgentError::cancelled().with_usage(Some(usage))),
            "typed-cancellation",
            &gate,
        )
        .unwrap();
        assert_eq!(output.status, AgentRunStatus::Cancelled);
        assert_eq!(output.usage.as_ref().unwrap().total_tokens, Some(30));
        assert!(output.content.is_empty());
        assert!(matches!(
            gate.take_after_persistence(&output).as_slice(),
            [AgentEvent::Done {
                status: Some(AgentRunStatus::Cancelled),
                content: None,
                ..
            }]
        ));
    }
}

/// Cheap scheduling weight for already-owned runtime state. This is a conservative estimate,
/// not a second serialized checkpoint or an RSS accounting claim. Runtime output may grow while
/// the reservation is held; the logical Turn limit continues to bound those live producers.
pub(in crate::application::agent) fn runtime_work_estimated_bytes(input: &AgentChatInput) -> usize {
    let message_bytes = input.messages.iter().fold(0_usize, |bytes, message| {
        bytes
            .saturating_add(message.content.len())
            .saturating_add(
                message
                    .conversation_turn_trace
                    .as_ref()
                    .map_or(0, |trace| trace.items.len().saturating_mul(4096)),
            )
            .saturating_add(
                message
                    .conversation_model_context_items
                    .len()
                    .saturating_mul(4096),
            )
    });
    let attachment_bytes = input
        .attachments
        .iter()
        .chain(input.context_image_attachments.iter())
        .fold(0_usize, |bytes, attachment| {
            bytes.saturating_add(attachment.data.len())
        });
    let checkpoint_bytes = input.resume_checkpoint.as_ref().map_or(0, |checkpoint| {
        checkpoint.context_items.len().saturating_mul(4096)
    });
    64_usize
        .saturating_mul(1024)
        .saturating_add(message_bytes)
        .saturating_add(attachment_bytes)
        .saturating_add(checkpoint_bytes)
        .saturating_mul(2)
}

fn runtime_wake_fence_error(
    trusted: &mycopilot_core::TrustedAgentWakeTurnAdmission,
    error: impl std::fmt::Display,
) -> AgentError {
    AgentError::structured(
        "agent_wake_execution_fenced",
        error.to_string(),
        // The original claim token is a Host capability and never enters UI notifications.
        serde_json::json!({"wakeId": trusted.wake_id}),
    )
}
