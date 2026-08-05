//! Host-owned validation and normalization for persisted MCP configuration.

use super::*;

pub(super) fn normalize_new_config(
    config: McpServerConfig,
) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    let config = normalize_config(config)?;
    if config.enabled || config.trust != McpTrustLevel::Untrusted {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(config)
}

pub(super) fn normalize_config(
    mut config: McpServerConfig,
) -> Result<McpServerConfig, McpRegistryPersistenceError> {
    if config.scope != McpServerScope::User
        || config.display_name.trim().is_empty()
        || config.display_name.len() > MAX_DISPLAY_NAME_BYTES
        || config
            .display_name
            .chars()
            .any(is_unsafe_display_name_character)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    if !matches!(
        config.trust,
        McpTrustLevel::Untrusted | McpTrustLevel::UserApproved
    ) || !(1..=MAX_CONNECT_TIMEOUT_MS).contains(&config.connect_timeout_ms)
        || !(1..=MAX_REQUEST_TIMEOUT_MS).contains(&config.request_timeout_ms)
        || !(1..=MAX_SHUTDOWN_TIMEOUT_MS).contains(&config.shutdown_timeout_ms)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let McpTransportConfig::Stdio(stdio) = &mut config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    if !stdio.environment.is_empty()
        || stdio.arguments.len() > MAX_ARGUMENTS
        || stdio
            .arguments
            .iter()
            .any(|argument| argument.len() > MAX_ARGUMENT_BYTES || argument.contains('\0'))
        || stdio
            .arguments
            .iter()
            .map(String::len)
            .try_fold(0usize, usize::checked_add)
            .is_none_or(|total| total > MAX_ARGUMENTS_TOTAL_BYTES)
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    stdio.program = normalize_absolute_path(&stdio.program)?;
    stdio.cwd = normalize_absolute_path(&stdio.cwd)?;
    if path_text(&stdio.program)?.len() > MAX_PATH_BYTES
        || path_text(&stdio.cwd)?.len() > MAX_PATH_BYTES
    {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(config)
}

fn is_unsafe_display_name_character(character: char) -> bool {
    character.is_control()
        || matches!(
            character,
            '\u{0085}'
                | '\u{061c}'
                | '\u{200b}'..='\u{200f}'
                | '\u{2028}'..='\u{202e}'
                | '\u{2060}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
        )
}

pub(super) fn normalize_absolute_path(path: &Path) -> Result<PathBuf, McpRegistryPersistenceError> {
    if !path.is_absolute() {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() || normalized.as_os_str().is_empty() {
                    return Err(McpRegistryPersistenceError::InvalidConfig);
                }
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    if !normalized.is_absolute() {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    path_text(&normalized)?;
    Ok(normalized)
}

pub(super) fn path_text(path: &Path) -> Result<&str, McpRegistryPersistenceError> {
    path.to_str()
        .filter(|value| !value.contains('\0'))
        .ok_or(McpRegistryPersistenceError::InvalidConfig)
}
