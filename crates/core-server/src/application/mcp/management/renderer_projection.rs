//! Bounded, control-safe text and enum projection for renderer-facing DTOs.

use super::*;

pub(super) fn connection_state(state: McpServerState) -> McpConnectionStateDto {
    match state {
        McpServerState::Disabled => McpConnectionStateDto::Disabled,
        McpServerState::Starting => McpConnectionStateDto::Starting,
        McpServerState::Discovering => McpConnectionStateDto::Discovering,
        McpServerState::Ready => McpConnectionStateDto::Ready,
        McpServerState::Stopping => McpConnectionStateDto::Stopping,
        McpServerState::Error => McpConnectionStateDto::Error,
        McpServerState::Backoff => McpConnectionStateDto::Backoff,
        McpServerState::Degraded => McpConnectionStateDto::Degraded,
        _ => McpConnectionStateDto::Error,
    }
}

pub(super) fn catalog_completeness(value: &McpCatalogCompleteness) -> McpCatalogCompletenessDto {
    match value {
        McpCatalogCompleteness::Complete => McpCatalogCompletenessDto::Complete,
        McpCatalogCompleteness::Partial(_) => McpCatalogCompletenessDto::Partial,
        McpCatalogCompleteness::Stale(_) => McpCatalogCompletenessDto::Stale,
        McpCatalogCompleteness::Failed(_) => McpCatalogCompletenessDto::Failed,
        _ => McpCatalogCompletenessDto::Failed,
    }
}

pub(super) fn diagnostic_code(kind: McpCatalogDiagnosticKind) -> &'static str {
    match kind {
        McpCatalogDiagnosticKind::InvalidRawName => "invalidRawName",
        McpCatalogDiagnosticKind::DescriptionLimitExceeded => "descriptionLimitExceeded",
        McpCatalogDiagnosticKind::InvalidSchema => "invalidSchema",
        McpCatalogDiagnosticKind::DuplicateRawName => "duplicateRawName",
        McpCatalogDiagnosticKind::NormalizationCollision => "normalizationCollision",
        McpCatalogDiagnosticKind::ModelNameCollision => "modelNameCollision",
        McpCatalogDiagnosticKind::ReservedModelName => "reservedModelName",
        _ => "unknown",
    }
}

pub(super) fn path_text(
    path: &std::path::Path,
    operation: McpManagementOperationDto,
) -> Result<String, McpManagementFailure> {
    path.to_str().map(str::to_string).ok_or_else(|| {
        standalone_failure(
            operation,
            McpManagementErrorCodeDto::InvalidState,
            McpManagementRecoveryDto::DoNotRetry,
            "The persisted MCP path cannot be represented safely.",
            None,
        )
    })
}

pub(super) fn bounded_text(value: &str, max_bytes: usize) -> String {
    truncate_text(value, max_bytes).0
}

pub(super) fn safe_error_code(value: &str) -> String {
    let mut characters = value.chars();
    let valid = characters
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '-')
        })
        && value.len() <= 128;
    if valid {
        value.to_string()
    } else {
        "mcp.serverError".to_string()
    }
}

pub(super) fn truncate_text(value: &str, max_bytes: usize) -> (String, bool) {
    let mut sanitized_changed = false;
    let sanitized = value
        .chars()
        .map(|character| {
            if is_unsafe_renderer_text_character(character) {
                sanitized_changed = true;
                '\u{fffd}'
            } else {
                character
            }
        })
        .collect::<String>();
    if sanitized.len() <= max_bytes {
        return (sanitized, sanitized_changed);
    }
    let mut end = max_bytes.min(sanitized.len());
    while end > 0 && !sanitized.is_char_boundary(end) {
        end -= 1;
    }
    (sanitized[..end].to_string(), true)
}

pub(super) fn is_unsafe_renderer_text_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{00ad}'
                | '\u{0600}'..='\u{0605}'
                | '\u{061c}'
                | '\u{06dd}'
                | '\u{070f}'
                | '\u{0890}'..='\u{0891}'
                | '\u{08e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1345f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

pub(super) fn nonnegative_ms(value: i64) -> u64 {
    u64::try_from(value).unwrap_or_default()
}

pub(super) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
