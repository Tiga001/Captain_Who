/// Small incremental validation state. Historical payload bytes are validated once; identities
/// needed for cross-item invariants survive until the publication cursor is invalidated.
#[derive(Debug, Default)]
pub(crate) struct ConversationTraceValidationState {
    previous_sequence: Option<u64>,
    pending_call: Option<(String, String, bool)>,
    call_ids: BTreeSet<String>,
    command_call_ids: BTreeSet<String>,
    command_sessions: BTreeMap<String, (String, bool)>,
    context_compactions: BTreeMap<String, bool>,
    backend_state_ids: BTreeSet<String>,
    context_material_ids: BTreeSet<String>,
}

impl ConversationTraceValidationState {
    pub(crate) fn append<'a>(
        &mut self,
        items: impl IntoIterator<Item = &'a ConversationTurnTraceItem>,
        terminal_status: ConversationTurnTraceTerminalStatus,
    ) -> Result<(), String> {
        let Self {
            mut previous_sequence,
            mut pending_call,
            mut call_ids,
            mut command_call_ids,
            mut command_sessions,
            mut context_compactions,
            mut backend_state_ids,
            mut context_material_ids,
        } = std::mem::take(self);
        for item in items {
            let sequence = item.sequence();
            if previous_sequence.is_some_and(|previous| sequence <= previous) {
                return Err("conversation trace sequence must be strictly increasing".to_string());
            }
            previous_sequence = Some(sequence);

            match item {
                ConversationTurnTraceItem::ContextMaterial {
                    event_id,
                    material_kind,
                    content,
                    images,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err("context material cannot split a tool exchange".to_string());
                    }
                    validate_context_material(
                        event_id,
                        *material_kind,
                        content,
                        images,
                        *created_at,
                    )?;
                    if !context_material_ids.insert(event_id.clone()) {
                        return Err("context material event identity is duplicated".to_string());
                    }
                }
                ConversationTurnTraceItem::BackendState {
                    event_id,
                    content,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err("Backend state cannot split a tool exchange".to_string());
                    }
                    validate_backend_state(event_id, content, *created_at)?;
                    if !backend_state_ids.insert(event_id.clone()) {
                        return Err("Backend state event identity is duplicated".to_string());
                    }
                }
                ConversationTurnTraceItem::AssistantNarration {
                    content,
                    provider_turn_id,
                    first_tool_call_id,
                    ..
                } => {
                    validate_narration_binding(
                        provider_turn_id.as_deref(),
                        first_tool_call_id.as_deref(),
                    )?;
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace narration cannot split a tool exchange".to_string()
                        );
                    }
                    if content.trim().is_empty() {
                        return Err("conversation trace narration cannot be empty".to_string());
                    }
                    ensure_no_binary_text("assistant narration", content)?;
                }
                ConversationTurnTraceItem::UserGuidance {
                    guidance_id,
                    client_message_id,
                    content,
                    attachments,
                    folder_references,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace user guidance cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if guidance_id.trim().is_empty()
                        || client_message_id.trim().is_empty()
                        || (content.trim().is_empty()
                            && attachments.is_empty()
                            && folder_references.is_empty())
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace user guidance identity is invalid".to_string()
                        );
                    }
                    ensure_no_binary_text("user guidance", content)?;
                    let mut attachment_ids = BTreeSet::new();
                    for attachment in attachments {
                        if attachment.id.trim().is_empty()
                            || attachment.name.trim().is_empty()
                            || !attachment_ids.insert(attachment.id.as_str())
                        {
                            return Err("conversation trace user guidance attachment is invalid"
                                .to_string());
                        }
                        ensure_no_binary_text("user guidance attachment name", &attachment.name)?;
                        if let Some(mime_type) = &attachment.mime_type {
                            ensure_no_binary_text("user guidance attachment MIME type", mime_type)?;
                        }
                    }
                    for reference in folder_references {
                        reference.validate().map_err(|error| {
                            format!("conversation trace folder reference is invalid: {error}")
                        })?;
                    }
                }
                ConversationTurnTraceItem::WorkflowDelivery {
                    input_id,
                    instance_id,
                    workflow_name,
                    content,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some()
                        || input_id.trim().is_empty()
                        || instance_id.trim().is_empty()
                        || workflow_name.trim().is_empty()
                        || content.trim().is_empty()
                        || *created_at < 0
                    {
                        return Err("conversation workflow delivery has invalid identity or splits a tool exchange".into());
                    }
                    ensure_no_binary_text("workflow delivery", content)?;
                }
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    receipt_id,
                    message_id,
                    sender_agent_id,
                    sender_task_name,
                    sender_task_path,
                    content,
                    created_at,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace Agent mailbox delivery cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if receipt_id.trim().is_empty()
                        || message_id.trim().is_empty()
                        || sender_agent_id.trim().is_empty()
                        || sender_task_name.trim().is_empty()
                        || sender_task_path.trim().is_empty()
                        || content.trim().is_empty()
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace Agent mailbox delivery identity is invalid"
                                .to_string(),
                        );
                    }
                    ensure_no_binary_text("Agent mailbox delivery", content)?;
                }
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    provenance,
                    operation,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace contains an unresolved tool call".to_string()
                        );
                    }
                    if call_id.trim().is_empty() || !is_provider_safe_tool_name(tool) {
                        return Err("conversation trace tool call identity is invalid".to_string());
                    }
                    validate_tool_identity(tool, provenance)?;
                    if !call_ids.insert(call_id.clone()) {
                        return Err(format!(
                            "conversation trace contains duplicate tool call id: {call_id}"
                        ));
                    }
                    if tool == "run_command" {
                        command_call_ids.insert(call_id.clone());
                    }
                    ensure_no_binary_value("tool operation", operation)?;
                    pending_call = Some((
                        call_id.clone(),
                        tool.clone(),
                        matches!(provenance, AgentToolIdentity::Unregistered { .. }),
                    ));
                }
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    status,
                    success,
                    observation,
                    error,
                    archive,
                    ..
                } => {
                    let Some((expected_id, expected_tool, was_unregistered)) = pending_call.take()
                    else {
                        return Err(
                            "conversation trace tool result has no matching call".to_string()
                        );
                    };
                    if expected_id != *call_id || expected_tool != *tool {
                        return Err(
                            "conversation trace tool result does not match its call".to_string()
                        );
                    }
                    if *success != (*status == ConversationTraceToolResultStatus::Succeeded) {
                        return Err(
                            "conversation trace tool result success/status is inconsistent"
                                .to_string(),
                        );
                    }
                    if was_unregistered && *success {
                        return Err(
                            "unregistered conversation trace tool call cannot succeed".to_string()
                        );
                    }
                    ensure_no_binary_value("tool observation", observation)?;
                    if let Some(error) = error {
                        ensure_no_binary_text("tool error", error)?;
                    }
                    archive.validate()?;
                }
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase,
                    session_id,
                    call_id,
                    status,
                    exit_code,
                    archive,
                    created_at,
                    ..
                } => {
                    if pending_call
                        .as_ref()
                        .is_some_and(|(pending_call_id, _, _)| pending_call_id != call_id)
                    {
                        return Err(
                            "conversation trace command session lifecycle does not match the pending tool call"
                                .to_string(),
                        );
                    }
                    let session_suffix = session_id.strip_prefix("cmd_");
                    if session_suffix.is_none_or(|suffix| {
                        suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
                    }) || call_id.trim().is_empty()
                        || call_id.len() > 1_024
                        || call_id.chars().any(char::is_control)
                        || *created_at < 0
                    {
                        return Err(
                            "conversation trace command session lifecycle identity is invalid"
                                .to_string(),
                        );
                    }
                    if !command_call_ids.contains(call_id.as_str()) {
                        return Err(
                            "conversation trace command session lifecycle references a missing run_command call"
                                .to_string(),
                        );
                    }
                    match phase {
                        ConversationCommandSessionLifecyclePhase::Started => {
                            if !matches!(
                                *status,
                                AgentCommandSessionStatus::Starting
                                    | AgentCommandSessionStatus::Running
                            ) || exit_code.is_some()
                                || *archive != ConversationHistoryArchiveTraceMetadata::default()
                            {
                                return Err(
                                    "conversation trace command session started item is invalid"
                                        .to_string(),
                                );
                            }
                            if command_sessions
                                .insert(session_id.clone(), (call_id.clone(), false))
                                .is_some()
                            {
                                return Err(
                                    "conversation trace contains duplicate command session start"
                                        .to_string(),
                                );
                            }
                        }
                        ConversationCommandSessionLifecyclePhase::Terminal => {
                            if !status.is_terminal()
                                || (*status != AgentCommandSessionStatus::Exited
                                    && exit_code.is_some())
                            {
                                return Err(
                                    "conversation trace command session terminal item is invalid"
                                        .to_string(),
                                );
                            }
                            if let Some((started_call_id, terminal_seen)) =
                                command_sessions.get_mut(session_id.as_str())
                            {
                                if *started_call_id != *call_id || *terminal_seen {
                                    return Err(
                                        "conversation trace command session terminal identity is inconsistent"
                                            .to_string(),
                                    );
                                }
                                *terminal_seen = true;
                            } else {
                                // Startup reconciliation can append a terminal record to a trace
                                // whose pre-handoff start was not durably observed.
                                command_sessions
                                    .insert(session_id.clone(), (call_id.clone(), true));
                            }
                        }
                    }
                    archive.validate()?;
                }
                ConversationTurnTraceItem::ContextCompactionLifecycle {
                    phase,
                    operation_id,
                    outcome,
                    ..
                } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace context compaction cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if operation_id.trim().is_empty()
                        || operation_id.len() > 2_048
                        || operation_id.chars().any(char::is_control)
                    {
                        return Err(
                            "conversation trace context compaction identity is invalid".to_string()
                        );
                    }
                    match phase {
                        ConversationContextCompactionLifecyclePhase::Started => {
                            if outcome.is_some()
                                || context_compactions
                                    .insert(operation_id.clone(), false)
                                    .is_some()
                            {
                                return Err(
                                    "conversation trace context compaction start is invalid"
                                        .to_string(),
                                );
                            }
                        }
                        ConversationContextCompactionLifecyclePhase::Finished => {
                            let Some(finished) = context_compactions.get_mut(operation_id.as_str())
                            else {
                                return Err(
                                    "conversation trace context compaction finish is missing its start"
                                        .to_string(),
                                );
                            };
                            if *finished || outcome.is_none() {
                                return Err(
                                    "conversation trace context compaction finish is invalid"
                                        .to_string(),
                                );
                            }
                            *finished = true;
                        }
                    }
                }
                ConversationTurnTraceItem::RuntimeError { message, code, .. } => {
                    if pending_call.is_some() {
                        return Err(
                            "conversation trace Runtime error cannot split a tool exchange"
                                .to_string(),
                        );
                    }
                    if message.trim().is_empty() {
                        return Err("conversation trace Runtime error cannot be empty".to_string());
                    }
                    ensure_no_binary_text("Runtime error", message)?;
                    if let Some(code) = code {
                        if code.trim().is_empty() || code.len() > 128 {
                            return Err(
                                "conversation trace Runtime error code is invalid".to_string()
                            );
                        }
                        ensure_no_binary_text("Runtime error code", code)?;
                    }
                }
            }
        }
        if pending_call.is_some()
            && terminal_status != ConversationTurnTraceTerminalStatus::InProgress
        {
            return Err("conversation trace ends with an unresolved tool call".to_string());
        }
        if terminal_status.is_terminal() && context_compactions.values().any(|finished| !finished) {
            return Err(
                "terminal conversation trace contains an unfinished context compaction".to_string(),
            );
        }
        *self = Self {
            previous_sequence,
            pending_call,
            call_ids,
            command_call_ids,
            command_sessions,
            context_compactions,
            backend_state_ids,
            context_material_ids,
        };
        Ok(())
    }
}
