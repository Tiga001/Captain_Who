use super::*;

pub(super) fn enforce_command_policy(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> Result<(), CommandExecutionError> {
    let evaluation = evaluate_command_policy_at(
        command,
        permissions,
        authorization_source,
        workspace_root,
        cwd,
    );
    match evaluation.decision {
        CommandPolicyDecision::Allow => Ok(()),
        CommandPolicyDecision::RequireExplicitApproval | CommandPolicyDecision::Deny => {
            Err(CommandExecutionError::from_policy(evaluation))
        }
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

pub(super) fn resolve_command_cwd(
    root: Option<&Path>,
    cwd: Option<&str>,
    write_permission: AgentWritePermission,
) -> Result<PathBuf, String> {
    let Some(raw_cwd) = cwd
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != ".")
    else {
        return root.map(Path::to_path_buf).ok_or_else(|| {
            "当前没有 workspace；命令必须提供绝对 cwd 或系统路径别名。".to_string()
        });
    };

    let expanded = expand_system_path(raw_cwd)?;
    let cwd_path = expanded.as_deref().unwrap_or_else(|| Path::new(raw_cwd));
    let resolved = if cwd_path.is_absolute() {
        cwd_path.to_path_buf()
    } else {
        let Some(root) = root else {
            return Err("没有 workspace 时，命令 cwd 必须是绝对路径或系统路径别名。".to_string());
        };
        root.join(clean_relative_path(raw_cwd)?)
    };

    let canonical = resolved
        .canonicalize()
        .map_err(|error| format!("命令工作目录不可访问：{error}"))?;
    if !canonical.is_dir() {
        return Err("命令工作目录不是目录。".to_string());
    }

    if let Some(root) = root {
        if !canonical.starts_with(root) && write_permission != AgentWritePermission::All {
            return Err(
                "命令工作目录必须位于已选择的 workspace 内；workspace 外 cwd 需要 write=all 权限。"
                    .to_string(),
            );
        }
    } else if write_permission != AgentWritePermission::All {
        return Err("没有 workspace 时，命令 cwd 需要 write=all 权限。".to_string());
    }

    Ok(canonical)
}

pub(super) fn clean_relative_path(path: &str) -> Result<PathBuf, String> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err("相对 cwd 不能是绝对路径。".to_string());
    }

    let mut cleaned = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => cleaned.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("命令 cwd 不能包含 `..`。".to_string()),
            _ => return Err("命令 cwd 包含不支持的路径片段。".to_string()),
        }
    }

    if cleaned.as_os_str().is_empty() {
        Err("命令 cwd 不能为空。".to_string())
    } else {
        Ok(cleaned)
    }
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
