use super::*;

pub(super) fn enforce_command_policy_in_workspace(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    workspace: &crate::workspace::WorkspaceResolver,
    cwd: Option<&Path>,
) -> Result<(), CommandExecutionError> {
    let evaluation = evaluate_command_policy_in_workspace(
        command,
        permissions,
        authorization_source,
        workspace,
        cwd,
    );
    match evaluation.decision {
        CommandPolicyDecision::Allow => Ok(()),
        _ => Err(CommandExecutionError::from_policy(evaluation)),
    }
}

pub(super) fn canonicalize_workspace_root(path: &Path) -> Result<PathBuf, String> {
    path.canonicalize()
        .map_err(|error| format!("workspace 不可访问：{error}"))
        .and_then(|path| {
            if path.is_dir() {
                Ok(path)
            } else {
                Err("workspace 路径不是目录。".to_string())
            }
        })
}

#[cfg(test)]
pub(super) fn resolve_command_cwd(
    root: Option<&Path>,
    cwd: Option<&str>,
    write_permission: AgentWritePermission,
) -> Result<PathBuf, String> {
    resolve_command_cwd_in_workspace(
        &crate::workspace::WorkspaceResolver::from_primary(root),
        cwd,
        write_permission,
    )
}

pub(crate) fn resolve_command_cwd_in_workspace(
    workspace: &crate::workspace::WorkspaceResolver,
    cwd: Option<&str>,
    write_permission: AgentWritePermission,
) -> Result<PathBuf, String> {
    let raw = cwd
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != ".");
    if raw.is_some_and(|raw| {
        !Path::new(raw).is_absolute()
            && Path::new(raw)
                .components()
                .any(|part| part == Component::ParentDir)
    }) {
        return Err("命令 cwd 不能包含 `..`。".to_string());
    }
    let resolved = match raw {
        Some(raw) => workspace.resolve_input(raw)?,
        None => workspace.canonical_primary()?,
    };
    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("命令工作目录不可访问：{error}"))?;
    if !canonical.is_dir() {
        return Err("命令工作目录不是目录。".to_string());
    }
    if workspace.containing_root(&canonical)?.is_none()
        && write_permission != AgentWritePermission::All
    {
        return Err(
            "命令工作目录必须位于已选择的 workspace 内；workspace 外 cwd 需要 write=all 权限。"
                .to_string(),
        );
    }
    Ok(canonical)
}

pub(crate) fn spawn_bounded_output_reader<R>(
    mut reader: R,
) -> thread::JoinHandle<std::io::Result<(String, bool)>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut retained = Vec::with_capacity(MAX_OUTPUT_BYTES);
        let mut buffer = [0_u8; 8 * 1024];
        let mut truncated = false;
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            let remaining = MAX_OUTPUT_BYTES.saturating_sub(retained.len());
            let retained_from_chunk = remaining.min(read);
            retained.extend_from_slice(&buffer[..retained_from_chunk]);
            truncated |= retained_from_chunk < read;
        }
        let mut output = String::from_utf8_lossy(&retained).into_owned();
        if truncated {
            output.push_str("\n...[truncated]");
        }
        Ok((output, truncated))
    })
}

pub(crate) fn join_output_reader(
    reader: thread::JoinHandle<std::io::Result<(String, bool)>>,
    stream: &str,
) -> Result<(String, bool), String> {
    reader
        .join()
        .map_err(|_| format!("读取命令 {stream} 的线程异常退出。"))?
        .map_err(|error| format!("读取命令 {stream} 失败：{error}"))
}

pub(super) fn relative_cwd(root: Option<&Path>, cwd: &Path) -> String {
    let Some(root) = root else {
        return cwd.to_string_lossy().to_string();
    };

    match cwd.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => relative.to_string_lossy().to_string(),
        Err(_) => cwd.to_string_lossy().to_string(),
    }
}
