use super::*;

const RUN_NOT_STEERABLE_MESSAGE: &str = "The agent run has finished or no longer accepts guidance.";
const MAX_GUIDANCE_ATTACHMENTS: usize = 8;

/// Owns the inbox only until another execution segment adopts it. Failure paths which retain a
/// durable recovery record must still stop accepting messages once their worker has exited.
pub(super) struct ActiveRunSteeringCleanup {
    service: AgentService,
    run_id: String,
    queue: Option<AgentSteerInputQueue>,
    notifications: CoreServerNotificationSender,
}

impl ActiveRunSteeringCleanup {
    pub(super) fn disarm(&mut self) {
        self.queue = None;
    }
}

impl Drop for ActiveRunSteeringCleanup {
    fn drop(&mut self) {
        if let Some(queue) = self.queue.as_ref() {
            if let Err(error) = self.service.unregister_active_run_control(
                &self.run_id,
                queue,
                AgentSteerRunRejectionCode::RunNotSteerable,
                "The agent execution stopped before the guidance could be applied.",
                &self.notifications,
            ) {
                eprintln!("failed to settle guidance after Host execution: {error}");
            }
        }
    }
}

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
        // This namespace authenticates Host-owned answer guidance, including frozen fork history.
        // Reject before journaling or emitting a GuidanceRejected projection: even a rejected
        // external message must never look like a formal human-interaction response in the UI.
        // Native answer delivery binds/enqueues its trusted record directly, not via this API.
        if client_message_id.starts_with("human-answer-") {
            return Err("clientMessageId 使用了 Host 保留的回答标识前缀。"
                .to_string()
                .into());
        }
        self.authorize_user_conversation_write(&conversation_id)?;

        let existing = self
            .storage
            .load_agent_run_guidance_by_client_message(&run_id, &client_message_id)?;
        let existing_attachments = existing
            .as_ref()
            .map(|record| self.storage.load_input_attachments(&record.attachment_ids))
            .transpose()?
            .unwrap_or_default();
        if let Some(existing) = existing.as_ref() {
            if !same_guidance_request(
                &self.storage,
                existing,
                &existing_attachments,
                &conversation_id,
                &run_id,
                &content,
                &input,
            ) {
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

        // Admission, approval handoff and terminal close all serialize through this registry
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
        control.steering_notifications = Some(notifications.clone());
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
        if let Err((code, message)) = validate_guidance_attachments(
            &input.attachments,
            &self.storage,
            control.model_capabilities,
        ) {
            return Ok(reject_without_journal_transition(
                &notifications,
                &run_id,
                &guidance_id,
                &client_message_id,
                &content,
                code,
                &message,
                created_at,
            ));
        }

        let mut persisted_attachments = existing_attachments;
        if existing.is_none() {
            let attachment_ids = input
                .attachments
                .iter()
                .map(|attachment| attachment.id.clone())
                .collect::<Vec<_>>();
            let record = AgentRunGuidanceRecord {
                guidance_id: guidance_id.clone(),
                client_message_id: client_message_id.clone(),
                run_id: run_id.clone(),
                conversation_id: conversation_id.clone(),
                assistant_message_id: control.assistant_message_id.clone(),
                content: content.clone(),
                status: AgentGuidanceStatus::Queued,
                attachment_ids,
                applied_trace_sequence: None,
                terminal_reason: None,
                created_at,
                updated_at: created_at,
            };
            let store_result = self.storage.store_agent_run_guidance_with_attachments(
                record,
                control.project_id.as_deref(),
                &input.attachments,
            );
            let outcome = match store_result {
                Ok(outcome) => outcome,
                Err(error) => {
                    eprintln!("failed to persist steering attachments: {error}");
                    return Ok(reject_without_journal_transition(
                        &notifications,
                        &run_id,
                        &guidance_id,
                        &client_message_id,
                        &content,
                        AgentSteerRunRejectionCode::AttachmentPersistenceFailed,
                        "The guidance attachments could not be persisted. Retry the guidance.",
                        created_at,
                    ));
                }
            };
            match outcome {
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
            persisted_attachments = match self.storage.load_input_attachments(
                &input
                    .attachments
                    .iter()
                    .map(|attachment| attachment.id.clone())
                    .collect::<Vec<_>>(),
            ) {
                Ok(attachments) => attachments,
                Err(error) => {
                    eprintln!("failed to reload persisted steering attachments: {error}");
                    self.reject_queued_guidance(
                        &notifications,
                        &run_id,
                        &guidance_id,
                        &client_message_id,
                        &content,
                        AgentSteerRunRejectionCode::AttachmentPersistenceFailed,
                        "The persisted guidance attachments could not be verified.",
                        created_at,
                    )?;
                    return Ok(rejected_output(
                        guidance_id,
                        AgentSteerRunRejectionCode::AttachmentPersistenceFailed,
                        "The persisted guidance attachments could not be verified.",
                    ));
                }
            };
        }

        let attachment_library = if persisted_attachments.is_empty() {
            None
        } else {
            match self
                .storage
                .build_attachment_library_context_for_active_run(
                    &conversation_id,
                    control.project_id.as_deref(),
                    &run_id,
                ) {
                Ok(library) => Some(library),
                Err(error) => {
                    eprintln!("failed to build steering attachment library: {error}");
                    self.reject_queued_guidance(
                        &notifications,
                        &run_id,
                        &guidance_id,
                        &client_message_id,
                        &content,
                        AgentSteerRunRejectionCode::AttachmentPersistenceFailed,
                        "The guidance attachment library could not be prepared.",
                        created_at,
                    )?;
                    return Ok(rejected_output(
                        guidance_id,
                        AgentSteerRunRejectionCode::AttachmentPersistenceFailed,
                        "The guidance attachment library could not be prepared.",
                    ));
                }
            }
        };
        let trace_attachments = persisted_attachments
            .iter()
            .map(|attachment| mycopilot_core::ConversationTraceAttachment {
                id: attachment.id.clone(),
                kind: attachment.kind,
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: attachment.size_bytes,
            })
            .collect::<Vec<_>>();
        let runtime_input = AgentSteerInput {
            guidance_id: guidance_id.clone(),
            client_message_id: client_message_id.clone(),
            content: content.clone(),
            attachments: persisted_attachments,
            attachment_library,
            created_at,
        };
        let queued_notifications = notifications.clone();
        let queued_run_id = run_id.clone();
        let queued_guidance_id = guidance_id.clone();
        let queued_client_message_id = client_message_id.clone();
        let queued_content = content.clone();
        let queued_attachments = trace_attachments.clone();
        let outcome = control
            .steer_input
            .enqueue_with(runtime_input, move || {
                let _ = queued_notifications.send(agent_event_notification(
                    AgentEvent::GuidanceQueued {
                        run_id: queued_run_id,
                        guidance_id: queued_guidance_id,
                        client_message_id: queued_client_message_id,
                        content: queued_content,
                        attachments: queued_attachments,
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
        project_id: Option<&str>,
        model_capabilities: ModelCapabilities,
        permissions: AgentPermissions,
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
                    project_id: project_id.map(ToString::to_string),
                    model_capabilities,
                    permissions,
                    steer_state: ActiveRunSteerState::Accepting,
                    steer_input: steer_input.clone(),
                    steering_notifications: None,
                },
            );
        steer_input
    }

    /// Approval execution is part of the same logical Run. Its waiting inbox already accepts
    /// messages while no Runtime is attached; continuation must adopt it without resetting FIFO.
    pub(super) fn resume_active_run_control(
        &self,
        run_id: &str,
        conversation_id: &str,
        assistant_message_id: &str,
        project_id: Option<&str>,
        model_capabilities: ModelCapabilities,
        permissions: AgentPermissions,
    ) -> AgentSteerInputQueue {
        let mut active = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(control) = active.get_mut(run_id) {
            // Run identity was validated against the durable Turn before reaching this boundary.
            // A cancelled/closed control must remain closed, never be reopened by a late worker.
            if matches!(control.steer_state, ActiveRunSteerState::Accepting) {
                // Claim the inbox under a fresh identity before the old Host worker exits.
                // Its cleanup guard can then only close the retired waiting queue.
                control.steer_input = control.steer_input.handoff_pending();
            }
            return control.steer_input.clone();
        }
        let steer_input = AgentSteerInputQueue::new();
        active.insert(
            run_id.to_string(),
            ActiveRunControl {
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                project_id: project_id.map(ToString::to_string),
                model_capabilities,
                permissions,
                steer_state: ActiveRunSteerState::Accepting,
                steer_input: steer_input.clone(),
                steering_notifications: None,
            },
        );
        steer_input
    }

    pub(super) fn active_run_steering_cleanup(
        &self,
        run_id: &str,
        notifications: CoreServerNotificationSender,
    ) -> ActiveRunSteeringCleanup {
        let queue = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .map(|control| control.steer_input.clone());
        ActiveRunSteeringCleanup {
            service: self.clone(),
            run_id: run_id.to_string(),
            queue,
            notifications,
        }
    }

    pub(super) fn segment_steering_cleanup(
        &self,
        run_id: &str,
        queue: &AgentSteerInputQueue,
        notifications: CoreServerNotificationSender,
    ) -> ActiveRunSteeringCleanup {
        ActiveRunSteeringCleanup {
            service: self.clone(),
            run_id: run_id.to_string(),
            queue: Some(queue.clone()),
            notifications,
        }
    }

    pub(super) fn handoff_active_run_steering(
        &self,
        run_id: &str,
        expected_steer_input: &AgentSteerInputQueue,
    ) -> Option<AgentSteerInputQueue> {
        let mut active = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let control = active.get_mut(run_id)?;
        if !control.steer_input.is_same_queue(expected_steer_input)
            || !matches!(control.steer_state, ActiveRunSteerState::Accepting)
        {
            return None;
        }
        let successor = control.steer_input.handoff_pending();
        control.steer_input = successor.clone();
        Some(successor)
    }

    pub(super) fn bind_active_run_steering_notifications(
        &self,
        run_id: &str,
        expected_steer_input: &AgentSteerInputQueue,
        notifications: CoreServerNotificationSender,
    ) {
        let mut active = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(control) = active.get_mut(run_id) {
            if control.steer_input.is_same_queue(expected_steer_input) {
                control.steering_notifications = Some(notifications);
            }
        }
    }

    /// Also covers cancellation/failure during approved Host work, before a Runtime resumes.
    pub(super) fn finish_active_run_steering(&self, run_id: &str, message: &str) {
        let current = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(run_id)
            .map(|control| {
                (
                    control.steer_input.clone(),
                    control.steering_notifications.clone(),
                )
            });
        let Some((queue, notifications)) = current else {
            return;
        };
        let notifications =
            notifications.unwrap_or_else(|| tokio::sync::mpsc::unbounded_channel().0);
        if let Err(error) = self.unregister_active_run_control(
            run_id,
            &queue,
            AgentSteerRunRejectionCode::RunNotSteerable,
            message,
            &notifications,
        ) {
            eprintln!("failed to settle terminal Run guidance: {error}");
        }
    }

    /// Stop admission immediately, leaving final settlement to the current worker. It may have
    /// already drained an input whose trace acknowledgement is still committing concurrently.
    pub(super) fn fence_cancelled_run_steering(&self, run_id: &str) {
        let mut active = self
            .active_runs
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(control) = active.get_mut(run_id) {
            control.steer_state = ActiveRunSteerState::Closed {
                code: AgentSteerRunRejectionCode::RunNotSteerable,
                message: "The agent run was cancelled and no longer accepts guidance.".to_string(),
            };
            control.steer_input.close();
        }
    }

    pub(super) fn refresh_agent_input_attachment_library(
        &self,
        agent_input: &mut AgentChatInput,
    ) -> Result<(), String> {
        let Some(context) = agent_input.context.as_mut() else {
            return Ok(());
        };
        let Some(conversation_id) = context.conversation_id.as_deref() else {
            return Ok(());
        };
        context.attachment_library = Some(
            self.storage
                .build_attachment_library_context(conversation_id, context.project_id.as_deref())?,
        );
        Ok(())
    }

    pub(super) fn close_active_run_steering(
        &self,
        run_id: &str,
        code: AgentSteerRunRejectionCode,
        message: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.close_active_run_steering_inner(run_id, None, code, message, notifications, false)
    }

    pub(super) fn unregister_active_run_control(
        &self,
        run_id: &str,
        expected_steer_input: &AgentSteerInputQueue,
        code: AgentSteerRunRejectionCode,
        message: &str,
        notifications: &CoreServerNotificationSender,
    ) -> Result<(), String> {
        self.close_active_run_steering_inner(
            run_id,
            Some(expected_steer_input),
            code,
            message,
            notifications,
            true,
        )
    }

    fn close_active_run_steering_inner(
        &self,
        run_id: &str,
        expected_steer_input: Option<&AgentSteerInputQueue>,
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
        if expected_steer_input.is_some_and(|expected| !control.steer_input.is_same_queue(expected))
        {
            return Ok(());
        }
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
    storage: &StorageService,
    existing: &AgentRunGuidanceRecord,
    persisted_attachments: &[mycopilot_core::AgentInputAttachment],
    conversation_id: &str,
    run_id: &str,
    content: &str,
    input: &AgentSteerRunInput,
) -> bool {
    existing.conversation_id == conversation_id
        && existing.run_id == run_id
        && existing.content == content
        && persisted_attachments.len() == input.attachments.len()
        && persisted_attachments
            .iter()
            .zip(&input.attachments)
            .all(|(persisted, candidate)| {
                attachments_have_same_identity(storage, persisted, candidate)
            })
}

fn attachments_have_same_identity(
    storage: &StorageService,
    persisted: &mycopilot_core::AgentInputAttachment,
    candidate: &mycopilot_core::AgentInputAttachment,
) -> bool {
    persisted.id == candidate.id
        && persisted.kind == candidate.kind
        && persisted.name == candidate.name
        && normalized_mime(persisted.mime_type.as_deref())
            == normalized_mime(candidate.mime_type.as_deref())
        && persisted.size_bytes == candidate.size_bytes
        && matches!(
            (
                storage.input_attachment_content_digest(persisted),
                storage.input_attachment_content_digest(candidate),
            ),
            (Ok(persisted), Ok(candidate)) if persisted == candidate
        )
}

fn validate_guidance_attachments(
    attachments: &[mycopilot_core::AgentInputAttachment],
    storage: &mycopilot_core::storage::service::StorageService,
    model_capabilities: ModelCapabilities,
) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    if attachments.is_empty() {
        return Ok(());
    }
    if !model_capabilities.image_input
        && attachments
            .iter()
            .any(|attachment| attachment.kind == mycopilot_core::AgentInputAttachmentKind::Image)
    {
        return Err((
            AgentSteerRunRejectionCode::ModelDoesNotSupportAttachments,
            "The active model does not support image attachments.".to_string(),
        ));
    }
    validate_guidance_attachment_limits(attachments)?;

    let mut seen_ids = HashSet::new();
    for attachment in attachments {
        let id = attachment.id.trim();
        let name = attachment.name.trim();
        if id.is_empty()
            || id != attachment.id
            || id.len() > 256
            || safe_path_component(id, "attachment") != id
            || !seen_ids.insert(id.to_string())
        {
            return attachment_validation_error(
                "Attachment ids must be unique, stable path components.",
            );
        }
        if name.is_empty()
            || name.len() > 255
            || name != attachment.name
            || Path::new(name).file_name().and_then(|value| value.to_str()) != Some(name)
            || name.chars().any(char::is_control)
        {
            return attachment_validation_error("Attachment names must be non-empty file names.");
        }
        if attachment.truncated == Some(true) {
            return attachment_validation_error(
                "Truncated attachment payloads cannot be admitted as guidance.",
            );
        }
        if attachment.size_bytes == 0 {
            return attachment_validation_error("Empty attachments are not supported.");
        }
        if attachment.encoding != mycopilot_core::AgentInputAttachmentEncoding::Managed {
            return attachment_validation_error(
                "Guidance attachments must use managed references.",
            );
        }
        validate_managed_guidance_type(storage, attachment)?;
    }

    Ok(())
}

fn validate_guidance_attachment_limits(
    attachments: &[mycopilot_core::AgentInputAttachment],
) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    if attachments.len() > MAX_GUIDANCE_ATTACHMENTS {
        return Err((
            AgentSteerRunRejectionCode::AttachmentLimitExceeded,
            format!("A guidance message supports at most {MAX_GUIDANCE_ATTACHMENTS} attachments."),
        ));
    }
    Ok(())
}

fn attachment_validation_error(message: &str) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    Err((
        AgentSteerRunRejectionCode::AttachmentValidationFailed,
        message.to_string(),
    ))
}

fn normalized_mime(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn validate_managed_guidance_type(
    storage: &mycopilot_core::storage::service::StorageService,
    attachment: &mycopilot_core::AgentInputAttachment,
) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    use std::io::{Read, Seek};
    let error = |message: String| {
        (
            AgentSteerRunRejectionCode::AttachmentValidationFailed,
            message,
        )
    };
    let mut file = storage
        .open_validated_managed_input_attachment(attachment)
        .map_err(error)?;
    let extension = Path::new(&attachment.name)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime = normalized_mime(attachment.mime_type.as_deref())
        .ok_or_else(|| error("Attachment MIME type is required.".into()))?;
    if mime.len() > 127 || mime.chars().any(char::is_control) {
        return attachment_validation_error("Attachment MIME type is invalid.");
    }
    let mut prefix = [0u8; 64];
    let length = file.read(&mut prefix).map_err(|e| error(e.to_string()))?;
    file.rewind().map_err(|e| error(e.to_string()))?;
    let prefix = &prefix[..length];
    let valid = if attachment.kind == mycopilot_core::AgentInputAttachmentKind::Image {
        image_type_matches(&extension, &mime, prefix)
            && storage
                .load_input_attachment_preview(attachment)
                .map_err(error)?
                .is_some()
    } else {
        match extension.as_str() {
            "pdf" => mime == "application/pdf" && prefix.starts_with(b"%PDF-"),
            "doc" => {
                mime == "application/msword"
                    && prefix.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")
            }
            "docx" => {
                mime == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    && is_ooxml_file(&mut file, "word/")
            }
            "pptx" => {
                mime == "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                    && is_ooxml_file(&mut file, "ppt/")
            }
            "xlsx" => {
                mime == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                    && is_ooxml_file(&mut file, "xl/")
            }
            "csv" => matches!(mime.as_str(), "text/csv" | "text/plain") && is_utf8_reader(file),
            "tsv" => {
                matches!(mime.as_str(), "text/tab-separated-values" | "text/plain")
                    && is_utf8_reader(file)
            }
            extension if is_supported_text_extension(extension) => {
                is_textual_mime(&mime) && is_utf8_reader(file)
            }
            _ => false,
        }
    };
    if valid {
        Ok(())
    } else {
        attachment_validation_error(
            "Attachment kind, extension, MIME type, encoding, or file signature is inconsistent.",
        )
    }
}

fn is_utf8_reader(mut input: impl std::io::Read) -> bool {
    let mut buffer = vec![0u8; 64 * 1024 + 4];
    let mut carried = 0usize;
    loop {
        let Ok(read) = input.read(&mut buffer[carried..64 * 1024]) else {
            return false;
        };
        if read == 0 {
            return carried == 0;
        }
        let length = carried + read;
        match std::str::from_utf8(&buffer[..length]) {
            Ok(_) => carried = 0,
            Err(error) if error.error_len().is_none() => {
                carried = length - error.valid_up_to();
                if carried > 3 {
                    return false;
                }
                buffer.copy_within(length - carried..length, 0);
            }
            Err(_) => return false,
        }
    }
}

fn is_ooxml_file(file: &mut std::fs::File, required_prefix: &str) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(size) = file.seek(SeekFrom::End(0)) else {
        return false;
    };
    let tail_size = size.min(65_557) as usize;
    if file.seek(SeekFrom::End(-(tail_size as i64))).is_err() {
        return false;
    }
    let mut tail = vec![0u8; tail_size];
    if file.read_exact(&mut tail).is_err() {
        return false;
    }
    let Some(offset) = tail.windows(4).rposition(|bytes| bytes == b"PK\x05\x06") else {
        return false;
    };
    if offset + 22 > tail.len() {
        return false;
    }
    let directory_bytes = u32::from_le_bytes(tail[offset + 12..offset + 16].try_into().unwrap());
    // Bound only ZIP metadata, not the uploaded original or expanded document contents.
    if directory_bytes > 8 * 1024 * 1024 || file.rewind().is_err() {
        return false;
    }
    let Ok(mut archive) = zip::ZipArchive::new(file) else {
        return false;
    };
    (0..archive.len()).any(|index| {
        archive
            .by_index(index)
            .is_ok_and(|entry| entry.name().starts_with(required_prefix))
    })
}

fn image_type_matches(extension: &str, mime_type: &str, bytes: &[u8]) -> bool {
    let Ok(format) = image::guess_format(bytes) else {
        return false;
    };
    match format {
        image::ImageFormat::Png => extension == "png" && mime_type == "image/png",
        image::ImageFormat::Jpeg => {
            matches!(extension, "jpg" | "jpeg") && mime_type == "image/jpeg"
        }
        image::ImageFormat::Gif => extension == "gif" && mime_type == "image/gif",
        image::ImageFormat::WebP => extension == "webp" && mime_type == "image/webp",
        _ => false,
    }
}

fn is_textual_mime(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json"
                | "application/xml"
                | "application/javascript"
                | "application/x-javascript"
                | "application/x-yaml"
                | "application/yaml"
                | "image/svg+xml"
        )
}

fn is_supported_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "text"
            | "md"
            | "markdown"
            | "mdx"
            | "rst"
            | "log"
            | "json"
            | "jsonl"
            | "yaml"
            | "yml"
            | "toml"
            | "ini"
            | "cfg"
            | "conf"
            | "env"
            | "lock"
            | "properties"
            | "plist"
            | "rc"
            | "gitignore"
            | "gitattributes"
            | "editorconfig"
            | "py"
            | "pyi"
            | "ipynb"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "mjs"
            | "cjs"
            | "html"
            | "htm"
            | "css"
            | "scss"
            | "sass"
            | "less"
            | "xml"
            | "sql"
            | "graphql"
            | "gql"
            | "proto"
            | "prisma"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "ps1"
            | "bat"
            | "cmd"
            | "rs"
            | "go"
            | "java"
            | "kt"
            | "kts"
            | "c"
            | "h"
            | "cpp"
            | "cc"
            | "cxx"
            | "hpp"
            | "hh"
            | "hxx"
            | "cs"
            | "php"
            | "rb"
            | "swift"
            | "scala"
            | "r"
            | "m"
            | "pl"
            | "pm"
            | "lua"
            | "dart"
            | "ex"
            | "exs"
            | "erl"
            | "hrl"
            | "clj"
            | "cljs"
            | "cljc"
            | "edn"
            | "fs"
            | "fsi"
            | "fsx"
            | "elm"
            | "hs"
            | "lhs"
            | "jl"
            | "ml"
            | "mli"
            | "nim"
            | "nims"
            | "zig"
            | "v"
            | "vh"
            | "sv"
            | "svh"
            | "sol"
            | "tf"
            | "tfvars"
            | "hcl"
            | "gradle"
            | "groovy"
            | "dockerfile"
            | "cmake"
            | "make"
            | "mk"
            | "tex"
            | "bib"
            | "vue"
            | "svelte"
            | "astro"
    )
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

#[cfg(test)]
mod attachment_budget_tests {
    use super::*;
    use base64::Engine;

    fn attachment(id: usize, size_bytes: u64) -> mycopilot_core::AgentInputAttachment {
        mycopilot_core::AgentInputAttachment {
            id: format!("attachment-budget-{id}"),
            kind: mycopilot_core::AgentInputAttachmentKind::File,
            name: format!("file-{id}.txt"),
            mime_type: Some("text/plain".to_string()),
            size_bytes,
            encoding: mycopilot_core::AgentInputAttachmentEncoding::Base64,
            data: String::new(),
            truncated: None,
        }
    }

    #[test]
    fn managed_guidance_keeps_count_limit_without_counting_original_file_bytes() {
        let mut large = attachment(0, 100 * 1024 * 1024);
        large.encoding = mycopilot_core::AgentInputAttachmentEncoding::Managed;
        assert!(validate_guidance_attachment_limits(&[large.clone()]).is_ok());
        assert!(validate_guidance_attachment_limits(&vec![large; 9]).is_err());
    }

    #[test]
    fn streaming_utf8_validator_handles_codepoints_across_chunks_and_rejects_bad_tail() {
        let mut bytes = vec![b'a'; 64 * 1024 - 1];
        bytes.extend_from_slice("你好".as_bytes());
        assert!(is_utf8_reader(std::io::Cursor::new(&bytes)));
        bytes.push(0xff);
        assert!(!is_utf8_reader(std::io::Cursor::new(&bytes)));
        bytes.pop();
        bytes.pop();
        assert!(!is_utf8_reader(std::io::Cursor::new(&bytes)));
    }

    #[test]
    fn managed_guidance_validates_content_type_after_reference_authentication() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = mycopilot_core::storage::service::StorageService::open(
            &temporary.path().join("storage.sqlite"),
        )
        .unwrap();
        let imported = |id: &str, name: &str, mime: &str, bytes: &[u8]| {
            let token = storage
                .begin_attachment_import(mycopilot_core::AttachmentImportInput {
                    id: id.into(),
                    kind: mycopilot_core::AgentInputAttachmentKind::File,
                    name: name.into(),
                    mime_type: Some(mime.into()),
                    size_bytes: bytes.len() as u64,
                })
                .unwrap();
            storage
                .append_attachment_import(
                    &token,
                    0,
                    &base64::engine::general_purpose::STANDARD.encode(bytes),
                )
                .unwrap();
            storage.finish_attachment_import(&token).unwrap()
        };
        let valid = imported(
            "valid",
            "message.txt",
            "text/plain",
            "跨分块的文字".as_bytes(),
        );
        assert!(validate_managed_guidance_type(&storage, &valid).is_ok());
        let bad_utf8 = imported("bad-utf8", "message.txt", "text/plain", &[0xff]);
        assert!(validate_managed_guidance_type(&storage, &bad_utf8).is_err());
        let spoofed_pdf = imported(
            "spoofed",
            "document.pdf",
            "application/pdf",
            b"ordinary text",
        );
        assert!(validate_managed_guidance_type(&storage, &spoofed_pdf).is_err());
        let spoofed_zip = imported(
            "spoofed-zip",
            "document.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            b"PK fake archive",
        );
        assert!(validate_managed_guidance_type(&storage, &spoofed_zip).is_err());
    }
}
