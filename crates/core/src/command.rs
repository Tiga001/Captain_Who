use crate::system_paths::expand_system_path;
use crate::{
    AgentCancellationToken, AgentCommandRequest, AgentCommandRiskLevel, AgentPermissions,
    AgentWritePermission,
};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandExecutionResult {
    pub command: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CommandRunState {
    inner: Arc<CommandRunStateInner>,
}

#[derive(Debug, Default)]
struct CommandRunStateInner {
    running: Mutex<HashMap<String, RunningCommand>>,
}

#[derive(Debug, Clone)]
struct RunningCommand {
    run_id: String,
    cancel_flag: Arc<AtomicBool>,
}

pub struct CommandRunGuard {
    action_id: String,
    cancel_flag: Arc<AtomicBool>,
    state: CommandRunState,
}

impl CommandRunState {
    pub fn register(&self, action_id: &str, run_id: &str) -> CommandRunGuard {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        running.insert(
            action_id.to_string(),
            RunningCommand {
                run_id: run_id.to_string(),
                cancel_flag: cancel_flag.clone(),
            },
        );

        CommandRunGuard {
            action_id: action_id.to_string(),
            cancel_flag,
            state: self.clone(),
        }
    }

    pub fn cancel(&self, action_id: &str) -> bool {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(command) = running.get(action_id) else {
            return false;
        };

        command.cancel_flag.store(true, Ordering::SeqCst);
        true
    }

    pub fn cancel_run(&self, run_id: &str) -> usize {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut cancelled = 0;
        for command in running.values().filter(|command| command.run_id == run_id) {
            command.cancel_flag.store(true, Ordering::SeqCst);
            cancelled += 1;
        }
        cancelled
    }

    fn unregister(&self, action_id: &str) {
        let mut running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        running.remove(action_id);
    }
}

impl CommandRunGuard {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }
}

impl Drop for CommandRunGuard {
    fn drop(&mut self) {
        self.state.unregister(&self.action_id);
    }
}

pub fn run_approved_command(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, String> {
    validate_command_request(request)?;
    let root = workspace_root
        .map(canonicalize_workspace_root)
        .transpose()?;
    let cwd = resolve_command_cwd(root.as_deref(), request.cwd.as_deref(), permissions.write)?;

    run_shell_command(
        &cwd,
        root.as_deref(),
        request,
        cancellation_token,
        action_cancel_flag,
    )
}

fn canonicalize_workspace_root(path: &Path) -> Result<PathBuf, String> {
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

fn resolve_command_cwd(
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

fn clean_relative_path(path: &str) -> Result<PathBuf, String> {
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

fn validate_command_request(request: &AgentCommandRequest) -> Result<(), String> {
    let command = request.command.trim();
    if command.is_empty() {
        return Err("命令不能为空。".to_string());
    }
    if command.contains('\0') || command.contains('\n') || command.contains('\r') {
        return Err("命令不能包含空字符或换行符。".to_string());
    }

    let lower = command.to_ascii_lowercase();
    let tokens = shell_like_tokens(&lower);
    let program = tokens
        .first()
        .map(String::as_str)
        .unwrap_or_default()
        .trim_matches(|character| matches!(character, '"' | '\''));

    if has_blocked_risk_level(request.risk_level) {
        return Err("命令风险等级过高，已被安全策略阻止。".to_string());
    }
    if has_blocked_command_pattern(&lower, program, &tokens) {
        return Err("命令包含被阻止的高风险操作。".to_string());
    }
    if has_interactive_pattern(program, &tokens) {
        return Err("run_command 只允许非交互命令。请使用右侧栏终端执行交互式 shell。".to_string());
    }
    if has_shell_write_operator(&lower) {
        return Err(
            "run_command 不允许使用 shell 重定向或写文件操作；请使用 apply_patch 修改文件。"
                .to_string(),
        );
    }

    Ok(())
}

fn has_blocked_risk_level(risk_level: Option<AgentCommandRiskLevel>) -> bool {
    matches!(
        risk_level,
        Some(AgentCommandRiskLevel::Destructive) | Some(AgentCommandRiskLevel::Network)
    )
}

fn has_blocked_command_pattern(command: &str, program: &str, tokens: &[String]) -> bool {
    matches!(
        program,
        "sudo"
            | "touch"
            | "mkdir"
            | "mv"
            | "cp"
            | "rm"
            | "ln"
            | "truncate"
            | "diskutil"
            | "launchctl"
            | "dd"
            | "mkfs"
            | "shutdown"
            | "reboot"
            | "halt"
            | "poweroff"
    ) || program == "rm" && has_recursive_force(tokens)
        || matches!(program, "chmod" | "chown") && has_recursive_flag(tokens)
        || program == "git" && has_blocked_git_pattern(tokens)
        || has_package_install_pattern(tokens)
        || command.contains(" rm -rf /")
        || command.contains(" rm -fr /")
        || command.starts_with("rm -rf /")
        || command.starts_with("rm -fr /")
}

fn has_recursive_force(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        let token = token.as_str();
        token == "-rf"
            || token == "-fr"
            || token == "-r"
            || token == "-f"
            || token.starts_with("-") && token.contains('r') && token.contains('f')
    })
}

fn has_recursive_flag(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        let token = token.to_ascii_lowercase();
        token == "-r" || token.starts_with("-r")
    })
}

fn has_blocked_git_pattern(tokens: &[String]) -> bool {
    matches!(
        (
            tokens.get(1).map(String::as_str),
            tokens.get(2).map(String::as_str)
        ),
        (Some("reset"), Some("--hard"))
    ) || matches!(
        tokens.get(1).map(String::as_str),
        Some("clean") | Some("push")
    )
}

fn has_package_install_pattern(tokens: &[String]) -> bool {
    matches!(
        (
            tokens.first().map(String::as_str),
            tokens.get(1).map(String::as_str)
        ),
        (Some("npm"), Some("install"))
            | (Some("npm"), Some("i"))
            | (Some("npm"), Some("ci"))
            | (Some("pnpm"), Some("install"))
            | (Some("pnpm"), Some("i"))
            | (Some("pnpm"), Some("add"))
            | (Some("yarn"), Some("install"))
            | (Some("yarn"), Some("add"))
            | (Some("pip"), Some("install"))
            | (Some("pip3"), Some("install"))
            | (Some("brew"), Some("install"))
            | (Some("cargo"), Some("install"))
    ) || matches!(
        (
            tokens.first().map(String::as_str),
            tokens.get(1).map(String::as_str),
            tokens.get(2).map(String::as_str),
            tokens.get(3).map(String::as_str)
        ),
        (Some("python"), Some("-m"), Some("pip"), Some("install"))
            | (Some("python3"), Some("-m"), Some("pip"), Some("install"))
    )
}

fn has_interactive_pattern(program: &str, tokens: &[String]) -> bool {
    matches!(
        program,
        "ssh" | "vim" | "vi" | "nano" | "less" | "more" | "top" | "htop" | "watch"
    ) || matches!(program, "python" | "python3" | "node") && tokens.len() == 1
        || tokens
            .iter()
            .skip(1)
            .any(|token| token == "-i" || token == "--interactive")
}

fn has_shell_write_operator(command: &str) -> bool {
    command.contains(" >")
        || command.contains(">")
        || command.contains(" tee ")
        || command.contains(" tee -")
        || command.contains(" sed -i")
        || command.contains(" perl -pi")
}

fn shell_like_tokens(command: &str) -> Vec<String> {
    command
        .split_whitespace()
        .map(|token| token.trim_matches(|character| matches!(character, '"' | '\'' | ';')))
        .filter(|token| !token.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn run_shell_command(
    cwd: &Path,
    root: Option<&Path>,
    request: &AgentCommandRequest,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, String> {
    let timeout_ms = request
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let started = Instant::now();
    let mut command = shell_command(&request.command);
    let mut child = command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TERM", "dumb")
        .env("CI", "1")
        .spawn()
        .map_err(|error| format!("启动命令失败：{error}"))?;

    let deadline = Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let mut cancelled = false;
    loop {
        if cancellation_token.is_cancelled()
            || action_cancel_flag
                .as_ref()
                .map(|flag| flag.load(Ordering::SeqCst))
                .unwrap_or(false)
        {
            cancelled = true;
            let _ = child.kill();
            break;
        }

        match child
            .try_wait()
            .map_err(|error| format!("等待命令失败：{error}"))?
        {
            Some(_) => break,
            None if started.elapsed() >= deadline => {
                timed_out = true;
                let _ = child.kill();
                break;
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|error| format!("读取命令输出失败：{error}"))?;
    let (stdout, stdout_truncated) = truncate_output(&output.stdout);
    let (stderr, stderr_truncated) = truncate_output(&output.stderr);

    Ok(AgentCommandExecutionResult {
        command: request.command.clone(),
        cwd: relative_cwd(root, cwd),
        exit_code: output.status.code(),
        stdout,
        stderr,
        timed_out,
        cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated,
        stderr_truncated,
        error: None,
    })
}

#[cfg(target_os = "windows")]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("cmd.exe");
    shell.arg("/C").arg(command);
    shell
}

#[cfg(not(target_os = "windows"))]
fn shell_command(command: &str) -> Command {
    let mut shell = Command::new("/bin/sh");
    shell.arg("-lc").arg(command);
    shell
}

fn relative_cwd(root: Option<&Path>, cwd: &Path) -> String {
    let Some(root) = root else {
        return cwd.to_string_lossy().to_string();
    };

    match cwd.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => relative.to_string_lossy().to_string(),
        Err(_) => cwd.to_string_lossy().to_string(),
    }
}

fn truncate_output(output: &[u8]) -> (String, bool) {
    if output.len() <= MAX_OUTPUT_BYTES {
        return (String::from_utf8_lossy(output).to_string(), false);
    }

    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && std::str::from_utf8(&output[..end]).is_err() {
        end -= 1;
    }
    let mut value = String::from_utf8_lossy(&output[..end]).to_string();
    value.push_str("\n...[truncated]");
    (value, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentApprovalStatus, AgentCommandPermission, AgentReadPermission};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn rejects_cwd_outside_workspace() {
        let workspace = TestWorkspace::new();
        let error = resolve_command_cwd(
            Some(&workspace.path),
            Some("../outside"),
            AgentWritePermission::WorkspaceOnly,
        )
        .unwrap_err();

        assert!(error.contains(".."));
    }

    #[test]
    fn rejects_absolute_cwd_outside_workspace_without_full_write() {
        let workspace = TestWorkspace::new();
        let outside = TestWorkspace::new();

        let error = resolve_command_cwd(
            Some(&workspace.path),
            Some(&outside.path.to_string_lossy()),
            AgentWritePermission::WorkspaceOnly,
        )
        .unwrap_err();

        assert!(error.contains("write=all"));
    }

    #[test]
    fn allows_absolute_cwd_outside_workspace_with_full_write() {
        let workspace = TestWorkspace::new();
        let outside = TestWorkspace::new();

        let resolved = resolve_command_cwd(
            Some(&workspace.path),
            Some(&outside.path.to_string_lossy()),
            AgentWritePermission::All,
        )
        .unwrap();

        assert_eq!(resolved, outside.path);
    }

    #[test]
    fn blocks_dangerous_commands() {
        for command in [
            "rm -rf /",
            "sudo whoami",
            "git reset --hard",
            "git clean -fd",
            "git push",
            "pnpm add left-pad",
            "npm install",
            "python -m pip install requests",
            "cargo install ripgrep",
        ] {
            let error = validate_command_request(&request(command, None)).unwrap_err();
            assert!(
                error.contains("阻止") || error.contains("风险"),
                "{command}: {error}"
            );
        }
    }

    #[test]
    fn runs_simple_command_in_workspace() {
        let workspace = TestWorkspace::new();
        let result = run_approved_command(
            Some(&workspace.path),
            &request("printf hello", Some(5_000)),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "hello");
        assert_eq!(result.cwd, ".");
        assert!(!result.cancelled);
    }

    #[test]
    fn runs_simple_command_outside_workspace_with_full_write() {
        let workspace = TestWorkspace::new();
        let outside = TestWorkspace::new();
        let mut request = request("printf outside", Some(5_000));
        request.cwd = Some(outside.path.to_string_lossy().to_string());

        let result = run_approved_command(
            Some(&workspace.path),
            &request,
            AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "outside");
        assert_eq!(result.cwd, outside.path.to_string_lossy());
    }

    #[test]
    fn times_out_long_command() {
        let workspace = TestWorkspace::new();
        let result = run_approved_command(
            Some(&workspace.path),
            &request("sleep 2", Some(50)),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert!(result.timed_out);
    }

    fn request(command: &str, timeout_ms: Option<u64>) -> AgentCommandRequest {
        AgentCommandRequest {
            id: "tool-1".to_string(),
            command: command.to_string(),
            cwd: None,
            timeout_ms,
            approval_status: AgentApprovalStatus::Required,
            risk_level: Some(AgentCommandRiskLevel::ReadOnly),
            reason: None,
        }
    }

    struct TestWorkspace {
        path: PathBuf,
    }

    impl TestWorkspace {
        fn new() -> Self {
            let unique = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("mycopilot-command-test-{unique}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).unwrap();
            let path = path.canonicalize().unwrap();

            Self { path }
        }
    }

    impl Drop for TestWorkspace {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}
