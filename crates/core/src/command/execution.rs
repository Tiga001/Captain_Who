use super::*;

pub fn run_authorized_command(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_authorized_command_with_output_observer(
        workspace_root,
        request,
        permissions,
        authorization_source,
        cancellation_token,
        action_cancel_flag,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_authorized_command_with_output_observer(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    output_observer: Option<ProcessOutputObserver>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    if request.runtime.is_some() || request.runtime_binding.is_some() || !request.inputs.is_empty()
    {
        // A host-bound runtime or file input must never be interpreted as an ordinary PATH/shell
        // command by a compatibility caller that has not supplied the managed provider.
        return run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer(
            workspace_root,
            request,
            permissions,
            authorization_source,
            cancellation_token,
            action_cancel_flag,
            None,
            None,
            output_observer,
        );
    }
    let root = workspace_root
        .map(canonicalize_workspace_root)
        .transpose()?;
    let cwd = resolve_command_cwd(root.as_deref(), request.cwd.as_deref(), permissions.write)?;
    // Re-evaluate against the resolved cwd immediately before spawning the shell. In particular,
    // this prevents a relative destructive target such as `rm -rf .` from becoming catastrophic
    // when a caller selects a filesystem root as cwd. The request's declared `risk_level` is
    // intentionally ignored; it is display metadata, not an authorization capability.
    enforce_command_policy(
        &request.command,
        permissions,
        authorization_source,
        root.as_deref(),
        Some(&cwd),
    )?;

    // Artifact observation deliberately starts only after authoritative command policy succeeds.
    // It is best-effort telemetry around the exact same process execution and never grants path,
    // write, or command authority.
    let observer = CommandArtifactObserver::prepare(
        root.as_deref(),
        &cwd,
        request.observe.as_ref(),
        permissions,
    );
    let observation_cancellation = cancellation_token.clone();
    let before = observer.as_ref().map(|observer| {
        observer.capture(
            AgentCommandArtifactObservationPhase::Before,
            Some(&observation_cancellation),
        )
    });
    let execution = run_shell_command_with_output_observer(
        &cwd,
        root.as_deref(),
        request,
        cancellation_token,
        action_cancel_flag,
        output_observer,
    );
    let artifact_observation = observer.as_ref().zip(before).map(|(observer, before)| {
        // The process may have failed, timed out, or been cancelled after producing a file.
        // Always perform the independently bounded after snapshot instead of inheriting the
        // process cancellation token.
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        observer.finish(before, after)
    });
    match execution {
        Ok(mut result) => {
            result.artifact_observation = artifact_observation;
            Ok(result)
        }
        Err(error) => {
            Err(CommandExecutionError::from(error).with_artifact_observation(artifact_observation))
        }
    }
}

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

#[cfg(test)]
pub(super) fn run_shell_command(
    cwd: &Path,
    root: Option<&Path>,
    request: &AgentCommandRequest,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, String> {
    run_shell_command_with_output_observer(
        cwd,
        root,
        request,
        cancellation_token,
        action_cancel_flag,
        None,
    )
}

pub(super) fn run_shell_command_with_output_observer(
    cwd: &Path,
    root: Option<&Path>,
    request: &AgentCommandRequest,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    output_observer: Option<ProcessOutputObserver>,
) -> Result<AgentCommandExecutionResult, String> {
    // An approval can be cancelled after the backend registers the frozen action but before its
    // worker reaches this blocking executor. Consume that intent before spawning a process so a
    // successfully acknowledged pre-start cancellation cannot produce side effects.
    if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return Ok(AgentCommandExecutionResult {
            command: request.command.clone(),
            cwd: relative_cwd(root, cwd),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled: true,
            duration_ms: 0,
            stdout_truncated: false,
            stderr_truncated: false,
            output_capture: ProcessOutputCaptureMetadata::default(),
            stdout_spool: ProcessOutputSpool::default(),
            stderr_spool: ProcessOutputSpool::default(),
            error: None,
            policy_evaluation: None,
            artifact_observation: None,
            input_files: Vec::new(),
            runtime: None,
        });
    }
    // The managed-session kernel treats an absent hard timeout as genuinely
    // unbounded.  Only this legacy adapter injects the old 120 second default,
    // then waits through a Running handoff so the current product contract does
    // not change during round one.
    let hard_timeout = Duration::from_millis(
        request
            .timeout_ms
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .clamp(1, MAX_TIMEOUT_MS),
    );
    let plan = CommandSpawnPlan::shell(
        request.command.clone(),
        cwd.to_path_buf(),
        root,
        Some(hard_timeout),
    );
    let manager = CommandSessionManager::default();
    let scope = CommandSessionScopeId::new(format!("legacy:{}", request.id))
        .map_err(|error| error.to_string())?;
    let cancellation = cancellation_token.clone();
    let action_cancellation = action_cancel_flag.clone();
    let cancel_probe: Arc<dyn Fn() -> bool + Send + Sync> =
        Arc::new(move || command_cancel_requested(&cancellation, action_cancellation.as_ref()));
    let outcome = manager
        .start_plan(
            scope,
            plan,
            CommandStartOptions::default(),
            output_observer,
            Some(cancel_probe),
        )
        .map_err(|error| error.to_string())?;
    let session_id = match outcome {
        CommandStartOutcome::Exited(terminal) => return Ok(terminal.execution),
        CommandStartOutcome::Running(snapshot) => snapshot.session_id,
    };

    loop {
        if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
            manager
                .force_terminate(&session_id, Duration::from_secs(3))
                .map_err(|error| error.to_string())?;
        }
        if let Some(terminal) = manager
            .wait_terminal_result(&session_id, Duration::from_millis(50))
            .map_err(|error| error.to_string())?
        {
            return Ok(terminal.execution);
        }
    }
}

pub(super) fn command_cancel_requested(
    cancellation_token: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> bool {
    cancellation_token.is_cancelled()
        || action_cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
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
