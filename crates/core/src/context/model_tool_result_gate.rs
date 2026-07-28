//! Central, provider-neutral budget gate for model-visible Tool results.
//!
//! Tool-owned projections decide semantics. This module only applies the product-wide model
//! budget after that projection, while keeping the result valid JSON and measuring the complete
//! [`LlmMessage`] that will be sent to the provider.

use super::measurement::ContextTextBudget;
use crate::llm::LlmMessage;
use crate::protocol::AgentToolResult;
use serde_json::{json, Map, Value};
use std::collections::HashSet;

pub(crate) const MODEL_TOOL_RESULT_MAX_TOKENS: u64 = 10_000;

const DETAIL_SCALE: u64 = 1_000_000;
const INITIAL_HEADROOM_PERCENT: u64 = 95;
const ADAPTIVE_HEADROOM_PERCENT: u64 = 90;
const MAX_COMPACTION_ATTEMPTS: usize = 10;
const MAX_STRUCTURAL_DEPTH: usize = 64;
const SMALL_STRING_CHARS: usize = 256;
const MIN_RETAINED_STRING_CHARS: usize = 32;
const SMALL_COLLECTION_ITEMS: usize = 8;

const PROTECTED_FIELDS: &[&str] = &[
    "truncated",
    "truncatedAtSource",
    "truncated_at_source",
    "source",
    "originalBytes",
    "original_bytes",
    "estimatedOriginalTokens",
    "estimated_original_tokens",
    "cursor",
    "next",
    "nextCursor",
    "next_cursor",
    "nextStartByte",
    "nextStartLine",
    "nextAfterPath",
    "navigation",
    "continueWith",
    "continue_with",
    "open",
    "historyOpen",
    "history_open",
    "recovery",
    "status",
    "error",
];

const OPAQUE_RECOVERY_FIELDS: &[&str] = &[
    "cursor",
    "next",
    "nextCursor",
    "next_cursor",
    "nextStartByte",
    "nextStartLine",
    "nextAfterPath",
    "navigation",
    "continueWith",
    "continue_with",
    "open",
    "historyOpen",
    "history_open",
    "recovery",
];

/// Pre-encoded recovery routes supplied by the runtime after exact-history archival.
///
/// Values stay provider-neutral so live execution, approval recovery, and restart rebuilding can
/// share this gate without importing a private `conversation_history` route type.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct ModelToolResultRecovery {
    pub(crate) continue_with: Option<Value>,
    pub(crate) history_open: Option<Value>,
    pub(crate) recovery: Option<Value>,
    /// Authoritative source-truncation state from the Archive/Tool boundary.
    ///
    /// `None` falls back to inspecting conventional fields in the semantic model result.
    pub(crate) truncated_at_source: Option<bool>,
}

/// A model-ready Tool-result payload and the estimate for its complete provider message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelToolResultGateOutput {
    pub(crate) content: String,
    pub(crate) estimated_tokens: u64,
    pub(crate) truncated: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct ModelToolResultGate {
    budget: ContextTextBudget,
}

impl ModelToolResultGate {
    pub(super) fn new(budget: ContextTextBudget) -> Self {
        assert_eq!(
            budget.max_tokens(),
            MODEL_TOOL_RESULT_MAX_TOKENS,
            "ModelToolResultGate must use the fixed product budget"
        );
        Self { budget }
    }

    /// Encodes and, when necessary, structurally compacts a semantic model projection.
    ///
    /// Recovery metadata is added only when compaction occurs. A Tool-owned `continueWith` route
    /// remains authoritative; generic exact-history recovery fills that field only when the Tool
    /// did not already provide a more specific continuation.
    pub(crate) fn project(
        &self,
        call_id: &str,
        is_error: bool,
        model_result: &AgentToolResult,
        recovery: Option<&ModelToolResultRecovery>,
    ) -> ModelToolResultGateOutput {
        let mut payload = semantic_payload(model_result, is_error);
        let truncated_at_source = recovery
            .and_then(|recovery| recovery.truncated_at_source)
            .unwrap_or_else(|| source_was_truncated(&payload));
        if truncated_at_source {
            payload = mark_source_truncation(payload);
        }
        let original_content = serialize_value(&payload).unwrap_or_else(|| "{}".to_string());
        let original_estimated_tokens = self.estimate_message(call_id, &original_content, is_error);

        if original_estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS {
            return ModelToolResultGateOutput {
                content: original_content,
                estimated_tokens: original_estimated_tokens,
                truncated: false,
            };
        }

        let original_bytes = u64::try_from(original_content.len()).unwrap_or(u64::MAX);
        let marked_payload = marked_truncated_payload(
            payload,
            original_bytes,
            original_estimated_tokens,
            truncated_at_source,
            recovery,
        );

        let mut factor = MODEL_TOOL_RESULT_MAX_TOKENS
            .saturating_mul(DETAIL_SCALE)
            .checked_div(original_estimated_tokens)
            .unwrap_or(0)
            .saturating_mul(INITIAL_HEADROOM_PERCENT)
            / 100;

        for _ in 0..MAX_COMPACTION_ATTEMPTS {
            let candidate = compact_value(&marked_payload, factor, 0, false);
            let candidate_content = serialize_value(&candidate).unwrap_or_else(|| "{}".to_string());
            let candidate_tokens = self.estimate_message(call_id, &candidate_content, is_error);
            if candidate_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS {
                return ModelToolResultGateOutput {
                    content: candidate_content,
                    estimated_tokens: candidate_tokens,
                    truncated: true,
                };
            }

            if factor == 0 {
                break;
            }
            let proportional = factor
                .saturating_mul(MODEL_TOOL_RESULT_MAX_TOKENS)
                .checked_div(candidate_tokens.max(1))
                .unwrap_or(0)
                .saturating_mul(ADAPTIVE_HEADROOM_PERCENT)
                / 100;
            factor = if proportional >= factor {
                factor / 2
            } else {
                proportional
            };
        }

        if let Some(output) = self.output_if_fits(
            call_id,
            is_error,
            compact_value(&marked_payload, 0, 0, false),
            true,
        ) {
            return output;
        }

        for max_string_chars in [256, 128, 64, 16, 0] {
            let candidate =
                protected_fail_safe_payload(&marked_payload, is_error, max_string_chars);
            if let Some(output) = self.output_if_fits(call_id, is_error, candidate, true) {
                return output;
            }
        }

        let final_payload = json!({
            "truncated": true,
            "truncatedAtSource": truncated_at_source,
            "originalBytes": original_bytes,
            "estimatedOriginalTokens": original_estimated_tokens,
            "status": "result_omitted",
            "error": if is_error {
                "Tool result exceeded the model context budget."
            } else {
                "Tool result omitted from the model projection because it exceeded the context budget."
            },
        });
        if let Some(output) = self.output_if_fits(call_id, is_error, final_payload, true) {
            return output;
        }

        // A validated Tool call id is bounded by the protocol, so the empty JSON object must fit.
        // Keeping this last branch total makes malformed internal callers fail closed instead of
        // returning invalid JSON.
        let content = "{}".to_string();
        let estimated_tokens = self.estimate_message(call_id, &content, is_error);
        debug_assert!(
            estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS,
            "validated Tool call id alone exceeded the model Tool-result budget"
        );
        ModelToolResultGateOutput {
            estimated_tokens,
            content,
            truncated: true,
        }
    }

    pub(crate) fn would_truncate(
        &self,
        call_id: &str,
        is_error: bool,
        model_result: &AgentToolResult,
    ) -> bool {
        self.would_truncate_with_source(
            call_id,
            is_error,
            model_result,
            source_was_truncated(&semantic_payload(model_result, is_error)),
        )
    }

    pub(crate) fn would_truncate_with_source(
        &self,
        call_id: &str,
        is_error: bool,
        model_result: &AgentToolResult,
        truncated_at_source: bool,
    ) -> bool {
        let mut payload = semantic_payload(model_result, is_error);
        if truncated_at_source {
            payload = mark_source_truncation(payload);
        }
        let content = serialize_value(&payload).unwrap_or_else(|| "{}".to_string());
        self.estimate_message(call_id, &content, is_error) > MODEL_TOOL_RESULT_MAX_TOKENS
    }

    pub(crate) fn admits_message(&self, message: &LlmMessage) -> bool {
        self.budget.estimate_message(message) <= MODEL_TOOL_RESULT_MAX_TOKENS
    }

    fn output_if_fits(
        &self,
        call_id: &str,
        is_error: bool,
        payload: Value,
        truncated: bool,
    ) -> Option<ModelToolResultGateOutput> {
        let content = serialize_value(&payload)?;
        let estimated_tokens = self.estimate_message(call_id, &content, is_error);
        (estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS).then_some(ModelToolResultGateOutput {
            content,
            estimated_tokens,
            truncated,
        })
    }

    fn estimate_message(&self, call_id: &str, content: &str, is_error: bool) -> u64 {
        self.budget
            .estimate_message(&LlmMessage::tool_result(call_id, content, is_error))
    }
}

fn mark_source_truncation(payload: Value) -> Value {
    let mut object = match payload {
        Value::Object(object) => object,
        payload => {
            let mut object = Map::new();
            object.insert("result".to_string(), payload);
            object
        }
    };
    object.insert("truncated".to_string(), Value::Bool(true));
    object.insert("truncatedAtSource".to_string(), Value::Bool(true));
    Value::Object(object)
}

fn semantic_payload(model_result: &AgentToolResult, is_error: bool) -> Value {
    let mut payload = model_result.result.clone().unwrap_or_else(|| {
        json!({
            "status": if model_result.ok { "completed" } else { "failed" }
        })
    });
    if is_error || !model_result.ok {
        if let Some(error) = model_result
            .error
            .as_deref()
            .map(str::trim)
            .filter(|error| !error.is_empty())
        {
            match payload.as_object_mut() {
                Some(object) => {
                    object
                        .entry("error".to_string())
                        .or_insert_with(|| Value::String(error.to_string()));
                }
                None => {
                    payload = json!({
                        "result": payload,
                        "error": error,
                    });
                }
            }
        }
    }
    payload
}

fn marked_truncated_payload(
    payload: Value,
    original_bytes: u64,
    original_estimated_tokens: u64,
    truncated_at_source: bool,
    recovery: Option<&ModelToolResultRecovery>,
) -> Value {
    let mut object = match payload {
        Value::Object(object) => object,
        payload => {
            let mut object = Map::new();
            object.insert("result".to_string(), payload);
            object
        }
    };
    object.insert("truncated".to_string(), Value::Bool(true));
    object.insert(
        "originalBytes".to_string(),
        Value::Number(original_bytes.into()),
    );
    object.insert(
        "estimatedOriginalTokens".to_string(),
        Value::Number(original_estimated_tokens.into()),
    );
    object.insert(
        "truncatedAtSource".to_string(),
        Value::Bool(truncated_at_source),
    );
    if let Some(recovery) = recovery {
        if let Some(value) = &recovery.continue_with {
            object
                .entry("continueWith".to_string())
                .or_insert_with(|| value.clone());
        }
        if let Some(value) = &recovery.history_open {
            object.insert("historyOpen".to_string(), value.clone());
        }
        if let Some(value) = &recovery.recovery {
            object.insert("recovery".to_string(), value.clone());
        }
    }
    Value::Object(object)
}

fn source_was_truncated(value: &Value) -> bool {
    crate::tools::value_contains_unrecoverable_source_truncation(value)
}

fn compact_value(value: &Value, factor: u64, depth: usize, protected_subtree: bool) -> Value {
    // Recovery routes and continuation cursors are opaque capabilities. Rewriting any byte can
    // invalidate them, so ordinary structural compaction treats the complete protected value as
    // atomic. Only the explicit final metadata fail-safe may replace an oversized protected value.
    if protected_subtree {
        return value.clone();
    }
    if depth >= MAX_STRUCTURAL_DEPTH {
        return json!({
            "truncated": true,
            "reason": "maximum_structure_depth",
        });
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(text) => compact_string(text, factor),
        Value::Array(values) => {
            let regular_indices = values
                .iter()
                .enumerate()
                .filter(|(_, child)| !contains_opaque_recovery_field(child, depth + 1))
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let retain_regular = retained_collection_items(regular_indices.len(), factor);
            let retained_regular_indices = retained_index_set(&regular_indices, retain_regular);
            let omitted = regular_indices.len().saturating_sub(retain_regular);
            let mut output =
                Vec::with_capacity(values.len().saturating_sub(omitted).saturating_add(1));
            let mut inserted_omission = false;
            for (index, child) in values.iter().enumerate() {
                if contains_opaque_recovery_field(child, depth + 1)
                    || retained_regular_indices.contains(&index)
                {
                    output.push(compact_value(child, factor, depth + 1, false));
                } else if !inserted_omission {
                    output.push(json!({
                        "truncated": true,
                        "omittedItems": omitted,
                    }));
                    inserted_omission = true;
                }
            }
            Value::Array(output)
        }
        Value::Object(object) => {
            let regular_keys = object
                .iter()
                .filter(|(key, child)| {
                    !is_protected_field(key) && !contains_opaque_recovery_field(child, depth + 1)
                })
                .map(|(key, _)| key.clone())
                .collect::<Vec<_>>();
            let retain_regular = retained_collection_items(regular_keys.len(), factor);
            let retained_regular_keys = retained_key_set(&regular_keys, retain_regular);
            let mut output = Map::new();
            let mut omitted = 0_usize;
            for (key, child) in object {
                let child_is_protected = is_protected_field(key);
                let contains_protected_route =
                    !child_is_protected && contains_opaque_recovery_field(child, depth + 1);
                if child_is_protected
                    || contains_protected_route
                    || retained_regular_keys.contains(key)
                {
                    output.insert(
                        key.clone(),
                        compact_value(child, factor, depth + 1, child_is_protected),
                    );
                } else {
                    omitted = omitted.saturating_add(1);
                }
            }
            if omitted > 0 {
                insert_omitted_field_count(&mut output, omitted);
            }
            Value::Object(output)
        }
    }
}

fn compact_string(value: &str, factor: u64) -> Value {
    let total_chars = value.chars().count();
    if total_chars <= SMALL_STRING_CHARS {
        return Value::String(value.to_string());
    }
    let scaled = u64::try_from(total_chars)
        .unwrap_or(u64::MAX)
        .saturating_mul(factor)
        / DETAIL_SCALE;
    let retained_chars = usize::try_from(scaled)
        .unwrap_or(usize::MAX)
        .max(MIN_RETAINED_STRING_CHARS)
        .min(total_chars);
    if retained_chars >= total_chars {
        Value::String(value.to_string())
    } else {
        Value::String(truncate_prefix_chars(value, retained_chars))
    }
}

fn retained_collection_items(total: usize, factor: u64) -> usize {
    if total <= SMALL_COLLECTION_ITEMS {
        return total;
    }
    if factor == 0 {
        return 0;
    }
    let total = u64::try_from(total).unwrap_or(u64::MAX);
    usize::try_from(total.saturating_mul(factor).div_ceil(DETAIL_SCALE))
        .unwrap_or(usize::MAX)
        .min(usize::try_from(total).unwrap_or(usize::MAX))
}

fn split_retained_items(retained: usize) -> (usize, usize) {
    let head = retained.div_ceil(2);
    (head, retained.saturating_sub(head))
}

fn retained_key_set(keys: &[String], retained: usize) -> HashSet<String> {
    if retained >= keys.len() {
        return keys.iter().cloned().collect();
    }
    let (head, tail) = split_retained_items(retained);
    let mut output = HashSet::with_capacity(retained);
    output.extend(keys.iter().take(head).cloned());
    if tail > 0 {
        output.extend(keys.iter().skip(keys.len() - tail).cloned());
    }
    output
}

fn retained_index_set(indices: &[usize], retained: usize) -> HashSet<usize> {
    if retained >= indices.len() {
        return indices.iter().copied().collect();
    }
    let (head, tail) = split_retained_items(retained);
    let mut output = HashSet::with_capacity(retained);
    output.extend(indices.iter().take(head).copied());
    if tail > 0 {
        output.extend(indices.iter().skip(indices.len() - tail).copied());
    }
    output
}

fn insert_omitted_field_count(output: &mut Map<String, Value>, omitted: usize) {
    let key = if output.contains_key("omittedFields") {
        "_modelOmittedFields"
    } else {
        "omittedFields"
    };
    output.insert(
        key.to_string(),
        Value::Number(u64::try_from(omitted).unwrap_or(u64::MAX).into()),
    );
}

fn truncate_prefix_chars(value: &str, retained_chars: usize) -> String {
    let total_chars = value.chars().count();
    if retained_chars >= total_chars {
        return value.to_string();
    }
    let prefix = value.chars().take(retained_chars).collect::<String>();
    format!(
        "{prefix}…{} chars omitted",
        total_chars.saturating_sub(retained_chars)
    )
}

fn protected_fail_safe_payload(
    marked_payload: &Value,
    is_error: bool,
    max_string_chars: usize,
) -> Value {
    let mut output = Map::new();
    output.insert("truncated".to_string(), Value::Bool(true));
    if let Some(source) = marked_payload.as_object() {
        for key in PROTECTED_FIELDS {
            if *key == "truncated" {
                continue;
            }
            if let Some(value) = source.get(*key) {
                let value = if is_opaque_recovery_field(key) {
                    // A shortened cursor is worse than no cursor: it looks actionable but cannot
                    // resolve. Preserve opaque recovery capabilities byte-for-byte or let this
                    // candidate fail and fall through to the explicit result-omitted payload.
                    value.clone()
                } else {
                    compact_fail_safe_value(value, max_string_chars, 0)
                };
                output.insert((*key).to_string(), value);
            }
        }
    }
    output
        .entry("status".to_string())
        .or_insert_with(|| Value::String("result_omitted".to_string()));
    if is_error {
        output.entry("error".to_string()).or_insert_with(|| {
            Value::String("Tool result exceeded the model context budget.".to_string())
        });
    }
    Value::Object(output)
}

fn compact_fail_safe_value(value: &Value, max_string_chars: usize, depth: usize) -> Value {
    if depth >= 4 {
        return Value::String("[omitted]".to_string());
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => value.clone(),
        Value::String(value) => {
            let total_chars = value.chars().count();
            if max_string_chars == 0 {
                Value::String("[omitted]".to_string())
            } else if total_chars <= max_string_chars {
                Value::String(value.clone())
            } else {
                Value::String(truncate_prefix_chars(value, max_string_chars))
            }
        }
        Value::Array(values) => {
            let retained = values.len().min(4);
            let (head, tail) = split_retained_items(retained);
            let mut output = values
                .iter()
                .take(head)
                .map(|value| compact_fail_safe_value(value, max_string_chars, depth + 1))
                .collect::<Vec<_>>();
            if retained < values.len() {
                output.push(json!({
                    "truncated": true,
                    "omittedItems": values.len() - retained,
                }));
            }
            if tail > 0 {
                output.extend(
                    values
                        .iter()
                        .skip(values.len() - tail)
                        .map(|value| compact_fail_safe_value(value, max_string_chars, depth + 1)),
                );
            }
            Value::Array(output)
        }
        Value::Object(object) => {
            let mut output = Map::new();
            for (key, value) in object.iter().take(8) {
                let key = if max_string_chars == 0 {
                    truncate_prefix_chars(key, key.chars().count().min(16))
                } else {
                    truncate_prefix_chars(key, key.chars().count().min(max_string_chars.max(16)))
                };
                output.insert(
                    key,
                    compact_fail_safe_value(value, max_string_chars, depth + 1),
                );
            }
            if output.len() < object.len() {
                let omitted = object.len() - output.len();
                insert_omitted_field_count(&mut output, omitted);
            }
            Value::Object(output)
        }
    }
}

fn is_protected_field(key: &str) -> bool {
    PROTECTED_FIELDS.contains(&key)
}

fn contains_opaque_recovery_field(value: &Value, depth: usize) -> bool {
    if depth >= MAX_STRUCTURAL_DEPTH {
        return false;
    }
    match value {
        Value::Object(object) => object.iter().any(|(key, child)| {
            is_opaque_recovery_field(key) || contains_opaque_recovery_field(child, depth + 1)
        }),
        Value::Array(values) => values
            .iter()
            .any(|value| contains_opaque_recovery_field(value, depth + 1)),
        _ => false,
    }
}

fn is_opaque_recovery_field(key: &str) -> bool {
    OPAQUE_RECOVERY_FIELDS.contains(&key)
}

fn serialize_value(value: &Value) -> Option<String> {
    serde_json::to_string(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gate() -> ModelToolResultGate {
        ModelToolResultGate::new(ContextTextBudget::heuristic(MODEL_TOOL_RESULT_MAX_TOKENS))
    }

    fn successful_result(value: Value) -> AgentToolResult {
        AgentToolResult {
            call_id: "semantic-call".to_string(),
            tool: "test_tool".to_string(),
            ok: true,
            result: Some(value),
            error: None,
        }
    }

    fn ascii_result_at_message_tokens(target: u64) -> AgentToolResult {
        let gate = gate();
        let call_id = "c";
        let mut low = 0_usize;
        let mut high = usize::try_from(target.saturating_mul(4)).unwrap_or(usize::MAX);
        while low < high {
            let middle = low + (high - low) / 2;
            let content = serde_json::to_string(&Value::String("a".repeat(middle))).unwrap();
            let estimate = gate.estimate_message(call_id, &content, false);
            if estimate < target {
                low = middle.saturating_add(1);
            } else {
                high = middle;
            }
        }
        let result = successful_result(Value::String("a".repeat(low)));
        let content = serialize_value(result.result.as_ref().unwrap()).unwrap();
        assert_eq!(gate.estimate_message(call_id, &content, false), target);
        result
    }

    #[test]
    fn exact_boundary_9999_is_unchanged() {
        let gate = gate();
        let result = ascii_result_at_message_tokens(9_999);

        let output = gate.project("c", false, &result, None);

        assert_eq!(output.estimated_tokens, 9_999);
        assert!(!output.truncated);
        assert_eq!(
            serde_json::from_str::<Value>(&output.content).unwrap(),
            result.result.unwrap()
        );
    }

    #[test]
    fn exact_boundary_10000_is_unchanged() {
        let gate = gate();
        let result = ascii_result_at_message_tokens(10_000);

        let output = gate.project("c", false, &result, None);

        assert_eq!(output.estimated_tokens, 10_000);
        assert!(!output.truncated);
    }

    #[test]
    fn exact_boundary_10001_is_structurally_truncated() {
        let gate = gate();
        let result = ascii_result_at_message_tokens(10_001);

        let output = gate.project("c", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(payload["truncated"], true);
        assert_eq!(payload["truncatedAtSource"], false);
        assert_eq!(payload["estimatedOriginalTokens"], 10_001);
        assert!(payload["originalBytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn unicode_and_long_single_line_remain_valid_and_keep_a_contiguous_prefix() {
        let gate = gate();
        for text in [
            format!("HEAD{}TAIL", "a".repeat(80_000)),
            format!("开头{}结尾", "天地玄黄".repeat(20_000)),
            format!("🙂BEGIN{}END🚀", "🧪".repeat(20_000)),
        ] {
            let result = successful_result(json!({
                "status": "completed",
                "text": text,
            }));
            let output = gate.project("unicode-call", false, &result, None);
            let payload: Value = serde_json::from_str(&output.content).unwrap();
            let compacted = payload["text"].as_str().unwrap();

            assert!(output.truncated);
            assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
            assert!(compacted.contains("chars omitted"));
            let (retained_prefix, omission) = compacted
                .split_once('…')
                .expect("compacted string must mark the omitted suffix");
            assert!(!retained_prefix.is_empty());
            assert!(text.starts_with(retained_prefix));
            assert!(omission.ends_with(" chars omitted"));
            assert!(!compacted.contains("…TAIL"));
            assert!(!compacted.contains("…结尾"));
            assert!(!compacted.ends_with("END🚀"));
        }
    }

    #[test]
    fn protected_routes_survive_and_tool_continue_with_remains_authoritative() {
        let gate = gate();
        let result = successful_result(json!({
            "status": "completed",
            "source": { "tool": "web_fetch", "truncated": true },
            "cursor": "cursor-1",
            "next": "cursor-2",
            "navigation": { "older": "older-1", "newer": "newer-1" },
            "continueWith": { "tool": "tool_specific", "cursor": "old" },
            "historyOpen": "hist-old",
            "body": "x".repeat(100_000),
        }));
        let recovery = ModelToolResultRecovery {
            continue_with: Some(json!({
                "tool": "conversation_history",
                "open": "hist-v1-page-2",
            })),
            history_open: Some(json!(format!(
                "hist-v1-exact-{}-tail",
                "opaque".repeat(100)
            ))),
            recovery: Some(json!({ "kind": "exact_history" })),
            truncated_at_source: Some(false),
        };
        let expected_history_open = recovery.history_open.clone().unwrap();

        let output = gate.project("protected-call", false, &result, Some(&recovery));
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["source"]["tool"], "web_fetch");
        assert_eq!(payload["cursor"], "cursor-1");
        assert_eq!(payload["next"], "cursor-2");
        assert_eq!(payload["navigation"]["older"], "older-1");
        assert_eq!(payload["continueWith"]["tool"], "tool_specific");
        assert_eq!(payload["continueWith"]["cursor"], "old");
        assert_eq!(payload["historyOpen"], expected_history_open);
        assert_eq!(payload["recovery"]["kind"], "exact_history");
        assert_eq!(payload["truncatedAtSource"], false);
    }

    #[test]
    fn history_item_open_route_survives_array_compaction_byte_for_byte() {
        let gate = gate();
        let opaque_open = format!("hist-v1-turn-{}-page-2", "opaque".repeat(100));
        let mut items = (0..128)
            .map(|index| {
                json!({
                    "summary": format!("ordinary history item {index}"),
                    "detail": "x".repeat(4_000),
                })
            })
            .collect::<Vec<_>>();
        items.insert(
            64,
            json!({
                "summary": "the history item that must remain addressable",
                "open": opaque_open.clone(),
                "detail": "y".repeat(4_000),
            }),
        );
        let result = successful_result(json!({
            "view": "turn_list",
            "items": items,
        }));

        let output = gate.project("history-list", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();
        let retained_item = payload["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item.get("open").is_some())
            .expect("history item with an open route must not be dropped");

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(retained_item["open"].as_str(), Some(opaque_open.as_str()));
    }

    #[test]
    fn nested_existing_recovery_routes_keep_their_path_and_opaque_value() {
        let gate = gate();
        let opaque = format!("hist-v1-{}-tail", "opaque".repeat(100));
        let mut result_object = Map::new();
        for index in 0..32 {
            result_object.insert(format!("bulk-{index:02}"), json!("x".repeat(8_000)));
        }
        result_object.insert(
            "nested".to_string(),
            json!({
                "large": "y".repeat(80_000),
                "routing": {
                    "historyOpen": opaque.clone(),
                    "cursor": "cursor-nested",
                }
            }),
        );
        let result = successful_result(Value::Object(result_object));

        let output = gate.project("nested-route", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(payload["nested"]["routing"]["historyOpen"], opaque);
        assert_eq!(payload["nested"]["routing"]["cursor"], "cursor-nested");
    }

    #[test]
    fn source_truncation_is_detected_when_archive_metadata_is_unavailable() {
        let gate = gate();
        let result = successful_result(json!({
            "status": "completed",
            "source": { "truncated": true },
            "body": "x".repeat(100_000),
        }));

        let output = gate.project("source-call", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert_eq!(payload["truncatedAtSource"], true);
    }

    #[test]
    fn stream_source_truncation_and_paginated_projection_are_distinguished_without_archive() {
        let gate = gate();
        let source_cut = successful_result(json!({
            "status": "completed",
            "execution": {
                "stdoutTruncated": true
            }
        }));
        let source_output = gate.project("source-stream", false, &source_cut, None);
        let source_payload: Value = serde_json::from_str(&source_output.content).unwrap();

        assert!(!source_output.truncated);
        assert_eq!(source_payload["truncatedAtSource"], true);

        let page = successful_result(json!({
            "status": "completed",
            "truncated": true,
            "content": "bounded page",
            "nextStartByte": 12
        }));
        let page_output = gate.project("semantic-page", false, &page, None);
        let page_payload: Value = serde_json::from_str(&page_output.content).unwrap();

        assert!(!page_output.truncated);
        assert!(page_payload.get("truncatedAtSource").is_none());
        assert_eq!(page_payload["nextStartByte"], 12);
    }

    #[test]
    fn small_source_truncation_is_explicit_without_length_compaction() {
        let gate = gate();
        let result = successful_result(json!({
            "status": "completed",
            "truncated": true,
            "body": "small source-side fragment",
        }));
        let recovery = ModelToolResultRecovery {
            truncated_at_source: Some(true),
            ..Default::default()
        };

        let output = gate.project("small-source-call", false, &result, Some(&recovery));
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(!output.truncated);
        assert_eq!(payload["truncated"], true);
        assert_eq!(payload["truncatedAtSource"], true);
    }

    #[test]
    fn provider_bound_frame_rejects_a_pre_gate_oversized_tool_message() {
        let gate = gate();
        let frame =
            crate::context::ContextFrame::new(vec![crate::context::ContextItem::tool_result(
                "legacy-call",
                "x".repeat(100_000),
                false,
                crate::context::ContextMetadata::new(
                    crate::context::ContextSource::ConversationTrace,
                    crate::context::ContextScope::Conversation,
                    crate::context::ContextRetention::Retained,
                ),
            )]);

        let error = frame.ensure_model_tool_results_fit(&gate).unwrap_err();

        assert!(error.to_string().contains("超过统一 10K token 上限"));
    }

    #[test]
    fn an_oversized_opaque_recovery_token_is_never_rewritten() {
        let gate = gate();
        let result = successful_result(json!({
            "status": "completed",
            "body": "x".repeat(100_000),
        }));
        let opaque = format!("hist-v1-{}-tail", "opaque".repeat(20_000));
        let recovery = ModelToolResultRecovery {
            history_open: Some(json!(opaque.clone())),
            ..Default::default()
        };

        let output = gate.project("oversized-route", false, &result, Some(&recovery));
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert!(
            payload.get("historyOpen").is_none()
                || payload["historyOpen"].as_str() == Some(opaque.as_str())
        );
        assert_eq!(payload["status"], "result_omitted");
        assert!(payload["error"].as_str().unwrap().contains("omitted"));
    }

    #[test]
    fn large_arrays_are_trimmed_without_breaking_json() {
        let gate = gate();
        let values = (0..30_000)
            .map(|index| json!({ "index": index, "value": format!("row-{index}") }))
            .collect::<Vec<_>>();
        let result = successful_result(json!({
            "status": "completed",
            "items": values,
        }));

        let output = gate.project("array-call", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert!(payload["items"].as_array().is_some());
        assert!(payload["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item.get("omittedItems").is_some()));
    }

    #[test]
    fn deep_structures_fail_safe_to_valid_bounded_json() {
        let gate = gate();
        let mut value = Value::String("leaf".repeat(40_000));
        for depth in (0..96).rev() {
            value = json!({ format!("level-{depth}"): value });
        }
        let result = successful_result(json!({
            "status": "completed",
            "nested": value,
        }));

        let output = gate.project("deep-call", false, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(payload["status"], "completed");
        assert_eq!(payload["truncated"], true);
    }

    #[test]
    fn error_field_is_preserved_and_compacted_as_valid_json() {
        let gate = gate();
        let result = AgentToolResult {
            call_id: "semantic-error".to_string(),
            tool: "test_tool".to_string(),
            ok: false,
            result: Some(json!({
                "status": "failed",
                "detail": "d".repeat(60_000),
            })),
            error: Some(format!("START{}END", "错误🙂".repeat(20_000))),
        };

        let output = gate.project("error-call", true, &result, None);
        let payload: Value = serde_json::from_str(&output.content).unwrap();

        assert!(output.truncated);
        assert!(output.estimated_tokens <= MODEL_TOOL_RESULT_MAX_TOKENS);
        assert_eq!(payload["status"], "failed");
        assert!(payload["error"].as_str().unwrap().contains("chars omitted"));
    }
}
