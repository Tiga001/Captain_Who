//! Deterministic protection against model-driven duplicate tool failure loops.
//!
//! The guard fingerprints the tool and its semantic arguments, deliberately excluding the
//! presentation-only `reason`. Two real executions are allowed so transient failures retain one
//! ordinary retry. Later identical attempts are settled locally and never reach a Tool or Host
//! executor. The state can be reconstructed from the durable conversation trace, so approval
//! pause/resume does not reset the circuit breaker.

use crate::conversation_trace::{
    ConversationTraceSnapshot, ConversationTraceToolResultStatus, ConversationTurnTraceItem,
};
use crate::protocol::{AgentError, AgentToolCall, AgentToolResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const MAX_EXECUTED_FAILURES: u32 = 2;
const MAX_LOCAL_BLOCKS_BEFORE_TERMINATION: u32 = 2;
const REPEATED_CALL_ERROR_CODE: &str = "agent.repeated_tool_call_blocked";
const REPEATED_LOOP_ERROR_CODE: &str = "agent.repeated_tool_loop";
const DUPLICATE_IN_BATCH_ERROR_CODE: &str = "agent.duplicate_tool_call_in_batch";
const FINGERPRINT_PREFIX: &str = "tool-call-sha256-v1:";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct FailureRecord {
    executed_failures: u32,
    local_blocks: u32,
}

#[derive(Debug, Clone)]
pub(super) struct ToolFailureGuardBlock {
    pub(super) result: AgentToolResult,
    pub(super) terminate_after_result: bool,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ToolFailureGuard {
    failures: BTreeMap<String, FailureRecord>,
}

impl ToolFailureGuard {
    pub(super) fn from_trace(snapshot: &ConversationTraceSnapshot) -> Self {
        let mut guard = Self::default();
        let mut pending: Option<(&str, &str, &Value)> = None;

        for item in &snapshot.committed_prefix().items {
            match item {
                ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    operation,
                    ..
                } => pending = Some((call_id, tool, operation)),
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    status,
                    success,
                    observation,
                    ..
                } => {
                    let Some((pending_id, pending_tool, operation)) = pending.take() else {
                        continue;
                    };
                    if pending_id != call_id || pending_tool != tool {
                        continue;
                    }
                    let fingerprint = semantic_tool_call_fingerprint(tool, operation);
                    let error_code = observation_error_code(observation);
                    if *success || matches!(status, ConversationTraceToolResultStatus::Succeeded) {
                        guard.failures.remove(&fingerprint);
                    } else if error_code == Some(REPEATED_CALL_ERROR_CODE) {
                        let record = guard.failures.entry(fingerprint).or_default();
                        record.local_blocks = record.local_blocks.saturating_add(1);
                    } else if error_code == Some(DUPLICATE_IN_BATCH_ERROR_CODE) {
                        // A same-response duplicate never reached a Tool or Host executor and must
                        // not consume the cross-response failure budget after checkpoint restore.
                    } else {
                        let record = guard.failures.entry(fingerprint).or_default();
                        record.executed_failures = record.executed_failures.saturating_add(1);
                    }
                }
                ConversationTurnTraceItem::AssistantNarration { .. }
                | ConversationTurnTraceItem::UserGuidance { .. } => {}
            }
        }

        guard
    }

    /// Returns a locally settled failure once the real execution retry budget is exhausted.
    ///
    /// A block is counted before returning. The second local block asks the runtime to terminate
    /// after the synthetic ToolResult has paired the model's ToolCall in context and audit.
    pub(super) fn before_call(&mut self, call: &AgentToolCall) -> Option<ToolFailureGuardBlock> {
        let fingerprint = semantic_tool_call_fingerprint(&call.tool, &call.args);
        let record = self.failures.get_mut(&fingerprint)?;
        if record.executed_failures < MAX_EXECUTED_FAILURES {
            return None;
        }

        record.local_blocks = record.local_blocks.saturating_add(1);
        let terminate_after_result = record.local_blocks >= MAX_LOCAL_BLOCKS_BEFORE_TERMINATION;
        let message = if terminate_after_result {
            format!(
                "Tool `{}` repeated the same failed semantic call after the runtime correction. The run is stopping to prevent a loop.",
                call.tool
            )
        } else {
            format!(
                "Tool `{}` already failed twice with the same semantic arguments. This duplicate attempt was not executed. Change the arguments or use another capability.",
                call.tool
            )
        };
        Some(ToolFailureGuardBlock {
            result: AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({
                    "type": "runtime_guard",
                    "code": "repeatedToolCallBlocked",
                    "errorCode": REPEATED_CALL_ERROR_CODE,
                    "recovery": "changeArgumentsOrUseDifferentCapability",
                    "tool": call.tool,
                    "semanticFingerprint": fingerprint,
                    "executedFailureCount": record.executed_failures,
                    "blockedAttemptCount": record.local_blocks,
                    "terminal": terminate_after_result,
                    "message": message,
                })),
                error: Some(message),
            },
            terminate_after_result,
        })
    }

    pub(super) fn observe(&mut self, call: &AgentToolCall, result: &AgentToolResult) {
        let fingerprint = semantic_tool_call_fingerprint(&call.tool, &call.args);
        if result.ok {
            self.failures.remove(&fingerprint);
            return;
        }
        let error_code = result.result.as_ref().and_then(observation_error_code);
        if error_code == Some(REPEATED_CALL_ERROR_CODE)
            || error_code == Some(DUPLICATE_IN_BATCH_ERROR_CODE)
        {
            // Neither runtime-local settlement reached a Tool or Host executor. The repeat block
            // was already counted by `before_call`; a batch duplicate is intentionally uncounted.
            return;
        }
        let record = self.failures.entry(fingerprint).or_default();
        record.executed_failures = record.executed_failures.saturating_add(1);
    }

    pub(super) fn terminal_error(call: &AgentToolCall) -> AgentError {
        AgentError::structured(
            REPEATED_LOOP_ERROR_CODE,
            format!(
                "Tool `{}` continued repeating an unchanged failed call after a runtime correction.",
                call.tool
            ),
            json!({
                "type": "runtime_guard",
                "code": "repeatedToolLoop",
                "recovery": "startNewRunWithChangedArgumentsOrCapability",
                "tool": call.tool,
            }),
        )
    }
}

fn observation_error_code(observation: &Value) -> Option<&str> {
    observation
        .get("errorCode")
        .and_then(Value::as_str)
        .or_else(|| observation.get("code").and_then(Value::as_str))
}

/// Stable identity for one model-visible semantic operation.
///
/// The top-level `reason` is presentation/audit metadata and deliberately does not affect
/// execution identity. Keeping this helper shared by the per-run failure guard and the
/// per-response batch guard prevents the two protections from disagreeing about equivalence.
pub(super) fn semantic_tool_call_fingerprint(tool: &str, args: &Value) -> String {
    let normalized_args = canonicalize_office_operation_alias(tool, args);
    let mut hasher = Sha256::new();
    hash_component(&mut hasher, b"tool", tool.as_bytes());
    hasher.update(b"args");
    hash_json_value(&mut hasher, normalized_args.as_ref().unwrap_or(args), true);
    format!("{FINGERPRINT_PREFIX}{:x}", hasher.finalize())
}

fn canonicalize_office_operation_alias(tool: &str, args: &Value) -> Option<Value> {
    if !matches!(
        tool,
        "office_document" | "office_spreadsheet" | "office_presentation"
    ) {
        return None;
    }
    let object = args.as_object()?;
    let operation = object.get("operation")?.as_str()?;
    let canonical = canonical_office_operation(operation);
    if canonical == operation {
        return None;
    }
    let mut normalized = object.clone();
    normalized.insert(
        "operation".to_string(),
        Value::String(canonical.to_string()),
    );
    Some(Value::Object(normalized))
}

fn canonical_office_operation(operation: &str) -> &str {
    match operation {
        "add_text" => "addText",
        "insert_image" | "addImage" | "add_image" => "insertImage",
        "add_table" => "addTable",
        "add_header" => "addHeader",
        "add_footer" => "addFooter",
        "replace_text" => "replaceText",
        "format_text" => "formatText",
        "remove_block" => "removeBlock",
        "move_block" => "moveBlock",
        "add_sheet" => "addSheet",
        "write_cell" => "writeCell",
        "set_formula" => "setFormula",
        "format_range" => "formatRange",
        "freeze_panes" => "freezePanes",
        "add_conditional_format" => "addConditionalFormat",
        "add_chart" => "addChart",
        "remove_sheet" => "removeSheet",
        "move_sheet" => "moveSheet",
        "add_slide" => "addSlide",
        "add_shape" => "addShape",
        "remove_slide" => "removeSlide",
        "move_slide" => "moveSlide",
        _ => operation,
    }
}

fn hash_component(hasher: &mut Sha256, tag: &[u8], bytes: &[u8]) {
    hasher.update(tag);
    hasher.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(bytes);
}

fn hash_json_value(hasher: &mut Sha256, value: &Value, root: bool) {
    match value {
        Value::Null => hasher.update(b"n"),
        Value::Bool(value) => hasher.update(if *value { b"b1" } else { b"b0" }),
        Value::Number(value) => hash_component(hasher, b"d", value.to_string().as_bytes()),
        Value::String(value) => hash_component(hasher, b"s", value.as_bytes()),
        Value::Array(values) => {
            hasher.update(b"a");
            hasher.update(
                u64::try_from(values.len())
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            for value in values {
                hash_json_value(hasher, value, false);
            }
        }
        Value::Object(values) => {
            hasher.update(b"o");
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            let semantic_key_count = keys
                .iter()
                .filter(|key| !(root && key.as_str() == "reason"))
                .count();
            hasher.update(
                u64::try_from(semantic_key_count)
                    .unwrap_or(u64::MAX)
                    .to_be_bytes(),
            );
            for key in keys {
                if root && key == "reason" {
                    continue;
                }
                hash_component(hasher, b"k", key.as_bytes());
                hash_json_value(hasher, &values[key], false);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::AgentApprovalStatus;

    fn call(id: &str, args: Value) -> AgentToolCall {
        AgentToolCall {
            id: id.to_string(),
            tool: "office_document".to_string(),
            args,
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        }
    }

    fn failure(call: &AgentToolCall, code: &str) -> AgentToolResult {
        AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: false,
            result: Some(json!({"errorCode": code})),
            error: Some("failed".to_string()),
        }
    }

    fn success(call: &AgentToolCall) -> AgentToolResult {
        AgentToolResult {
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({"status": "ok"})),
            error: None,
        }
    }

    #[test]
    fn fingerprint_is_order_independent_and_ignores_reason() {
        let left = semantic_tool_call_fingerprint(
            "office_document",
            &json!({"operation": "insertImage", "reason": "first", "path": "a.docx"}),
        );
        let right = semantic_tool_call_fingerprint(
            "office_document",
            &json!({"path": "a.docx", "reason": "second", "operation": "insertImage"}),
        );
        assert_eq!(left, right);
    }

    #[test]
    fn fingerprint_canonicalizes_supported_office_operation_aliases() {
        for (tool, canonical, aliases) in [
            (
                "office_document",
                "insertImage",
                vec!["insert_image", "addImage", "add_image"],
            ),
            (
                "office_spreadsheet",
                "addChart",
                vec!["add_chart", "addChart", "addChart"],
            ),
            (
                "office_presentation",
                "addSlide",
                vec!["add_slide", "addSlide", "addSlide"],
            ),
        ] {
            let expected = semantic_tool_call_fingerprint(
                tool,
                &json!({"operation": canonical, "filePath": "artifact"}),
            );
            for alias in aliases {
                assert_eq!(
                    semantic_tool_call_fingerprint(
                        tool,
                        &json!({"filePath": "artifact", "operation": alias})
                    ),
                    expected,
                    "{tool} alias {alias} should share the canonical fingerprint"
                );
            }
        }
    }

    #[test]
    fn two_real_failures_then_local_block_then_termination() {
        let mut guard = ToolFailureGuard::default();
        let first = call("call-1", json!({"operation": "insertImage"}));
        assert!(guard.before_call(&first).is_none());
        guard.observe(&first, &failure(&first, "invalidParameters"));

        let second = call(
            "call-2",
            json!({"reason": "retry", "operation": "insertImage"}),
        );
        assert!(guard.before_call(&second).is_none());
        guard.observe(&second, &failure(&second, "invalidParameters"));

        let third = call("call-3", json!({"operation": "insertImage"}));
        let third_block = guard
            .before_call(&third)
            .expect("third call must be blocked");
        assert!(!third_block.terminate_after_result);
        assert_eq!(
            third_block
                .result
                .result
                .as_ref()
                .and_then(observation_error_code),
            Some(REPEATED_CALL_ERROR_CODE)
        );
        guard.observe(&third, &third_block.result);

        let fourth = call("call-4", json!({"operation": "insertImage"}));
        assert!(
            guard
                .before_call(&fourth)
                .expect("fourth call must be blocked")
                .terminate_after_result
        );
    }

    #[test]
    fn changed_semantic_arguments_use_a_separate_budget() {
        let mut guard = ToolFailureGuard::default();
        let original = call("call-1", json!({"operation": "get", "target": "body"}));
        guard.observe(&original, &failure(&original, "invalidParameters"));
        guard.observe(&original, &failure(&original, "invalidParameters"));

        let changed = call("call-2", json!({"operation": "get", "target": "header"}));
        assert!(guard.before_call(&changed).is_none());
    }

    #[test]
    fn a_success_resets_the_failure_budget() {
        let mut guard = ToolFailureGuard::default();
        let operation = call("call-1", json!({"operation": "validate"}));
        guard.observe(&operation, &failure(&operation, "engineUnavailable"));
        guard.observe(&operation, &failure(&operation, "engineUnavailable"));
        guard.observe(&operation, &success(&operation));
        assert!(guard.before_call(&operation).is_none());
    }

    #[test]
    fn durable_trace_restores_the_circuit_breaker() {
        let args = json!({"operation": "insertImage", "reason": "show image"});
        let fingerprint = semantic_tool_call_fingerprint("office_document", &args);
        let snapshot = ConversationTraceSnapshot {
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-1".to_string(),
                    tool: "office_document".to_string(),
                    operation: args.clone(),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-1".to_string(),
                    tool: "office_document".to_string(),
                    status: ConversationTraceToolResultStatus::Failed,
                    success: false,
                    observation: json!({"errorCode": "invalidParameters"}),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: Some("failed".to_string()),
                    truncated: false,
                    archive: Default::default(),
                },
                ConversationTurnTraceItem::ToolCall {
                    sequence: 2,
                    call_id: "call-2".to_string(),
                    tool: "office_document".to_string(),
                    operation: json!({"operation": "insertImage", "reason": "retry"}),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 3,
                    call_id: "call-2".to_string(),
                    tool: "office_document".to_string(),
                    status: ConversationTraceToolResultStatus::Failed,
                    success: false,
                    observation: json!({"errorCode": "invalidParameters"}),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: Some("failed".to_string()),
                    truncated: false,
                    archive: Default::default(),
                },
            ],
            model_context_items: Vec::new(),
            next_sequence: 4,
            truncated: false,
        };
        let guard = ToolFailureGuard::from_trace(&snapshot);
        assert_eq!(
            guard.failures.get(&fingerprint),
            Some(&FailureRecord {
                executed_failures: 2,
                local_blocks: 0,
            })
        );
    }

    #[test]
    fn batch_duplicate_result_never_consumes_the_failure_budget_live_or_after_restore() {
        let operation = call("call-duplicate", json!({"operation": "insertImage"}));
        let duplicate_result = failure(&operation, DUPLICATE_IN_BATCH_ERROR_CODE);
        let mut live_guard = ToolFailureGuard::default();
        live_guard.observe(&operation, &duplicate_result);
        assert!(live_guard.failures.is_empty());
        assert!(live_guard.before_call(&operation).is_none());

        let snapshot = ConversationTraceSnapshot {
            items: vec![
                ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: operation.id.clone(),
                    tool: operation.tool.clone(),
                    operation: operation.args.clone(),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: operation.id.clone(),
                    tool: operation.tool.clone(),
                    status: ConversationTraceToolResultStatus::Failed,
                    success: false,
                    observation: json!({
                        "errorCode": DUPLICATE_IN_BATCH_ERROR_CODE
                    }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: Some("duplicate was not executed".to_string()),
                    truncated: false,
                    archive: Default::default(),
                },
            ],
            model_context_items: Vec::new(),
            next_sequence: 2,
            truncated: false,
        };
        let restored_guard = ToolFailureGuard::from_trace(&snapshot);
        assert!(restored_guard.failures.is_empty());
    }
}
