use mycopilot_core::human_interaction::{HumanInteractionAnswer, HumanInteractionRequestSnapshot};
use mycopilot_core::storage::human_interaction_repository::{
    HostHumanInteractionOwner, HumanInteractionApprovalPredecessor, HumanInteractionSyncAdmission,
    HumanInteractionSyncBinding, HumanInteractionSyncResume,
};

/// This bridge receives native Runtime authority, never renderer/model supplied ownership.
struct StoredBlockingHumanInput {
    service: AgentService,
    input: AgentChatInput,
    token: AgentCancellationToken,
    notifications: CoreServerNotificationSender,
    predecessor: Option<HumanInteractionSyncBinding>,
    approval_predecessor: Option<(PendingActionRecord, PendingActionStatus)>,
}

impl mycopilot_core::AgentHumanInteractionRuntimeHost for StoredBlockingHumanInput {
    fn async_execution_ready(&self) -> bool {
        true
    }
    fn accept_async(
        &self,
        request: mycopilot_core::AgentAsyncUserInputRequest,
    ) -> AgentResult<mycopilot_core::AgentAsyncUserInputAccepted> {
        self.accept_async_question(request)
    }
    fn natural_sampling_state(
        &self,
        request: mycopilot_core::AgentSamplingBoundaryRequest,
    ) -> AgentResult<mycopilot_core::AgentHumanInteractionSamplingState> {
        self.ignored_questions_at_sampling(request)
    }

    fn suspend(&self, suspension: mycopilot_core::AgentUserInputSuspension) -> AgentResult<()> {
        let fail = |error: String| AgentError::new(format!("无法持久化用户提问：{error}"));
        let context = self
            .input
            .context
            .as_ref()
            .ok_or_else(|| fail("missing Host context".into()))?;
        if context.collaboration_identity.is_some()
            || context.conversation_id.as_deref() != Some(&suspension.conversation_id)
            || self.input.assistant_message_id.as_deref() != Some(&suspension.assistant_message_id)
            || self.token.is_cancelled()
        {
            return Err(fail("invalid or cancelled question owner".into()));
        }
        let root = self
            .service
            .storage
            .get_agent_node_by_conversation(&suspension.conversation_id)
            .map_err(|e| fail(e.to_string()))?
            .filter(|node| node.parent_agent_id.is_none())
            .ok_or_else(|| fail("question owner is not a root agent".into()))?;
        let mut input = agent_input_with_run_checkpoint(&self.input, &suspension.checkpoint);
        self.service
            .refresh_agent_input_attachment_library(&mut input)
            .map_err(fail)?;
        let envelope = PersistedAgentResumeInput::from_agent_input(&input)
            .map(|value| value.encode())
            .map_err(|e| fail(e.to_string()))?;
        let checkpoint = serde_json::from_str(&envelope).map_err(|e| fail(e.to_string()))?;
        let mut pending = self
            .service
            .pending_actions
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let approval_predecessor = self
            .approval_predecessor
            .as_ref()
            .map(|(previous, target)| {
                let current = pending
                    .get(&previous.storage_id)
                    .ok_or_else(|| "approval predecessor missing".to_string())?;
                Ok::<_, String>(HumanInteractionApprovalPredecessor {
                    storage_id: current.storage_id.clone(),
                    renderer_action_id: current.snapshot.action_id.clone(),
                    expected_status: pending_status_label(current.snapshot.status).into(),
                    terminal_status: pending_status_label(*target).into(),
                    terminal_agent_input_json: persisted_pending_agent_input_json(
                        &current.agent_input,
                        *target,
                    )?,
                })
            })
            .transpose()
            .map_err(fail)?;
        let previous_usage = self
            .service
            .usage_contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&suspension.run_id)
            .cloned();
        let usage = self.service.prepare_run_usage_record(
            &suspension.run_id,
            AgentRunStatus::WaitingForUserInput,
            suspension.segment_usage,
            None,
        );
        let admission = HumanInteractionSyncAdmission {
            owner: HostHumanInteractionOwner {
                agent_id: root.agent_id,
                conversation_id: suspension.conversation_id.clone(),
                run_id: suspension.run_id.clone(),
                assistant_message_id: suspension.assistant_message_id.clone(),
                tool_call_id: suspension.call.id,
            },
            input: suspension.questions,
            checkpoint,
            usage,
            content: String::new(),
            predecessor: self.predecessor.clone(),
            pending_action_predecessor: approval_predecessor,
        };
        // The same lock arbitrates Stop and publication. SQLite independently checks stop/owner.
        let cancellations = self
            .service
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.token.is_cancelled()
            || !cancellations
                .get(&suspension.run_id)
                .is_some_and(|token| token.shares_state_with(&self.token))
        {
            restore_run_usage_state(&self.service, &suspension.run_id, &previous_usage);
            return Err(fail("question execution segment was retired".into()));
        }
        let snapshot = match self
            .service
            .storage
            .admit_sync_human_interaction(&admission)
        {
            Ok(snapshot) => snapshot,
            Err(error) => {
                restore_run_usage_state(&self.service, &suspension.run_id, &previous_usage);
                return Err(fail(error.to_string()));
            }
        };
        if let Some((previous, target)) = &self.approval_predecessor {
            if let Some(current) = pending.get_mut(&previous.storage_id) {
                current.snapshot.status = *target;
            }
        }
        // A rejected admission leaves the current segment steerable. Once committed, Runtime
        // must suspend even if this rebuildable process-control cleanup reports an error.
        if let Err(error) = self.service.close_active_run_steering(
            &suspension.run_id,
            AgentSteerRunRejectionCode::RunNotSteerable,
            "The agent is waiting for human input and no longer accepts guidance.",
            &self.notifications,
        ) {
            eprintln!("failed to close steering after committed human suspension: {error}");
        }
        self.service
            .seed_trace_snapshot_from_checkpoint(&suspension.run_id, Some(&suspension.checkpoint));
        #[cfg(test)]
        run_before_waiting_publication_arbitration_hook(&suspension.run_id);
        let _ = self.notifications.send(serde_json::json!({"jsonrpc":"2.0",
            "method":mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,"params":snapshot}));
        Ok(())
    }
}

fn human_answer_continuation(
    resume: &HumanInteractionSyncResume,
    checkpoint: &AgentRunCheckpoint,
) -> Result<mycopilot_core::AgentUserInputResume, String> {
    let binding = &resume.binding;
    if checkpoint.run_id != binding.run_id
        || checkpoint.pending_tool_call_id != binding.tool_call_id
        || checkpoint.pause_reason != mycopilot_core::AgentRunCheckpointPauseReason::UserInput
    {
        return Err("user input checkpoint binding mismatch".into());
    }
    let call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| &item.tool_calls)
        .find(|call| call.id == binding.tool_call_id && call.name == "request_user_input")
        .ok_or_else(|| "original question ToolCall missing".to_string())?;
    let response = resume
        .request
        .response
        .as_ref()
        .ok_or_else(|| "question response missing".to_string())?;
    if response.response_id != binding.response_id || response.request_id != binding.request_id {
        return Err("question response binding mismatch".into());
    }
    let mut answers = Vec::new();
    for (question, answer) in resume.request.questions.iter().zip(&response.answers) {
        let value = match answer {
            HumanInteractionAnswer::Option {
                question_id,
                option_id,
            } if question_id == &question.id => {
                let option = question
                    .options
                    .as_ref()
                    .and_then(|options| options.iter().find(|option| &option.id == option_id))
                    .ok_or_else(|| "question option missing".to_string())?;
                serde_json::json!({"questionId":question.id,"question":question.title,"kind":"option","optionId":option_id,"answer":option.label})
            }
            HumanInteractionAnswer::Text { question_id, text } if question_id == &question.id => {
                serde_json::json!({"questionId":question.id,"question":question.title,"kind":"text","answer":text})
            }
            HumanInteractionAnswer::Skipped { question_id } if question_id == &question.id => {
                serde_json::json!({"questionId":question.id,"question":question.title,"kind":"skipped","answer":"已跳过"})
            }
            _ => return Err("question answer binding mismatch".into()),
        };
        answers.push(value);
    }
    if answers.len() != resume.request.questions.len() {
        return Err("incomplete response".into());
    }
    Ok(mycopilot_core::AgentUserInputResume {
        request_id: binding.request_id.clone(),
        response_id: binding.response_id.clone(),
        checkpoint: checkpoint.clone(),
        continuation: AgentToolContinuation {
            call: AgentToolCall {
                id: call.id.clone(),
                tool: call.name.clone(),
                args: call.args.clone(),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
            result: AgentToolResult {
                call_id: call.id.clone(),
                tool: call.name.clone(),
                ok: true,
                result: Some(
                    serde_json::json!({"type":"human_interaction_response","schemaVersion":1,"requestId":binding.request_id,"responseId":binding.response_id,"answers":answers}),
                ),
                error: None,
                exact_archive_file: None,
            },
        },
    })
}

impl AgentService {
    pub(crate) fn accept_human_input_response(
        &self,
        input: &mycopilot_core::human_interaction::HumanInteractionSubmitInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<
        HumanInteractionRequestSnapshot,
        mycopilot_core::human_interaction::HumanInteractionError,
    > {
        // Shared with native admission and waiting-state publication: a fast submission cannot
        // publish revision N+1 and then receive the retiring segment's revision N notification.
        let _publication = self.cancellations.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = self.storage.submit_human_interaction(input)?;
        let _ = notifications.send(serde_json::json!({"jsonrpc":"2.0",
            "method":mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,"params":snapshot}));
        Ok(snapshot)
    }

    /// Event-driven drain: called after submit, retiring segment, and startup. Never polls humans.
    pub(crate) fn schedule_ready_human_input_resumes(
        &self,
        notifications: CoreServerNotificationSender,
    ) {
        let Ok(ready) = self.storage.list_sync_human_interaction_ready_resumes() else {
            return;
        };
        for request in ready {
            let mut cancellations = self.cancellations.lock().unwrap_or_else(|e| e.into_inner());
            if cancellations.contains_key(&request.run_id) {
                continue;
            }
            if self
                .ensure_turn_concurrency_permit(&request.run_id)
                .is_err()
            {
                continue;
            }
            let claim = match self
                .storage
                .claim_sync_human_interaction(&request.request_id)
            {
                Ok(Some(claim)) => claim,
                Ok(None) => {
                    self.release_turn_concurrency_permit(&request.run_id);
                    continue;
                }
                Err(error) => {
                    self.release_turn_concurrency_permit(&request.run_id);
                    eprintln!("failed to claim human response: {error}");
                    continue;
                }
            };
            let token = AgentCancellationToken::default();
            cancellations.insert(request.run_id.clone(), token.clone());
            drop(cancellations);
            let service = self.clone();
            let notifications = notifications.clone();
            tokio::spawn(async move {
                match service.prepare_human_input_segment(&claim, token.clone()) {
                    Ok(segment) => {
                        service
                            .complete_runtime_segment(segment, notifications)
                            .await
                    }
                    Err(error) => {
                        eprintln!("refused human input continuation: {error}");
                        let durable_terminal =
                            service.cancel_waiting_human_input_run(&claim.binding.run_id);
                        service.unregister_cancellation_if_current(&claim.binding.run_id, &token);
                        if let Ok(Some((snapshot, _))) = service
                            .storage
                            .load_sync_human_interaction_for_run(&claim.binding.run_id)
                        {
                            let _ = notifications.send(serde_json::json!({"jsonrpc":"2.0","method":mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,"params":snapshot}));
                        }
                        let _ = notifications.send(agent_event_notification(AgentEvent::Error {
                            run_id: Some(claim.binding.run_id.clone()), trace_sequence: None,
                            message: "用户回答已保存，但原运行无法安全恢复。请检查当前模型设置后开始新的运行。".into(),
                            recoverable: false, code: Some("human_input_resume_failed".into()), details: None,
                        }));
                        if durable_terminal {
                            // Preparation cannot consume the queued answers. Once the failed
                            // suspended Turn has durably released its lease, give earlier saved
                            // async responses a new route without requiring another user event.
                            service.publish_human_delivery_changes(
                                &claim.binding.conversation_id,
                                &notifications,
                            );
                            service.schedule_human_input_deliveries(notifications);
                        }
                    }
                }
            });
        }
    }

    fn prepare_human_input_segment(
        &self,
        resume: &HumanInteractionSyncResume,
        token: AgentCancellationToken,
    ) -> Result<PreparedRuntimeTurnSegment, String> {
        let binding = &resume.binding;
        let decoded = PersistedAgentResumeInput::decode(
            &serde_json::to_string(&resume.checkpoint).map_err(|e| e.to_string())?,
        )
        .map_err(|_| "invalid human input resume envelope".to_string())?;
        let input = restore_agent_input_secrets(&self.storage, decoded)?;
        let checkpoint = input
            .resume_checkpoint
            .as_ref()
            .ok_or_else(|| "human checkpoint missing".to_string())?;
        let native_resume = human_answer_continuation(resume, checkpoint)?;
        let context = input
            .context
            .as_ref()
            .ok_or_else(|| "human context missing".to_string())?;
        if context.collaboration_identity.is_some()
            || context.conversation_id.as_deref() != Some(&binding.conversation_id)
            || input.assistant_message_id.as_deref() != Some(&binding.assistant_message_id)
            || self.is_agent_input_scope_deleting(&input)
            || token.is_cancelled()
            || self.agent_tree_run_is_stopped(&binding.run_id)?
        {
            return Err("human input owner no longer executable".into());
        }
        self.ensure_conversation_turn_owner(
            &binding.conversation_id,
            &binding.run_id,
            &binding.assistant_message_id,
        )?;
        self.ensure_turn_concurrency_permit(&binding.run_id)
            .map_err(|e| e.to_string())?;
        self.restore_human_input_usage(&resume.request, &input)?;
        self.seed_trace_snapshot_from_checkpoint(&binding.run_id, Some(checkpoint));
        let skill_resources = self
            .restore_skill_resource_session(&input)
            .map_err(|e| e.to_string())?;
        let mcp_tools = self.capture_mcp_tool_runtime(&input);
        let projection = self.context_window_tool_projection_with_mcp_and_automation_report(
            &input,
            skill_resources.clone(),
            mcp_tools.clone(),
            None,
        )?;
        let assistant_created_at = self
            .storage
            .get_assistant_message_created_at(
                &binding.conversation_id,
                &binding.assistant_message_id,
            )?
            .ok_or_else(|| "original assistant missing".to_string())?;
        // Fence before any resumed queued tool or Provider call can execute; claimed-only is restartable.
        #[cfg(test)]
        run_before_human_resume_execution_hook(&binding.run_id);
        if !self
            .storage
            .mark_sync_human_interaction_execution_started(binding)
            .map_err(|e| e.to_string())?
        {
            return Err("human response execution claim was revoked".into());
        }
        // Publish accepting control only after every fallible preparation step and the durable
        // execution CAS. A refused claim otherwise leaves a queue with no worker to drain it.
        let steer_input = self.register_active_run_control(
            &binding.run_id,
            &binding.conversation_id,
            &binding.assistant_message_id,
            context.project_id.as_deref(),
            input.model_capabilities,
            context.permissions,
        );
        Ok(PreparedRuntimeTurnSegment {
            run_id: binding.run_id.clone(),
            conversation_id: binding.conversation_id.clone(),
            assistant_message_id: binding.assistant_message_id.clone(),
            assistant_created_at,
            agent_input: input,
            human_input_resume: Some((native_resume, binding.clone())),
            skill_resources,
            mcp_tools,
            automation_report_sink: None,
            context_window_tool_projection: RunContextToolProjection::new(projection),
            cancellation_token: token,
            steer_input,
            pending_action_predecessor_settlement: None,
            invalidate_mcp_payload_on_pending_store_failure: false,
            steering_close_error_context: "无法关闭答题恢复的引导通道",
        })
    }

    fn restore_human_input_usage(
        &self,
        request: &HumanInteractionRequestSnapshot,
        input: &AgentChatInput,
    ) -> Result<(), String> {
        let mut contexts = self
            .usage_contexts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if contexts.contains_key(&request.run_id) {
            return Ok(());
        }
        let record = self
            .storage
            .load_agent_usage_for_owner(
                &request.run_id,
                &request.conversation_id,
                &request.assistant_message_id,
            )?
            .ok_or_else(|| "durable waiting usage is unavailable".to_string())?;
        let key = input
            .provider_protocol_key
            .as_ref()
            .ok_or_else(|| "Provider identity missing".to_string())?;
        let semantics = mycopilot_core::resolve_provider_runtime_capabilities(key)
            .map_err(|e| e.to_string())?
            .usage();
        contexts.insert(
            request.run_id.clone(),
            AgentRunUsageState {
                context: AgentRunUsageContext {
                    conversation_id: record.conversation_id,
                    assistant_message_id: record.message_id,
                    run_id: record.run_id,
                    project_id: record.project_id,
                    model_id: record.model_id,
                    model_name: record.model_name,
                    provider_usage_semantics: semantics,
                    input_price: record.input_price,
                    cached_input_price: record.cached_input_price,
                    output_price: record.output_price,
                    started_at: record
                        .started_at
                        .ok_or_else(|| "usage start time missing".to_string())?,
                },
                usage: Some(AgentUsage {
                    input_tokens: record.input_tokens,
                    output_tokens: record.output_tokens,
                    output_thinking_tokens: record.output_thinking_tokens,
                    total_tokens: record.total_tokens,
                    cached_input_tokens: record.cached_input_tokens,
                    cache_creation_input_tokens: record.cache_creation_input_tokens,
                    billable_request_count: Some(record.billable_request_count),
                }),
                status: AgentRunStatus::WaitingForUserInput,
                error: record.error,
            },
        );
        Ok(())
    }

    pub(super) fn cancel_waiting_human_input_run(&self, run_id: &str) -> bool {
        let Ok(Some((request, envelope))) =
            self.storage.load_sync_human_interaction_for_run(run_id)
        else {
            return false;
        };
        if !self
            .storage
            .get_conversation_turn_trace(&request.assistant_message_id)
            .ok()
            .flatten()
            .is_some_and(|trace| {
                trace.run_id == run_id
                    && trace.terminal_status == ConversationTurnTraceTerminalStatus::InProgress
            })
        {
            return false;
        }
        if self
            .storage
            .cancel_sync_human_interactions_for_run(run_id)
            .is_err()
        {
            return false;
        }
        let decoded = serde_json::to_string(&envelope)
            .ok()
            .and_then(|json| PersistedAgentResumeInput::decode(&json).ok());
        if let Some(decoded) = decoded {
            let _ = self.restore_human_input_usage(&request, &decoded.agent_input);
            self.seed_trace_snapshot_from_checkpoint(
                run_id,
                decoded.agent_input.resume_checkpoint.as_ref(),
            );
        }
        self.persist_cancelled_runs_with_reason(
            &[run_id.to_string()],
            "Agent run was stopped while waiting for human input.",
        );
        if self
            .storage
            .get_conversation_turn_trace(&request.assistant_message_id)
            .ok()
            .flatten()
            .is_some_and(|trace| {
                trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress
            })
        {
            self.release_conversation_turn_if_current(&request.conversation_id, run_id);
            self.release_turn_concurrency_permit(run_id);
            self.notify_durable_turn_observers(&request.assistant_message_id);
            return true;
        }
        false
    }
}
