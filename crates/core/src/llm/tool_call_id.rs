use super::{LlmMessage, LlmMessageRole};
use crate::protocol::{AgentError, AgentResult};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// Conservative common denominator for OpenAI-compatible and Anthropic-compatible tool IDs.
///
/// A call is assigned this application-owned canonical identity exactly once when a model response
/// enters the runtime. Execution, approval, events, audit, checkpoints, conversation traces, and
/// later model requests all carry the same opaque value. The generated representation is always
/// ASCII, URL-safe, and 47 bytes.
const MODEL_TOOL_CALL_ID_MAX_BYTES: usize = 64;

const MODEL_TOOL_CALL_ID_PREFIX: &str = "tc1_";
const MODEL_TOOL_CALL_ID_LENGTH_BYTES: usize = 47;
const MODEL_TOOL_CALL_ID_HASH_DOMAIN: &[u8] = b"mycopilot:model-tool-call-id:v1";
const MODEL_RESPONSE_DOMAIN: &[u8] = b"model-response";

/// Derives the identity used for one tool call accepted from a model response.
///
/// Provider IDs are untrusted, transient correlation hints. The runtime includes the complete raw
/// value in the digest, adds request/index coordinates so duplicate provider IDs cannot collide,
/// and then discards it. Only the returned canonical identity crosses the runtime boundary.
pub(crate) fn model_response_tool_call_id(
    run_id: &str,
    model_request_index: usize,
    tool_index: usize,
    provider_call_id: &str,
) -> String {
    let model_request_index = u64::try_from(model_request_index).unwrap_or(u64::MAX);
    let tool_index = u64::try_from(tool_index).unwrap_or(u64::MAX);
    derive_model_tool_call_id(
        MODEL_RESPONSE_DOMAIN,
        &[
            run_id.as_bytes(),
            &model_request_index.to_be_bytes(),
            &tool_index.to_be_bytes(),
            provider_call_id.as_bytes(),
        ],
    )
}

fn derive_model_tool_call_id(domain: &[u8], fields: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    update_length_delimited(&mut hasher, MODEL_TOOL_CALL_ID_HASH_DOMAIN);
    update_length_delimited(&mut hasher, domain);
    for field in fields {
        update_length_delimited(&mut hasher, field);
    }
    let digest = hasher.finalize();
    let id = format!(
        "{MODEL_TOOL_CALL_ID_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(digest)
    );
    debug_assert_eq!(id.len(), MODEL_TOOL_CALL_ID_LENGTH_BYTES);
    debug_assert!(is_model_tool_call_id_character_safe(&id));
    id
}

fn update_length_delimited(hasher: &mut Sha256, value: &[u8]) {
    let length = u64::try_from(value.len()).unwrap_or(u64::MAX);
    hasher.update(length.to_be_bytes());
    hasher.update(value);
}

/// Validates the complete provider-facing tool protocol immediately before network I/O.
///
/// ContextFrame performs richer group validation while a run is assembled. This independent
/// transport guard also protects compaction, restored checkpoints, future callers, and both API
/// styles from accidentally emitting an invalid request.
pub(crate) fn validate_model_tool_protocol(messages: &[LlmMessage]) -> AgentResult<()> {
    let mut seen_call_ids = BTreeSet::new();
    let mut unresolved_calls = BTreeMap::<String, usize>::new();

    for (message_index, message) in messages.iter().enumerate() {
        match message.role {
            LlmMessageRole::Assistant => {
                if message.tool_call_id.is_some() {
                    return Err(protocol_error(
                        "assistantToolResultId",
                        message_index,
                        "assistant message unexpectedly contains tool_call_id",
                    ));
                }
                if !unresolved_calls.is_empty() {
                    return Err(protocol_error(
                        "interleavedAssistant",
                        message_index,
                        "assistant message appears before the preceding tool calls are settled",
                    ));
                }
                for (tool_index, call) in message.tool_calls.iter().enumerate() {
                    validate_model_tool_call_id_at(&call.id, message_index, Some(tool_index))?;
                    if !seen_call_ids.insert(call.id.clone()) {
                        return Err(protocol_error(
                            "duplicateToolCallId",
                            message_index,
                            &format!("tool call at index {tool_index} reuses an earlier ID"),
                        ));
                    }
                    unresolved_calls.insert(call.id.clone(), message_index);
                }
            }
            LlmMessageRole::Tool => {
                if !message.tool_calls.is_empty() {
                    return Err(protocol_error(
                        "toolMessageContainsCalls",
                        message_index,
                        "tool result message unexpectedly contains tool calls",
                    ));
                }
                let call_id = message.tool_call_id.as_deref().ok_or_else(|| {
                    protocol_error(
                        "missingToolResultId",
                        message_index,
                        "tool result message is missing tool_call_id",
                    )
                })?;
                validate_model_tool_call_id_at(call_id, message_index, None)?;
                if unresolved_calls.remove(call_id).is_none() {
                    return Err(protocol_error(
                        "unpairedToolResult",
                        message_index,
                        "tool result does not match an unresolved tool call",
                    ));
                }
            }
            LlmMessageRole::System | LlmMessageRole::User => {
                if message.tool_call_id.is_some() || !message.tool_calls.is_empty() {
                    return Err(protocol_error(
                        "invalidRoleToolFields",
                        message_index,
                        "system or user message unexpectedly contains tool protocol fields",
                    ));
                }
                if !unresolved_calls.is_empty() {
                    return Err(protocol_error(
                        "interleavedMessage",
                        message_index,
                        "another message appears before the preceding tool calls are settled",
                    ));
                }
            }
        }
    }

    if unresolved_calls.is_empty() {
        Ok(())
    } else {
        Err(AgentError::structured(
            "agent.invalid_model_tool_protocol",
            "模型请求包含尚未配对结果的工具调用。",
            serde_json::json!({
                "type": "model_tool_protocol",
                "code": "unresolvedToolCalls",
                "count": unresolved_calls.len(),
                "recovery": "retryRun",
            }),
        ))
    }
}

/// Validates one application-owned ID at persistence or reconstruction boundaries.
pub(crate) fn validate_model_tool_call_id(id: &str) -> AgentResult<()> {
    validate_model_tool_call_id_at(id, 0, None)
}

fn validate_model_tool_call_id_at(
    id: &str,
    message_index: usize,
    tool_index: Option<usize>,
) -> AgentResult<()> {
    if id.is_empty() {
        return Err(id_error("emptyToolCallId", message_index, tool_index, 0));
    }
    if id.len() > MODEL_TOOL_CALL_ID_MAX_BYTES {
        return Err(id_error(
            "toolCallIdTooLong",
            message_index,
            tool_index,
            id.len(),
        ));
    }
    if !is_model_tool_call_id_character_safe(id) {
        return Err(id_error(
            "unsafeToolCallIdCharacters",
            message_index,
            tool_index,
            id.len(),
        ));
    }
    if id.len() != MODEL_TOOL_CALL_ID_LENGTH_BYTES || !id.starts_with(MODEL_TOOL_CALL_ID_PREFIX) {
        return Err(id_error(
            "nonCanonicalToolCallId",
            message_index,
            tool_index,
            id.len(),
        ));
    }
    Ok(())
}

fn is_model_tool_call_id_character_safe(id: &str) -> bool {
    id.bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn id_error(
    code: &'static str,
    message_index: usize,
    tool_index: Option<usize>,
    length: usize,
) -> AgentError {
    AgentError::structured(
        "agent.invalid_model_tool_call_id",
        "模型请求包含不兼容的工具调用标识。",
        serde_json::json!({
            "type": "model_tool_protocol",
            "code": code,
            "messageIndex": message_index,
            "toolIndex": tool_index,
            "length": length,
            "maxLength": MODEL_TOOL_CALL_ID_MAX_BYTES,
            "canonicalLength": MODEL_TOOL_CALL_ID_LENGTH_BYTES,
            "canonicalPrefix": MODEL_TOOL_CALL_ID_PREFIX,
            "recovery": "retryRun",
        }),
    )
}

fn protocol_error(code: &'static str, message_index: usize, detail: &str) -> AgentError {
    AgentError::structured(
        "agent.invalid_model_tool_protocol",
        "模型请求中的工具调用与结果没有正确配对。",
        serde_json::json!({
            "type": "model_tool_protocol",
            "code": code,
            "messageIndex": message_index,
            "detail": detail,
            "recovery": "retryRun",
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::LlmToolCall;
    use serde_json::json;

    fn call(id: impl Into<String>) -> LlmToolCall {
        LlmToolCall {
            id: id.into(),
            name: "read_file".to_string(),
            args: json!({ "path": "src/lib.rs" }),
        }
    }

    #[test]
    fn derived_ids_are_fixed_length_ascii_safe_and_stable_across_platforms() {
        let id = model_response_tool_call_id(
            "run-跨平台/with spaces",
            usize::MAX,
            usize::MAX,
            &"provider/".repeat(100),
        );
        assert_eq!(id.len(), 47);
        assert!(id.starts_with(MODEL_TOOL_CALL_ID_PREFIX));
        assert!(is_model_tool_call_id_character_safe(&id));
        assert_eq!(
            id,
            model_response_tool_call_id(
                "run-跨平台/with spaces",
                usize::MAX,
                usize::MAX,
                &"provider/".repeat(100),
            )
        );
    }

    #[test]
    fn identity_domains_and_every_coordinate_participate_in_the_digest() {
        let base = model_response_tool_call_id("run", 1, 2, "provider");
        assert_eq!(base, "tc1_gjQEVEbI7o4nbBwgzDS8PNbwabWpzsip9ZUHSKk4Erg");
        assert_ne!(
            base,
            model_response_tool_call_id("other-run", 1, 2, "provider")
        );
        assert_ne!(base, model_response_tool_call_id("run", 2, 2, "provider"));
        assert_ne!(base, model_response_tool_call_id("run", 1, 3, "provider"));
        assert_ne!(base, model_response_tool_call_id("run", 1, 2, "other"));
    }

    #[test]
    fn transport_protocol_accepts_complete_multi_call_exchange() {
        let first = model_response_tool_call_id("run", 0, 0, "first");
        let second = model_response_tool_call_id("run", 0, 1, "second");
        let messages = vec![
            LlmMessage::text(LlmMessageRole::User, "inspect"),
            LlmMessage::assistant("", vec![call(&first), call(&second)]),
            LlmMessage::tool_result(&first, "one", false),
            LlmMessage::tool_result(&second, "two", true),
        ];
        validate_model_tool_protocol(&messages).unwrap();
    }

    #[test]
    fn transport_protocol_rejects_length_charset_duplicates_and_unpaired_results() {
        let duplicate = model_response_tool_call_id("run", 0, 0, "duplicate");
        let orphan = model_response_tool_call_id("run", 0, 1, "orphan");
        let cases = [
            vec![LlmMessage::assistant("", vec![call("x".repeat(65))])],
            vec![LlmMessage::assistant("", vec![call("unsafe:id")])],
            vec![LlmMessage::assistant("", vec![call("short-legacy-id")])],
            vec![
                LlmMessage::assistant("", vec![call(&duplicate), call(&duplicate)]),
                LlmMessage::tool_result(&duplicate, "one", false),
            ],
            vec![LlmMessage::tool_result(orphan, "result", false)],
        ];

        for messages in cases {
            assert!(validate_model_tool_protocol(&messages).is_err());
        }
    }
}
