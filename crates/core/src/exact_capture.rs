//! Shared safety semantics for exact textual Tool Result capture.
//!
//! This limit protects the harness from hostile or accidentally unbounded sources. It is not a
//! model-context budget: safely captured content is archived first and the fixed 10K Model Result
//! Gate independently decides how much enters a model request.

use std::fs::File;
use std::sync::Arc;
use tempfile::NamedTempFile;

/// Maximum safely retained textual payload for one Tool call.
///
/// Process stdout and stderr share this allowance. Document parsers, Web Fetch, and Git Diff use
/// the same product constant at their source boundary.
pub const EXACT_TEXT_CAPTURE_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Stable stop reason emitted whenever the shared hard capture ceiling discards source bytes.
pub const EXACT_TEXT_CAPTURE_STOP_REASON: &str = "exact_text_capture_safety_limit";

/// Backend-only, fully materialized exact Tool Result projection.
///
/// The owning handle deletes the temporary file after all cloned Tool Results have crossed the
/// Archive boundary. Serde deliberately never sees this type; only the zstd archive repository
/// receives its UTF-8 file.
#[derive(Clone)]
pub struct ExactToolResultArchiveFile {
    file: Arc<NamedTempFile>,
}

impl ExactToolResultArchiveFile {
    pub fn new(file: NamedTempFile) -> Self {
        Self {
            file: Arc::new(file),
        }
    }

    pub fn path(&self) -> &std::path::Path {
        self.file.path()
    }

    pub fn reopen(&self) -> std::io::Result<File> {
        self.file.reopen()
    }
}

impl std::fmt::Debug for ExactToolResultArchiveFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExactToolResultArchiveFile")
            .field("present", &true)
            .finish_non_exhaustive()
    }
}

/// Returns the largest UTF-8 prefix that fits the shared byte limit and the number of omitted
/// bytes. This helper must only be used at the true safety boundary, never as a model projection.
pub fn bounded_utf8_prefix(value: &str) -> (&str, u64) {
    let maximum = usize::try_from(EXACT_TEXT_CAPTURE_MAX_BYTES).unwrap_or(usize::MAX);
    if value.len() <= maximum {
        return (value, 0);
    }
    let mut end = maximum.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (
        &value[..end],
        u64::try_from(value.len().saturating_sub(end)).unwrap_or(u64::MAX),
    )
}

/// Detects a consumer-preview cut whose omitted suffix remains present in an exact sidecar.
///
/// The declaration is recursive because audit-failure envelopes may nest one process execution
/// under `execution`. Unlike `truncatedAtSource`, this condition is fully recoverable.
pub fn value_has_recoverable_preview_truncation(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Object(object) => {
            ["stdoutPreviewTruncated", "stderrPreviewTruncated"]
                .into_iter()
                .any(|field| object.get(field).and_then(serde_json::Value::as_bool) == Some(true))
                || object
                    .values()
                    .any(value_has_recoverable_preview_truncation)
        }
        serde_json::Value::Array(values) => {
            values.iter().any(value_has_recoverable_preview_truncation)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_prefix_never_splits_utf8() {
        let prefix = "a".repeat(EXACT_TEXT_CAPTURE_MAX_BYTES as usize - 1);
        let text = format!("{prefix}你好");
        let (captured, omitted) = bounded_utf8_prefix(&text);
        assert_eq!(captured, prefix);
        assert_eq!(omitted, "你好".len() as u64);
    }
}
