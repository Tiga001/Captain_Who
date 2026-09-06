fn checkpoint_tool_result_trace_item(
    sequence: u64,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> ConversationTurnTraceItem {
    let status = result_status(result);
    let success = status == ConversationTraceToolResultStatus::Succeeded;
    let (observation, result_redacted) =
        sanitize_runtime_tool_result(&call.tool, result.result.as_ref().unwrap_or(&Value::Null));
    let (error, error_redacted) = result
        .error
        .as_deref()
        .map(sanitize_runtime_text)
        .map(|(value, redacted)| (Some(value), redacted))
        .unwrap_or((None, false));
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation,
        approval_status: call.approval_status,
        error,
        truncated: result_redacted || error_redacted,
        archive: Default::default(),
    }
}

fn project_durable_trace_items(
    items: &[ConversationTurnTraceItem],
) -> (Vec<ConversationTurnTraceItem>, bool) {
    let mut projected = Vec::with_capacity(items.len());
    let mut operations = BTreeMap::<String, Value>::new();
    let mut failure_signatures = BTreeMap::<String, String>::new();
    let mut trace_truncated = false;

    for item in items {
        let projected_item = match item {
            ConversationTurnTraceItem::BackendState { .. } => item.clone(),
            ConversationTurnTraceItem::AssistantNarration {
                sequence,
                content,
                truncated,
            } => {
                let (content, projection_truncated) = project_narration(content);
                trace_truncated |= *truncated || projection_truncated;
                ConversationTurnTraceItem::AssistantNarration {
                    sequence: *sequence,
                    content,
                    truncated: *truncated || projection_truncated,
                }
            }
            ConversationTurnTraceItem::UserGuidance {
                sequence,
                guidance_id,
                client_message_id,
                content,
                attachments,
                created_at,
                truncated,
            } => {
                let (content, content_truncated) = project_user_guidance(content);
                let mut attachment_truncated = false;
                let attachments = attachments
                    .iter()
                    .map(|attachment| {
                        let (name, name_truncated) = project_attachment_text(&attachment.name);
                        let (mime_type, mime_truncated) = attachment
                            .mime_type
                            .as_deref()
                            .map(project_attachment_text)
                            .map(|(value, truncated)| (Some(value), truncated))
                            .unwrap_or((None, false));
                        attachment_truncated |= name_truncated || mime_truncated;
                        ConversationTraceAttachment {
                            id: attachment.id.clone(),
                            kind: attachment.kind,
                            name,
                            mime_type,
                            size_bytes: attachment.size_bytes,
                        }
                    })
                    .collect();
                let item_truncated = *truncated || content_truncated || attachment_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::UserGuidance {
                    sequence: *sequence,
                    guidance_id: guidance_id.clone(),
                    client_message_id: client_message_id.clone(),
                    content,
                    attachments,
                    created_at: *created_at,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence,
                receipt_id,
                message_id,
                sender_agent_id,
                sender_task_name,
                sender_task_path,
                kind,
                content,
                created_at,
                truncated,
            } => {
                let (content, content_truncated) = project_user_guidance(content);
                let item_truncated = *truncated || content_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::AgentMailboxDelivery {
                    sequence: *sequence,
                    receipt_id: receipt_id.clone(),
                    message_id: message_id.clone(),
                    sender_agent_id: sender_agent_id.clone(),
                    sender_task_name: sender_task_name.clone(),
                    sender_task_path: sender_task_path.clone(),
                    kind: *kind,
                    content,
                    created_at: *created_at,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
                provenance,
                operation,
                approval_status,
                truncated,
            } => {
                operations.insert(call_id.clone(), operation.clone());
                let operation = project_tool_call(tool, operation);
                let item_truncated = *truncated || operation.truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::ToolCall {
                    sequence: *sequence,
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    provenance: provenance.clone(),
                    operation: operation.value,
                    approval_status: *approval_status,
                    truncated: item_truncated,
                }
            }
            ConversationTurnTraceItem::ToolResult {
                sequence,
                call_id,
                tool,
                status,
                success,
                observation,
                approval_status,
                error,
                truncated,
                archive,
            } => {
                let (observation, projected_error, error_truncated) = project_tool_result(
                    tool,
                    operations.get(call_id),
                    observation,
                    error.as_deref(),
                );
                let mut projected_observation = observation.value;
                let mut projected_error = projected_error;
                let mut projection_truncated = observation.truncated || error_truncated;

                if !*success
                    && is_repeat_failure_eligible(tool)
                    && projected_observation
                        .get("repeatedFailure")
                        .and_then(Value::as_bool)
                        != Some(true)
                {
                    let signature = serde_json::to_string(&json!({
                        "tool": tool,
                        "status": status,
                        "observation": &projected_observation,
                        "error": &projected_error,
                    }))
                    .unwrap_or_default();
                    if let Some(first_call_id) = failure_signatures.get(&signature) {
                        projected_observation = json!({
                            "repeatedFailure": true,
                            "sameAsCallId": first_call_id,
                            "detail": "Unchanged failure omitted from conversation history."
                        });
                        projected_error = None;
                        projection_truncated = true;
                    } else {
                        failure_signatures.insert(signature, call_id.clone());
                    }
                }

                let item_truncated = *truncated || projection_truncated;
                let mut archive = archive.clone();
                // `truncated` can already be true because the Runtime checkpoint sanitizer
                // replaced binary/base64 content before this durable projection runs. Treat the
                // complete canonical item truncation bit as the archive fact as well; otherwise a
                // wait_agent ToolResult precommitted from the raw result and the later terminal
                // projection differ only in archive metadata.
                archive.history_projection_truncated |= item_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::ToolResult {
                    sequence: *sequence,
                    call_id: call_id.clone(),
                    tool: tool.clone(),
                    status: *status,
                    success: *success,
                    observation: projected_observation,
                    approval_status: *approval_status,
                    error: projected_error,
                    truncated: item_truncated,
                    archive,
                }
            }
            ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence,
                phase,
                session_id,
                call_id,
                status,
                exit_code,
                latest_sequence,
                output_truncated,
                archive,
                created_at,
            } => ConversationTurnTraceItem::CommandSessionLifecycle {
                sequence: *sequence,
                phase: *phase,
                session_id: session_id.clone(),
                call_id: call_id.clone(),
                status: *status,
                exit_code: *exit_code,
                latest_sequence: *latest_sequence,
                output_truncated: *output_truncated,
                archive: archive.clone(),
                created_at: *created_at,
            },
            ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence,
                phase,
                operation_id,
                outcome,
            } => ConversationTurnTraceItem::ContextCompactionLifecycle {
                sequence: *sequence,
                phase: *phase,
                operation_id: operation_id.clone(),
                outcome: *outcome,
            },
            ConversationTurnTraceItem::RuntimeError {
                sequence,
                message,
                recoverable,
                code,
                truncated,
            } => {
                let (message, message_truncated) = project_terminal_error(message);
                let (code, code_truncated) = code
                    .as_deref()
                    .map(sanitize_runtime_text)
                    .map(|(value, truncated)| (Some(value), truncated))
                    .unwrap_or((None, false));
                let item_truncated = *truncated || message_truncated || code_truncated;
                trace_truncated |= item_truncated;
                ConversationTurnTraceItem::RuntimeError {
                    sequence: *sequence,
                    message,
                    recoverable: *recoverable,
                    code,
                    truncated: item_truncated,
                }
            }
        };
        projected.push(projected_item);
    }

    (projected, trace_truncated)
}

/// Produces the canonical persisted trace item for a ToolResult.
///
/// Durable settlement verification uses this same projection so an audit receipt cannot be paired
/// with a trace item carrying different (or less complete) result/error evidence.
pub(crate) fn projected_tool_result_trace_item(
    sequence: u64,
    call: &AgentToolCall,
    result: &AgentToolResult,
) -> ConversationTurnTraceItem {
    let status = result_status(result);
    let success = status == ConversationTraceToolResultStatus::Succeeded;
    let (observation, error, error_truncated) = project_tool_result(
        &call.tool,
        Some(&call.args),
        result.result.as_ref().unwrap_or(&Value::Null),
        result.error.as_deref(),
    );
    let history_projection_truncated = observation.truncated || error_truncated;
    ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        status,
        success,
        observation: observation.value,
        approval_status: call.approval_status,
        error,
        truncated: history_projection_truncated,
        archive: ConversationHistoryArchiveTraceMetadata {
            history_projection_truncated,
            ..Default::default()
        },
    }
}

pub(crate) fn canonical_tool_result_for_context(result: &AgentToolResult) -> AgentToolResult {
    let mut canonical = result.clone();
    if let Some(value) = canonical.result.as_mut() {
        *value = sanitize_runtime_tool_result(&canonical.tool, value).0;
    }
    if let Some(error) = canonical.error.as_mut() {
        *error = sanitize_runtime_text(error).0;
    }
    canonical
}

pub(crate) fn render_tool_observation(result: &AgentToolResult) -> String {
    render_tool_observation_with_history_ref(result, None)
}

pub(crate) fn render_tool_observation_with_history_ref(
    result: &AgentToolResult,
    history_ref: Option<&crate::ContextHistoryRef>,
) -> String {
    render_tool_observation_with_projection(result, history_ref, None)
}

pub(crate) fn render_tool_observation_with_projection(
    result: &AgentToolResult,
    _history_ref: Option<&crate::ContextHistoryRef>,
    _archive: Option<&ConversationHistoryArchiveTraceMetadata>,
) -> String {
    let mut payload = result
        .result
        .clone()
        .unwrap_or_else(|| json!({ "status": if result.ok { "completed" } else { "failed" } }));
    if !result.ok {
        if let Some(error) = result
            .error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
        {
            match payload.as_object_mut() {
                Some(object)
                    if !object
                        .values()
                        .any(|value| value_contains_text(value, error)) =>
                {
                    object.insert("error".to_string(), Value::String(error.to_string()));
                }
                Some(_) => {}
                None => {
                    payload = json!({
                        "result": payload,
                        "error": error,
                    });
                }
            }
        }
    }
    serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
}

fn value_contains_text(value: &Value, expected: &str) -> bool {
    match value {
        Value::String(value) => value.trim() == expected,
        Value::Array(values) => values
            .iter()
            .any(|value| value_contains_text(value, expected)),
        Value::Object(values) => values
            .values()
            .any(|value| value_contains_text(value, expected)),
        _ => false,
    }
}

fn result_status(result: &AgentToolResult) -> ConversationTraceToolResultStatus {
    let value = result.result.as_ref().unwrap_or(&Value::Null);
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if status.contains("reject") {
        ConversationTraceToolResultStatus::Rejected
    } else if status.contains("conflict") {
        ConversationTraceToolResultStatus::Conflict
    } else if status.contains("cancel")
        || value
            .get("cancelled")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        ConversationTraceToolResultStatus::Cancelled
    } else if result.ok && result.error.is_none() && !status.contains("fail") {
        ConversationTraceToolResultStatus::Succeeded
    } else {
        ConversationTraceToolResultStatus::Failed
    }
}

fn sanitize_optional_text(value: Option<&str>) -> (Option<String>, bool) {
    value
        .map(sanitize_text)
        .map(|(value, redacted)| (Some(value), redacted))
        .unwrap_or((None, false))
}

/// Maximum size of the canonical model-visible wrapper for a Host-authenticated collaboration
/// fact. The durable trace stores the same envelope shape at the smaller history budget while the
/// Mailbox retains the original payload unchanged.
pub(crate) const AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES: usize = 32 * 1_024;

/// Produces the canonical model-visible representation used by the live Runtime, durable model
/// log and checkpoint/resume. The durable trace uses the same encoder with its history budget.
pub(crate) fn project_agent_mailbox_model_envelope(
    sender_agent_id: &str,
    sender_task_name: &str,
    sender_task_path: &str,
    kind: crate::AgentMailboxKind,
    payload: &str,
) -> Result<(String, bool), String> {
    project_agent_mailbox_envelope_with_budget(
        sender_agent_id,
        sender_task_name,
        sender_task_path,
        kind,
        payload,
        AGENT_MAILBOX_MODEL_ENVELOPE_MAX_BYTES,
    )
}

fn project_agent_mailbox_envelope_with_budget(
    sender_agent_id: &str,
    sender_task_name: &str,
    sender_task_path: &str,
    kind: crate::AgentMailboxKind,
    payload: &str,
    maximum_bytes: usize,
) -> Result<(String, bool), String> {
    const SUFFIX: &str = "\n...[agent mailbox payload truncated]";
    let encode = |payload: &str, truncated: bool| {
        serde_json::to_string(&serde_json::json!({
            "type": "agent_collaboration_input",
            "origin": "agent",
            "isHuman": false,
            "senderAgentId": sender_agent_id,
            "senderTaskName": sender_task_name,
            "senderTaskPath": sender_task_path,
            "kind": kind.as_str(),
            "payload": payload,
            "payloadTruncated": truncated,
        }))
        .map_err(|error| format!("cannot encode Agent collaboration envelope: {error}"))
    };
    let full = encode(payload, false)?;
    if full.len() <= maximum_bytes {
        return Ok((full, false));
    }
    let boundaries = std::iter::once(0)
        .chain(payload.char_indices().map(|(index, _)| index).skip(1))
        .chain(std::iter::once(payload.len()))
        .collect::<Vec<_>>();
    let mut low = 0usize;
    let mut high = boundaries.len().saturating_sub(1);
    let mut best = String::new();
    while low <= high {
        let middle = low + (high - low) / 2;
        let boundary = boundaries[middle];
        let projected = format!("{}{}", &payload[..boundary], SUFFIX);
        let encoded = encode(&projected, true)?;
        if encoded.len() <= maximum_bytes {
            best = encoded;
            low = middle.saturating_add(1);
        } else if middle == 0 {
            break;
        } else {
            high = middle.saturating_sub(1);
        }
    }
    if best.is_empty() {
        return Err("Agent collaboration identity exceeds the model envelope budget".to_string());
    }
    Ok((best, true))
}

fn sanitize_text(value: &str) -> (String, bool) {
    sanitize_runtime_text(value)
}

fn sanitize_value(value: &Value) -> (Value, bool) {
    sanitize_runtime_value(value)
}

fn ensure_no_binary_text(label: &str, value: &str) -> Result<(), String> {
    if sanitize_text(value).1 {
        return Err(format!(
            "conversation trace {label} contains binary material"
        ));
    }
    Ok(())
}

fn ensure_no_binary_value(label: &str, value: &Value) -> Result<(), String> {
    // Durable values may intentionally retain a binary-named field with the canonical omission
    // marker. Validate whether sanitization would change the value instead of treating the
    // already-safe marker as fresh binary material.
    if sanitize_value(value).0 != *value {
        return Err(format!(
            "conversation trace {label} contains binary material"
        ));
    }
    Ok(())
}

fn is_provider_safe_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}
