use mycopilot_core::human_interaction::HumanInteractionMode;
use mycopilot_core::storage::human_interaction_repository::{
    async_human_interaction_answer_content, HumanInteractionAsyncPending,
    HumanInteractionAsyncTurnAdmission,
};

impl StoredBlockingHumanInput {
    fn accept_async_question(
        &self,
        request: mycopilot_core::AgentAsyncUserInputRequest,
    ) -> AgentResult<mycopilot_core::AgentAsyncUserInputAccepted> {
        let fail = |error: String| AgentError::new(format!("无法接纳异步用户提问：{error}"));
        let context = self
            .input
            .context
            .as_ref()
            .ok_or_else(|| fail("missing Host context".into()))?;
        if context.collaboration_identity.is_some()
            || context.conversation_id.as_deref() != Some(request.conversation_id.as_str())
            || self.input.assistant_message_id.as_deref()
                != Some(request.assistant_message_id.as_str())
            || self
                .input
                .prompt_preferences
                .as_ref()
                .is_some_and(|p| p.automation_execution_context.is_some())
        {
            return Err(fail("invalid question owner".into()));
        }
        let root = self
            .service
            .storage
            .get_agent_node_by_conversation(&request.conversation_id)
            .map_err(|e| fail(e.to_string()))?
            .filter(|node| node.parent_agent_id.is_none())
            .ok_or_else(|| fail("question owner is not a root agent".into()))?;
        // Serialize publication against both fast responses and Stop. SQLite is the final
        // authority for the current root/Run identity and the live settings revision.
        let cancellations = self
            .service
            .cancellations
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if self.token.is_cancelled()
            || !cancellations
                .get(&request.run_id)
                .is_some_and(|token| token.shares_state_with(&self.token))
        {
            return Err(fail("question execution segment was retired".into()));
        }
        // A first-turn root is created when the Harness attaches, after initial Turn admission.
        // Persist its exact Host-authenticated permissions before accepting any future input.
        self.service
            .storage
            .record_agent_effective_permissions_for_active_turn(
                &root.agent_id,
                &request.conversation_id,
                &request.run_id,
                &request.assistant_message_id,
                context.permissions,
            )
            .map_err(|e| fail(e.to_string()))?;
        let snapshot = self
            .service
            .storage
            .admit_async_human_interaction(
                &HostHumanInteractionOwner {
                    agent_id: root.agent_id,
                    conversation_id: request.conversation_id,
                    run_id: request.run_id,
                    assistant_message_id: request.assistant_message_id,
                    tool_call_id: request.call.id,
                },
                &request.questions,
            )
            .map_err(|e| fail(e.to_string()))?;
        emit_human_request_snapshot(&self.notifications, &snapshot);
        Ok(mycopilot_core::AgentAsyncUserInputAccepted {
            request_id: snapshot.request_id,
        })
    }

    fn ignored_questions_at_sampling(
        &self,
        request: mycopilot_core::AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<mycopilot_core::AgentHumanInteractionIgnoredEvent>> {
        if self.input.context.as_ref().is_none_or(|context| {
            context.collaboration_identity.is_some()
                || context.conversation_id.as_deref() != Some(request.conversation_id.as_str())
        })
            || self.input.assistant_message_id.as_deref()
                != Some(request.assistant_message_id.as_str())
            || self.input.prompt_preferences.as_ref().is_some_and(|preferences| preferences.automation_execution_context.is_some())
        {
            return Err(AgentError::new("invalid human interaction sampling owner"));
        }
        let cancellations = self.service.cancellations.lock().unwrap_or_else(|error| error.into_inner());
        if self.token.is_cancelled()
            || !cancellations.get(&request.run_id).is_some_and(|token| token.shares_state_with(&self.token))
        {
            return Err(AgentError::new("human interaction execution segment was retired"));
        }
        self.service.storage.bind_human_interaction_ignored_at_sampling(&request)
            .map_err(|error| AgentError::new(error.to_string()))
    }
}

fn emit_human_request_snapshot(
    notifications: &CoreServerNotificationSender,
    snapshot: &HumanInteractionRequestSnapshot,
) {
    let _ = notifications.send(serde_json::json!({"jsonrpc":"2.0",
        "method":mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,"params":snapshot}));
}

impl AgentService {
    fn human_requests_for_conversation(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<HumanInteractionRequestSnapshot>, String> {
        let mut items = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .storage
                .list_human_interaction_requests(
                    &mycopilot_core::human_interaction::HumanInteractionListInput {
                        conversation_id: conversation_id.into(),
                        cursor,
                        limit: 100,
                    },
                )
                .map_err(|e| e.to_string())?;
            items.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                return Ok(items);
            }
        }
    }

    pub(crate) fn accept_human_input_ignore(
        &self,
        input: &mycopilot_core::human_interaction::HumanInteractionIgnoreInput,
        notifications: &CoreServerNotificationSender,
    ) -> Result<
        HumanInteractionRequestSnapshot,
        mycopilot_core::human_interaction::HumanInteractionError,
    > {
        let _publication = self.cancellations.lock().unwrap_or_else(|e| e.into_inner());
        let snapshot = self.storage.ignore_human_interaction(input)?;
        emit_human_request_snapshot(notifications, &snapshot);
        Ok(snapshot)
    }

    pub(crate) fn schedule_human_input_deliveries(
        &self,
        notifications: CoreServerNotificationSender,
    ) {
        self.schedule_ready_human_input_resumes(notifications.clone());
        self.schedule_async_human_input_deliveries(notifications);
    }

    /// Events are wake-up hints only; response acceptance order and all delivery claims live in
    /// SQLite. Serializing this short coordinator prevents overlapping scans choosing two routes.
    pub(crate) fn schedule_async_human_input_deliveries(
        &self,
        notifications: CoreServerNotificationSender,
    ) {
        let _drain = self
            .human_input_delivery_dispatch
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let pending = match self
            .storage
            .list_pending_async_human_interaction_deliveries()
        {
            Ok(pending) => pending,
            Err(error) => {
                eprintln!("failed to list pending human answers: {error}");
                return;
            }
        };
        let mut blocked = HashSet::new();
        for candidate in pending {
            let conversation_id = candidate.request.conversation_id.clone();
            if blocked.contains(&conversation_id) {
                continue;
            }
            match self.try_deliver_async_human_answer(&candidate, &notifications) {
                Ok(true) => self.publish_human_delivery_changes(&conversation_id, &notifications),
                Ok(false) => {
                    blocked.insert(conversation_id);
                }
                Err(error) => {
                    // An accepted answer never reopens. Capacity, paused Run, stale revisions and
                    // temporary preparation failures retain a durable pending/failed receipt.
                    eprintln!("human answer delivery deferred: {error}");
                    blocked.insert(conversation_id.clone());
                    self.publish_human_delivery_changes(&conversation_id, &notifications);
                }
            }
        }
    }

    pub(super) fn publish_human_delivery_changes(
        &self,
        conversation_id: &str,
        notifications: &CoreServerNotificationSender,
    ) {
        match self.human_requests_for_conversation(conversation_id) {
            Ok(requests) => {
                for request in requests
                    .into_iter()
                    .filter(|r| r.mode == HumanInteractionMode::Async && r.delivery.is_some())
                {
                    emit_human_request_snapshot(notifications, &request);
                }
            }
            Err(error) => eprintln!("failed to reload human answer receipts: {error}"),
        }
    }

    fn try_deliver_async_human_answer(
        &self,
        candidate: &HumanInteractionAsyncPending,
        notifications: &CoreServerNotificationSender,
    ) -> Result<bool, String> {
        let request = &candidate.request;
        let response = request.response.as_ref().ok_or("human response missing")?;
        let delivery = request.delivery.as_ref().ok_or("human delivery missing")?;
        let content = async_human_interaction_answer_content(request).map_err(|e| e.to_string())?;
        {
            let mut active = self.active_runs.lock().unwrap_or_else(|e| e.into_inner());
            if let Some((run_id, control)) = active
                .iter_mut()
                .find(|(_, c)| c.conversation_id == request.conversation_id)
            {
                if !matches!(control.steer_state, ActiveRunSteerState::Accepting) {
                    return Ok(false);
                }
                control.steering_notifications = Some(notifications.clone());
                let guidance_id = create_id("guidance");
                let record = AgentRunGuidanceRecord {
                    client_message_id: format!("human-answer-{guidance_id}"),
                    guidance_id,
                    run_id: run_id.clone(),
                    conversation_id: request.conversation_id.clone(),
                    assistant_message_id: control.assistant_message_id.clone(),
                    content: content.clone(),
                    status: AgentGuidanceStatus::Queued,
                    attachment_ids: Vec::new(),
                    applied_trace_sequence: None,
                    terminal_reason: None,
                    created_at: response.created_at,
                    updated_at: now_ms().max(response.created_at),
                };
                let Some(_binding) = self
                    .storage
                    .bind_async_human_interaction_to_guidance(
                        &response.response_id,
                        &record,
                        delivery.revision,
                    )
                    .map_err(|e| e.to_string())?
                else {
                    return Ok(false);
                };
                let queued = control.steer_input.enqueue_with(
                    AgentSteerInput {
                        guidance_id: record.guidance_id.clone(),
                        client_message_id: record.client_message_id.clone(),
                        content: content.clone(),
                        attachments: Vec::new(),
                        attachment_library: None,
                        created_at: record.created_at,
                    },
                    || {
                        let _ = notifications.send(agent_event_notification(
                            AgentEvent::GuidanceQueued {
                                run_id: run_id.clone(),
                                guidance_id: record.guidance_id.clone(),
                                client_message_id: record.client_message_id.clone(),
                                content: content.clone(),
                                attachments: Vec::new(),
                                created_at: record.created_at,
                            },
                        ));
                    },
                );
                let queued = match queued {
                    Ok(queued) => queued,
                    Err(error) => {
                        self.storage
                            .mark_agent_run_guidance_terminal(
                                &record.guidance_id,
                                AgentGuidanceStatus::Rejected,
                                "Human answer could not enter the runtime queue.",
                                now_ms(),
                            )
                            .map_err(|e| e.to_string())?;
                        return Err(error.to_string());
                    }
                };
                if queued == AgentSteerEnqueueOutcome::Closed {
                    self.storage
                        .mark_agent_run_guidance_terminal(
                            &record.guidance_id,
                            AgentGuidanceStatus::Rejected,
                            "Run closed before human answer enqueue.",
                            now_ms(),
                        )
                        .map_err(|e| e.to_string())?;
                    return Ok(false);
                }
                return Ok(true);
            }
        }
        // No worker is insufficient proof of idleness. The normal root admission below checks
        // the durable logical Turn, approval/sync waits, compaction, deletion and revision again.
        let conversation = self
            .storage
            .load_conversation(&request.conversation_id)?
            .ok_or("human answer conversation was removed")?;
        let root = self
            .storage
            .get_agent_node_by_conversation(&request.conversation_id)
            .map_err(|e| e.to_string())?
            .filter(|n| n.parent_agent_id.is_none())
            .ok_or("human answer root missing")?;
        let permissions = match self
            .storage
            .get_agent_effective_permission_snapshot(&root.agent_id)
            .map_err(|e| e.to_string())?
        {
            Some(snapshot) => snapshot.permissions,
            // The fork engine cannot certify a branch-local snapshot: the schema requires the
            // exact active Turn to record one, and a forked Conversation starts idle. Answering
            // an inherited question therefore resumes with the least-authority baseline; the
            // answer Turn records the branch's own snapshot at admission.
            None => mycopilot_core::AgentPermissions::default(),
        };
        let user_message_id = candidate
            .user_message_id
            .clone()
            .unwrap_or_else(|| create_id("message"));
        let input = AgentConversationTurnInput {
            conversation_id: Some(conversation.id),
            project_id: conversation.project_id,
            model_id: conversation.model_id.ok_or("human answer model missing")?,
            context_window_indicator_enabled: true,
            content,
            attachments: Vec::new(),
            skills: Vec::new(),
            title: None,
            user_message_id: Some(user_message_id.clone()),
            assistant_message_id: None,
            max_tokens: None,
            temperature: None,
            prompt_preferences: None,
            permissions,
        };
        self.start_human_root_turn_with_response(
            input,
            None,
            None,
            Some(HumanInteractionAsyncTurnAdmission {
                response_id: response.response_id.clone(),
                expected_delivery_revision: delivery.revision,
                user_message_id,
            }),
            notifications.clone(),
        )
        .map_err(|e| e.to_string())?;
        Ok(true)
    }
}
