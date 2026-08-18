// Tool-call parsing, execution wrappers, result redaction, and runtime event helpers.
use super::{
    AgentEventStream, AgentHostActionExecutor, DEFAULT_MAX_TOKENS, DEFAULT_TEMPERATURE,
    MAX_MAX_TOKENS,
};
use crate::cancellation::AgentCancellationToken;
#[cfg(test)]
use crate::conversation_trace::render_tool_observation;
use crate::llm::{
    model_response_tool_call_id, LlmImage, LlmMessage, LlmMessageRole, LlmRuntimeToolCallBinding,
    LlmToolCall,
};
use crate::protocol::{
    AgentApprovalStatus, AgentChatOutput, AgentError, AgentEvent, AgentProposedAction, AgentResult,
    AgentRunCheckpoint, AgentRunStatus, AgentStateSnapshot, AgentTodoState, AgentToolCall,
    AgentToolDefinition, AgentToolResult, AgentUsage, ModelCapabilities,
};
use crate::tools::{AgentToolCancellationSettlement, ToolExecutionContext, ToolRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

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

#[cfg(test)]
pub(super) fn tool_calls_from_response(
    native_tool_calls: Vec<LlmToolCall>,
    content: &str,
    run_id: &str,
    iteration: usize,
) -> Vec<LlmToolCall> {
    tool_call_bindings_from_response(native_tool_calls, content, run_id, iteration)
        .into_iter()
        .map(|binding| binding.runtime_call)
        .collect()
}

pub(super) fn tool_call_bindings_from_response(
    native_tool_calls: Vec<LlmToolCall>,
    content: &str,
    run_id: &str,
    iteration: usize,
) -> Vec<LlmRuntimeToolCallBinding> {
    let calls = if native_tool_calls.is_empty() {
        parse_tool_call_request(content)
            .map(|request| {
                vec![LlmToolCall {
                    // The value is only a source correlation hint. Every accepted call is assigned
                    // a runtime-owned provider-safe identity below.
                    id: "text-fallback".to_string(),
                    name: request.tool,
                    args: request.args,
                }]
            })
            .unwrap_or_default()
    } else {
        native_tool_calls
    };

    calls
        .into_iter()
        .enumerate()
        .map(|(provider_tool_index, provider_call)| {
            let runtime_call = LlmToolCall {
                id: model_response_tool_call_id(
                    run_id,
                    iteration,
                    provider_tool_index,
                    &provider_call.id,
                ),
                args: normalize_tool_arguments(&provider_call.name, provider_call.args.clone()),
                name: provider_call.name.clone(),
            };
            LlmRuntimeToolCallBinding::new(provider_tool_index, &provider_call, runtime_call)
        })
        .collect()
}

/// Canonicalizes provider Tool arguments before they enter the runtime queue or durable context.
///
/// Tool inputs are JSON objects in every supported provider protocol. Some compatible gateways
/// serialize that object one additional time and return it as a JSON string. Decode at most two
/// such layers, and only accept an object at each repair boundary. Arbitrary strings, arrays, and
/// malformed JSON remain untouched so the Tool's typed validator can reject them normally. A
/// valid `run_command.command` is additionally normalized here so approval, Trace, Checkpoint,
/// policy, and process launch all receive the same LF representation.
fn normalize_tool_arguments(tool: &str, value: Value) -> Value {
    let mut value = normalize_stringified_object(value);
    if tool == "run_command" {
        let command = value
            .as_object()
            .and_then(|object| object.get("command"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(command) = command {
            if let Ok(canonical) = crate::command::normalize_command_text(&command) {
                value["command"] = Value::String(canonical);
            }
        }
    }
    if tool == "image_generation" {
        return normalize_image_generation_request(value);
    }
    value
}

fn normalize_stringified_object(value: Value) -> Value {
    let Value::String(encoded) = &value else {
        return value;
    };
    let mut candidate = encoded.clone();
    for _ in 0..2 {
        match serde_json::from_str::<Value>(candidate.trim()) {
            Ok(decoded @ Value::Object(_)) => return decoded,
            Ok(Value::String(next_candidate)) => candidate = next_candidate,
            _ => return value,
        }
    }
    value
}

/// Repairs a bounded model compatibility defect observed with otherwise valid image requests.
///
/// Some models preserve the outer Tool object but serialize the typed `request` union once more.
/// Decode exactly one layer and only when it yields an object. The image Tool's existing strict
/// deserializer remains authoritative for operations, fields, enums, and all validation.
fn normalize_image_generation_request(mut value: Value) -> Value {
    let Value::Object(arguments) = &mut value else {
        return value;
    };
    let Some(Value::String(encoded)) = arguments.get("request") else {
        return value;
    };
    let Ok(decoded @ Value::Object(_)) = serde_json::from_str::<Value>(encoded.trim()) else {
        return value;
    };
    arguments.insert("request".to_string(), decoded);
    value
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

#[cfg(test)]
pub(super) fn build_tool_observation_message(result: &AgentToolResult) -> String {
    render_tool_observation(result)
}

pub(super) async fn execute_registered_tool(
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
    call: AgentToolCall,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<crate::tools::RegisteredToolExecution> {
    registry
        .execute_async(context, call, cancellation_token)
        .await
}

pub(super) async fn execute_host_action_on_blocking_thread(
    executor: AgentHostActionExecutor,
    action: AgentProposedAction,
    expected_call: AgentToolCall,
    checkpoint: Option<AgentRunCheckpoint>,
    cancellation_token: AgentCancellationToken,
) -> AgentResult<AgentToolResult> {
    let execution_token = cancellation_token.clone();
    let handle = tokio::task::spawn_blocking(move || executor(action, checkpoint, execution_token));
    // A host action may have crossed its atomic commit boundary when cancellation arrives.
    // Never detach a mutating blocking task and guess its outcome: the cancellation token is
    // delivered to the executor, then we wait for its authoritative committed/cancelled result.
    let settlement = match handle.await {
        Ok(settlement) => settlement,
        Err(error) => Err(AgentError::structured(
            "agent.host_executor_join_failed",
            format!("Host action execution thread failed: {error}"),
            json!({
                "type": "host_execution",
                "code": "hostExecutorJoinFailed",
                "outcome": "indeterminate",
                "recovery": "inspectAuthoritativeStateBeforeRetry",
            }),
        )),
    };

    Ok(match settlement {
        Ok(result) if result.call_id == expected_call.id && result.tool == expected_call.tool => {
            result
        }
        Ok(result) => failed_tool_call_result(
            &expected_call,
            AgentError::structured(
                "agent.host_result_identity_mismatch",
                "Host returned a ToolResult for a different call or tool.",
                json!({
                    "type": "host_execution",
                    "code": "hostResultIdentityMismatch",
                    "outcome": "indeterminate",
                    "recovery": "inspectAuthoritativeStateBeforeRetry",
                    "expectedCallId": expected_call.id,
                    "expectedTool": expected_call.tool,
                    "returnedCallId": result.call_id,
                    "returnedTool": result.tool,
                }),
            ),
        ),
        Err(error) if error.code().is_some() || error.details().is_some() => {
            failed_tool_call_result(&expected_call, error)
        }
        Err(error) => failed_tool_call_result(
            &expected_call,
            AgentError::structured(
                "agent.host_executor_failed",
                error.to_string(),
                json!({
                    "type": "host_execution",
                    "code": "hostExecutorFailed",
                    "outcome": "indeterminate",
                    "recovery": "inspectAuthoritativeStateBeforeRetry",
                }),
            ),
        ),
    })
}

pub(super) fn cancellation_preempts_tool_result(
    auto_execute_host_action: bool,
    settlement: AgentToolCancellationSettlement,
    cancellation_requested: bool,
    result: &AgentToolResult,
) -> bool {
    !auto_execute_host_action
        && settlement == AgentToolCancellationSettlement::Interruptible
        && (cancellation_requested || result.error.as_deref() == Some("agent run 已取消。"))
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
        AgentProposedAction::McpToolCall { approval } => {
            approval.call.approval_status = AgentApprovalStatus::Approved;
        }
        AgentProposedAction::BuiltinCapabilityActivation { approval } => {
            approval.approval_status = AgentApprovalStatus::Approved;
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
        AgentProposedAction::SkillInstallation { installation } => {
            installation.approval_status = AgentApprovalStatus::Approved;
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
        exact_archive_file: None,
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

pub(super) fn llm_image_message_from_tool_result(
    result: &AgentToolResult,
    model_capabilities: ModelCapabilities,
) -> Option<LlmMessage> {
    // Defense in depth: the producing tool already checks this frozen backend capability before
    // reading or encoding bytes. The runtime independently refuses any accidental image-bearing
    // result when the selected base model is text-only.
    if !model_capabilities.image_input
        || !result.ok
        || !matches!(result.tool.as_str(), "read_image" | "image_generation")
    {
        return None;
    }

    let result_value = result.result.as_ref()?;
    let image = result_value.get("image")?;
    let mime_type = image
        .get("mimeType")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| {
            matches!(
                *value,
                "image/png" | "image/jpeg" | "image/gif" | "image/webp"
            )
        })?;
    let data_base64 = image
        .get("dataBase64")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())?;
    let path_key = if result.tool == "image_generation" {
        "savedPath"
    } else {
        "path"
    };
    let path = result_value
        .get(path_key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("image");
    let action = if result.tool == "image_generation" {
        "generated"
    } else {
        "returned"
    };
    let quoted_path = serde_json::to_string(path).ok()?;

    let mut message = LlmMessage::text(
        LlmMessageRole::User,
        format!(
            "The {} tool {action} visual input for path {quoted_path}. Inspect the attached image before continuing.",
            result.tool
        ),
    );
    message
        .images_mut()
        .expect("user tool-result image messages support images")
        .push(LlmImage {
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
    format!("run-{}", Uuid::new_v4().simple())
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
    use crate::tools::{AgentTool, AgentToolPermissionPolicy};
    use serde_json::json;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn assert_runtime_owned_tool_call_id(id: &str) {
        assert!(id.starts_with("tc1_"), "unexpected tool-call ID: {id}");
        assert_eq!(id.len(), 47, "unexpected canonical ID length: {id}");
        assert!(
            id.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
            "tool-call ID contains provider-unsafe characters: {id}"
        );
    }

    #[test]
    fn structured_apply_patch_failure_keeps_typed_code_out_of_user_error_text() {
        let call = AgentToolCall {
            id: "patch-existing".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "operation": "create",
                "filePath": "existing.txt",
                "content": "replacement"
            }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let error = AgentError::structured(
            "agent.apply_patch.file_exists",
            "文件已存在。",
            json!({
                "type": "structured_edit_error",
                "code": "file_exists",
                "errorCode": "agent.apply_patch.file_exists",
                "recovery": "useUpdateOrChooseAnotherPath",
            }),
        );

        let result = failed_tool_call_result(&call, error);

        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("文件已存在。"));
        let details = result.result.expect("structured apply_patch failure");
        assert_eq!(details["type"], "structured_edit_error");
        assert_eq!(details["code"], "file_exists");
        assert_eq!(details["errorCode"], "agent.apply_patch.file_exists");
        assert_eq!(details["recovery"], "useUpdateOrChooseAnotherPath");
        assert!(!result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("structured_edit_error"));
        assert!(!result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("file_exists"));
    }

    #[test]
    fn native_tool_calls_replace_untrusted_unicode_ids_and_disambiguate_duplicates() {
        let provider_id = format!("重复/unsafe:{}", "工具调用🔧".repeat(80));
        let calls = tool_calls_from_response(
            vec![
                LlmToolCall {
                    id: provider_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "first.txt" }),
                },
                LlmToolCall {
                    id: provider_id,
                    name: "read_file".to_string(),
                    args: json!({ "path": "second.txt" }),
                },
            ],
            "",
            "run-native-tool-id",
            7,
        );

        assert_eq!(calls.len(), 2);
        assert_runtime_owned_tool_call_id(&calls[0].id);
        assert_runtime_owned_tool_call_id(&calls[1].id);
        assert_ne!(calls[0].id, calls[1].id);
        assert_eq!(
            calls
                .iter()
                .map(|call| call.id.as_str())
                .collect::<HashSet<_>>()
                .len(),
            2
        );
        assert_eq!(calls[0].args["path"], "first.txt");
        assert_eq!(calls[1].args["path"], "second.txt");
    }

    #[test]
    fn plaintext_fallback_receives_the_same_runtime_owned_id_contract() {
        let content = r#"{
            "type": "tool_call",
            "tool": "read_file",
            "args": { "path": "notes.md" }
        }"#;
        let first = tool_calls_from_response(Vec::new(), content, "run-fallback", 3);
        let repeated = tool_calls_from_response(Vec::new(), content, "run-fallback", 3);
        let next_request = tool_calls_from_response(Vec::new(), content, "run-fallback", 4);

        assert_eq!(first.len(), 1);
        assert_runtime_owned_tool_call_id(&first[0].id);
        assert_eq!(first[0].id, repeated[0].id);
        assert_ne!(first[0].id, next_request[0].id);
        assert_eq!(first[0].name, "read_file");
        assert_eq!(first[0].args["path"], "notes.md");
        assert!(!first[0].id.contains("text-fallback"));
    }

    #[test]
    fn provider_stringified_tool_objects_are_normalized_before_validation() {
        let native = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-call".to_string(),
                name: "office_document".to_string(),
                args: Value::String(
                    r#"{"operation":"status","reason":"Check the document engine"}"#.to_string(),
                ),
            }],
            "",
            "run-stringified-native",
            0,
        );
        assert_eq!(native[0].args["operation"], "status");

        let fallback = tool_calls_from_response(
            Vec::new(),
            r#"{
                "type": "tool_call",
                "tool": "office_document",
                "args": "{\"operation\":\"status\",\"reason\":\"Check the document engine\"}"
            }"#,
            "run-stringified-fallback",
            0,
        );
        assert_eq!(fallback[0].args["operation"], "status");
    }

    #[test]
    fn provider_double_stringified_tool_object_is_normalized_within_the_bound() {
        let object = r#"{"operation":"status","reason":"Check the engine"}"#;
        let double_encoded = serde_json::to_string(object).unwrap();
        let calls = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-double-string".to_string(),
                name: "office_document".to_string(),
                args: Value::String(double_encoded),
            }],
            "",
            "run-double-stringified-native",
            0,
        );

        assert_eq!(calls[0].args["operation"], "status");
        assert_eq!(calls[0].args["reason"], "Check the engine");
    }

    #[test]
    fn run_command_enters_the_runtime_queue_with_canonical_lf_without_trimming() {
        let calls = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-command-crlf".to_string(),
                name: "run_command".to_string(),
                args: json!({
                    "command": "\r\n  printf one\rprintf two\r\n",
                    "reason": "Exercise canonical command transport"
                }),
            }],
            "",
            "run-command-crlf",
            0,
        );

        assert_eq!(calls[0].args["command"], "\n  printf one\nprintf two\n");
    }

    #[test]
    fn image_generation_stringified_request_object_is_normalized_before_validation() {
        let generate = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-image-generate".to_string(),
                name: "image_generation".to_string(),
                args: json!({
                    "request": r#"{"operation":"generate","prompt":"A lighthouse","sizePreset":"2K"}"#,
                    "reason": "Create the requested illustration."
                }),
            }],
            "",
            "run-image-generate",
            0,
        );
        assert_eq!(generate[0].args["request"]["operation"], "generate");
        assert_eq!(generate[0].args["request"]["prompt"], "A lighthouse");
        assert_eq!(generate[0].args["request"]["sizePreset"], "2K");

        let edit = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-image-edit".to_string(),
                name: "image_generation".to_string(),
                args: json!({
                    "request": r#"{"operation":"edit","prompt":"Make it dusk","inputPath":"@attachments/a/image.png"}"#,
                    "reason": "Apply the requested visual edit."
                }),
            }],
            "",
            "run-image-edit",
            0,
        );
        assert_eq!(edit[0].args["request"]["operation"], "edit");
        assert_eq!(
            edit[0].args["request"]["inputPath"],
            "@attachments/a/image.png"
        );
    }

    #[test]
    fn image_generation_nested_request_repair_is_strict_and_single_layer() {
        for encoded in [
            "not json".to_string(),
            "[1,2,3]".to_string(),
            "\"generate\"".to_string(),
            serde_json::to_string(
                r#"{"operation":"generate","prompt":"A double encoded request"}"#,
            )
            .unwrap(),
        ] {
            let original = json!({
                "request": encoded,
                "reason": "Generate an image."
            });
            assert_eq!(
                normalize_tool_arguments("image_generation", original.clone()),
                original
            );
        }

        let unrelated = json!({
            "request": r#"{"operation":"generate","prompt":"Do not reinterpret me"}"#,
            "reason": "An unrelated Tool contract."
        });
        assert_eq!(
            normalize_tool_arguments("unrelated_tool", unrelated.clone()),
            unrelated
        );
    }

    #[test]
    fn arbitrary_top_level_strings_are_not_reinterpreted() {
        let calls = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-call".to_string(),
                name: "office_document".to_string(),
                args: Value::String("status".to_string()),
            }],
            "",
            "run-invalid-string",
            0,
        );
        assert_eq!(calls[0].args, Value::String("status".to_string()));

        let encoded_array = Value::String("[1,2,3]".to_string());
        let calls = tool_calls_from_response(
            vec![LlmToolCall {
                id: "provider-array".to_string(),
                name: "office_document".to_string(),
                args: encoded_array.clone(),
            }],
            "",
            "run-invalid-array",
            0,
        );
        assert_eq!(calls[0].args, encoded_array);
    }

    #[tokio::test]
    async fn cancelling_a_host_action_waits_for_its_authoritative_result() {
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let executor_started = Arc::clone(&started);
        let executor_finished = Arc::clone(&finished);
        let executor: AgentHostActionExecutor =
            Arc::new(move |_action, _checkpoint, cancellation| {
                executor_started.store(true, Ordering::SeqCst);
                while !cancellation.is_cancelled() {
                    std::thread::yield_now();
                }
                executor_finished.store(true, Ordering::SeqCst);
                Ok(AgentToolResult {
                    exact_archive_file: None,
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
        let expected_call = match &action {
            AgentProposedAction::ToolCall { call } => call.clone(),
            _ => unreachable!(),
        };
        let task = tokio::spawn(async move {
            execute_host_action_on_blocking_thread(
                executor,
                action,
                expected_call,
                None,
                task_cancellation,
            )
            .await
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

    #[tokio::test]
    async fn host_executor_error_becomes_a_paired_indeterminate_tool_result() {
        let expected_call = AgentToolCall {
            id: "write-error".to_string(),
            tool: "write_file".to_string(),
            args: json!({ "filePath": "report.txt" }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        };
        let action = AgentProposedAction::ToolCall {
            call: expected_call.clone(),
        };
        let executor: AgentHostActionExecutor = Arc::new(|_action, _checkpoint, _cancellation| {
            Err(AgentError::new("host channel closed"))
        });

        let result = execute_host_action_on_blocking_thread(
            executor,
            action,
            expected_call.clone(),
            None,
            AgentCancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.call_id, expected_call.id);
        assert_eq!(result.tool, expected_call.tool);
        assert!(!result.ok);
        let details = result.result.expect("structured host failure");
        assert_eq!(details["errorCode"], "agent.host_executor_failed");
        assert_eq!(details["outcome"], "indeterminate");
        assert_eq!(details["recovery"], "inspectAuthoritativeStateBeforeRetry");
    }

    #[tokio::test]
    async fn host_result_identity_mismatch_is_not_forwarded_as_another_calls_success() {
        let expected_call = AgentToolCall {
            id: "write-expected".to_string(),
            tool: "write_file".to_string(),
            args: json!({ "filePath": "report.txt" }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        };
        let action = AgentProposedAction::ToolCall {
            call: expected_call.clone(),
        };
        let executor: AgentHostActionExecutor = Arc::new(|_action, _checkpoint, _cancellation| {
            Ok(AgentToolResult {
                exact_archive_file: None,
                call_id: "write-wrong".to_string(),
                tool: "run_command".to_string(),
                ok: true,
                result: Some(json!({ "status": "committed" })),
                error: None,
            })
        });

        let result = execute_host_action_on_blocking_thread(
            executor,
            action,
            expected_call.clone(),
            None,
            AgentCancellationToken::new(),
        )
        .await
        .unwrap();

        assert_eq!(result.call_id, expected_call.id);
        assert_eq!(result.tool, expected_call.tool);
        assert!(!result.ok);
        let details = result.result.expect("identity mismatch details");
        assert_eq!(details["errorCode"], "agent.host_result_identity_mismatch");
        assert_eq!(details["code"], "hostResultIdentityMismatch");
        assert_eq!(details["outcome"], "indeterminate");
        assert_eq!(details["returnedCallId"], "write-wrong");
        assert_eq!(details["returnedTool"], "run_command");
    }

    struct AuthoritativeCancellationTool {
        started: Arc<AtomicBool>,
        finished: Arc<AtomicBool>,
        cancellation: AgentCancellationToken,
    }

    impl AgentTool for AuthoritativeCancellationTool {
        fn exposure(&self) -> crate::tools::AgentToolExposure {
            crate::tools::AgentToolExposure::Stable
        }

        fn definition(&self) -> AgentToolDefinition {
            AgentToolDefinition {
                name: "authoritative_cancellation_test".to_string(),
                description: "test".to_string(),
                input_schema: json!({
                    "type": "object",
                    "properties": {},
                    "additionalProperties": false
                }),
                safety: crate::protocol::AgentToolSafety::ReadOnly,
                requires_workspace: false,
                requires_approval: false,
                approval_mode: crate::protocol::AgentToolApprovalMode::Never,
            }
        }

        fn execute(&self, _context: &ToolExecutionContext, _args: Value) -> AgentResult<Value> {
            self.started.store(true, Ordering::SeqCst);
            while !self.cancellation.is_cancelled() {
                std::thread::yield_now();
            }
            self.finished.store(true, Ordering::SeqCst);
            Ok(json!({ "status": "cancelled", "authoritative": true }))
        }

        fn permission_policy(&self) -> AgentToolPermissionPolicy {
            AgentToolPermissionPolicy::Default
        }

        fn cancellation_settlement(&self) -> AgentToolCancellationSettlement {
            AgentToolCancellationSettlement::Authoritative
        }
    }

    #[tokio::test]
    async fn cancelling_an_authoritative_tool_waits_for_its_terminal_result() {
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let cancellation = AgentCancellationToken::new();
        let mut registry = ToolRegistry::defaults_with_search(None);
        registry
            .register_extension_tool(
                "test",
                Box::new(AuthoritativeCancellationTool {
                    started: Arc::clone(&started),
                    finished: Arc::clone(&finished),
                    cancellation: cancellation.clone(),
                }),
            )
            .unwrap();
        let registry = Arc::new(registry);
        let context =
            ToolExecutionContext::from_run_context(None).with_cancellation(cancellation.clone());
        let call = AgentToolCall {
            id: "authoritative-1".to_string(),
            tool: "authoritative_cancellation_test".to_string(),
            args: json!({}),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let task_cancellation = cancellation.clone();
        let task = tokio::spawn(async move {
            execute_registered_tool(registry, context, call, task_cancellation).await
        });
        while !started.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
        cancellation.cancel();

        let result = task.await.unwrap().unwrap();
        assert!(finished.load(Ordering::SeqCst));
        assert!(result.result.ok);
        assert_eq!(
            result
                .result
                .result
                .as_ref()
                .and_then(|value| value["authoritative"].as_bool()),
            Some(true)
        );
        assert!(!cancellation_preempts_tool_result(
            false,
            AgentToolCancellationSettlement::Authoritative,
            true,
            &result.result,
        ));
    }

    #[test]
    fn authoritative_cancelled_result_is_published_before_the_run_stops() {
        let result = AgentToolResult {
            exact_archive_file: None,
            call_id: "authoritative-1".to_string(),
            tool: "authoritative_cancellation_test".to_string(),
            ok: false,
            result: None,
            error: Some("agent run 已取消。".to_string()),
        };

        assert!(!cancellation_preempts_tool_result(
            false,
            AgentToolCancellationSettlement::Authoritative,
            true,
            &result,
        ));
        assert!(cancellation_preempts_tool_result(
            false,
            AgentToolCancellationSettlement::Interruptible,
            true,
            &result,
        ));
    }
}
