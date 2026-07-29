use super::*;

pub(crate) fn workspace_root_optional(input: &AgentChatInput) -> Option<PathBuf> {
    input
        .context
        .as_ref()
        .and_then(|context| context.workspace.as_ref())
        .and_then(|workspace| workspace.root_path.as_ref())
        .map(PathBuf::from)
}

pub(crate) fn permissions_from_input(input: &AgentChatInput) -> AgentPermissions {
    input
        .context
        .as_ref()
        .map(|context| context.permissions)
        .unwrap_or_default()
}

pub(crate) fn create_conversation_title(message: &str) -> String {
    let first_line = message
        .lines()
        .next()
        .unwrap_or("新对话")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if first_line.chars().count() > 24 {
        format!("{}...", first_line.chars().take(24).collect::<String>())
    } else if first_line.is_empty() {
        "新对话".to_string()
    } else {
        first_line
    }
}

pub(crate) fn create_id(prefix: &str) -> String {
    let counter = ID_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{}-{counter}", now_ms())
}

pub(crate) fn safe_path_component(value: &str, fallback: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|character| match character {
            '/' | '\\' | ':' | '\0' => '_',
            character if character.is_control() => '_',
            character => character,
        })
        .collect::<String>();

    let sanitized = sanitized.trim();
    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        fallback.to_string()
    } else {
        sanitized.to_string()
    }
}

pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0)
}

pub(crate) fn serialize_json<T: Serialize>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_else(|error| {
        json!({
            "serializationError": error.to_string()
        })
        .to_string()
    })
}
