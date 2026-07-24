use super::*;

const RUN_NOT_STEERABLE_MESSAGE: &str =
    "The agent run has finished, is waiting for approval, or no longer accepts guidance.";

impl AgentService {
    pub fn steer_run(
        &self,
        input: AgentSteerRunInput,
        notifications: CoreServerNotificationSender,
    ) -> Result<AgentSteerRunOutput, AgentServiceError> {
        let conversation_id = input.conversation_id.trim().to_string();
        let run_id = input.expected_run_id.trim().to_string();
        let client_message_id = input.client_message_id.trim().to_string();
        let content = input.content.trim().to_string();
        if conversation_id.is_empty()
            || run_id.is_empty()
            || client_message_id.is_empty()
            || content.is_empty()
        {
            return Err(
                "conversationId、expectedRunId、clientMessageId 和 content 均不能为空。"
                    .to_string()
                    .into(),
            );
        }

        let existing = self
            .storage
            .load_agent_run_guidance_by_client_message(&run_id, &client_message_id)?;
        if let Some(existing) = existing.as_ref() {
            if !same_guidance_request(existing, &conversation_id, &run_id, &content, &input) {
                let rejected_guidance_id = create_id("guidance");
                return Ok(reject_without_journal_transition(
                    &notifications,
                    &run_id,
                    &rejected_guidance_id,
                    &client_message_id,
                    &content,
                    AgentSteerRunRejectionCode::IdentityConflict,
                    "The clientMessageId is already bound to different guidance content.",
                    now_ms(),
                ));
            }
            match existing.status {
                AgentGuidanceStatus::Applied => {
                    return Ok(applied_output(existing.guidance_id.clone()));
                }
                AgentGuidanceStatus::Rejected | AgentGuidanceStatus::Abandoned => {
                    return Ok(rejected_output(
                        existing.guidance_id.clone(),
                        AgentSteerRunRejectionCode::RunNotSteerable,
                        existing
                            .terminal_reason
                            .clone()
                            .unwrap_or_else(|| RUN_NOT_STEERABLE_MESSAGE.to_string()),
                    ));
                }
                AgentGuidanceStatus::Queued => {}
            }
        }

        let guidance_id = existing
            .as_ref()
            .map(|record| record.guidance_id.clone())
            .unwrap_or_else(|| create_id("guidance"));
        let created_at = existing
            .as_ref()
            .map(|record| record.created_at)
            .unwrap_or_else(now_ms);

        // Admission, approval fencing and terminal close all serialize through this registry
        // lock. The runtime queue has its own final-response close fence for the smaller race
        // between the last model response and this host-side admission path.
        let mut active_runs = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(control) = active_runs.get_mut(&run_id) else {
            if existing.is_some() {
                self.reject_queued_guidance(
                    &notifications,
                    &run_id,
                    &guidance_id,
                    &client_message_id,
                    &content,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    RUN_NOT_STEERABLE_MESSAGE,
                    created_at,
                )?;
                return Ok(rejected_output(
                    guidance_id,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    RUN_NOT_STEERABLE_MESSAGE,
                ));
            }
            return Ok(reject_without_journal_transition(
                &notifications,
                &run_id,
                &guidance_id,
                &client_message_id,
                &content,
                AgentSteerRunRejectionCode::RunNotSteerable,
                RUN_NOT_STEERABLE_MESSAGE,
                created_at,
            ));
        };
        if control.conversation_id != conversation_id {
            return Ok(reject_without_journal_transition(
                &notifications,
                &run_id,
                &guidance_id,
                &client_message_id,
                &content,
                AgentSteerRunRejectionCode::ConversationMismatch,
                "The active run belongs to a different conversation.",
                created_at,
            ));
        }
        if let ActiveRunSteerState::Closed { code, message } = &control.steer_state {
            if existing.is_some() {
                self.reject_queued_guidance(
                    &notifications,
                    &run_id,
                    &guidance_id,
                    &client_message_id,
                    &content,
                    *code,
                    message,
                    created_at,
                )?;
                return Ok(rejected_output(guidance_id, *code, message));
            }
            return Ok(reject_without_journal_transition(
                &notifications,
                &run_id,
                &guidance_id,
                &client_message_id,
                &content,
                *code,
                message,
                created_at,
            ));
        }
        if !input.attachments.is_empty() {
            let code = if !control.model_capabilities.image_input
                && input.attachments.iter().any(|attachment| {
                    attachment.kind == mycopilot_core::AgentInputAttachmentKind::Image
                }) {
                AgentSteerRunRejectionCode::ModelDoesNotSupportAttachments
            } else {
                AgentSteerRunRejectionCode::AttachmentsNotSupported
            };
            let message = match code {
                AgentSteerRunRejectionCode::ModelDoesNotSupportAttachments => {
                    "The active model does not support image attachments."
                }
                _ => "Steering attachments are not enabled in the text-only beta.",
            };
            return Ok(reject_without_journal_transition(
                &notifications,
                &run_id,
                &guidance_id,
                &client_message_id,
                &content,
                code,
                message,
                created_at,
            ));
        }

        if existing.is_none() {
            let record = AgentRunGuidanceRecord {
                guidance_id: guidance_id.clone(),
                client_message_id: client_message_id.clone(),
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                assistant_message_id: control.assistant_message_id.clone(),
                content: content.clone(),
                status: AgentGuidanceStatus::Queued,
                attachment_ids: Vec::new(),
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at,
                updated_at: created_at,
            };
            match self.storage.store_agent_run_guidance(record)? {
                AgentRunGuidanceStoreOutcome::Inserted
                | AgentRunGuidanceStoreOutcome::Idempotent => {}
                AgentRunGuidanceStoreOutcome::Conflict { .. } => {
                    return Ok(reject_without_journal_transition(
                        &notifications,
                        &run_id,
                        &guidance_id,
                        &client_message_id,
                        &content,
                        AgentSteerRunRejectionCode::IdentityConflict,
                        "The guidance identity conflicts with an existing durable request.",
                        created_at,
                    ));
                }
            }
        }

        let runtime_input = AgentSteerInput {
            guidance_id: guidance_id.clone(),
            client_message_id: client_message_id.clone(),
            content: content.clone(),
            attachments: Vec::new(),
            created_at,
        };
        let queued_notifications = notifications.clone();
        let queued_run_id = run_id.clone();
        let queued_guidance_id = guidance_id.clone();
        let queued_client_message_id = client_message_id.clone();
        let queued_content = content.clone();
        let outcome = control
            .steer_input
            .enqueue_with(runtime_input, move || {
                let _ = queued_notifications.send(agent_event_notification(
                    AgentEvent::GuidanceQueued {
                        run_id: queued_run_id,
                        guidance_id: queued_guidance_id,
                        client_message_id: queued_client_message_id,
                        content: queued_content,
                        attachments: Vec::new(),
                        created_at,
                    },
                ));
            })
            .map_err(|error| error.to_string())?;
        match outcome {
            AgentSteerEnqueueOutcome::Queued | AgentSteerEnqueueOutcome::Duplicate => {
                Ok(queued_output(guidance_id))
            }
            AgentSteerEnqueueOutcome::Closed => {
                self.reject_queued_guidance(
                    &notifications,
                    &run_id,
                    &guidance_id,
                    &client_message_id,
                    &content,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    RUN_NOT_STEERABLE_MESSAGE,
                    created_at,
                )?;
                Ok(rejected_output(
                    guidance_id,
                    AgentSteerRunRejectionCode::RunNotSteerable,
                    RUN_NOT_STEERABLE_MESSAGE,
                ))
            }
        }
    }

    pub(super) fn register_active_run_control(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        model_capabilities: ModelCapabilities,
    ) -> AgentSteerInputQueue {
        let steer_input = AgentSteerInputQueue::new();
        self.active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                run_id.to_string(),
                ActiveRunControl {
                    conversation_id: conversation_id.to_string(),
                    assistant_message_id: assistant_message_id.to_string(),
                    model_capabilities,
                    steer_state: ActiveRunSteerState::Accepting,
                    steer_input: steer_input.clone(),
                },
            );
        steer_input
    }

    pub(super) fn close_active_run_steering(
        &self,
        run_id: &str,
        code: AgentSteerRunRejectionCode,
        message: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.close_active_run_steering_inner(run_id, code, message, notifications, false)
    }

    pub(super) fn unregister_active_run_control(
        &self,
        run_id: &str,
        code: AgentSteerRunRejectionCode,
        message: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.close_active_run_steering_inner(run_id, code, message, notifications, true)
    }

    fn close_active_run_steering_inner(
        &self,
        run_id: &str,
        code: AgentSteerRunRejectionCode,
        message: &str,
        notifications: &CoreServerNotificationSender,
        remove: bool,
    ) -> Result<(), String> {
        let mut active_runs = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(control) = active_runs.get_mut(run_id) else {
            return Ok(());
        };
        control.steer_state = ActiveRunSteerState::Closed {
            code,
            message: message.to_string(),
        };
        let pending = control.steer_input.close_and_take_pending();
        let mut first_error = None;
        for input in pending {
            if let Err(error) = self.reject_queued_guidance(
                notifications,
                run_id,
                &input.guidance_id,
                &input.client_message_id,
                &input.content,
                code,
                message,
                input.created_at,
            ) {
                first_error.get_or_insert_with(|| error.to_string());
            }
        }
        match self.storage.list_queued_agent_run_guidances() {
            Ok(records) => {
                for record in records.into_iter().filter(|record| record.run_id == run_id) {
                    if let Err(error) = self.reject_queued_guidance(
                        notifications,
                        run_id,
                        &record.guidance_id,
                        &record.client_message_id,
                        &record.content,
                        code,
                        message,
                        record.created_at,
                    ) {
                        first_error.get_or_insert_with(|| error.to_string());
                    }
                }
            }
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
        if remove {
            active_runs.remove(run_id);
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn reject_queued_guidance(
        &self,
        notifications: &CoreServerNotificationSender,
        run_id: &str,
        guidance_id: &str,
        client_message_id: &str,
        content: &str,
        code: AgentSteerRunRejectionCode,
        message: &str,
        created_at: i64,
    ) -> Result<(), AgentServiceError> {
        match self.storage.mark_agent_run_guidance_terminal(
            guidance_id,
            AgentGuidanceStatus::Rejected,
            message,
            now_ms().max(created_at),
        )? {
            AgentRunGuidanceTransitionOutcome::Updated
            | AgentRunGuidanceTransitionOutcome::Idempotent => {}
            AgentRunGuidanceTransitionOutcome::NotFound => {
                return Err(format!("guidance journal not found: {guidance_id}").into());
            }
            AgentRunGuidanceTransitionOutcome::Conflict { current_status } => {
                return Err(format!(
                    "guidance journal transition conflict for {guidance_id}: {}",
                    current_status.as_str()
                )
                .into());
            }
        }
        let _ = notifications.send(agent_event_notification(AgentEvent::GuidanceRejected {
            run_id: run_id.to_string(),
            guidance_id: guidance_id.to_string(),
            client_message_id: client_message_id.to_string(),
            content: content.to_string(),
            rejection_code: code,
            message: message.to_string(),
            created_at,
        }));
        Ok(())
    }
}

fn same_guidance_request(
    existing: &AgentRunGuidanceRecord,
    conversation_id: &str,
    run_id: &str,
    content: &str,
    input: &AgentSteerRunInput,
) -> bool {
    existing.conversation_id == conversation_id
        && existing.run_id == run_id
        && existing.content == content
        && existing.attachment_ids
            == input
                .attachments
                .iter()
                .map(|attachment| attachment.id.clone())
                .collect::<Vec<_>>()
}

fn queued_output(guidance_id: String) -> AgentSteerRunOutput {
    AgentSteerRunOutput {
        guidance_id,
        status: AgentSteerRunResultStatus::Queued,
        rejection_code: None,
        message: None,
    }
}

fn applied_output(guidance_id: String) -> AgentSteerRunOutput {
    AgentSteerRunOutput {
        guidance_id,
        status: AgentSteerRunResultStatus::Applied,
        rejection_code: None,
        message: None,
    }
}

fn rejected_output(
    guidance_id: String,
    code: AgentSteerRunRejectionCode,
    message: impl Into<String>,
) -> AgentSteerRunOutput {
    AgentSteerRunOutput {
        guidance_id,
        status: AgentSteerRunResultStatus::Rejected,
        rejection_code: Some(code),
        message: Some(message.into()),
    }
}

#[allow(clippy::too_many_arguments)]
fn reject_without_journal_transition(
    notifications: &CoreServerNotificationSender,
    run_id: &str,
    guidance_id: &str,
    client_message_id: &str,
    content: &str,
    code: AgentSteerRunRejectionCode,
    message: &str,
    created_at: i64,
) -> AgentSteerRunOutput {
    let _ = notifications.send(agent_event_notification(AgentEvent::GuidanceRejected {
        run_id: run_id.to_string(),
        guidance_id: guidance_id.to_string(),
        client_message_id: client_message_id.to_string(),
        content: content.to_string(),
        rejection_code: code,
        message: message.to_string(),
        created_at,
    }));
    rejected_output(guidance_id.to_string(), code, message)
}
