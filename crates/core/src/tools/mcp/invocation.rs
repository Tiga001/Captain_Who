//! Invocation validation, bounded result projection, persistence, and lifecycle events.

use super::*;

const MCP_APPROVAL_REJECTED_CODE: &str = "mcp.approval_rejected";
const MCP_REJECTION_WITHOUT_FEEDBACK_MESSAGE: &str = "The user explicitly rejected this external MCP tool call. Its arguments and call reason were already validated, but the historical arguments are redacted for privacy. The MCP server was not called. Do not retry this call or an equivalent call unless the user gives a new explicit instruction.";
const MCP_REJECTION_WITH_FEEDBACK_MESSAGE: &str = "The user explicitly rejected this external MCP tool call. Its arguments and call reason were already validated, but the historical arguments are redacted for privacy. The MCP server was not called. Follow userFeedback, and do not repeat the same call unchanged.";

fn invocation_result_value(
    provenance: &AgentMcpToolProvenance,
    result: &McpToolInvocationResult,
) -> AgentResult<Value> {
    let (content, text_truncated, blocks_truncated) = bounded_content(&result.content);
    let content = serde_json::to_value(content)
        .map_err(|_| AgentError::new("MCP tool content could not be normalized."))?;
    let mut object = Map::from_iter([
        ("schemaVersion".to_string(), json!(1)),
        ("type".to_string(), json!("mcp_tool")),
        ("external".to_string(), json!(true)),
        ("status".to_string(), json!("completed")),
        (
            "outcome".to_string(),
            json!(if result.is_error {
                "tool_error"
            } else {
                "succeeded"
            }),
        ),
        ("dispatchCertainty".to_string(), json!("response_received")),
        ("content".to_string(), content),
        ("isError".to_string(), json!(result.is_error)),
        (
            "provenance".to_string(),
            serde_json::to_value(provenance)
                .map_err(|_| AgentError::new("MCP provenance could not be normalized."))?,
        ),
    ]);
    let mut structured_truncated = false;
    if let Some(structured) = &result.structured_content {
        if !json_structure_within_limits(
            structured,
            MAX_MCP_STRUCTURED_RESULT_DEPTH,
            MAX_MCP_STRUCTURED_RESULT_NODES,
        ) {
            structured_truncated = true;
            object.insert(
                "structuredContent".to_string(),
                json!({
                    "_mycopilot": {
                        "omitted": true,
                        "reason": "structured_content_structure_limit",
                    }
                }),
            );
        } else {
            let structured_bytes = serde_json::to_vec(structured)
                .map_err(|_| AgentError::new("MCP structured content could not be normalized."))?;
            if structured_bytes.len() <= MAX_MCP_STRUCTURED_RESULT_BYTES {
                object.insert("structuredContent".to_string(), structured.clone());
            } else {
                structured_truncated = true;
                object.insert(
                    "structuredContent".to_string(),
                    json!({
                        "_mycopilot": {
                            "omitted": true,
                            "reason": "structured_content_limit",
                            "originalBytes": structured_bytes.len(),
                        }
                    }),
                );
            }
        }
    }
    if result.truncated_at_source || text_truncated || blocks_truncated || structured_truncated {
        object.insert("truncatedAtSource".to_string(), Value::Bool(true));
        object.insert(
            "diagnostics".to_string(),
            json!({
                "textTruncated": text_truncated,
                "contentBlocksTruncated": blocks_truncated,
                "structuredContentTruncated": structured_truncated,
                "upstreamContentTruncated": result.truncated_at_source,
            }),
        );
    } else {
        object.insert("truncatedAtSource".to_string(), Value::Bool(false));
    }
    Ok(Value::Object(object))
}

fn json_structure_within_limits(value: &Value, max_depth: usize, max_nodes: usize) -> bool {
    let mut pending = vec![(value, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        if depth > max_depth {
            return false;
        }
        let Some(next_nodes) = nodes.checked_add(1) else {
            return false;
        };
        nodes = next_nodes;
        if nodes > max_nodes {
            return false;
        }
        match value {
            Value::Array(array) => pending.extend(
                array
                    .iter()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            ),
            Value::Object(object) => pending.extend(
                object
                    .values()
                    .rev()
                    .map(|child| (child, depth.saturating_add(1))),
            ),
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }
    true
}

fn bounded_content(content: &[McpToolContentBlock]) -> (Vec<McpToolContentBlock>, bool, bool) {
    let mut remaining_text_bytes = MAX_MCP_TEXT_RESULT_BYTES;
    let mut output = Vec::new();
    let mut text_truncated = false;
    let blocks_truncated = content.len() > MAX_MCP_CONTENT_BLOCKS;

    for block in content.iter().take(MAX_MCP_CONTENT_BLOCKS) {
        match block {
            McpToolContentBlock::Text { text } => {
                if remaining_text_bytes == 0 {
                    text_truncated = true;
                    continue;
                }
                if text.len() <= remaining_text_bytes {
                    output.push(block.clone());
                    remaining_text_bytes -= text.len();
                } else {
                    let retained = truncate_utf8(text, remaining_text_bytes);
                    output.push(McpToolContentBlock::Text {
                        text: format!(
                            "{retained}\n[MCP text output truncated by host; original block was {} bytes.]",
                            text.len()
                        ),
                    });
                    remaining_text_bytes = 0;
                    text_truncated = true;
                }
            }
            McpToolContentBlock::Omitted { .. } => output.push(block.clone()),
        }
    }
    (output, text_truncated, blocks_truncated)
}

pub(super) fn catalog_within_runtime_budget(tools: &[McpAgentToolDescriptor]) -> bool {
    if tools.len() > MCP_RUNTIME_MAX_TOOL_DEFINITIONS {
        return false;
    }
    let mut bytes = 0_usize;
    for tool in tools {
        let Some(next) = bytes
            .checked_add(tool.provenance.model_tool_name.len())
            .and_then(|value| value.checked_add(tool.provenance.raw_tool_name.len()))
            .and_then(|value| value.checked_add(tool.description.as_ref().map_or(0, String::len)))
        else {
            return false;
        };
        bytes = next;
        for schema in std::iter::once(&tool.input_schema).chain(tool.output_schema.as_ref()) {
            let Ok(encoded) = serde_json::to_vec(schema) else {
                return false;
            };
            let Some(next) = bytes.checked_add(encoded.len()) else {
                return false;
            };
            bytes = next;
            if bytes > MCP_RUNTIME_MAX_CATALOG_BYTES {
                return false;
            }
        }
    }
    bytes <= MCP_RUNTIME_MAX_CATALOG_BYTES
}

pub(super) fn project_mcp_tool_call(
    call: &crate::protocol::AgentToolCall,
) -> crate::protocol::AgentToolCall {
    let mut projected = call.clone();
    // MCP arguments are execution-only until the Host has a Secret Store and an explicit
    // per-tool persistence policy. Server-authored schemas and field names cannot prove that a
    // scalar is safe to retain in traces, events, durable model history, or approval checkpoints.
    projected.args = Value::Object(Map::new());
    projected.reason = None;
    projected
}

/// Separates the Host-only display reason from the exact arguments sent to the MCP Server.
///
/// The reserved field is injected only into the Provider-facing schema. It is required, bounded,
/// removed before digest/sealing/schema validation, and never reaches the external Server.
pub(super) fn split_model_mcp_arguments(arguments: &Value) -> AgentResult<(Value, String)> {
    let Value::Object(mut server_arguments) = arguments.clone() else {
        return Err(AgentError::structured(
            "mcp.invalid_tool_arguments",
            "MCP tool arguments must be a JSON object.",
            json!({
                "type": "mcp_tool_arguments",
                "code": "rootMustBeObject",
            }),
        ));
    };
    let reason = match server_arguments.remove(MCP_CALL_REASON_FIELD) {
        Some(Value::String(reason)) => reason,
        Some(_) => {
            return Err(AgentError::structured(
                "mcp.call_reason_required",
                "The MCP Tool Call reason must be a string.",
                json!({
                    "type": "mcp_tool_arguments",
                    "code": "callReasonRequired",
                    "retryable": false,
                }),
            ));
        }
        // Compatibility fallback for providers that omit a newly required schema property. This
        // text is Host-owned and reveals no argument value.
        None => "Use this MCP tool to complete the current request.".to_string(),
    };
    let reason = reason
        .chars()
        .map(|character| {
            if is_unsafe_mcp_display_character(character) {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let reason = truncate_utf8(reason.trim(), MAX_MCP_CALL_REASON_BYTES);
    if reason.is_empty() {
        return Err(AgentError::structured(
            "mcp.call_reason_required",
            "The MCP Tool Call reason must not be empty.",
            json!({
                "type": "mcp_tool_arguments",
                "code": "callReasonRequired",
                "retryable": false,
            }),
        ));
    }
    let server_arguments = Value::Object(server_arguments);
    validate_mcp_argument_shape(&server_arguments)?;
    Ok((server_arguments, reason))
}

fn is_unsafe_mcp_display_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'
                | '\u{2066}'..='\u{2069}'
                | '\u{feff}'
        )
}

/// Computes the stable digest used to bind a Host-sealed MCP argument payload.
///
/// Object keys are recursively sorted before SHA-256 so semantically identical JSON objects bind
/// to the same value regardless of provider key ordering.
pub fn mcp_tool_arguments_digest(arguments: &Value) -> AgentResult<String> {
    validate_mcp_argument_shape(arguments)?;
    let canonical = canonical_json(arguments);
    let bytes = serde_json::to_vec(&canonical)
        .map_err(|_| AgentError::new("MCP tool arguments could not be canonicalized."))?;
    Ok(lower_hex(&Sha256::digest(bytes)))
}

fn validate_mcp_argument_shape(arguments: &Value) -> AgentResult<()> {
    if !arguments.is_object() {
        return Err(AgentError::structured(
            "mcp.invalid_arguments",
            "MCP tool arguments must be a JSON object.",
            json!({"retryable": false}),
        ));
    }

    let mut pending = vec![(arguments, 1_usize)];
    let mut nodes = 0_usize;
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_MCP_ARGUMENT_DEPTH {
            return Err(AgentError::structured(
                "mcp.arguments_limit_exceeded",
                "MCP tool arguments exceed the allowed nesting depth.",
                json!({"retryable": false, "limit": "depth"}),
            ));
        }
        nodes = nodes.checked_add(1).ok_or_else(|| {
            AgentError::structured(
                "mcp.arguments_limit_exceeded",
                "MCP tool arguments exceed the allowed node count.",
                json!({"retryable": false, "limit": "nodes"}),
            )
        })?;
        if nodes > MAX_MCP_ARGUMENT_NODES {
            return Err(AgentError::structured(
                "mcp.arguments_limit_exceeded",
                "MCP tool arguments exceed the allowed node count.",
                json!({"retryable": false, "limit": "nodes"}),
            ));
        }

        match value {
            Value::Array(values) => {
                pending.extend(
                    values
                        .iter()
                        .rev()
                        .map(|value| (value, depth.saturating_add(1))),
                );
            }
            Value::Object(object) => {
                if object.len() > MAX_MCP_ARGUMENT_OBJECT_PROPERTIES {
                    return Err(AgentError::structured(
                        "mcp.arguments_limit_exceeded",
                        "MCP tool arguments exceed the allowed object property count.",
                        json!({"retryable": false, "limit": "properties"}),
                    ));
                }
                pending.extend(
                    object
                        .values()
                        .rev()
                        .map(|value| (value, depth.saturating_add(1))),
                );
            }
            Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
        }
    }

    let encoded = serde_json::to_vec(arguments).map_err(|_| {
        AgentError::structured(
            "mcp.invalid_arguments",
            "MCP tool arguments could not be measured safely.",
            json!({"retryable": false}),
        )
    })?;
    if encoded.len() > MAX_MCP_RAW_ARGUMENT_BYTES {
        return Err(AgentError::structured(
            "mcp.arguments_limit_exceeded",
            "MCP tool arguments exceed the allowed byte size.",
            json!({"retryable": false, "limit": "bytes"}),
        ));
    }
    Ok(())
}

/// Revalidates raw arguments recovered by the Host against a public approval identity.
pub fn validate_mcp_approval_arguments(
    approval: &AgentMcpToolApproval,
    arguments: &Value,
) -> AgentResult<()> {
    validate_mcp_tool_approval(approval)?;
    if !arguments.is_object()
        || mcp_tool_arguments_digest(arguments)? != approval.identity.arguments_digest
    {
        return Err(AgentError::structured(
            "mcp.approval_arguments_mismatch",
            "The recovered MCP arguments do not match the approved invocation.",
            json!({
                "type": "mcp_approval",
                "code": "argumentsDigestMismatch",
                "retryable": false,
            }),
        ));
    }
    Ok(())
}

/// Converts an approved MCP response into the shared Agent ToolResult contract.
///
/// The Host remains responsible for one-time grant consumption and lifecycle journaling; this
/// helper applies the same bounded result projection used by the Agent Tool adapter.
pub fn mcp_tool_result_from_approved_invocation(
    approval: &AgentMcpToolApproval,
    result: &McpToolInvocationResult,
) -> AgentResult<AgentToolResult> {
    validate_mcp_tool_approval(approval)?;
    let value = invocation_result_value(&approval.identity.provenance, result)?;
    Ok(AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.provenance.model_tool_name.clone(),
        ok: !result.is_error,
        result: Some(value),
        error: result
            .is_error
            .then(|| "The MCP server reported a tool execution error.".to_string()),
    })
}

/// Converts a user's explicit MCP rejection into an ordinary, paired ToolResult.
///
/// Rejection happens before the external dispatch boundary. It is therefore a successful Host
/// decision result (`ok = true`), not a transport failure and never `OutcomeUnknown`. The optional
/// feedback is bounded Host input for the immediate model continuation; the durable projection
/// records only whether feedback was present.
pub fn mcp_tool_result_from_rejected_approval(
    approval: &AgentMcpToolApproval,
    feedback: Option<&str>,
) -> AgentResult<AgentToolResult> {
    validate_mcp_tool_approval(approval)?;
    let feedback = feedback
        .map(|feedback| {
            feedback
                .chars()
                .map(|character| {
                    if is_unsafe_mcp_display_character(character) {
                        ' '
                    } else {
                        character
                    }
                })
                .collect::<String>()
        })
        .map(|feedback| truncate_utf8(feedback.trim(), MAX_MCP_CALL_REASON_BYTES))
        .filter(|feedback| !feedback.is_empty());
    let mut value = Map::from_iter([
        ("schemaVersion".to_string(), json!(1)),
        ("type".to_string(), json!("mcp_tool")),
        ("external".to_string(), json!(true)),
        ("status".to_string(), json!("rejected")),
        ("outcome".to_string(), json!("rejected")),
        (
            "dispatchCertainty".to_string(),
            json!("definitely_not_dispatched"),
        ),
        ("isError".to_string(), json!(false)),
        ("code".to_string(), json!(MCP_APPROVAL_REJECTED_CODE)),
        ("retryable".to_string(), json!(false)),
    ]);
    if let Some(feedback) = feedback {
        value.insert("userFeedback".to_string(), Value::String(feedback));
    }
    Ok(AgentToolResult {
        exact_archive_file: None,
        call_id: approval.identity.call_id.clone(),
        tool: approval.identity.provenance.model_tool_name.clone(),
        ok: true,
        result: Some(Value::Object(value)),
        error: None,
    })
}

/// Produces the bounded current-model view of a typed MCP result.
///
/// Routing and audit provenance remain Host-owned. The model needs the Server's bounded content
/// and the wrapper outcome, but not config, catalog, or schema digests.
pub fn mcp_tool_result_model_projection(result: &AgentToolResult) -> AgentToolResult {
    let mut projected = crate::tools::canonical_tool_result_for_context(result);
    if let Some(object) = projected.result.as_mut().and_then(Value::as_object_mut) {
        object.remove("provenance");
        if is_explicit_user_rejected_mcp_result(object) {
            let has_feedback = object
                .get("userFeedback")
                .and_then(Value::as_str)
                .is_some_and(|feedback| !feedback.trim().is_empty());
            object.insert("decisionBy".to_string(), json!("user"));
            object.insert("executionAttempted".to_string(), json!(false));
            object.insert("argumentsValidated".to_string(), json!(true));
            object.insert("callReasonAccepted".to_string(), json!(true));
            object.insert("argumentsInHistoryRedacted".to_string(), json!(true));
            object.insert("code".to_string(), json!(MCP_APPROVAL_REJECTED_CODE));
            object.insert("retryable".to_string(), json!(false));
            object.insert(
                "retryPolicy".to_string(),
                json!(if has_feedback {
                    "follow_user_feedback_without_repeating_same_call"
                } else {
                    "new_explicit_user_instruction_required"
                }),
            );
            object.insert(
                "message".to_string(),
                json!(if has_feedback {
                    MCP_REJECTION_WITH_FEEDBACK_MESSAGE
                } else {
                    MCP_REJECTION_WITHOUT_FEEDBACK_MESSAGE
                }),
            );
        }
    }
    projected
}

fn is_explicit_user_rejected_mcp_result(value: &Map<String, Value>) -> bool {
    value.get("schemaVersion").and_then(Value::as_u64) == Some(1)
        && value.get("type").and_then(Value::as_str) == Some("mcp_tool")
        && value.get("external").and_then(Value::as_bool) == Some(true)
        && value.get("status").and_then(Value::as_str) == Some("rejected")
        && value.get("outcome").and_then(Value::as_str) == Some("rejected")
        && value.get("dispatchCertainty").and_then(Value::as_str)
            == Some("definitely_not_dispatched")
        && value.get("isError").and_then(Value::as_bool) == Some(false)
}

fn projected_mcp_status(
    result: &AgentToolResult,
    value: Option<&Map<String, Value>>,
) -> &'static str {
    match value
        .and_then(|value| value.get("status"))
        .and_then(Value::as_str)
    {
        Some("completed") => "completed",
        Some("rejected") => "rejected",
        Some("cancelled") => "cancelled",
        Some("expired") => "expired",
        Some("payload_unavailable") => "payload_unavailable",
        Some("policy_denied") => "policy_denied",
        Some("outcome_unknown") => "outcome_unknown",
        Some("failed") => "failed",
        _ if value
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            == Some("mcp.tool_outcome_unknown") =>
        {
            "outcome_unknown"
        }
        _ if result.ok => "completed",
        _ => "failed",
    }
}

fn projected_mcp_outcome(
    result: &AgentToolResult,
    value: Option<&Map<String, Value>>,
    status: &str,
    is_error: bool,
) -> &'static str {
    match value
        .and_then(|value| value.get("outcome"))
        .and_then(Value::as_str)
    {
        Some("succeeded") => "succeeded",
        Some("tool_error") => "tool_error",
        Some("output_too_large") => "output_too_large",
        Some("transport_error") => "transport_error",
        Some("timed_out") => "timed_out",
        Some("cancelled") => "cancelled",
        Some("rejected") => "rejected",
        Some("expired") => "expired",
        Some("payload_unavailable") => "payload_unavailable",
        Some("policy_denied") => "policy_denied",
        Some("outcome_unknown") => "outcome_unknown",
        _ => match status {
            "completed" if is_error || !result.ok => "tool_error",
            "completed" => "succeeded",
            "rejected" => "rejected",
            "cancelled" => "cancelled",
            "expired" => "expired",
            "payload_unavailable" => "payload_unavailable",
            "policy_denied" => "policy_denied",
            "outcome_unknown" => "outcome_unknown",
            _ => "transport_error",
        },
    }
}

fn projected_mcp_dispatch_certainty(
    value: Option<&Map<String, Value>>,
    status: &str,
) -> &'static str {
    match value
        .and_then(|value| value.get("dispatchCertainty"))
        .and_then(Value::as_str)
    {
        Some("definitely_not_dispatched") => "definitely_not_dispatched",
        Some("possibly_dispatched") => "possibly_dispatched",
        Some("response_received") => "response_received",
        _ => match status {
            "completed" => "response_received",
            "outcome_unknown" => "possibly_dispatched",
            _ => "definitely_not_dispatched",
        },
    }
}

fn safe_mcp_result_code(value: Option<&Map<String, Value>>) -> Option<&str> {
    value
        .and_then(|value| value.get("code"))
        .and_then(Value::as_str)
        .filter(|code| {
            !code.is_empty()
                && code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
}

/// Produces the only MCP ToolResult representation permitted in durable traces, checkpoints,
/// action audits, and history archives.
///
/// The live, already bounded result may be supplied to the model in the current process. Durable
/// state intentionally preserves only result identity and outcome so an external server cannot
/// smuggle returned content into SQLite or a replayable checkpoint. Keeping this projection in
/// core avoids subtle prefix drift between the runtime and its Host persistence adapter.
pub fn mcp_tool_result_persistence_projection(result: &AgentToolResult) -> AgentToolResult {
    let value = result.result.as_ref().and_then(Value::as_object);
    let status = projected_mcp_status(result, value);
    let is_error = value
        .and_then(|value| value.get("isError"))
        .and_then(Value::as_bool)
        .unwrap_or(!result.ok);
    let outcome = projected_mcp_outcome(result, value, status, is_error);
    let dispatch_certainty = projected_mcp_dispatch_certainty(value, status);
    let feedback_provided = value.is_some_and(|value| {
        value
            .get("feedbackProvided")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || value
                .get("userFeedback")
                .and_then(Value::as_str)
                .is_some_and(|feedback| !feedback.trim().is_empty())
    });
    let mut projected = Map::from_iter([
        ("schemaVersion".to_string(), json!(1)),
        ("type".to_string(), json!("mcp_tool")),
        ("external".to_string(), json!(true)),
        ("status".to_string(), json!(status)),
        ("outcome".to_string(), json!(outcome)),
        ("dispatchCertainty".to_string(), json!(dispatch_certainty)),
        ("contentOmitted".to_string(), json!(true)),
        ("isError".to_string(), json!(is_error)),
    ]);
    if feedback_provided {
        projected.insert("feedbackProvided".to_string(), json!(true));
    }
    if let Some(code) = safe_mcp_result_code(value) {
        projected.insert("code".to_string(), json!(code));
    }
    if let Some(retryable) = value
        .and_then(|value| value.get("retryable"))
        .and_then(Value::as_bool)
    {
        projected.insert("retryable".to_string(), json!(retryable));
    }
    AgentToolResult {
        exact_archive_file: None,
        call_id: result.call_id.clone(),
        tool: result.tool.clone(),
        ok: result.ok,
        result: Some(Value::Object(projected)),
        error: (!result.ok).then(|| "The external MCP tool reported an error.".to_string()),
    }
}

/// Complete Host-classified lifecycle projection used to build one presentation-safe event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpToolInvocationEventUpdate<'a> {
    pub state: AgentMcpToolInvocationState,
    pub dispatch_certainty: AgentMcpDispatchCertainty,
    pub outcome: Option<AgentMcpToolInvocationOutcome>,
    pub is_error: Option<bool>,
    pub error_code: Option<&'a str>,
    pub duration_ms: Option<u64>,
    pub output_truncated: bool,
}

/// Builds a presentation-safe lifecycle event from a frozen approval.
pub fn mcp_tool_invocation_event(
    approval: &AgentMcpToolApproval,
    update: McpToolInvocationEventUpdate<'_>,
) -> AgentResult<AgentMcpToolInvocationEvent> {
    validate_mcp_tool_approval(approval)?;
    let McpToolInvocationEventUpdate {
        state,
        dispatch_certainty,
        outcome,
        is_error,
        error_code,
        duration_ms,
        output_truncated,
    } = update;
    let valid_state = match state {
        AgentMcpToolInvocationState::PendingApproval | AgentMcpToolInvocationState::Approved => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome.is_none()
                && is_error.is_none()
                && error_code.is_none()
                && duration_ms.is_none()
                && !output_truncated
        }
        AgentMcpToolInvocationState::Dispatching | AgentMcpToolInvocationState::Running => {
            dispatch_certainty == AgentMcpDispatchCertainty::PossiblyDispatched
                && outcome.is_none()
                && is_error.is_none()
                && error_code.is_none()
                && duration_ms.is_none()
                && !output_truncated
        }
        AgentMcpToolInvocationState::Completed => {
            dispatch_certainty == AgentMcpDispatchCertainty::ResponseReceived
                && duration_ms.is_some()
                && matches!(
                    (outcome, is_error, error_code),
                    (
                        Some(AgentMcpToolInvocationOutcome::Succeeded),
                        Some(false),
                        None
                    ) | (
                        Some(AgentMcpToolInvocationOutcome::ToolError),
                        Some(true),
                        Some(_)
                    )
                )
        }
        AgentMcpToolInvocationState::Failed => {
            duration_ms.is_some()
                && error_code.is_some()
                && is_error == Some(true)
                && match outcome {
                    Some(AgentMcpToolInvocationOutcome::OutputTooLarge) => {
                        dispatch_certainty == AgentMcpDispatchCertainty::ResponseReceived
                    }
                    Some(AgentMcpToolInvocationOutcome::TimedOut) => {
                        dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                            && !output_truncated
                    }
                    Some(AgentMcpToolInvocationOutcome::TransportError) => {
                        matches!(
                            dispatch_certainty,
                            AgentMcpDispatchCertainty::DefinitelyNotDispatched
                                | AgentMcpDispatchCertainty::ResponseReceived
                        ) && (!output_truncated
                            || dispatch_certainty == AgentMcpDispatchCertainty::ResponseReceived)
                    }
                    _ => false,
                }
        }
        AgentMcpToolInvocationState::Cancelled => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::Cancelled)
                && is_error.is_none()
                && error_code.is_some()
                && !output_truncated
        }
        AgentMcpToolInvocationState::Rejected => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::Rejected)
                && is_error.is_none()
                && error_code.is_some()
                && !output_truncated
        }
        AgentMcpToolInvocationState::Expired => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::Expired)
                && is_error.is_none()
                && error_code.is_some()
                && !output_truncated
        }
        AgentMcpToolInvocationState::PayloadUnavailable => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::PayloadUnavailable)
                && is_error == Some(true)
                && error_code.is_some()
                && !output_truncated
        }
        AgentMcpToolInvocationState::PolicyDenied => {
            dispatch_certainty == AgentMcpDispatchCertainty::DefinitelyNotDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::PolicyDenied)
                && is_error.is_none()
                && error_code.is_some()
                && !output_truncated
        }
        AgentMcpToolInvocationState::OutcomeUnknown => {
            dispatch_certainty == AgentMcpDispatchCertainty::PossiblyDispatched
                && outcome == Some(AgentMcpToolInvocationOutcome::OutcomeUnknown)
                && is_error.is_none()
                && error_code.is_some()
                && !output_truncated
        }
    };
    if !valid_state
        || outcome == Some(AgentMcpToolInvocationOutcome::Succeeded) && error_code.is_some()
    {
        return Err(AgentError::new(
            "MCP invocation lifecycle state and outcome are inconsistent.",
        ));
    }
    let error_code = error_code.map(normalize_mcp_event_error_code).transpose()?;
    Ok(AgentMcpToolInvocationEvent {
        action_id: approval.identity.action_id.clone(),
        invocation_id: approval.identity.invocation_id.clone(),
        call_id: approval.identity.call_id.clone(),
        server_id: approval.identity.provenance.server_id.clone(),
        server_display_name: approval.summary.server_display_name.clone(),
        raw_tool_name: approval.identity.provenance.raw_tool_name.clone(),
        model_tool_name: approval.identity.provenance.model_tool_name.clone(),
        display_reason: approval.summary.display_reason.clone(),
        external: true,
        state,
        dispatch_certainty,
        outcome,
        is_error,
        error_code,
        duration_ms,
        output_truncated,
    })
}

fn normalize_mcp_event_error_code(code: &str) -> AgentResult<String> {
    let code = code.trim();
    if code.is_empty()
        || code.len() > 128
        || !code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AgentError::new(
            "MCP lifecycle error codes must be bounded safe identifiers.",
        ));
    }
    Ok(code.to_string())
}

pub(super) fn validate_mcp_tool_approval(approval: &AgentMcpToolApproval) -> AgentResult<()> {
    validate_provenance(&approval.identity.provenance).map_err(|_| {
        AgentError::structured(
            "mcp.invalid_approval_identity",
            "The MCP approval has invalid Tool provenance.",
            json!({
                "type": "mcp_approval",
                "code": "invalidProvenance",
                "retryable": false,
            }),
        )
    })?;
    let action_id = uuid::Uuid::parse_str(&approval.identity.action_id)
        .map_err(|_| AgentError::new("The MCP approval action id is not a valid UUID."))?;
    let invocation_id = uuid::Uuid::parse_str(&approval.identity.invocation_id)
        .map_err(|_| AgentError::new("The MCP approval invocation id is not a valid UUID."))?;
    if action_id.get_version() != Some(uuid::Version::Random)
        || action_id.is_nil()
        || approval.identity.action_id != action_id.to_string()
        || invocation_id.get_version() != Some(uuid::Version::Random)
        || invocation_id.is_nil()
        || approval.identity.invocation_id != invocation_id.to_string()
        || approval.identity.action_id == approval.identity.invocation_id
        || approval.identity.run_id.trim().is_empty()
        || approval.identity.run_id.trim() != approval.identity.run_id
        || approval.identity.run_id.len() > 2_048
        || approval.identity.run_id.chars().any(char::is_control)
        || crate::llm::validate_model_tool_call_id(&approval.identity.call_id).is_err()
        || !valid_digest(&approval.identity.arguments_digest)
        || approval.call.id != approval.identity.call_id
        || approval.call.tool != approval.identity.provenance.model_tool_name
        || approval
            .call
            .args
            .as_object()
            .is_none_or(|args| !args.is_empty())
        || !matches!(
            (approval.approval_mode, approval.call.approval_status),
            (AgentMcpApprovalMode::Prompt, AgentApprovalStatus::Required)
                | (AgentMcpApprovalMode::Auto, AgentApprovalStatus::Approved)
        )
        || approval.call.reason.is_some()
        || approval.created_at < 0
        || approval.expires_at <= approval.created_at
        || approval.expires_at.saturating_sub(approval.created_at) > MCP_APPROVAL_TTL_MS
        || !approval.summary.external
        || approval.summary.server_id != approval.identity.provenance.server_id
        || approval.summary.scope != approval.identity.provenance.scope
        || approval.summary.raw_tool_name != approval.identity.provenance.raw_tool_name
        || approval.summary.model_tool_name != approval.identity.provenance.model_tool_name
        || approval.summary.server_display_name.trim().is_empty()
        || approval.summary.server_display_name.len() > MAX_MCP_SERVER_DISPLAY_NAME_BYTES
        || approval
            .summary
            .server_display_name
            .chars()
            .any(char::is_control)
        || approval
            .summary
            .display_reason
            .as_ref()
            .is_some_and(|reason| {
                reason.trim().is_empty()
                    || reason.trim() != reason
                    || reason.len() > MAX_MCP_CALL_REASON_BYTES
                    || reason.chars().any(is_unsafe_mcp_display_character)
            })
    {
        return Err(AgentError::structured(
            "mcp.invalid_approval_identity",
            "The MCP approval identity is inconsistent.",
            json!({
                "type": "mcp_approval",
                "code": "invalidApprovalBinding",
                "retryable": false,
            }),
        ));
    }
    Ok(())
}

pub(super) fn summarize_mcp_arguments(arguments: &Value) -> AgentResult<AgentMcpArgumentSummary> {
    let encoded_bytes = serde_json::to_vec(arguments)
        .map_err(|_| AgentError::new("MCP tool arguments could not be summarized."))?
        .len();
    let mut summary = AgentMcpArgumentSummary {
        encoded_bytes: u64::try_from(encoded_bytes).unwrap_or(u64::MAX),
        top_level_property_count: arguments
            .as_object()
            .map(|object| u64::try_from(object.len()).unwrap_or(u64::MAX))
            .unwrap_or_default(),
        string_value_count: 0,
        number_value_count: 0,
        boolean_value_count: 0,
        null_value_count: 0,
        object_value_count: 0,
        array_value_count: 0,
        max_depth: 0,
        truncated: false,
    };
    let mut remaining = MAX_MCP_ARGUMENT_SUMMARY_NODES;
    summarize_mcp_argument_value(arguments, 0, &mut remaining, &mut summary);
    Ok(summary)
}

fn summarize_mcp_argument_value(
    value: &Value,
    depth: usize,
    remaining: &mut usize,
    summary: &mut AgentMcpArgumentSummary,
) {
    if *remaining == 0 || depth > MAX_MCP_ARGUMENT_SUMMARY_DEPTH {
        summary.truncated = true;
        return;
    }
    *remaining -= 1;
    summary.max_depth = summary
        .max_depth
        .max(u32::try_from(depth).unwrap_or(u32::MAX));
    match value {
        Value::Null => summary.null_value_count = summary.null_value_count.saturating_add(1),
        Value::Bool(_) => {
            summary.boolean_value_count = summary.boolean_value_count.saturating_add(1)
        }
        Value::Number(_) => {
            summary.number_value_count = summary.number_value_count.saturating_add(1)
        }
        Value::String(_) => {
            summary.string_value_count = summary.string_value_count.saturating_add(1)
        }
        Value::Array(values) => {
            summary.array_value_count = summary.array_value_count.saturating_add(1);
            for value in values {
                summarize_mcp_argument_value(value, depth.saturating_add(1), remaining, summary);
                if *remaining == 0 {
                    summary.truncated = true;
                    break;
                }
            }
        }
        Value::Object(values) => {
            summary.object_value_count = summary.object_value_count.saturating_add(1);
            for value in values.values() {
                summarize_mcp_argument_value(value, depth.saturating_add(1), remaining, summary);
                if *remaining == 0 {
                    summary.truncated = true;
                    break;
                }
            }
        }
    }
}

pub(super) fn canonical_json(value: &Value) -> Value {
    match value {
        Value::Array(values) => Value::Array(values.iter().map(canonical_json).collect()),
        Value::Object(values) => {
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let mut canonical = Map::new();
            for key in keys {
                canonical.insert(key.clone(), canonical_json(&values[key]));
            }
            Value::Object(canonical)
        }
        _ => value.clone(),
    }
}

pub(super) fn lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}
