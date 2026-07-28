//! Shared opaque pagination cursors for workspace search and attachment-list tools.
//!
//! A cursor contains only hashes and a page position. Query text, filesystem paths, and result
//! contents never appear in the token. Every continuation reruns normal path authorization,
//! verifies the normalized request identity, and then verifies the freshly computed result
//! snapshot before returning a page.

use super::ToolExecutionContext;
use crate::llm::LlmMessage;
use crate::protocol::{AgentError, AgentResult};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

const SEARCH_CURSOR_PREFIX: &str = "search_v1_";
const SEARCH_CURSOR_MAX_BYTES: usize = 2 * 1024;
const SEARCH_CURSOR_VERSION: u8 = 1;
const CURSOR_INTEGRITY_DOMAIN: &[u8] = b"mycopilot-search-cursor-integrity-v1";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum SearchCursorKind {
    Code,
    Files,
    AttachmentsConversation,
    AttachmentsProject,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SearchCursorPayload {
    version: u8,
    kind: SearchCursorKind,
    request_hash: String,
    snapshot_hash: String,
    next_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DecodedSearchCursor {
    kind: SearchCursorKind,
    pub(super) next_index: usize,
    pub(super) snapshot_hash: String,
}

/// Length-delimited SHA-256 input builder used for request and result-snapshot identities.
///
/// Length framing avoids ambiguous concatenations while keeping the cursor payload independent of
/// serde or platform-specific debug formatting.
pub(super) struct SearchFingerprint {
    hasher: Sha256,
}

impl SearchFingerprint {
    pub(super) fn new(domain: &str) -> Self {
        let mut value = Self {
            hasher: Sha256::new(),
        };
        value.field("domain", domain.as_bytes());
        value
    }

    pub(super) fn field(&mut self, name: &str, value: &[u8]) {
        update_length_delimited(&mut self.hasher, name.as_bytes());
        update_length_delimited(&mut self.hasher, value);
    }

    pub(super) fn string(&mut self, name: &str, value: &str) {
        self.field(name, value.as_bytes());
    }

    pub(super) fn usize(&mut self, name: &str, value: usize) {
        self.field(
            name,
            &u64::try_from(value).unwrap_or(u64::MAX).to_be_bytes(),
        );
    }

    pub(super) fn u64(&mut self, name: &str, value: u64) {
        self.field(name, &value.to_be_bytes());
    }

    pub(super) fn bool(&mut self, name: &str, value: bool) {
        self.field(name, &[u8::from(value)]);
    }

    pub(super) fn finish(self) -> String {
        format!("{:x}", self.hasher.finalize())
    }
}

pub(super) fn encode_search_cursor(
    kind: SearchCursorKind,
    request_hash: &str,
    snapshot_hash: &str,
    next_index: usize,
) -> AgentResult<String> {
    if next_index == 0 || !valid_hash(request_hash) || !valid_hash(snapshot_hash) {
        return Err(invalid_cursor_error(kind));
    }
    let payload = SearchCursorPayload {
        version: SEARCH_CURSOR_VERSION,
        kind,
        request_hash: request_hash.to_string(),
        snapshot_hash: snapshot_hash.to_string(),
        next_index: u64::try_from(next_index).map_err(|_| invalid_cursor_error(kind))?,
    };
    let payload_bytes = serde_json::to_vec(&payload).map_err(|_| invalid_cursor_error(kind))?;
    let integrity = cursor_integrity(&payload_bytes);
    Ok(format!(
        "{SEARCH_CURSOR_PREFIX}{}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_bytes),
        integrity
    ))
}

pub(super) fn decode_search_cursor(
    value: &str,
    expected_kind: SearchCursorKind,
    expected_request_hash: &str,
) -> AgentResult<DecodedSearchCursor> {
    let value = value.trim();
    if value.is_empty() || value.len() > SEARCH_CURSOR_MAX_BYTES {
        return Err(invalid_cursor_error(expected_kind));
    }
    let encoded = value
        .strip_prefix(SEARCH_CURSOR_PREFIX)
        .ok_or_else(|| invalid_cursor_error(expected_kind))?;
    let (payload, integrity) = encoded
        .rsplit_once('.')
        .ok_or_else(|| invalid_cursor_error(expected_kind))?;
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| invalid_cursor_error(expected_kind))?;
    if cursor_integrity(&payload_bytes) != integrity {
        return Err(invalid_cursor_error(expected_kind));
    }
    let payload = serde_json::from_slice::<SearchCursorPayload>(&payload_bytes)
        .map_err(|_| invalid_cursor_error(expected_kind))?;
    if payload.version != SEARCH_CURSOR_VERSION
        || !valid_hash(&payload.request_hash)
        || !valid_hash(&payload.snapshot_hash)
        || payload.next_index == 0
    {
        return Err(invalid_cursor_error(expected_kind));
    }
    if payload.kind != expected_kind || payload.request_hash != expected_request_hash {
        return Err(mismatched_cursor_error(expected_kind));
    }

    Ok(DecodedSearchCursor {
        kind: expected_kind,
        next_index: usize::try_from(payload.next_index)
            .map_err(|_| invalid_cursor_error(expected_kind))?,
        snapshot_hash: payload.snapshot_hash,
    })
}

pub(super) fn verify_search_snapshot(
    cursor: Option<&DecodedSearchCursor>,
    actual_snapshot_hash: &str,
    total: usize,
) -> AgentResult<()> {
    let Some(cursor) = cursor else {
        return Ok(());
    };
    if cursor.snapshot_hash != actual_snapshot_hash || cursor.next_index >= total {
        return Err(stale_cursor_error(cursor.kind));
    }
    Ok(())
}

/// Chooses the largest prefix whose final semantic model projection fits the run's Tool-result
/// budget. Cursors are built by the caller for each candidate length, so the chosen continuation
/// always advances by exactly the number of results the model can actually receive.
pub(super) fn largest_fitting_page_len(
    context: &ToolExecutionContext,
    available: usize,
    mut build_model_projection: impl FnMut(usize) -> AgentResult<serde_json::Value>,
) -> AgentResult<usize> {
    if available == 0 {
        return Ok(0);
    }

    let fits = |value: &serde_json::Value| -> AgentResult<bool> {
        let content = serde_json::to_string(value)
            .map_err(|error| AgentError::new(format!("分页结果无法序列化为模型投影：{error}")))?;
        let message = LlmMessage::tool_result(context.tool_call_id()?, content, false);
        Ok(context.text_output_budget().estimate_message(&message)
            <= context.text_output_budget().max_tokens())
    };

    if fits(&build_model_projection(available)?)? {
        return Ok(available);
    }

    let mut lower = 1usize;
    let mut upper = available;
    let mut best = 0usize;
    while lower <= upper {
        let middle = lower + (upper - lower) / 2;
        if fits(&build_model_projection(middle)?)? {
            best = middle;
            lower = middle.saturating_add(1);
        } else {
            upper = middle.saturating_sub(1);
        }
    }
    if best == 0 {
        return Err(AgentError::new(
            "单条结果无法与安全续读 cursor 一起放入 10K 模型结果预算；请缩小查询范围。",
        ));
    }
    Ok(best)
}

fn update_length_delimited(hasher: &mut Sha256, value: &[u8]) {
    hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    hasher.update(value);
}

fn cursor_integrity(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(CURSOR_INTEGRITY_DOMAIN);
    update_length_delimited(&mut hasher, payload);
    format!("{:x}", hasher.finalize())
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_attachment_cursor(kind: SearchCursorKind) -> bool {
    matches!(
        kind,
        SearchCursorKind::AttachmentsConversation | SearchCursorKind::AttachmentsProject
    )
}

fn invalid_cursor_error(kind: SearchCursorKind) -> AgentError {
    if is_attachment_cursor(kind) {
        return attachment_cursor_error("invalid_or_tampered");
    }
    AgentError::structured(
        "search.cursor_invalid",
        "搜索 cursor 无效或已被篡改；请移除 cursor 并重新搜索。",
        json!({
            "type": "search_cursor",
            "code": "invalidCursor",
            "recovery": "restartSearch"
        }),
    )
}

fn mismatched_cursor_error(kind: SearchCursorKind) -> AgentError {
    if is_attachment_cursor(kind) {
        return attachment_cursor_error("scope_or_filter_changed");
    }
    AgentError::structured(
        "search.cursor_mismatch",
        "搜索 cursor 与当前工具、query、过滤条件或 workspace 不匹配；请使用原参数继续，或重新搜索。",
        json!({
            "type": "search_cursor",
            "code": "cursorMismatch",
            "recovery": "restartSearch"
        }),
    )
}

fn stale_cursor_error(kind: SearchCursorKind) -> AgentError {
    if is_attachment_cursor(kind) {
        return attachment_cursor_error("catalog_changed");
    }
    AgentError::structured(
        "search.cursor_stale",
        "搜索结果在分页期间发生变化，cursor 已失效；请重新搜索以获得一致结果。",
        json!({
            "type": "search_cursor",
            "code": "cursorStale",
            "recovery": "restartSearch"
        }),
    )
}

fn attachment_cursor_error(reason: &'static str) -> AgentError {
    AgentError::structured(
        "attachments.cursor_invalid",
        "附件列表 cursor 已失效或与当前过滤、conversation、project、权限范围不匹配；请移除 cursor 并重新列表。",
        json!({
            "type": "attachment_list_cursor",
            "code": "cursorInvalid",
            "reason": reason,
            "recovery": "restartList",
            "retryWithout": ["cursor"]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_without_embedding_request_content() {
        let request_hash = "a".repeat(64);
        let snapshot_hash = "b".repeat(64);

        let cursor =
            encode_search_cursor(SearchCursorKind::Code, &request_hash, &snapshot_hash, 20)
                .unwrap();
        let decoded = decode_search_cursor(&cursor, SearchCursorKind::Code, &request_hash).unwrap();

        assert_eq!(decoded.next_index, 20);
        assert_eq!(decoded.snapshot_hash, snapshot_hash);
        assert!(!cursor.contains("/private/workspace"));
    }

    #[test]
    fn tampered_and_cross_tool_cursors_are_rejected() {
        let request_hash = "a".repeat(64);
        let snapshot_hash = "b".repeat(64);
        let cursor =
            encode_search_cursor(SearchCursorKind::Code, &request_hash, &snapshot_hash, 2).unwrap();
        let mut tampered = cursor.into_bytes();
        let index = tampered.len() / 2;
        tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
        let tampered = String::from_utf8(tampered).unwrap();

        assert!(decode_search_cursor(&tampered, SearchCursorKind::Code, &request_hash).is_err());

        let cursor =
            encode_search_cursor(SearchCursorKind::Code, &request_hash, &snapshot_hash, 2).unwrap();
        assert!(decode_search_cursor(&cursor, SearchCursorKind::Files, &request_hash).is_err());
    }
}
