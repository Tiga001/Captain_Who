//! Opt-in, body-free diagnostics for the final Provider JSON request.

use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{self, Write};

/// A digest and UTF-8 lengths, with no retained source value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LlmValueFingerprint {
    pub bytes: usize,
    /// Unicode scalar values, rather than graphemes or estimated tokens.
    pub chars: usize,
    pub sha256: String,
}

/// The position and safe role label of one final wire message.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct LlmMessageFingerprint {
    pub index: usize,
    /// Known protocol roles are preserved; unknown values are labeled `other`.
    pub role: String,
    /// Strings use their raw UTF-8 bytes; other content values use compact JSON.
    /// `None` means the content field is absent, distinct from JSON null.
    pub content: Option<LlmValueFingerprint>,
    /// The entire compact JSON message, including reasoning and tool calls.
    pub message: LlmValueFingerprint,
}

/// A safe comparison surface for the final JSON sent to a Provider.
///
/// Only the model ID and known protocol roles are emitted as plain text. Tool
/// schemas, arguments, content, IDs and Provider-private fields are only hashed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmRequestFingerprint {
    pub model: Option<String>,
    pub tools_count: usize,
    pub tools: Option<LlmValueFingerprint>,
    pub system: Option<LlmValueFingerprint>,
    pub messages: Vec<LlmMessageFingerprint>,
    /// Every field in the compact JSON payload, preserving array ordering.
    pub payload: LlmValueFingerprint,
}

/// Summarizes an already-built Provider payload without changing it or retaining
/// its body. The full-payload hash uses the same compact serde JSON serialization
/// as the HTTP transport, not a semantic or reordered context projection.
///
/// String content/system values are hashed without JSON quotes or escaping, so
/// their lengths describe the text. Structured content/system values and all
/// message/tool/payload values are hashed as compact JSON.
pub fn fingerprint_llm_request(payload: &Value) -> LlmRequestFingerprint {
    let messages = payload
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, message)| LlmMessageFingerprint {
            index,
            role: match message.get("role").and_then(Value::as_str) {
                Some(
                    role @ ("system" | "developer" | "user" | "assistant" | "tool" | "function"),
                ) => role.to_owned(),
                None => "missing".to_owned(),
                Some(_) => "other".to_owned(),
            },
            content: message.get("content").map(content_fingerprint),
            message: json_fingerprint(message),
        })
        .collect();

    LlmRequestFingerprint {
        model: payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned),
        tools_count: payload
            .get("tools")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        tools: payload.get("tools").map(json_fingerprint),
        system: payload.get("system").map(content_fingerprint),
        messages,
        payload: json_fingerprint(payload),
    }
}

pub(super) fn log_request_fingerprint_if_enabled(payload: &Value) {
    if std::env::var_os("MYCOPILOT_REQUEST_FINGERPRINT").as_deref()
        != Some(std::ffi::OsStr::new("1"))
    {
        return;
    }
    // Serialize only the safe summary. Never include the payload or a serializer
    // error containing source data in the log, even if diagnostic output fails.
    if let Ok(summary) = serde_json::to_string(&fingerprint_llm_request(payload)) {
        let _ = writeln!(io::stderr().lock(), "[request-fingerprint] {summary}");
    }
}

fn content_fingerprint(value: &Value) -> LlmValueFingerprint {
    match value.as_str() {
        Some(text) => LlmValueFingerprint {
            bytes: text.len(),
            chars: text.chars().count(),
            sha256: format!("sha256:{:x}", Sha256::digest(text.as_bytes())),
        },
        None => json_fingerprint(value),
    }
}

fn json_fingerprint(value: &Value) -> LlmValueFingerprint {
    // Stream serialization directly into the hasher: no raw body copy is kept.
    let mut writer = FingerprintWriter::default();
    serde_json::to_writer(&mut writer, value)
        .expect("serializing a JSON Value to an infallible hash writer cannot fail");
    LlmValueFingerprint {
        bytes: writer.bytes,
        chars: writer.chars,
        sha256: format!("sha256:{:x}", writer.hasher.finalize()),
    }
}

#[derive(Default)]
struct FingerprintWriter {
    hasher: Sha256,
    bytes: usize,
    chars: usize,
}

impl Write for FingerprintWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.hasher.update(bytes);
        self.bytes += bytes.len();
        // serde_json emits valid UTF-8. Counting leading bytes works even when
        // consecutive writes split a scalar's UTF-8 representation.
        self.chars += bytes.iter().filter(|byte| **byte & 0xc0 != 0x80).count();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn content_lengths_are_utf8_and_payload_matches_the_http_json_bytes() {
        let payload = json!({
            "model": "test-model",
            "system": "hello",
            "messages": [
                {"role": "user", "content": "谢谢\n\"x\""},
                {"role": "assistant", "content": [{"type": "text", "text": "好"}]}
            ],
            "tools": []
        });
        let before = payload.clone();
        let result = fingerprint_llm_request(&payload);
        let content = result.messages[0].content.as_ref().unwrap();
        assert_eq!((content.bytes, content.chars), (10, 6));
        assert_eq!(result.messages[0].role, "user");
        assert_eq!(result.messages[1].index, 1);
        assert_eq!(
            result.system.as_ref().unwrap().sha256,
            "sha256:2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        for (value, digest) in [
            (&payload, &result.payload),
            (&payload["messages"][0], &result.messages[0].message),
            (
                &payload["messages"][1]["content"],
                result.messages[1].content.as_ref().unwrap(),
            ),
        ] {
            let wire = serde_json::to_string(value).unwrap();
            assert_eq!(digest.bytes, wire.len());
            assert_eq!(digest.chars, wire.chars().count());
            assert_eq!(
                digest.sha256,
                format!("sha256:{:x}", Sha256::digest(wire.as_bytes()))
            );
        }
        assert_eq!(payload, before);
    }

    #[test]
    fn summary_never_retains_content_arguments_credentials_or_unknown_role_text() {
        let private = "PRIVATE_SENTINEL_9d1a";
        let payload = json!({
            "model": "test-model",
            "system": private,
            "api_key": private,
            "tools": [{"type": "function", "function": {"name": private, "parameters": {"secret": private}}}],
            "messages": [{
                "role": "assistant",
                "content": [{"type": "image", "source": {"data": private}}],
                "reasoning_content": private,
                "tool_calls": [{"id": private, "function": {"name": private, "arguments": private}}]
            }, {"role": private, "content": private}]
        });
        let result = fingerprint_llm_request(&payload);
        assert_eq!(result.tools_count, 1);
        assert_eq!(result.messages[1].role, "other");
        assert!(!serde_json::to_string(&result).unwrap().contains(private));
        assert!(!format!("{result:?}").contains(private));
    }

    #[test]
    fn message_hash_exposes_private_reasoning_and_tool_call_changes() {
        let payload = json!({
            "model": "test-model",
            "messages": [{
                "role": "assistant", "content": "same visible text",
                "reasoning_content": "first thought",
                "tool_calls": [{"id": "call_1", "function": {"arguments": "{}"}}]
            }]
        });
        let baseline = fingerprint_llm_request(&payload);
        for (pointer, value) in [
            ("/messages/0/reasoning_content", json!("second thought")),
            ("/messages/0/tool_calls/0/id", json!("call_2")),
            (
                "/messages/0/tool_calls/0/function/arguments",
                json!("{\"x\":1}"),
            ),
        ] {
            let mut changed = payload.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            let result = fingerprint_llm_request(&changed);
            assert_eq!(result.messages[0].content, baseline.messages[0].content);
            assert_ne!(result.messages[0].message, baseline.messages[0].message);
            assert_ne!(result.payload, baseline.payload);
        }
    }

    #[test]
    fn omitted_and_explicit_output_budgets_have_distinct_wire_fingerprints() {
        let omitted = json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "same prompt"}]
        });
        let baseline = fingerprint_llm_request(&omitted);
        let mut digests = std::collections::BTreeSet::from([baseline.payload.sha256.clone()]);
        for field in ["max_tokens", "max_completion_tokens"] {
            for limit in [Value::Null, json!(30_000), json!(180_000)] {
                let mut explicit = omitted.clone();
                explicit[field] = limit;
                let fingerprint = fingerprint_llm_request(&explicit);
                assert_eq!(fingerprint.messages, baseline.messages);
                assert!(digests.insert(fingerprint.payload.sha256));
            }
        }
    }

    #[test]
    fn array_order_and_absent_null_empty_fields_remain_distinguishable() {
        let mut payload = json!({
            "tools": [{"name": "first"}, {"name": "second"}],
            "messages": [
                {"role": "user"},
                {"role": "assistant", "content": null},
                {"role": "tool", "content": ""}
            ]
        });
        let baseline = fingerprint_llm_request(&payload);
        assert!(baseline.system.is_none());
        assert!(baseline.messages[0].content.is_none());
        assert_eq!(baseline.messages[1].content.as_ref().unwrap().bytes, 4);
        assert_eq!(baseline.messages[2].content.as_ref().unwrap().bytes, 0);
        payload["tools"].as_array_mut().unwrap().reverse();
        let tools_changed = fingerprint_llm_request(&payload);
        assert_ne!(tools_changed.tools, baseline.tools);
        assert_eq!(tools_changed.messages, baseline.messages);
        payload["messages"].as_array_mut().unwrap().reverse();
        let messages_changed = fingerprint_llm_request(&payload);
        assert_eq!(messages_changed.messages[0].role, "tool");
        assert_ne!(messages_changed.payload, tools_changed.payload);
    }
}
