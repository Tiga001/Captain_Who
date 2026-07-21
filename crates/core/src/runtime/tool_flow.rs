// Tool-call parsing, execution wrappers, result redaction, and runtime event helpers.
use super::{
    AgentEventStream, AgentHostActionExecutor, DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE,
    MAX_MAX_TOKENS, RUN_COUNTER,
};
use crate::cancellation::AgentCancellationToken;
use crate::conversation_trace::render_tool_observation;
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

/// Makes progressive Skill disclosure a model-request boundary.
///
/// Calls planned alongside `skills_activate` were generated before the model could read the
/// Skill's complete instructions. They are therefore discarded and must be reconsidered on the
/// next model request. Multiple activation calls may remain in one batch so the next request sees
/// all requested Skill instructions together.
pub(super) fn enforce_skill_activation_barrier(
    mut calls: Vec<LlmToolCall>,
) -> (Vec<LlmToolCall>, usize) {
    if !calls.iter().any(|call| call.name == "skills_activate") {
        return (calls, 0);
    }
    let original_len = calls.len();
    calls.retain(|call| call.name == "skills_activate");
    let deferred = original_len.saturating_sub(calls.len());
    (calls, deferred)
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
    render_tool_observation(result)
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
    // A host action may have crossed its atomic commit boundary when cancellation arrives.
    // Never detach a mutating blocking task and guess its outcome: the cancellation token is
    // delivered to the executor, then we wait for its authoritative committed/cancelled result.
    handle
        .await
        .map_err(|error| AgentError::new(format!("host 执行线程失败：{error}")))?
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
        AgentProposedAction::SkillMaterialization { materialization } => {
            materialization.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::SkillScript { script } => {
            script.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::OfficeOperation { office_operation } => {
            office_operation.approval_status = AgentApprovalStatus::Approved;
        }
    }
    action
}

pub(super) fn failed_tool_call_result(call: &AgentToolCall, error: AgentError) -> AgentToolResult {
    let structured_result = error.details().cloned().map(|mut details| {
        if let (Some(code), Some(object)) = (error.code(), details.as_object_mut()) {
            object
                .entry("errorCode".to_string())
                .or_insert_with(|| json!(code));
        }
        details
    });
    AgentToolResult {
        call_id: call.id.clone(),
        tool: call.tool.clone(),
        ok: false,
        result: structured_result,
        error: Some(error.to_string()),
    }
}

pub(super) fn redact_tool_result_for_event(result: &AgentToolResult) -> AgentToolResult {
    // The caller must pass the tool-owned event projection. Re-running the durable sanitizer here
    // would erase presentation-only fields such as read_image's bounded thumbnail.
    let mut redacted = result.clone();
    if redacted.tool == "write_file" {
        if let Some(object) = redacted.result.as_mut().and_then(Value::as_object_mut) {
            object.remove("tail");
        }
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
        conversation_turn_trace: None,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn skill_activation_barrier_defers_calls_planned_without_full_instructions() {
        let calls = vec![
            LlmToolCall {
                id: "read-before-skill".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "draft.docx" }),
            },
            LlmToolCall {
                id: "activate-documents".to_string(),
                name: "skills_activate".to_string(),
                args: json!({ "skillRef": "s1", "reason": "Need document guidance" }),
            },
            LlmToolCall {
                id: "activate-review".to_string(),
                name: "skills_activate".to_string(),
                args: json!({ "skillRef": "s2", "reason": "Need review guidance" }),
            },
        ];

        let (calls, deferred) = enforce_skill_activation_barrier(calls);

        assert_eq!(deferred, 1);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].id, "activate-documents");
        assert_eq!(calls[1].id, "activate-review");
    }

    #[test]
    fn ordinary_parallel_calls_are_not_changed_by_the_skill_barrier() {
        let calls = vec![LlmToolCall {
            id: "read-1".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "notes.md" }),
        }];

        let (filtered, deferred) = enforce_skill_activation_barrier(calls.clone());

        assert_eq!(deferred, 0);
        assert_eq!(filtered, calls);
    }

    #[tokio::test]
    async fn cancelling_a_host_action_waits_for_its_authoritative_result() {
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let executor_started = Arc::clone(&started);
        let executor_finished = Arc::clone(&finished);
        let executor: AgentHostActionExecutor = Arc::new(move |_action, cancellation| {
            executor_started.store(true, Ordering::SeqCst);
            while !cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            executor_finished.store(true, Ordering::SeqCst);
            Ok(AgentToolResult {
                call_id: "write-1".to_string(),
                tool: "write_file".to_string(),
                ok: false,
                result: Some(json!({ "cancelled": true })),
                error: Some("cancelled before commit".to_string()),
            })
        });
        let action = AgentProposedAction::ToolCall {
            call: AgentToolCall {
                id: "write-1".to_string(),
                tool: "write_file".to_string(),
                args: json!({}),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
        };
        let cancellation = AgentCancellationToken::new();
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            execute_host_action_on_blocking_thread(executor, action, task_cancellation).await
        });

        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        cancellation.cancel();

        let result = task.await.unwrap().unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert!(!result.ok);
        assert_eq!(
            result
                .result
                .as_ref()
                .and_then(|value| value["cancelled"].as_bool()),
            Some(true)
        );
    }
}
