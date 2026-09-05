use super::*;

pub fn agent_event_notification(event: AgentEvent) -> Value {
    let mut params = json!(event);
    redact_renderer_mcp_binding_fields(&mut params);
    json!({
        "jsonrpc": "2.0",
        "method": AGENT_EVENT_NOTIFICATION_METHOD,
        "params": params
    })
}

/// Wraps one ordinary safe Agent event with the exact child identities required by an observer.
/// The nested event projection is deliberately identical to `agent.event`; this is routing
/// authority, not a second runtime event or presentation model.
pub(crate) fn child_observer_event_notification(
    identity: &AgentCollaborationIdentity,
    run_id: &str,
    assistant_message_id: &str,
    event: AgentEvent,
) -> Value {
    let safe = agent_event_notification(event);
    json!({
        "jsonrpc": "2.0",
        "method": mycopilot_protocol_rs::AGENT_COLLABORATION_OBSERVER_EVENT_NOTIFICATION_METHOD,
        "params": {
            "schemaVersion": mycopilot_protocol_rs::AGENT_COLLABORATION_SCHEMA_VERSION,
            "rootAgentId": identity.root_agent_id,
            "rootConversationId": identity.root_conversation_id,
            "agentId": identity.agent_id,
            "conversationId": identity.conversation_id,
            "runId": run_id,
            "assistantMessageId": assistant_message_id,
            "event": safe["params"].clone(),
        }
    })
}

/// Emits the legacy root event plus the identity-rich observer event for a trusted child Turn.
pub(crate) fn emit_agent_event_notifications(
    notifications: &crate::application::agent::CoreServerNotificationSender,
    collaboration_identity: Option<&AgentCollaborationIdentity>,
    run_id: &str,
    assistant_message_id: &str,
    event: AgentEvent,
) {
    let _ = notifications.send(agent_event_notification(event.clone()));
    if let Some(identity) = collaboration_identity {
        let _ = notifications.send(child_observer_event_notification(
            identity,
            run_id,
            assistant_message_id,
            event,
        ));
    }
}

/// Removes Host-only approval binding material before a value crosses the
/// core-server → Main/Renderer notification boundary.
///
/// The arguments digest is required for durable Host revalidation, but exposing
/// it would let an untrusted UI consumer brute-force low-entropy scalar
/// arguments. MCP actions already contain only the empty-object call
/// projection; this final guard strips the remaining execution-only
/// fingerprint from every nested action (including `done.proposedActions`). Command inputs,
/// runtime bindings and managed Office script transaction bindings remain authoritative in the
/// backend checkpoint, but the Renderer only needs the command/reason needed to make an approval
/// decision. Omitting those frozen bindings also prevents Host-only object and staging paths from
/// crossing the process boundary.
pub(super) fn redact_renderer_mcp_binding_fields(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                redact_renderer_mcp_binding_fields(value);
            }
        }
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("mcp_tool_call") {
                if let Some(identity) = object
                    .get_mut("approval")
                    .and_then(Value::as_object_mut)
                    .and_then(|approval| approval.get_mut("identity"))
                    .and_then(Value::as_object_mut)
                {
                    identity.remove("argumentsDigest");
                }
            }
            if object.get("type").and_then(Value::as_str) == Some("command") {
                if let Some(command) = object.get_mut("command").and_then(Value::as_object_mut) {
                    command.remove("inputs");
                    command.remove("runtimeBinding");
                    command.remove("managedOfficeScript");
                }
            }
            // `fileChange` is used both by a proposed-action value (`type=file_change`) and by
            // the standalone `file_change_proposed` event. Do not key this privacy boundary off
            // either outer discriminator: every Renderer-visible `fileChange` value has the same
            // public DTO, and `execution` is always Host-only durable recovery authority.
            if let Some(file_change) = object.get_mut("fileChange").and_then(Value::as_object_mut) {
                file_change.remove("execution");
            }
            for value in object.values_mut() {
                redact_renderer_mcp_binding_fields(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

pub(crate) fn resolve_project(
    storage: &StorageService,
    project_id: Option<&str>,
) -> Result<Option<ProjectRecord>, String> {
    let Some(project_id) = project_id else {
        return Ok(None);
    };

    let project = storage
        .load_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("未找到项目：{project_id}"))?;
    if project
        .path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .is_none()
    {
        return Err(format!(
            "项目「{}」没有绑定本地 workspace 路径，请重新选择项目目录。",
            project.name
        ));
    }
    Ok(Some(project))
}

/// Existing conversation history is bound to the workspace that produced it. A normal turn may
/// omit that project id, but it may not migrate the conversation to another project implicitly.
/// Project migration needs a dedicated operation that can validate and update every dependent
/// artifact atomically.
pub(crate) fn resolve_conversation_project_id(
    existing: Option<&ChatConversationRecord>,
    requested_project_id: Option<String>,
) -> Result<Option<String>, String> {
    let Some(existing) = existing else {
        return Ok(requested_project_id);
    };
    let stored_project_id = normalized_optional(existing.project_id.as_deref());
    if requested_project_id.is_some() && requested_project_id != stored_project_id {
        return Err(format!(
            "会话 `{}` 已绑定到另一个项目；普通消息不能迁移会话项目。",
            existing.id
        ));
    }
    Ok(stored_project_id)
}

pub(crate) fn message_attachments_from_input(
    attachments: &[AgentInputAttachment],
    created_at: i64,
) -> Vec<ChatMessageAttachmentRecord> {
    attachments
        .iter()
        .map(|attachment| {
            let preview_data = if attachment.kind == AgentInputAttachmentKind::Image
                && attachment.encoding == AgentInputAttachmentEncoding::Base64
                && attachment
                    .mime_type
                    .as_deref()
                    .is_some_and(|mime_type| mime_type.starts_with("image/"))
            {
                Some(attachment.data.clone())
            } else {
                None
            };

            ChatMessageAttachmentRecord {
                id: safe_path_component(&attachment.id, "attachment"),
                kind: input_attachment_kind_label(attachment.kind).to_string(),
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: attachment.size_bytes,
                preview_mime_type: preview_data.as_ref().and(attachment.mime_type.clone()),
                preview_data,
                created_at,
            }
        })
        .collect()
}

pub(crate) fn agent_prompt_preferences_from_record(
    record: AgentPromptPreferencesRecord,
) -> AgentPromptPreferences {
    AgentPromptPreferences {
        work_mode: Some(match record.work_mode.as_str() {
            "general" => AgentPromptWorkMode::General,
            _ => AgentPromptWorkMode::Coding,
        }),
        tone: Some(match record.tone.as_str() {
            "friendly" => AgentPromptTone::Friendly,
            _ => AgentPromptTone::Pragmatic,
        }),
        detail_level: Some(match record.detail_level.as_str() {
            "low" => AgentPromptDetailLevel::Low,
            "high" => AgentPromptDetailLevel::High,
            _ => AgentPromptDetailLevel::Medium,
        }),
        custom_instructions: normalized_optional(Some(&record.custom_instructions)),
        updated_at: Some(record.updated_at),
        automation_execution_context: None,
    }
}

pub(crate) fn input_attachment_kind_label(kind: AgentInputAttachmentKind) -> &'static str {
    match kind {
        AgentInputAttachmentKind::File => "file",
        AgentInputAttachmentKind::Image => "image",
    }
}

pub(crate) fn search_mode_from_storage(value: &str) -> AgentSearchMode {
    match value {
        "disabled" => AgentSearchMode::Disabled,
        "tavily" => AgentSearchMode::Tavily,
        _ => AgentSearchMode::Auto,
    }
}

pub(crate) fn normalized_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

pub(crate) fn non_empty(value: String) -> Option<String> {
    let value = value.trim().to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

pub(crate) fn upsert_message(messages: &mut Vec<ChatMessageRecord>, next: ChatMessageRecord) {
    if let Some(existing) = messages.iter_mut().find(|message| message.id == next.id) {
        *existing = next;
    } else {
        messages.push(next);
    }
}

pub(crate) fn status_for_run(status: AgentRunStatus) -> Option<&'static str> {
    match status {
        AgentRunStatus::Completed | AgentRunStatus::Cancelled => Some("sent"),
        AgentRunStatus::WaitingForApproval
        | AgentRunStatus::WaitingForUserInput
        | AgentRunStatus::Running
        | AgentRunStatus::Idle => Some("pending"),
        AgentRunStatus::Failed => Some("error"),
    }
}

pub(crate) fn run_status_label(status: AgentRunStatus) -> &'static str {
    match status {
        AgentRunStatus::Idle => "idle",
        AgentRunStatus::Running => "running",
        AgentRunStatus::WaitingForApproval => "waiting_for_approval",
        AgentRunStatus::WaitingForUserInput => "waiting_for_user_input",
        AgentRunStatus::Completed => "completed",
        AgentRunStatus::Failed => "failed",
        AgentRunStatus::Cancelled => "cancelled",
    }
}

pub(crate) fn is_terminal_run_status(status: AgentRunStatus) -> bool {
    matches!(
        status,
        AgentRunStatus::Completed | AgentRunStatus::Failed | AgentRunStatus::Cancelled
    )
}
