use super::*;
use base64::Engine;

const RUN_NOT_STEERABLE_MESSAGE: &str =
    "The agent run has finished, is waiting for approval, or no longer accepts guidance.";
const MAX_GUIDANCE_ATTACHMENTS: usize = 8;
const MAX_GUIDANCE_ATTACHMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_GUIDANCE_TOTAL_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_RUN_GUIDANCE_ATTACHMENT_BYTES: u64 = 64 * 1024 * 1024;

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
        let existing_attachments = existing
            .as_ref()
            .map(|record| self.storage.load_input_attachments(&record.attachment_ids))
            .transpose()?
            .unwrap_or_default();
        if let Some(existing) = existing.as_ref() {
            if !same_guidance_request(
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
        let run_attachment_bytes = self.storage.agent_run_guidance_attachment_bytes(&run_id)?;
        let existing_attachment_bytes = existing_attachments
            .iter()
            .map(|attachment| attachment.size_bytes)
            .sum::<u64>();
        if let Err((code, message)) = validate_guidance_attachments(
            &input.attachments,
            control.model_capabilities,
            run_attachment_bytes.saturating_sub(existing_attachment_bytes),
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
                    steer_state: ActiveRunSteerState::Accepting,
                    steer_input: steer_input.clone(),
                },
            );
        steer_input
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
            .all(|(persisted, candidate)| attachments_have_same_identity(persisted, candidate))
}

fn attachments_have_same_identity(
    persisted: &mycopilot_core::AgentInputAttachment,
    candidate: &mycopilot_core::AgentInputAttachment,
) -> bool {
    persisted.id == candidate.id
        && persisted.kind == candidate.kind
        && persisted.name == candidate.name
        && normalized_mime(persisted.mime_type.as_deref())
            == normalized_mime(candidate.mime_type.as_deref())
        && persisted.size_bytes == candidate.size_bytes
        && decoded_attachment_bytes(persisted).ok() == decoded_attachment_bytes(candidate).ok()
}

fn validate_guidance_attachments(
    attachments: &[mycopilot_core::AgentInputAttachment],
    model_capabilities: ModelCapabilities,
    prior_run_bytes: u64,
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
    validate_guidance_attachment_limits(attachments, prior_run_bytes)?;

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
        let bytes = decoded_attachment_bytes(attachment).map_err(|message| {
            (
                AgentSteerRunRejectionCode::AttachmentValidationFailed,
                message,
            )
        })?;
        if bytes.len() as u64 != attachment.size_bytes {
            return attachment_validation_error(
                "Attachment sizeBytes does not match the decoded payload size.",
            );
        }
        validate_attachment_type(attachment, &bytes)?;
    }

    Ok(())
}

fn validate_guidance_attachment_limits(
    attachments: &[mycopilot_core::AgentInputAttachment],
    prior_run_bytes: u64,
) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    if attachments.len() > MAX_GUIDANCE_ATTACHMENTS {
        return Err((
            AgentSteerRunRejectionCode::AttachmentLimitExceeded,
            format!("A guidance message supports at most {MAX_GUIDANCE_ATTACHMENTS} attachments."),
        ));
    }
    if attachments
        .iter()
        .any(|attachment| attachment.size_bytes > MAX_GUIDANCE_ATTACHMENT_BYTES)
    {
        return Err((
            AgentSteerRunRejectionCode::AttachmentLimitExceeded,
            "A single guidance attachment cannot exceed 8 MiB.".to_string(),
        ));
    }
    let total_bytes = attachments
        .iter()
        .try_fold(0_u64, |total, attachment| {
            total.checked_add(attachment.size_bytes)
        })
        .ok_or_else(|| {
            (
                AgentSteerRunRejectionCode::AttachmentLimitExceeded,
                "Guidance attachment size overflow.".to_string(),
            )
        })?;
    if total_bytes > MAX_GUIDANCE_TOTAL_ATTACHMENT_BYTES {
        return Err((
            AgentSteerRunRejectionCode::AttachmentLimitExceeded,
            "One guidance message cannot contain more than 32 MiB of attachments.".to_string(),
        ));
    }
    if prior_run_bytes
        .checked_add(total_bytes)
        .is_none_or(|total| total > MAX_RUN_GUIDANCE_ATTACHMENT_BYTES)
    {
        return Err((
            AgentSteerRunRejectionCode::AttachmentLimitExceeded,
            "The active run cannot accept more than 64 MiB of guidance attachments.".to_string(),
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

fn decoded_attachment_bytes(
    attachment: &mycopilot_core::AgentInputAttachment,
) -> Result<Vec<u8>, String> {
    match attachment.encoding {
        mycopilot_core::AgentInputAttachmentEncoding::Utf8 => {
            Ok(attachment.data.as_bytes().to_vec())
        }
        mycopilot_core::AgentInputAttachmentEncoding::Base64 => {
            base64::engine::general_purpose::STANDARD
                .decode(attachment.data.as_bytes())
                .map_err(|_| "Attachment contains invalid base64 data.".to_string())
        }
    }
}

fn normalized_mime(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
}

fn validate_attachment_type(
    attachment: &mycopilot_core::AgentInputAttachment,
    bytes: &[u8],
) -> Result<(), (AgentSteerRunRejectionCode, String)> {
    let extension = Path::new(&attachment.name)
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let Some(mime_type) = normalized_mime(attachment.mime_type.as_deref()) else {
        return attachment_validation_error("Attachment MIME type is required.");
    };
    if mime_type.len() > 127 || mime_type.chars().any(char::is_control) {
        return attachment_validation_error("Attachment MIME type is invalid.");
    }

    let valid = match attachment.kind {
        mycopilot_core::AgentInputAttachmentKind::Image => {
            validate_image_type(&extension, &mime_type, bytes)
        }
        mycopilot_core::AgentInputAttachmentKind::File => {
            validate_file_type(&extension, &mime_type, bytes)
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

fn validate_image_type(extension: &str, mime_type: &str, bytes: &[u8]) -> bool {
    let Ok(format) = image::guess_format(bytes) else {
        return false;
    };
    let identity_matches = match format {
        image::ImageFormat::Png => extension == "png" && mime_type == "image/png",
        image::ImageFormat::Jpeg => {
            matches!(extension, "jpg" | "jpeg") && mime_type == "image/jpeg"
        }
        image::ImageFormat::Gif => extension == "gif" && mime_type == "image/gif",
        image::ImageFormat::WebP => extension == "webp" && mime_type == "image/webp",
        _ => false,
    };
    identity_matches && image::load_from_memory_with_format(bytes, format).is_ok()
}

fn validate_file_type(extension: &str, mime_type: &str, bytes: &[u8]) -> bool {
    match extension {
        "pdf" => mime_type == "application/pdf" && bytes.starts_with(b"%PDF-"),
        "doc" => {
            mime_type == "application/msword"
                && bytes.starts_with(b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1")
        }
        "docx" => {
            mime_type == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                && is_ooxml_payload(bytes, "word/")
        }
        "pptx" => {
            mime_type == "application/vnd.openxmlformats-officedocument.presentationml.presentation"
                && is_ooxml_payload(bytes, "ppt/")
        }
        "xlsx" => {
            mime_type == "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
                && is_ooxml_payload(bytes, "xl/")
        }
        "csv" => {
            matches!(mime_type, "text/csv" | "text/plain") && std::str::from_utf8(bytes).is_ok()
        }
        "tsv" => {
            matches!(mime_type, "text/tab-separated-values" | "text/plain")
                && std::str::from_utf8(bytes).is_ok()
        }
        extension if is_supported_text_extension(extension) => {
            is_textual_mime(mime_type) && std::str::from_utf8(bytes).is_ok()
        }
        _ => false,
    }
}

fn is_ooxml_payload(bytes: &[u8], required_prefix: &str) -> bool {
    let reader = std::io::Cursor::new(bytes);
    let Ok(mut archive) = zip::ZipArchive::new(reader) else {
        return false;
    };
    (0..archive.len()).any(|index| {
        archive
            .by_index(index)
            .is_ok_and(|entry| entry.name().starts_with(required_prefix))
    })
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
    fn guidance_and_run_attachment_byte_budgets_are_independent() {
        let over_guidance = (0..5)
            .map(|index| attachment(index, MAX_GUIDANCE_ATTACHMENT_BYTES))
            .collect::<Vec<_>>();
        assert_eq!(
            validate_guidance_attachment_limits(&over_guidance, 0)
                .unwrap_err()
                .0,
            AgentSteerRunRejectionCode::AttachmentLimitExceeded
        );

        let one_byte = vec![attachment(0, 1)];
        assert_eq!(
            validate_guidance_attachment_limits(&one_byte, MAX_RUN_GUIDANCE_ATTACHMENT_BYTES,)
                .unwrap_err()
                .0,
            AgentSteerRunRejectionCode::AttachmentLimitExceeded
        );
        assert!(validate_guidance_attachment_limits(
            &one_byte,
            MAX_RUN_GUIDANCE_ATTACHMENT_BYTES - 1,
        )
        .is_ok());
    }
}
