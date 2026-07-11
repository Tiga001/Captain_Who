// Tool-call parsing, execution wrappers, result redaction, and runtime event helpers.
use super::{
    AgentEventStream, AgentHostActionExecutor, DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE,
    MAX_MAX_TOKENS, RUN_COUNTER,
};
use crate::cancellation::AgentCancellationToken;
use crate::llm::{LlmImage, LlmMessage, LlmMessageRole, LlmToolCall};
use crate::protocol::{
    AgentApprovalStatus, AgentChatOutput, AgentError, AgentEvent, AgentProposedAction, AgentResult,
    AgentRunStatus, AgentStateSnapshot, AgentTodoState, AgentToolCall, AgentToolDefinition,
    AgentToolResult, AgentUsage,
};
use crate::tools::{ToolExecutionContext, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) struct ToolCallRequest {
    pub(super) tool: String,
    pub(super) args: Value,
}

#[derive(Debug, Deserialize)]
struct ToolCallEnvelope {
    #[serde(rename = "type")]
    kind: Option<String>,
    tool: Option<String>,
    args: Option<Value>,
    call: Option<NestedToolCallEnvelope>,
}

#[derive(Debug, Deserialize)]
struct NestedToolCallEnvelope {
    tool: String,
    args: Option<Value>,
}

pub(super) fn parse_tool_call_request(content: &str) -> Option<ToolCallRequest> {
    let value = extract_json_value(content)?;
    let envelope: ToolCallEnvelope = serde_json::from_value(value).ok()?;
    let kind = envelope.kind.as_deref().unwrap_or("tool_call");
    if kind != "tool_call" {
        return None;
    }

    if let Some(call) = envelope.call {
        return Some(ToolCallRequest {
            tool: call.tool,
            args: call.args.unwrap_or_else(|| json!({})),
        });
    }

    envelope.tool.map(|tool| ToolCallRequest {
        tool,
        args: envelope.args.unwrap_or_else(|| json!({})),
    })
}

pub(super) fn tool_calls_from_response(
    native_tool_calls: Vec<LlmToolCall>,
    content: &str,
    run_id: &str,
    iteration: usize,
) -> Vec<LlmToolCall> {
    if !native_tool_calls.is_empty() {
        return native_tool_calls;
    }

    parse_tool_call_request(content)
        .map(|request| {
            vec![LlmToolCall {
                id: format!("tool-{run_id}-{}", iteration + 1),
                name: request.tool,
                args: request.args,
            }]
        })
        .unwrap_or_default()
}

fn extract_json_value(content: &str) -> Option<Value> {
    let trimmed = content.trim();
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    if let Some(stripped) = strip_json_code_fence(trimmed) {
        if let Ok(value) = serde_json::from_str::<Value>(stripped.trim()) {
            return Some(value);
        }
    }

    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end <= start {
        return None;
    }

    serde_json::from_str::<Value>(&trimmed[start..=end]).ok()
}

fn strip_json_code_fence(content: &str) -> Option<&str> {
    let without_start = content
        .strip_prefix("```json")
        .or_else(|| content.strip_prefix("```"))?;
    without_start.strip_suffix("```")
}

pub(super) fn build_tool_observation_message(result: &AgentToolResult) -> String {
    let payload = if result.ok {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": true,
            "result": result.result
        })
    } else {
        json!({
            "type": "tool_result",
            "tool": result.tool,
            "callId": result.call_id,
            "ok": false,
            "error": result.error
        })
    };
    let payload = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string());

    format!(
        "Tool result observation. Use this result to continue. Do not repeat the same tool call unless more information is needed.\n```json\n{payload}\n```"
    )
}

pub(super) async fn execute_tool_on_blocking_thread(
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
    call: AgentToolCall,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let handle = tokio::task::spawn_blocking(move || registry.execute(&context, &call));
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        result = handle => {
            result.map_err(|error| AgentError::new(format!("工具执行线程失败：{error}")))
        }
    }
}

pub(super) async fn execute_host_action_on_blocking_thread(
    executor: AgentHostActionExecutor,
    action: AgentProposedAction,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let execution_token = cancellation_token.clone();
    let handle = tokio::task::spawn_blocking(move || executor(action, execution_token));
    tokio::select! {
        _ = cancellation_token.cancelled() => Err(AgentError::cancelled()),
        result = handle => {
            result
                .map_err(|error| AgentError::new(format!("host 执行线程失败：{error}")))?
        }
    }
}

pub(super) fn approve_proposed_action(mut action: AgentProposedAction) -> AgentProposedAction {
    match &mut action {
        AgentProposedAction::Command { command } => {
            command.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::Diff { diff } => {
            diff.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::FileWrite { file_write } => {
            file_write.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::ToolCall { call } => {
            call.approval_status = AgentApprovalStatus::Approved;
        }
    }
    action
}

pub(super) fn failed_tool_call_result(call: &AgentToolCall, error: AgentError) -> AgentToolResult {
    AgentToolResult {
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: None,
        error: Some(error.to_string()),
    }
}

pub(super) fn redact_tool_result_for_llm(result: &AgentToolResult) -> AgentToolResult {
    let mut redacted = result.clone();
    if let Some(value) = redacted.result.as_mut() {
        redact_base64_fields(value);
    }

    redacted
}

pub(super) fn redact_tool_result_for_event(result: &AgentToolResult) -> AgentToolResult {
    let mut redacted = redact_tool_result_for_llm(result);
    if redacted.tool == "write_file" {
        if let Some(object) = redacted.result.as_mut().and_then(Value::as_object_mut) {
            object.remove("tail");
        }
    }
    redacted
}

pub(super) fn redact_tool_call_for_event(call: &AgentToolCall) -> AgentToolCall {
    let mut redacted = call.clone();
    if redacted.tool != "write_file" {
        return redacted;
    }
    let Some(args) = redacted.args.as_object_mut() else {
        return redacted;
    };
    if let Some(content) = args.get("content").and_then(Value::as_str) {
        let content_bytes = content.len() as u64;
        args.insert("content".to_string(), json!("[stored in private draft]"));
        args.insert("contentBytes".to_string(), json!(content_bytes));
    }
    redacted
}

pub(super) fn file_draft_from_tool_result(
    result: &AgentToolResult,
) -> Option<crate::protocol::AgentFileDraftSnapshot> {
    if result.tool != "write_file" || !result.ok {
        return None;
    }
    let draft = result.result.as_ref()?.get("draft")?.clone();
    serde_json::from_value(draft).ok()
}

fn redact_base64_fields(value: &mut Value) {
    match value {
        Value::Object(object) => {
            for (key, item) in object.iter_mut() {
                if key == "dataBase64" {
                    *item = json!("[redacted]");
                } else {
                    redact_base64_fields(item);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_base64_fields(item);
            }
        }
        _ => {}
    }
}

pub(super) fn llm_image_message_from_tool_result(result: &AgentToolResult) -> Option<LlmMessage> {
    if !result.ok || result.tool != "read_image" {
        return None;
    }

    let result_value = result.result.as_ref()?;
    let image = result_value.get("image")?;
    let mime_type = image
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let data_base64 = image
        .get("dataBase64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let path = result_value
        .get("path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image");

    let mut message = LlmMessage::text(
        LlmMessageRole::User,
        format!(
            "The read_image tool returned visual input for `{path}`. Inspect the attached image before continuing."
        ),
    );
    message.images.push(LlmImage {
        mime_type: mime_type.to_string(),
        data_base64: data_base64.to_string(),
    });

    Some(message)
}

pub(super) fn extract_reason_from_args(args: &Value) -> Option<String> {
    args.get("reason")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .map(ToString::to_string)
}

pub(super) fn sanitize_max_tokens(max_tokens: Option<u32>) -> u32 {
    max_tokens
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_MAX_TOKENS)
        .min(MAX_MAX_TOKENS)
}

pub(super) fn sanitize_temperature(temperature: Option<f32>) -> f32 {
    match temperature {
        Some(value) if value.is_finite() => value.clamp(0.0, 2.0),
        _ => DEFAULT_TEMPERATURE,
    }
}

pub(super) fn state_event(
    run_id: &str,
    status: AgentRunStatus,
    active_run_id: Option<String>,
    last_error: Option<String>,
) -> AgentEvent {
    AgentEvent::State {
        run_id: run_id.to_string(),
        state: AgentStateSnapshot {
            status,
            active_run_id,
            last_error,
            updated_at: now_ms(),
        },
    }
}

pub(super) fn done_event(
    run_id: &str,
    success: bool,
    status: AgentRunStatus,
    content: Option<String>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
    proposed_actions: Vec<AgentProposedAction>,
) -> AgentEvent {
    AgentEvent::Done {
        run_id: run_id.to_string(),
        success,
        status: Some(status),
        content,
        usage,
        finish_reason,
        proposed_actions,
    }
}

pub(super) fn cancelled_output(
    run_id: String,
    mut event_stream: AgentEventStream,
    tool_definitions: Vec<AgentToolDefinition>,
    todo: Option<AgentTodoState>,
    usage: Option<AgentUsage>,
    finish_reason: Option<String>,
) -> AgentChatOutput {
    event_stream.emit(state_event(&run_id, AgentRunStatus::Cancelled, None, None));
    event_stream.emit(done_event(
        &run_id,
        false,
        AgentRunStatus::Cancelled,
        None,
        usage.clone(),
        finish_reason.clone(),
        Vec::new(),
    ));

    AgentChatOutput {
        content: String::new(),
        status: AgentRunStatus::Cancelled,
        run_id,
        events: event_stream.into_events(),
        tool_definitions,
        todo,
        usage,
        finish_reason,
        proposed_actions: Vec::new(),
    }
}

pub(super) fn generate_run_id() -> String {
    let counter = RUN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("run-{}-{counter}", now_ms())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
