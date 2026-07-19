use crate::system_paths::expand_system_path;
use crate::{
    AgentCancellationToken, AgentCommandRequest, AgentCommandRiskLevel, AgentCommandSafetyPolicy,
    AgentPermissions, AgentReadPermission, AgentWritePermission,
};
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;
const MAX_OUTPUT_BYTES: usize = 128 * 1024;
pub(crate) const MAX_COMMAND_CHARS: usize = 2_000;

/// Identifies who authorized a command execution.
///
/// This is deliberately separate from the approval status stored on a request: an automatically
/// approved action and a user-approved action may both be represented as `Approved` by the wire
/// protocol, but they must not receive the same command privileges.
///
/// The trusted host must choose this value from its own approval state. It must never be accepted
/// from model arguments, renderer payloads, or another untrusted request field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAuthorizationSource {
    Automatic,
    ExplicitUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicyDecision {
    Allow,
    RequireExplicitApproval,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRiskClass {
    ReadOnly,
    SafeWorkspaceWrite,
    DirectWrite,
    Network,
    PackageManagement,
    HighImpact,
    Unknown,
    Catastrophic,
    Unsupported,
    ExternalRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyFinding {
    pub segment_index: usize,
    pub program: String,
    pub risk: CommandRiskClass,
    /// Stable, machine-readable identifier. Callers must branch on this rather than `reason`.
    pub code: String,
    /// Human-readable diagnostic. This is not a wire-level discriminator.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyEvaluation {
    pub decision: CommandPolicyDecision,
    /// Stable, machine-readable summary code.
    pub code: String,
    pub reason: String,
    pub risk_level: AgentCommandRiskLevel,
    pub findings: Vec<CommandPolicyFinding>,
}

/// Failure returned before a command process could be started.
///
/// Operational failures retain a human-readable message. Policy failures additionally carry the
/// authoritative structured evaluation so host adapters can preserve stable codes and findings
/// all the way to the Tool Result instead of parsing localized error text.
#[derive(Debug, Clone)]
pub struct CommandExecutionError {
    message: String,
    policy_evaluation: Option<CommandPolicyEvaluation>,
}

impl CommandExecutionError {
    fn from_policy(evaluation: CommandPolicyEvaluation) -> Self {
        let prefix = match evaluation.decision {
            CommandPolicyDecision::RequireExplicitApproval => "命令需要用户明确批准后才能执行",
            CommandPolicyDecision::Deny => "命令已被安全策略拒绝",
            CommandPolicyDecision::Allow => "命令策略校验失败",
        };
        Self {
            message: format!("{prefix}（{}）：{}", evaluation.code, evaluation.reason),
            policy_evaluation: Some(evaluation),
        }
    }

    pub fn policy_evaluation(&self) -> Option<&CommandPolicyEvaluation> {
        self.policy_evaluation.as_ref()
    }
}

impl From<String> for CommandExecutionError {
    fn from(message: String) -> Self {
        Self {
            message,
            policy_evaluation: None,
        }
    }
}

impl std::fmt::Display for CommandExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandExecutionError {}

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_evaluation: Option<CommandPolicyEvaluation>,
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

pub fn run_authorized_command(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
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

    run_shell_command(
        &cwd,
        root.as_deref(),
        request,
        cancellation_token,
        action_cancel_flag,
    )
    .map_err(CommandExecutionError::from)
}

fn enforce_command_policy(
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

const MAX_POLICY_NESTING: usize = 8;

#[derive(Debug)]
struct ShellSegment {
    tokens: Vec<String>,
    has_write_redirection: bool,
    input_redirections: Vec<ShellInputRedirection>,
}

#[derive(Debug)]
struct ShellInputRedirection {
    target: String,
    dynamic: bool,
}

#[derive(Debug)]
struct LexedCommand {
    segments: Vec<ShellSegment>,
    embedded_commands: Vec<String>,
}

#[derive(Debug)]
struct CommandSyntaxError {
    code: &'static str,
    reason: &'static str,
}

/// Evaluates a command using the same classifier used to populate proposal risk metadata.
///
/// `RequireExplicitApproval` is a routing decision, not an execution failure: callers should
/// surface an approval request and call [`run_authorized_command`] again with
/// [`CommandAuthorizationSource::ExplicitUser`] only after the exact frozen action is approved.
///
/// This compatibility entry point performs command classification with unrestricted read scope.
/// Runtime authorization should use [`evaluate_command_policy_with_context`] so permission and
/// workspace/cwd scope are enforced together.
///
/// This is a conservative shell-envelope policy, not an OS sandbox. Opaque Python/Node code is
/// classified as `Unknown`: guarded automatic execution routes it to a human, while full access or
/// explicit approval deliberately trusts that code. Shell constructs that would obscure the
/// executable itself remain `Unsupported` under every authorization source. A small allowlist of
/// conventional local build/test tasks is classified as `SafeWorkspaceWrite`; this intentionally
/// trusts the selected workspace's build definitions (for example `build.rs` or package scripts).
/// On Windows, execution fails closed until the classifier and process-tree termination have
/// equivalent native implementations (including Job Object containment).
pub fn evaluate_command_policy(
    command: &str,
    safety_policy: AgentCommandSafetyPolicy,
    authorization_source: CommandAuthorizationSource,
) -> CommandPolicyEvaluation {
    evaluate_command_policy_at(
        command,
        AgentPermissions {
            read: AgentReadPermission::All,
            command_safety: safety_policy,
            ..AgentPermissions::default()
        },
        authorization_source,
        None,
        None,
    )
}

/// Evaluates the command against the complete trusted permission snapshot used for execution.
///
/// In particular, `read=workspace_only` prevents obvious workspace-external reads from being
/// silently auto-approved. The trusted host must supply this snapshot; command arguments cannot
/// widen it.
pub fn evaluate_command_policy_with_permissions(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
) -> CommandPolicyEvaluation {
    evaluate_command_policy_at(command, permissions, authorization_source, None, None)
}

/// Evaluates a command with the lexical workspace/cwd context available during runtime preflight.
/// Execution repeats this check with canonical paths immediately before process creation.
pub fn evaluate_command_policy_with_context(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> CommandPolicyEvaluation {
    evaluate_command_policy_at(
        command,
        permissions,
        authorization_source,
        workspace_root,
        cwd,
    )
}

#[cfg(windows)]
fn evaluate_command_policy_at(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> CommandPolicyEvaluation {
    let _ = (
        command,
        permissions,
        authorization_source,
        workspace_root,
        cwd,
    );
    CommandPolicyEvaluation {
        decision: CommandPolicyDecision::Deny,
        code: "command.unsupported.windows_execution".to_string(),
        reason: "Windows 命令执行尚未具备等价的分类器和 Job Object 进程树约束。".to_string(),
        risk_level: AgentCommandRiskLevel::Unknown,
        findings: vec![CommandPolicyFinding {
            segment_index: 0,
            program: String::new(),
            risk: CommandRiskClass::Unsupported,
            code: "command.unsupported.windows_execution".to_string(),
            reason: "Windows 命令执行尚未具备等价的分类器和 Job Object 进程树约束。".to_string(),
        }],
    }
}

#[cfg(not(windows))]
fn evaluate_command_policy_at(
    command: &str,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    workspace_root: Option<&Path>,
    cwd: Option<&Path>,
) -> CommandPolicyEvaluation {
    let mut findings = match analyze_command(command, permissions.read, cwd, 0) {
        Ok(findings) => findings,
        Err(error) => {
            return CommandPolicyEvaluation {
                decision: CommandPolicyDecision::Deny,
                code: error.code.to_string(),
                reason: error.reason.to_string(),
                risk_level: AgentCommandRiskLevel::Unknown,
                findings: vec![CommandPolicyFinding {
                    segment_index: 0,
                    program: String::new(),
                    risk: CommandRiskClass::Unsupported,
                    code: error.code.to_string(),
                    reason: error.reason.to_string(),
                }],
            };
        }
    };
    if permissions.read == AgentReadPermission::WorkspaceOnly
        && cwd.is_some_and(|cwd| workspace_root.is_none_or(|root| !cwd.starts_with(root)))
    {
        findings.push(finding(
            0,
            "<cwd>",
            CommandRiskClass::ExternalRead,
            "command.scope.external_cwd",
            "read=workspace_only 下，命令工作目录位于 workspace 外。",
        ));
    }
    let risk_level = aggregate_risk_level(&findings);

    if let Some(finding) = findings.iter().find(|finding| {
        matches!(
            finding.risk,
            CommandRiskClass::Catastrophic | CommandRiskClass::Unsupported
        )
    }) {
        return CommandPolicyEvaluation {
            decision: CommandPolicyDecision::Deny,
            code: finding.code.clone(),
            reason: finding.reason.clone(),
            risk_level,
            findings,
        };
    }

    let guarded_automatic = permissions.command_safety == AgentCommandSafetyPolicy::Guarded
        && authorization_source == CommandAuthorizationSource::Automatic;
    if guarded_automatic {
        let approval_findings = findings
            .iter()
            .filter(|finding| requires_explicit_approval(finding.risk))
            .collect::<Vec<_>>();
        if !approval_findings.is_empty() {
            let reason = approval_findings
                .iter()
                .map(|finding| finding.reason.as_str())
                .collect::<Vec<_>>()
                .join("；");
            return CommandPolicyEvaluation {
                decision: CommandPolicyDecision::RequireExplicitApproval,
                code: "command.explicit_approval_required".to_string(),
                reason,
                risk_level,
                findings,
            };
        }
    }

    CommandPolicyEvaluation {
        decision: CommandPolicyDecision::Allow,
        code: "command.allowed".to_string(),
        reason: "命令符合当前授权策略。".to_string(),
        risk_level,
        findings,
    }
}

/// Returns display metadata from the authoritative parser/classifier. It never trusts a model- or
/// client-supplied risk label.
pub fn classify_command_risk(command: &str) -> AgentCommandRiskLevel {
    evaluate_command_policy(
        command,
        AgentCommandSafetyPolicy::FullAccess,
        CommandAuthorizationSource::Automatic,
    )
    .risk_level
}

fn requires_explicit_approval(risk: CommandRiskClass) -> bool {
    matches!(
        risk,
        CommandRiskClass::DirectWrite
            | CommandRiskClass::Network
            | CommandRiskClass::PackageManagement
            | CommandRiskClass::HighImpact
            | CommandRiskClass::Unknown
            | CommandRiskClass::ExternalRead
    )
}

fn analyze_command(
    command: &str,
    read_permission: AgentReadPermission,
    cwd: Option<&Path>,
    depth: usize,
) -> Result<Vec<CommandPolicyFinding>, CommandSyntaxError> {
    if depth > MAX_POLICY_NESTING {
        return Err(CommandSyntaxError {
            code: "command.unsupported.nesting_limit",
            reason: "命令的嵌套层级超过安全分析上限。",
        });
    }
    validate_command_shape(command)?;
    validate_write_redirection_targets(command)?;
    let lexed = lex_command(command)?;
    let mut findings = Vec::new();

    for (segment_index, segment) in lexed.segments.iter().enumerate() {
        assess_segment(
            segment,
            segment_index,
            read_permission,
            cwd,
            depth,
            &mut findings,
        )?;
    }
    for embedded in lexed.embedded_commands {
        let offset = findings.len();
        let mut nested = analyze_command(&embedded, read_permission, cwd, depth + 1)?;
        for finding in &mut nested {
            finding.segment_index += offset;
        }
        findings.extend(nested);
    }

    Ok(findings)
}

fn validate_command_shape(command: &str) -> Result<(), CommandSyntaxError> {
    let command = command.trim();
    if command.is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.empty",
            reason: "命令不能为空。",
        });
    }
    if command.chars().count() > MAX_COMMAND_CHARS {
        return Err(CommandSyntaxError {
            code: "command.malformed.too_long",
            reason: "命令超过允许的长度上限。",
        });
    }
    if command.contains('\0') || command.contains('\n') || command.contains('\r') {
        return Err(CommandSyntaxError {
            code: "command.malformed.control_character",
            reason: "命令不能包含空字符或换行符。",
        });
    }
    Ok(())
}

fn validate_write_redirection_targets(command: &str) -> Result<(), CommandSyntaxError> {
    let chars = command.chars().collect::<Vec<_>>();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if escaped {
            escaped = false;
            index += 1;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            index += 1;
            continue;
        }
        if character != '>' {
            index += 1;
            continue;
        }

        index += 1;
        if chars.get(index) == Some(&'>') {
            index += 1;
        }
        if chars.get(index) == Some(&'&') {
            let target_start = index + 1;
            if let Some(next) = consume_fd_duplication(&chars, target_start) {
                index = next;
                continue;
            }
            index = target_start;
        }
        while chars.get(index).is_some_and(|value| value.is_whitespace()) {
            index += 1;
        }
        let (target, dynamic, next) = read_redirection_target(&chars, index)?;
        index = next;
        if dynamic {
            return Err(CommandSyntaxError {
                code: "command.unsupported.dynamic_redirection_target",
                reason: "无法静态核验动态生成的 shell 重定向目标。",
            });
        }
        if is_catastrophic_write_target(&target) {
            return Err(CommandSyntaxError {
                code: "command.catastrophic.storage_device",
                reason: "命令可能通过 shell 重定向直接覆盖存储设备。",
            });
        }
    }
    Ok(())
}

fn consume_fd_duplication(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) == Some(&'-') && is_shell_boundary(chars.get(start + 1).copied()) {
        return Some(start + 1);
    }
    let mut index = start;
    while chars.get(index).is_some_and(|value| value.is_ascii_digit()) {
        index += 1;
    }
    (index > start && is_shell_boundary(chars.get(index).copied())).then_some(index)
}

fn is_shell_boundary(character: Option<char>) -> bool {
    character.is_none_or(|value| value.is_whitespace() || matches!(value, ';' | '|' | '&' | '>'))
}

fn read_redirection_target(
    chars: &[char],
    start: usize,
) -> Result<(String, bool, usize), CommandSyntaxError> {
    let Some(first) = chars.get(start).copied() else {
        return Err(CommandSyntaxError {
            code: "command.malformed.redirection_target_missing",
            reason: "shell 输出重定向缺少目标。",
        });
    };
    if matches!(first, ';' | '|' | '&') {
        return Err(CommandSyntaxError {
            code: "command.malformed.redirection_target_missing",
            reason: "shell 输出重定向缺少目标。",
        });
    }

    let mut value = String::new();
    let mut dynamic = false;
    let mut quote = None;
    let mut escaped = false;
    let mut index = start;
    while index < chars.len() {
        let character = chars[index];
        if escaped {
            value.push(character);
            escaped = false;
            index += 1;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            index += 1;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            } else {
                dynamic |= active_quote == '"' && matches!(character, '$' | '`');
                value.push(character);
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            index += 1;
        } else if character.is_whitespace() || matches!(character, ';' | '|' | '&' | '<' | '>') {
            break;
        } else {
            dynamic |= matches!(character, '$' | '`' | '*' | '?' | '[' | '{' | '}')
                || character == '~' && value.is_empty();
            value.push(character);
            index += 1;
        }
    }
    if quote.is_some() || value.is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.redirection_target_missing",
            reason: "shell 输出重定向目标为空或未闭合。",
        });
    }
    Ok((value, dynamic, index))
}

fn assess_segment(
    segment: &ShellSegment,
    segment_index: usize,
    read_permission: AgentReadPermission,
    cwd: Option<&Path>,
    depth: usize,
    findings: &mut Vec<CommandPolicyFinding>,
) -> Result<(), CommandSyntaxError> {
    if let Some(split_command) = env_split_command(&segment.tokens)? {
        let effective_program = effective_program(&segment.tokens).ok_or(CommandSyntaxError {
            code: "command.malformed.missing_program",
            reason: "复合命令包含缺少程序名的片段。",
        })?;
        if effective_program
            .executable_indices
            .iter()
            .any(|index| is_dynamic_program_name(&segment.tokens[*index]))
        {
            findings.push(finding(
                segment_index,
                "env",
                CommandRiskClass::Unsupported,
                "command.unsupported.dynamic_program",
                "无法静态核验动态生成的 env wrapper 链。",
            ));
            return Ok(());
        }
        if effective_program
            .executable_indices
            .iter()
            .any(|index| is_explicit_program_path(&segment.tokens[*index]))
        {
            findings.push(finding(
                segment_index,
                "env",
                CommandRiskClass::Unknown,
                "command.risk.explicit_program_path",
                "显式 env wrapper 路径不能仅凭 basename 获得自动执行权限。",
            ));
        }
        let offset = findings.len();
        let mut nested = analyze_command(split_command, read_permission, cwd, depth + 1)?;
        for finding in &mut nested {
            finding.segment_index += offset;
        }
        findings.extend(nested);
        findings.push(finding(
            segment_index,
            "env",
            CommandRiskClass::Unknown,
            "command.risk.environment_override",
            "env split-string 会改变命令解析或执行环境，自动执行需要明确批准。",
        ));
        append_redirection_findings(segment, segment_index, "env", read_permission, findings);
        return Ok(());
    }
    let Some(effective_program) = effective_program(&segment.tokens) else {
        return Err(CommandSyntaxError {
            code: "command.malformed.missing_program",
            reason: "复合命令包含缺少程序名的片段。",
        });
    };
    let program_index = effective_program.index;
    let tokens = &segment.tokens[program_index..];
    let program = program_basename(&tokens[0]);

    if is_shell_reserved_program(&program) {
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::Unsupported,
            "command.unsupported.shell_reserved_syntax",
            "当前安全分析器不支持 shell 保留字、函数或分组语法。",
        ));
        return Ok(());
    }

    if effective_program
        .executable_indices
        .iter()
        .any(|index| is_dynamic_program_name(&segment.tokens[*index]))
    {
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::Unsupported,
            "command.unsupported.dynamic_program",
            "无法静态核验动态生成的可执行程序或命令 wrapper。",
        ));
        return Ok(());
    }

    append_invocation_provenance_findings(
        segment,
        segment_index,
        &effective_program,
        &program,
        findings,
    );

    if is_shell_program(&program) {
        if let Some(command_index) = shell_command_argument_index(tokens) {
            if tokens
                .iter()
                .take(command_index)
                .skip(1)
                .any(|option| shell_option_is_interactive(option))
            {
                findings.push(finding(
                    segment_index,
                    &program,
                    CommandRiskClass::Unsupported,
                    "command.unsupported.interactive",
                    "run_command 仅支持无需交互输入的命令。",
                ));
                return Ok(());
            }
            if shell_loads_startup_code(tokens, command_index) {
                findings.push(finding(
                    segment_index,
                    &program,
                    CommandRiskClass::Unknown,
                    "command.risk.shell_startup_code",
                    "shell login/rc/init 选项可能在 -c 命令前加载额外代码。",
                ));
            }
            if matches!(program.as_str(), "zsh" | "fish") {
                findings.push(finding(
                    segment_index,
                    &program,
                    CommandRiskClass::Unknown,
                    "command.risk.shell_default_startup",
                    "该 shell 即使使用 -c 也可能默认加载用户启动文件；自动执行需要明确批准。",
                ));
            }
            if !shell_command_options_are_closed(tokens, command_index, &program) {
                findings.push(finding(
                    segment_index,
                    &program,
                    CommandRiskClass::Unknown,
                    "command.risk.shell_option_grammar",
                    "shell -c 使用了自动授权闭集之外的启动或解析选项。",
                ));
            }
            let nested_command = tokens.get(command_index).ok_or(CommandSyntaxError {
                code: "command.malformed.shell_command_missing",
                reason: "shell -c 缺少待执行的命令字符串。",
            })?;
            let offset = findings.len();
            let mut nested = analyze_command(nested_command, read_permission, cwd, depth + 1)?;
            for finding in &mut nested {
                finding.segment_index += offset;
            }
            findings.extend(nested);
            if program == "fish"
                && tokens
                    .get(command_index.wrapping_sub(1))
                    .map(String::as_str)
                    == Some("-C")
            {
                append_shell_script_finding(
                    tokens.get(command_index + 1).map(String::as_str),
                    segment_index,
                    &program,
                    findings,
                );
            }
            append_redirection_findings(
                segment,
                segment_index,
                &program,
                read_permission,
                findings,
            );
            return Ok(());
        }
        if let Some(script) = shell_script_argument(tokens) {
            append_shell_script_finding(Some(script), segment_index, &program, findings);
            append_redirection_findings(
                segment,
                segment_index,
                &program,
                read_permission,
                findings,
            );
            return Ok(());
        }
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::Unsupported,
            "command.unsupported.opaque_shell",
            "run_command 不执行无法静态核验内容的交互式 shell 或 shell 脚本。",
        ));
        return Ok(());
    }

    let (risk, code, reason) = classify_segment(tokens, &program, cwd);
    findings.push(finding(segment_index, &program, risk, code, reason));
    if matches!(
        risk,
        CommandRiskClass::ReadOnly | CommandRiskClass::SafeWorkspaceWrite
    ) && read_permission == AgentReadPermission::WorkspaceOnly
        && has_obvious_external_read_argument(tokens, &program)
    {
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::ExternalRead,
            "command.scope.external_read",
            "read=workspace_only 下，命令读取了明显位于 workspace 外的路径。",
        ));
    }
    append_redirection_findings(segment, segment_index, &program, read_permission, findings);
    Ok(())
}

fn append_shell_script_finding(
    script: Option<&str>,
    segment_index: usize,
    program: &str,
    findings: &mut Vec<CommandPolicyFinding>,
) {
    let Some(script) = script else {
        return;
    };
    if is_opaque_shell_script_path(script) {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unsupported,
            "command.unsupported.dynamic_shell_script",
            "无法静态核验动态生成的 shell 脚本路径。",
        ));
    } else {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unknown,
            "command.risk.shell_script",
            "shell 脚本属于不透明代码；自动执行需要明确批准。",
        ));
    }
}

fn append_invocation_provenance_findings(
    segment: &ShellSegment,
    segment_index: usize,
    effective_program: &EffectiveProgram,
    program: &str,
    findings: &mut Vec<CommandPolicyFinding>,
) {
    if effective_program
        .executable_indices
        .iter()
        .any(|index| is_explicit_program_path(&segment.tokens[*index]))
    {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unknown,
            "command.risk.explicit_program_path",
            "显式可执行文件或命令 wrapper 路径不能仅凭 basename 获得自动执行权限。",
        ));
    }
    if has_environment_override(&segment.tokens, effective_program.index) {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unknown,
            "command.risk.environment_override",
            "命令覆盖或清除了执行环境；自动执行需要明确批准。",
        ));
    }
    if segment
        .tokens
        .iter()
        .skip(effective_program.index + 1)
        .any(|token| has_dynamic_argument(token))
    {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unknown,
            "command.risk.dynamic_argument",
            "命令参数包含环境展开或命令替换，无法按静态字面值自动授权。",
        ));
    }
    if segment
        .tokens
        .iter()
        .skip(effective_program.index + 1)
        .any(|token| has_shell_path_expansion(token))
    {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::Unknown,
            "command.risk.shell_path_expansion",
            "命令参数包含 shell glob、brace 或 home 展开，静态路径范围无法完整核验。",
        ));
    }
}

fn is_explicit_program_path(program: &str) -> bool {
    program.contains(['/', '\\']) || program.starts_with(['.', '~'])
}

fn has_environment_override(tokens: &[String], program_index: usize) -> bool {
    tokens
        .iter()
        .take(program_index)
        .any(|token| is_env_assignment(token) || program_basename(token) == "env")
}

fn has_dynamic_argument(argument: &str) -> bool {
    argument.contains(['$', '`'])
}

fn has_shell_path_expansion(argument: &str) -> bool {
    argument.starts_with('~') || argument.contains(['*', '?', '[', ']', '{', '}'])
}

fn is_shell_reserved_program(program: &str) -> bool {
    matches!(
        program,
        "!" | "{"
            | "}"
            | "if"
            | "then"
            | "elif"
            | "else"
            | "fi"
            | "for"
            | "while"
            | "until"
            | "do"
            | "done"
            | "case"
            | "esac"
            | "select"
            | "function"
            | "coproc"
            | "[["
            | "]]"
            | "trap"
            | "alias"
            | "unalias"
            | "declare"
            | "typeset"
            | "local"
            | "return"
            | "break"
            | "continue"
    )
}

fn has_obvious_external_read_argument(tokens: &[String], program: &str) -> bool {
    if !matches!(
        program,
        "ls" | "rg"
            | "grep"
            | "cat"
            | "head"
            | "tail"
            | "wc"
            | "stat"
            | "file"
            | "du"
            | "df"
            | "sort"
            | "cut"
            | "jq"
            | "find"
            | "cargo"
            | "npm"
            | "pnpm"
            | "yarn"
            | "bun"
            | "make"
            | "pip"
            | "pip3"
    ) {
        return false;
    }

    tokens.iter().skip(1).any(|argument| {
        if matches!(program, "rg" | "grep")
            && argument
                .strip_prefix("-f")
                .filter(|value| !value.is_empty())
                .is_some_and(is_obvious_external_path)
        {
            return true;
        }
        let candidate = argument
            .strip_prefix('-')
            .and_then(|option| option.split_once('=').map(|(_, value)| value))
            .unwrap_or(argument);
        is_obvious_external_path(candidate)
    })
}

fn is_obvious_external_path(argument: &str) -> bool {
    let argument = argument.trim_matches(['"', '\'']);
    let lower = argument.to_ascii_lowercase();
    if argument.starts_with('/')
        || argument.starts_with('\\')
        || argument.starts_with('~')
        || lower.starts_with("$home")
        || lower.starts_with("${home}")
        || lower.starts_with("file:///")
        || ["@home", "@desktop", "@documents", "@downloads"]
            .iter()
            .any(|alias| lower == *alias || lower.starts_with(&format!("{alias}/")))
    {
        return true;
    }

    Path::new(argument)
        .components()
        .any(|component| component == Component::ParentDir)
}

fn is_network_redirection_target(target: &str) -> bool {
    let lower = target
        .trim_matches(['"', '\''])
        .to_ascii_lowercase()
        .replace('\\', "/");
    lower.starts_with("/dev/tcp/") || lower.starts_with("/dev/udp/")
}

fn append_redirection_findings(
    segment: &ShellSegment,
    segment_index: usize,
    program: &str,
    read_permission: AgentReadPermission,
    findings: &mut Vec<CommandPolicyFinding>,
) {
    if segment.has_write_redirection {
        findings.push(finding(
            segment_index,
            program,
            CommandRiskClass::DirectWrite,
            "command.risk.write_redirection",
            "命令使用 shell 输出重定向写入文件。",
        ));
    }
    for redirection in &segment.input_redirections {
        if is_network_redirection_target(&redirection.target) {
            findings.push(finding(
                segment_index,
                program,
                CommandRiskClass::Network,
                "command.risk.network_redirection",
                "shell /dev/tcp 或 /dev/udp 输入重定向会访问网络。",
            ));
        }
        if read_permission == AgentReadPermission::WorkspaceOnly
            && is_obvious_external_path(&redirection.target)
        {
            findings.push(finding(
                segment_index,
                program,
                CommandRiskClass::ExternalRead,
                "command.scope.external_input_redirection",
                "read=workspace_only 下，shell 输入重定向读取了 workspace 外的路径。",
            ));
        }
        if redirection.dynamic {
            findings.push(finding(
                segment_index,
                program,
                CommandRiskClass::Unknown,
                "command.risk.dynamic_input_redirection",
                "无法静态核验动态展开的 shell 输入重定向目标。",
            ));
        }
    }
}

fn is_dynamic_program_name(program: &str) -> bool {
    program.contains('$')
        || program.contains('`')
        || program.contains("$()")
        || program.contains("``")
        || program.contains(['*', '?', '[', '{', '}'])
}

fn env_split_command(tokens: &[String]) -> Result<Option<&str>, CommandSyntaxError> {
    let mut index = 0;
    while tokens
        .get(index)
        .is_some_and(|token| is_env_assignment(token))
    {
        index += 1;
    }
    loop {
        let program = program_basename(tokens.get(index).ok_or(CommandSyntaxError {
            code: "command.malformed.missing_program",
            reason: "复合命令包含缺少程序名的片段。",
        })?);
        match program.as_str() {
            "env" => {
                index += 1;
                while let Some(option) = tokens.get(index) {
                    if matches!(option.as_str(), "-S" | "--split-string") {
                        return tokens.get(index + 1).map(String::as_str).map(Some).ok_or(
                            CommandSyntaxError {
                                code: "command.malformed.env_split_missing",
                                reason: "env -S 缺少待拆分执行的命令。",
                            },
                        );
                    }
                    if let Some(command) =
                        option.strip_prefix("-S").filter(|value| !value.is_empty())
                    {
                        return Ok(Some(command));
                    }
                    if let Some(command) = option.strip_prefix("--split-string=") {
                        if command.trim().is_empty() {
                            return Err(CommandSyntaxError {
                                code: "command.malformed.env_split_missing",
                                reason: "env --split-string 缺少待拆分执行的命令。",
                            });
                        }
                        return Ok(Some(command));
                    }
                    if matches!(option.as_str(), "-u" | "--unset" | "-C" | "--chdir") {
                        index += 2;
                    } else if option.starts_with('-') || is_env_assignment(option) {
                        index += 1;
                    } else {
                        break;
                    }
                }
            }
            "busybox" => {
                index += 1;
                if tokens
                    .get(index)
                    .is_some_and(|token| token.starts_with('-'))
                {
                    return Ok(None);
                }
            }
            "command" | "builtin" => {
                index += 1;
                while tokens
                    .get(index)
                    .is_some_and(|token| token.starts_with('-'))
                {
                    index += 1;
                }
            }
            _ => return Ok(None),
        }
        if index >= tokens.len() {
            return Ok(None);
        }
    }
}

fn classify_segment(
    tokens: &[String],
    program: &str,
    cwd: Option<&Path>,
) -> (CommandRiskClass, &'static str, &'static str) {
    if matches!(
        program,
        "sudo" | "sudoedit" | "doas" | "su" | "pkexec" | "run0"
    ) {
        return (
            CommandRiskClass::Catastrophic,
            "command.catastrophic.privilege_escalation",
            "run_command 永远不允许提权或切换系统用户。",
        );
    }
    if is_catastrophic_system_control(tokens, program) {
        return (
            CommandRiskClass::Catastrophic,
            "command.catastrophic.system_power",
            "run_command 永远不允许关闭、重启或破坏关键系统进程。",
        );
    }
    if program == "mkfs"
        || program.starts_with("mkfs.")
        || program == "diskutil" && has_disk_mutation(tokens)
        || program == "dd" && has_raw_device_output(tokens)
        || writes_raw_device(tokens, program)
    {
        return (
            CommandRiskClass::Catastrophic,
            "command.catastrophic.storage_device",
            "命令可能格式化、抹除或直接覆盖存储设备及系统控制接口。",
        );
    }
    if is_catastrophic_filesystem_operation(tokens, program, cwd) {
        return (
            CommandRiskClass::Catastrophic,
            "command.catastrophic.filesystem_root",
            "命令可能递归破坏根目录、系统目录或用户主目录。",
        );
    }
    if has_opaque_recursive_target(tokens, program) {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.dynamic_destructive_target",
            "无法静态核验递归删除或权限变更命令的动态目标。",
        );
    }
    if has_unsafe_recursive_symlink_traversal(tokens, program) {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.recursive_symlink_traversal",
            "递归 chmod/chown 的 symlink 跟随模式无法限制在已核验目录内。",
        );
    }
    if is_unsupported_interactive(tokens, program) {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.interactive",
            "run_command 仅支持无需交互输入的命令。",
        );
    }
    if matches!(program, "eval" | "exec") {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.dynamic_execution",
            "无法静态核验动态拼接后执行的命令。",
        );
    }
    if matches!(
        program,
        "." | "source"
            | "xargs"
            | "nohup"
            | "time"
            | "timeout"
            | "nice"
            | "setsid"
            | "powershell"
            | "powershell.exe"
            | "pwsh"
    ) {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.opaque_wrapper",
            "该命令包装器会隐藏、拆分或脱离无法静态核验的后续执行。",
        );
    }
    if program == "find"
        && tokens
            .iter()
            .any(|token| matches!(token.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir"))
    {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.find_exec",
            "无法可靠界定 find -exec/-ok 及其 dir 变体将执行的嵌套命令。",
        );
    }
    if program == "rg"
        && tokens.iter().any(|token| {
            matches!(token.as_str(), "--pre" | "--pre-glob")
                || token.starts_with("--pre=")
                || token.starts_with("--pre-glob=")
                || token == "--hostname-bin"
                || token.starts_with("--hostname-bin=")
                || token == "--search-zip"
                || token.starts_with('-')
                    && !token.starts_with("--")
                    && token.chars().skip(1).any(|character| character == 'z')
        })
    {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.rg_preprocessor",
            "ripgrep 的预处理器、hostname helper 或压缩搜索选项可启动外部程序。",
        );
    }
    if program == "sort"
        && tokens
            .iter()
            .any(|token| token == "--compress-program" || token.starts_with("--compress-program="))
    {
        return (
            CommandRiskClass::Unsupported,
            "command.unsupported.sort_compress_program",
            "sort --compress-program 可启动外部程序，无法作为只读命令自动核验。",
        );
    }
    if program == "git"
        && (tokens.get(1).is_some_and(|subcommand| {
            matches!(
                subcommand.to_ascii_lowercase().as_str(),
                "diff" | "log" | "show"
            )
        }) || tokens
            .iter()
            .any(|token| matches!(token.as_str(), "--ext-diff" | "--textconv")))
    {
        return (
            CommandRiskClass::Unknown,
            "command.risk.git_external_diff",
            "Git diff/log/show 的 external diff 或 textconv 配置可能启动外部程序，自动执行需要明确批准。",
        );
    }
    if program == "file"
        && tokens
            .iter()
            .any(|token| matches!(token.as_str(), "-C" | "--compile"))
    {
        return (
            CommandRiskClass::DirectWrite,
            "command.risk.file_compile",
            "file --compile 会生成 magic 数据库文件。",
        );
    }
    if program == "file"
        && tokens.iter().any(|token| {
            matches!(
                token.as_str(),
                "-S" | "--no-sandbox" | "-z" | "--uncompress"
            )
        })
    {
        return (
            CommandRiskClass::Unknown,
            "command.risk.file_helper",
            "file 的解压或禁用沙箱模式不能作为自动只读操作核验。",
        );
    }
    if is_git_worktree_read(tokens, program) {
        return (
            CommandRiskClass::Unknown,
            "command.risk.git_workspace_hook",
            "Git 工作树读取可能触发 fsmonitor、external diff 或 textconv helper。",
        );
    }
    if is_jq_environment_read(tokens, program) {
        return (
            CommandRiskClass::Unknown,
            "command.risk.environment_read",
            "jq env/$ENV 会暴露进程环境中的敏感信息。",
        );
    }
    if is_jq_module_load(tokens, program) {
        return (
            CommandRiskClass::Unknown,
            "command.risk.jq_module_load",
            "jq include/import/module 可从搜索路径加载额外代码。",
        );
    }
    if is_workspace_task(tokens, program)
        && (has_workspace_task_boundary_override(tokens, program)
            || !is_safe_workspace_task(tokens, program))
    {
        return (
            CommandRiskClass::Unknown,
            "command.risk.workspace_task_grammar",
            "workspace 任务包含闭集语法之外的 runner、配置或任务定义参数。",
        );
    }
    if is_package_management(tokens, program) {
        return (
            CommandRiskClass::PackageManagement,
            "command.risk.package_management",
            "命令会安装、删除或更新软件依赖。",
        );
    }
    if is_network_command(tokens, program) {
        return (
            CommandRiskClass::Network,
            "command.risk.network",
            "命令会访问网络或远程主机。",
        );
    }
    if is_high_impact_command(tokens, program) {
        return (
            CommandRiskClass::HighImpact,
            "command.risk.high_impact",
            "命令会删除、移动、覆盖或显著改变本地状态。",
        );
    }
    if is_direct_write_command(tokens, program) {
        return (
            CommandRiskClass::DirectWrite,
            "command.risk.direct_write",
            "命令会直接创建或修改文件。",
        );
    }
    if is_safe_workspace_task(tokens, program) {
        return (
            CommandRiskClass::SafeWorkspaceWrite,
            "command.risk.safe_workspace_write",
            "命令运行已明确允许的本地构建、检查或测试任务。",
        );
    }
    if is_known_read_only(tokens, program) {
        return (
            CommandRiskClass::ReadOnly,
            "command.risk.read_only",
            "命令只读取状态或向标准输出生成结果。",
        );
    }

    (
        CommandRiskClass::Unknown,
        "command.risk.unknown",
        "无法可靠判断该命令的副作用。",
    )
}

fn finding(
    segment_index: usize,
    program: &str,
    risk: CommandRiskClass,
    code: &str,
    reason: &str,
) -> CommandPolicyFinding {
    CommandPolicyFinding {
        segment_index,
        program: program.to_string(),
        risk,
        code: code.to_string(),
        reason: reason.to_string(),
    }
}

fn aggregate_risk_level(findings: &[CommandPolicyFinding]) -> AgentCommandRiskLevel {
    findings
        .iter()
        .map(|finding| match finding.risk {
            CommandRiskClass::Catastrophic | CommandRiskClass::HighImpact => {
                AgentCommandRiskLevel::Destructive
            }
            CommandRiskClass::PackageManagement | CommandRiskClass::Network => {
                AgentCommandRiskLevel::Network
            }
            CommandRiskClass::DirectWrite | CommandRiskClass::SafeWorkspaceWrite => {
                AgentCommandRiskLevel::WritesWorkspace
            }
            CommandRiskClass::ReadOnly => AgentCommandRiskLevel::ReadOnly,
            CommandRiskClass::Unknown
            | CommandRiskClass::Unsupported
            | CommandRiskClass::ExternalRead => AgentCommandRiskLevel::Unknown,
        })
        .max_by_key(|risk| match risk {
            AgentCommandRiskLevel::ReadOnly => 0,
            AgentCommandRiskLevel::WritesWorkspace => 1,
            AgentCommandRiskLevel::Unknown => 2,
            AgentCommandRiskLevel::Network => 3,
            AgentCommandRiskLevel::Destructive => 4,
        })
        .unwrap_or(AgentCommandRiskLevel::Unknown)
}

fn lex_command(command: &str) -> Result<LexedCommand, CommandSyntaxError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    let chars = command.chars().collect::<Vec<_>>();
    let mut segments = Vec::new();
    let mut embedded_commands = Vec::new();
    let mut tokens = Vec::new();
    let mut token = String::new();
    // Shell IO_NUMBER recognition is lexical: quoted or escaped digits immediately before a
    // redirection remain an ordinary argument. Preserve that provenance instead of guessing from
    // the dequoted token value later.
    let mut token_has_quoted_or_escaped_content = false;
    let mut write_redirection = false;
    let mut input_redirections = Vec::new();
    let mut quote = Quote::None;
    let mut index = 0;
    let mut just_saw_separator = false;

    while index < chars.len() {
        let character = chars[index];
        match quote {
            Quote::Single => {
                if character == '\'' {
                    quote = Quote::None;
                } else {
                    token.push(character);
                }
                index += 1;
            }
            Quote::Double => {
                if character == '"' {
                    quote = Quote::None;
                    index += 1;
                } else if character == '\\' {
                    let Some(next) = chars.get(index + 1) else {
                        return Err(CommandSyntaxError {
                            code: "command.malformed.trailing_escape",
                            reason: "命令以未完成的转义字符结尾。",
                        });
                    };
                    token.push(*next);
                    index += 2;
                } else if character == '$' && chars.get(index + 1) == Some(&'(') {
                    let (embedded, next) = extract_parenthesized_command(&chars, index + 2)?;
                    embedded_commands.push(embedded);
                    token.push_str("$()");
                    index = next;
                } else if character == '`' {
                    let (embedded, next) = extract_backtick_command(&chars, index + 1)?;
                    embedded_commands.push(embedded);
                    token.push_str("``");
                    index = next;
                } else {
                    token.push(character);
                    index += 1;
                }
            }
            Quote::None => {
                if character.is_whitespace() {
                    push_token(&mut tokens, &mut token);
                    token_has_quoted_or_escaped_content = false;
                    index += 1;
                    continue;
                }
                if character == '#' && token.is_empty() && !token_has_quoted_or_escaped_content {
                    break;
                }
                if character == '\'' {
                    token_has_quoted_or_escaped_content = true;
                    quote = Quote::Single;
                    index += 1;
                    continue;
                }
                if character == '"' {
                    token_has_quoted_or_escaped_content = true;
                    quote = Quote::Double;
                    index += 1;
                    continue;
                }
                if character == '\\' {
                    let Some(next) = chars.get(index + 1) else {
                        return Err(CommandSyntaxError {
                            code: "command.malformed.trailing_escape",
                            reason: "命令以未完成的转义字符结尾。",
                        });
                    };
                    token_has_quoted_or_escaped_content = true;
                    token.push(*next);
                    index += 2;
                    continue;
                }
                if character == '$' && chars.get(index + 1) == Some(&'(') {
                    let (embedded, next) = extract_parenthesized_command(&chars, index + 2)?;
                    embedded_commands.push(embedded);
                    token.push_str("$()");
                    index = next;
                    continue;
                }
                if character == '`' {
                    let (embedded, next) = extract_backtick_command(&chars, index + 1)?;
                    embedded_commands.push(embedded);
                    token.push_str("``");
                    index = next;
                    continue;
                }
                if matches!(character, '(' | ')') {
                    return Err(CommandSyntaxError {
                        code: "command.unsupported.shell_group",
                        reason: "当前安全分析器不支持 shell 分组或进程替换。",
                    });
                }
                if character == '<' && chars.get(index + 1) == Some(&'<') {
                    return Err(CommandSyntaxError {
                        code: "command.unsupported.heredoc",
                        reason: "run_command 不支持 heredoc 或 here-string。",
                    });
                }
                if character == '<' && chars.get(index + 1) == Some(&'(')
                    || character == '>' && chars.get(index + 1) == Some(&'(')
                {
                    return Err(CommandSyntaxError {
                        code: "command.unsupported.process_substitution",
                        reason: "run_command 不支持进程替换。",
                    });
                }
                if character == '>' || character == '&' && chars.get(index + 1) == Some(&'>') {
                    push_token(&mut tokens, &mut token);
                    token_has_quoted_or_escaped_content = false;
                    index += 1;
                    if chars.get(index) == Some(&'>') {
                        index += 1;
                    }
                    if chars.get(index) == Some(&'&') {
                        let target_start = index + 1;
                        if let Some(next) = consume_fd_duplication(&chars, target_start) {
                            index = next;
                        } else {
                            index = target_start;
                            write_redirection = true;
                        }
                    } else {
                        write_redirection = true;
                    }
                    continue;
                }
                if character == '<' {
                    if token.is_empty()
                        || token_has_quoted_or_escaped_content
                        || !token.chars().all(|character| character.is_ascii_digit())
                    {
                        push_token(&mut tokens, &mut token);
                    } else {
                        // A decimal word immediately adjacent to a redirection operator selects
                        // the file descriptor; it is not an argument passed to the program.
                        token.clear();
                    }
                    token_has_quoted_or_escaped_content = false;
                    let read_write = chars.get(index + 1) == Some(&'>');
                    let mut target_start = index + if read_write { 2 } else { 1 };
                    while chars
                        .get(target_start)
                        .is_some_and(|value| value.is_whitespace())
                    {
                        target_start += 1;
                    }
                    if chars.get(target_start) == Some(&'&') {
                        return Err(CommandSyntaxError {
                            code: "command.unsupported.input_fd_duplication",
                            reason: "当前安全分析器不支持 shell 输入文件描述符复制。",
                        });
                    }
                    let (target, dynamic, next) = read_redirection_target(&chars, target_start)?;
                    input_redirections.push(ShellInputRedirection { target, dynamic });
                    write_redirection |= read_write;
                    index = next;
                    continue;
                }
                if matches!(character, ';' | '|' | '&') {
                    if character == '&' && chars.get(index + 1) != Some(&'&') {
                        return Err(CommandSyntaxError {
                            code: "command.unsupported.background_execution",
                            reason:
                                "run_command 不允许 shell 后台执行；取消和超时必须覆盖整个命令。",
                        });
                    }
                    push_token(&mut tokens, &mut token);
                    token_has_quoted_or_escaped_content = false;
                    if tokens.is_empty() {
                        return Err(CommandSyntaxError {
                            code: "command.malformed.empty_segment",
                            reason: "复合命令包含空片段。",
                        });
                    }
                    segments.push(ShellSegment {
                        tokens: std::mem::take(&mut tokens),
                        has_write_redirection: write_redirection,
                        input_redirections: std::mem::take(&mut input_redirections),
                    });
                    write_redirection = false;
                    just_saw_separator = true;
                    index += 1;
                    if chars.get(index) == Some(&character) && matches!(character, '|' | '&') {
                        index += 1;
                    }
                    continue;
                }

                just_saw_separator = false;
                token.push(character);
                index += 1;
            }
        }
    }

    if quote != Quote::None {
        return Err(CommandSyntaxError {
            code: "command.malformed.unclosed_quote",
            reason: "命令包含未闭合的引号。",
        });
    }
    push_token(&mut tokens, &mut token);
    if just_saw_separator && tokens.is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.trailing_operator",
            reason: "复合命令不能以 shell 运算符结尾。",
        });
    }
    if !tokens.is_empty() {
        segments.push(ShellSegment {
            tokens,
            has_write_redirection: write_redirection,
            input_redirections,
        });
    }
    if segments.is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.empty",
            reason: "命令不能为空。",
        });
    }

    Ok(LexedCommand {
        segments,
        embedded_commands,
    })
}

fn push_token(tokens: &mut Vec<String>, token: &mut String) {
    if !token.is_empty() {
        tokens.push(std::mem::take(token));
    }
}

fn extract_parenthesized_command(
    chars: &[char],
    start: usize,
) -> Result<(String, usize), CommandSyntaxError> {
    let mut depth = 1;
    let mut quote = None;
    let mut escaped = false;
    for index in start..chars.len() {
        let character = chars[index];
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active_quote) = quote {
            if character == active_quote {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == '(' {
            depth += 1;
        } else if character == ')' {
            depth -= 1;
            if depth == 0 {
                return Ok((chars[start..index].iter().collect(), index + 1));
            }
        }
    }
    Err(CommandSyntaxError {
        code: "command.malformed.unclosed_substitution",
        reason: "命令包含未闭合的命令替换。",
    })
}

fn extract_backtick_command(
    chars: &[char],
    start: usize,
) -> Result<(String, usize), CommandSyntaxError> {
    let mut escaped = false;
    for index in start..chars.len() {
        let character = chars[index];
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == '`' {
            return Ok((chars[start..index].iter().collect(), index + 1));
        }
    }
    Err(CommandSyntaxError {
        code: "command.malformed.unclosed_substitution",
        reason: "命令包含未闭合的反引号命令替换。",
    })
}

#[derive(Debug)]
struct EffectiveProgram {
    index: usize,
    /// Every token that the policy parser treated as an executable wrapper or final program.
    /// Keeping this chain is security-significant: provenance checks must not be lost when a
    /// wrapper such as `env`, `busybox`, or `command` is unwrapped to classify its applet.
    executable_indices: Vec<usize>,
}

fn effective_program(tokens: &[String]) -> Option<EffectiveProgram> {
    let mut index = 0;
    while tokens
        .get(index)
        .is_some_and(|token| is_env_assignment(token))
    {
        index += 1;
    }
    let mut executable_indices = Vec::new();
    loop {
        let wrapper_index = index;
        let program = program_basename(tokens.get(index)?);
        executable_indices.push(index);
        if program == "env" {
            index += 1;
            while let Some(token) = tokens.get(index) {
                if matches!(token.as_str(), "-u" | "--unset" | "-C" | "--chdir") {
                    index += 2;
                } else if token.starts_with('-') || is_env_assignment(token) {
                    index += 1;
                } else {
                    break;
                }
            }
            if index >= tokens.len() {
                return Some(EffectiveProgram {
                    index: wrapper_index,
                    executable_indices,
                });
            }
        } else if program == "busybox" {
            index += 1;
            if tokens
                .get(index)
                .is_some_and(|token| token.starts_with('-'))
            {
                // BusyBox global options are not applet-neutral. In particular, `--install`
                // creates links. Treat the wrapper itself as opaque instead of skipping options
                // and accidentally granting an applet's read-only classification.
                return Some(EffectiveProgram {
                    index: wrapper_index,
                    executable_indices,
                });
            }
        } else if matches!(program.as_str(), "command" | "builtin") {
            index += 1;
            while tokens
                .get(index)
                .is_some_and(|token| token.starts_with('-'))
            {
                index += 1;
            }
        } else {
            return Some(EffectiveProgram {
                index,
                executable_indices,
            });
        }
    }
}

fn is_env_assignment(token: &str) -> bool {
    let Some((name, _)) = token.split_once('=') else {
        return false;
    };
    !name.is_empty()
        && name
            .chars()
            .all(|character| character == '_' || character.is_ascii_alphanumeric())
        && name
            .chars()
            .next()
            .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
}

fn program_basename(program: &str) -> String {
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase()
}

fn is_shell_program(program: &str) -> bool {
    matches!(
        program,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "cmd" | "cmd.exe"
    )
}

fn shell_command_argument_index(tokens: &[String]) -> Option<usize> {
    tokens
        .iter()
        .enumerate()
        .skip(1)
        .find_map(|(index, token)| {
            let option = token.to_ascii_lowercase();
            (option == "/c"
                || option == "-c"
                || option.starts_with('-')
                    && !option.starts_with("--")
                    && option.chars().skip(1).any(|character| character == 'c'))
            .then_some(index + 1)
        })
}

fn shell_script_argument(tokens: &[String]) -> Option<&str> {
    let mut index = 1;
    while let Some(token) = tokens.get(index) {
        if token == "--" {
            return tokens.get(index + 1).map(String::as_str);
        }
        if token == "-" || token == "/k" || shell_option_is_interactive(token) {
            return None;
        }
        if token.starts_with('-') || token.starts_with('+') {
            index += if shell_option_takes_value(token) {
                2
            } else {
                1
            };
            continue;
        }
        return Some(token);
    }
    None
}

fn shell_option_is_interactive(option: &str) -> bool {
    option == "--interactive"
        || option.starts_with('-')
            && !option.starts_with("--")
            && option.chars().skip(1).any(|character| character == 'i')
}

fn shell_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-O" | "+O" | "-o" | "+o" | "--rcfile" | "--init-file"
    )
}

fn shell_loads_startup_code(tokens: &[String], command_index: usize) -> bool {
    tokens.iter().take(command_index).skip(1).any(|option| {
        matches!(
            option.as_str(),
            "-l" | "--login" | "--rcfile" | "--init-file"
        ) || option.starts_with("--rcfile=")
            || option.starts_with("--init-file=")
            || option.starts_with('-')
                && !option.starts_with("--")
                && option.chars().skip(1).any(|character| character == 'l')
    })
}

fn shell_command_options_are_closed(
    tokens: &[String],
    command_index: usize,
    program: &str,
) -> bool {
    tokens.iter().take(command_index).skip(1).all(|option| {
        if program == "fish" && matches!(option.as_str(), "-c" | "-C") {
            return true;
        }
        if matches!(program, "cmd" | "cmd.exe") {
            return option.eq_ignore_ascii_case("/c");
        }
        if matches!(option.as_str(), "--noprofile" | "--norc" | "--posix") {
            return true;
        }
        option.starts_with('-')
            && !option.starts_with("--")
            && option
                .chars()
                .skip(1)
                .all(|character| matches!(character, 'c' | 'e' | 'u' | 'f' | 'n' | 'x'))
    })
}

fn is_opaque_shell_script_path(script: &str) -> bool {
    script.starts_with('~')
        || script.contains(['$', '`', '*', '?', '[', ']', '{', '}'])
        || script == "-"
}

fn lower_tokens(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .map(|token| token.to_ascii_lowercase())
        .collect()
}

fn is_catastrophic_system_control(tokens: &[String], program: &str) -> bool {
    if matches!(program, "shutdown" | "reboot" | "halt" | "poweroff") {
        return true;
    }
    if program == "systemctl" {
        let lower = lower_tokens(tokens);
        if lower.iter().skip(1).any(|token| {
            matches!(
                token.as_str(),
                "reboot"
                    | "poweroff"
                    | "halt"
                    | "kexec"
                    | "soft-reboot"
                    | "rescue"
                    | "emergency"
                    | "isolate"
            )
        }) {
            return true;
        }
        let direct_activation = lower.iter().skip(1).any(|token| {
            matches!(
                token.as_str(),
                "start"
                    | "restart"
                    | "try-restart"
                    | "reload-or-restart"
                    | "try-reload-or-restart"
                    | "reload-or-try-restart"
            )
        });
        let enables_and_starts = lower.iter().skip(1).any(|token| token == "--now")
            && lower
                .iter()
                .skip(1)
                .any(|token| matches!(token.as_str(), "enable" | "reenable" | "preset" | "link"));
        let activates_target = direct_activation || enables_and_starts;
        if activates_target
            && lower
                .iter()
                .skip(1)
                .any(|token| token.contains(['*', '?', '[']))
        {
            // systemctl expands unit-name patterns itself, even when the shell receives a quoted
            // literal. A pattern cannot be proven not to select reboot/power targets, so activation
            // with a unit glob is never authorized by this shell-envelope policy.
            return true;
        }
        return activates_target
            && lower.iter().skip(1).any(|token| {
                matches!(
                    token.as_str(),
                    "reboot.target"
                        | "poweroff.target"
                        | "halt.target"
                        | "kexec.target"
                        | "soft-reboot.target"
                        | "rescue.target"
                        | "emergency.target"
                        | "runlevel0.target"
                        | "runlevel6.target"
                        | "ctrl-alt-del.target"
                )
            });
    }
    if matches!(program, "init" | "telinit") {
        return tokens
            .iter()
            .skip(1)
            .any(|token| matches!(token.as_str(), "0" | "6"));
    }
    if program == "launchctl"
        && tokens
            .get(1)
            .is_some_and(|subcommand| subcommand.eq_ignore_ascii_case("reboot"))
    {
        return true;
    }
    program == "kill"
        && (tokens.iter().skip(1).any(|token| is_pid_one(token))
            || tokens.len() >= 3
                && tokens
                    .iter()
                    .skip(1)
                    .any(|token| is_negative_pid_one(token)))
}

fn is_pid_one(token: &str) -> bool {
    let digits = token.strip_prefix('+').unwrap_or(token);
    !digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
        && digits.trim_start_matches('0') == "1"
}

fn is_negative_pid_one(token: &str) -> bool {
    token.strip_prefix('-').is_some_and(is_pid_one)
}

fn has_disk_mutation(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "erasedisk"
                | "erasevolume"
                | "partitiondisk"
                | "zerodisk"
                | "randomdisk"
                | "secureerase"
                | "deletevolume"
                | "deletecontainer"
        )
    })
}

fn has_raw_device_output(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        token
            .to_ascii_lowercase()
            .strip_prefix("of=")
            .is_some_and(is_catastrophic_write_target)
    })
}

fn writes_raw_device(tokens: &[String], program: &str) -> bool {
    match program {
        "tee" | "truncate" => tokens
            .iter()
            .skip(1)
            .any(|token| is_catastrophic_write_target(token)),
        "cp" | "mv" => tokens
            .last()
            .is_some_and(|token| is_catastrophic_write_target(token)),
        "sort" => sort_output_targets(tokens).any(is_catastrophic_write_target),
        "find" => option_value_targets(tokens, &["-fprint", "-fprint0", "-fprintf", "-fls"])
            .any(is_catastrophic_write_target),
        _ => false,
    }
}

fn sort_output_targets(tokens: &[String]) -> impl Iterator<Item = &str> {
    tokens
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, token)| {
            if matches!(token.as_str(), "-o" | "--output") {
                tokens.get(index + 1).map(String::as_str)
            } else {
                token
                    .strip_prefix("--output=")
                    .filter(|value| !value.is_empty())
                    .or_else(|| token.strip_prefix("-o").filter(|value| !value.is_empty()))
            }
        })
}

fn sort_uses_explicit_temp_storage(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        matches!(token.as_str(), "-T" | "--temporary-directory")
            || token.starts_with("-T") && token.len() > 2
            || token.starts_with("--temporary-directory=")
    })
}

fn option_value_targets<'a>(
    tokens: &'a [String],
    options: &'a [&str],
) -> impl Iterator<Item = &'a str> {
    tokens
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, token)| {
            if options.contains(&token.as_str()) {
                tokens.get(index + 1).map(String::as_str)
            } else {
                options.iter().find_map(|option| {
                    token
                        .strip_prefix(&format!("{option}="))
                        .filter(|value| !value.is_empty())
                })
            }
        })
}

fn is_raw_device_path(target: &str) -> bool {
    let lower = target.trim_matches(['"', '\'']).to_ascii_lowercase();
    if lower.starts_with("\\\\.\\physicaldrive") {
        return true;
    }
    let normalized = normalize_lexical_path(Path::new(&lower));
    let lower = normalized.to_string_lossy();
    if matches!(
        lower.as_ref(),
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
    ) || lower.starts_with("/dev/fd/")
    {
        return false;
    }
    lower.starts_with("/dev/disk")
        || lower.starts_with("/dev/rdisk")
        || lower.starts_with("/dev/sd")
        || lower.starts_with("/dev/hd")
        || lower.starts_with("/dev/nvme")
        || lower.starts_with("/dev/mmcblk")
}

fn is_catastrophic_write_target(target: &str) -> bool {
    if is_raw_device_path(target) {
        return true;
    }
    let normalized = normalize_lexical_path(Path::new(
        target
            .trim_matches(['"', '\''])
            .to_ascii_lowercase()
            .as_str(),
    ));
    let normalized = normalized.to_string_lossy();
    if matches!(
        normalized.as_ref(),
        "/dev/null" | "/dev/stdout" | "/dev/stderr" | "/dev/tty"
    ) || normalized.starts_with("/dev/fd/")
    {
        return false;
    }
    normalized.starts_with("/dev/")
        || normalized == "/proc/sysrq-trigger"
        || normalized.starts_with("/proc/sys/")
        || normalized == "/sys"
        || normalized.starts_with("/sys/")
}

fn is_catastrophic_filesystem_operation(
    tokens: &[String],
    program: &str,
    cwd: Option<&Path>,
) -> bool {
    let lower = lower_tokens(tokens);
    if program == "rm" && has_recursive_flag(&lower) {
        return command_targets(&lower).any(|target| is_catastrophic_target(target, cwd));
    }
    if matches!(program, "chmod" | "chown") && has_recursive_flag(&lower) {
        return command_targets(&lower).any(|target| is_catastrophic_target(target, cwd));
    }
    if program == "find" && lower.iter().any(|token| token == "-delete") {
        return lower
            .get(1)
            .is_some_and(|target| is_catastrophic_target(target, cwd));
    }
    false
}

fn has_opaque_recursive_target(tokens: &[String], program: &str) -> bool {
    if matches!(program, "rm" | "chmod" | "chown") && has_recursive_flag(tokens) {
        return command_targets(tokens).any(is_opaque_destructive_target);
    }
    if program == "find" && tokens.iter().any(|token| token == "-delete") {
        return tokens
            .get(1)
            .is_none_or(|target| target.starts_with('-') || is_opaque_destructive_target(target));
    }
    false
}

fn has_unsafe_recursive_symlink_traversal(tokens: &[String], program: &str) -> bool {
    matches!(program, "chmod" | "chown")
        && has_recursive_flag(tokens)
        && tokens.iter().skip(1).any(|token| {
            matches!(token.as_str(), "-H" | "-L" | "--dereference")
                || token.starts_with('-')
                    && !token.starts_with("--")
                    && token
                        .chars()
                        .skip(1)
                        .any(|character| character == 'H' || character == 'L')
        })
}

fn is_opaque_destructive_target(target: &str) -> bool {
    target.contains(['$', '`', '*', '?', '[', ']', '{', '}']) || target.starts_with('~')
}

fn has_recursive_flag(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        token == "--recursive"
            || token.starts_with('-')
                && !token.starts_with("--")
                && token
                    .chars()
                    .skip(1)
                    .any(|character| character == 'r' || character == 'R')
    })
}

fn command_targets(tokens: &[String]) -> impl Iterator<Item = &str> {
    tokens
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|token| !token.starts_with('-') && *token != "--")
}

fn is_catastrophic_target(target: &str, cwd: Option<&Path>) -> bool {
    let target = target.trim_matches(['"', '\'']);
    let lower = target.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "/" | "/*"
            | "/**"
            | "~"
            | "~/"
            | "~/*"
            | "$home"
            | "$home/"
            | "$home/*"
            | "${home}"
            | "${home}/"
            | "${home}/*"
    ) {
        return true;
    }

    let without_glob = if target == "*" {
        "."
    } else {
        target
            .strip_suffix("/**")
            .or_else(|| target.strip_suffix("/*"))
            .unwrap_or(target)
    };
    let expanded_home = without_glob
        .strip_prefix("~/")
        .and_then(|relative| dirs::home_dir().map(|home| home.join(relative)));
    let path = expanded_home
        .as_deref()
        .unwrap_or_else(|| Path::new(without_glob));
    let resolved = if path.is_absolute() {
        normalize_lexical_path(path)
    } else if let Some(cwd) = cwd {
        normalize_lexical_path(&cwd.join(path))
    } else {
        return false;
    };
    is_sensitive_absolute_path(&resolved)
}

fn normalize_lexical_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(value) => normalized.push(value),
        }
    }
    normalized
}

fn is_sensitive_absolute_path(path: &Path) -> bool {
    if path == Path::new("/") {
        return true;
    }
    if dirs::home_dir().is_some_and(|home| path == home) {
        return true;
    }
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy().to_ascii_lowercase()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let Some(first) = components.first().map(String::as_str) else {
        return true;
    };
    if matches!(
        first,
        "system"
            | "library"
            | "applications"
            | "usr"
            | "etc"
            | "bin"
            | "sbin"
            | "boot"
            | "dev"
            | "lib"
            | "lib32"
            | "lib64"
            | "opt"
            | "proc"
            | "root"
            | "run"
            | "snap"
            | "srv"
            | "sys"
    ) {
        return true;
    }
    if matches!(first, "users" | "home") {
        return components.len() <= 2;
    }
    if first == "volumes" {
        return components.len() <= 2;
    }
    if first == "mnt" {
        return components.len() <= 2;
    }
    if first == "media" {
        return components.len() <= 3;
    }
    if first == "private" {
        if components.len() == 1 || components.get(1).is_some_and(|value| value == "etc") {
            return true;
        }
        return components.get(1).is_some_and(|value| value == "var")
            && (components.len() == 2
                || components.get(2).is_some_and(|value| {
                    matches!(value.as_str(), "db" | "log" | "run" | "spool")
                }));
    }
    first == "var"
        && (components.len() == 1
            || components.get(1).is_some_and(|value| {
                matches!(
                    value.as_str(),
                    "db" | "lib" | "log" | "run" | "spool" | "state"
                )
            }))
}

fn is_unsupported_interactive(tokens: &[String], program: &str) -> bool {
    matches!(
        program,
        "vim" | "vi" | "nano" | "less" | "more" | "top" | "htop" | "watch" | "read" | "passwd"
    ) || matches!(program, "python" | "python3" | "node") && tokens.len() == 1
        || program == "ssh" && tokens.len() <= 2
}

fn is_package_management(tokens: &[String], program: &str) -> bool {
    let lower = lower_tokens(tokens);
    let subcommand = lower.get(1).map(String::as_str).unwrap_or_default();
    if matches!(program, "pip" | "pip3") {
        return matches!(subcommand, "install" | "uninstall" | "download" | "wheel");
    }
    if matches!(program, "python" | "python3") {
        return matches!(
            (
                lower.get(1).map(String::as_str),
                lower.get(2).map(String::as_str),
                lower.get(3).map(String::as_str)
            ),
            (
                Some("-m"),
                Some("pip"),
                Some("install" | "uninstall" | "download" | "wheel")
            )
        );
    }
    if matches!(program, "npm" | "pnpm" | "yarn" | "bun") {
        return matches!(
            subcommand,
            "install" | "i" | "ci" | "add" | "remove" | "uninstall" | "update" | "upgrade"
        );
    }
    if program == "cargo" {
        return matches!(subcommand, "install" | "add" | "update");
    }
    if program == "brew" {
        return matches!(
            subcommand,
            "install" | "uninstall" | "update" | "upgrade" | "reinstall"
        );
    }
    if program == "uv" {
        return lower.iter().any(|token| {
            matches!(
                token.as_str(),
                "install" | "uninstall" | "add" | "remove" | "sync"
            )
        });
    }
    false
}

fn is_network_command(tokens: &[String], program: &str) -> bool {
    if matches!(
        program,
        "curl" | "wget" | "scp" | "rsync" | "ftp" | "sftp" | "ping" | "gh"
    ) {
        return true;
    }
    if program == "ssh" {
        return tokens.len() > 2;
    }
    if matches!(program, "pip" | "pip3") {
        return tokens.get(1).is_some_and(|subcommand| {
            subcommand.eq_ignore_ascii_case("index")
                || subcommand.eq_ignore_ascii_case("list")
                    && tokens
                        .iter()
                        .skip(2)
                        .any(|token| matches!(token.as_str(), "--outdated" | "--uptodate"))
        });
    }
    if program == "git" {
        return tokens.get(1).is_some_and(|subcommand| {
            matches!(
                subcommand.to_ascii_lowercase().as_str(),
                "clone" | "fetch" | "pull" | "push" | "ls-remote"
            )
        });
    }
    false
}

fn is_high_impact_command(tokens: &[String], program: &str) -> bool {
    if matches!(
        program,
        "rm" | "mv"
            | "cp"
            | "chmod"
            | "chown"
            | "ln"
            | "truncate"
            | "dd"
            | "kill"
            | "pkill"
            | "killall"
            | "launchctl"
    ) {
        return true;
    }
    if program == "find"
        && tokens
            .iter()
            .any(|token| matches!(token.as_str(), "-delete" | "-exec" | "-execdir"))
    {
        return true;
    }
    if program == "git" {
        return tokens.get(1).is_some_and(|subcommand| {
            matches!(
                subcommand.to_ascii_lowercase().as_str(),
                "reset" | "clean" | "checkout" | "restore" | "merge" | "rebase"
            )
        });
    }
    if program == "diskutil" {
        return !tokens.get(1).is_some_and(|subcommand| {
            matches!(subcommand.to_ascii_lowercase().as_str(), "list" | "info")
        });
    }
    false
}

fn is_direct_write_command(tokens: &[String], program: &str) -> bool {
    matches!(program, "touch" | "mkdir" | "tee")
        || program == "git" && !is_read_only_git(tokens)
        || program == "sort" && sort_output_targets(tokens).next().is_some()
        || program == "sort" && sort_uses_explicit_temp_storage(tokens)
        || program == "find"
            && tokens
                .iter()
                .any(|token| matches!(token.as_str(), "-fprint" | "-fprint0" | "-fprintf" | "-fls"))
        || program == "sed"
            && tokens
                .iter()
                .any(|token| token == "-i" || token.starts_with("-i"))
        || program == "perl" && tokens.iter().any(|token| token.starts_with("-pi"))
}

fn is_safe_workspace_task(tokens: &[String], program: &str) -> bool {
    let subcommand = tokens
        .get(1)
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if program == "cargo" {
        return matches!(subcommand.as_str(), "test" | "check" | "build" | "clippy")
            && matches_closed_cli_grammar(
                tokens,
                2,
                &[
                    "--all",
                    "--workspace",
                    "--release",
                    "--locked",
                    "--offline",
                    "--frozen",
                    "--all-targets",
                    "--all-features",
                    "--no-default-features",
                    "--no-run",
                    "--no-fail-fast",
                    "--doc",
                    "--lib",
                    "--bins",
                    "--tests",
                    "--benches",
                    "--examples",
                    "--verbose",
                    "--quiet",
                ],
                &[
                    "--package",
                    "--exclude",
                    "--features",
                    "--target",
                    "--jobs",
                    "--profile",
                    "--test",
                    "--bin",
                    "--bench",
                    "--example",
                    "--message-format",
                    "--color",
                ],
                "qv",
                "pj",
            );
    }
    if matches!(program, "npm" | "pnpm" | "yarn" | "bun") {
        if matches!(
            subcommand.as_str(),
            "test" | "build" | "lint" | "typecheck" | "check"
        ) {
            return tokens.len() == 2;
        }
        return subcommand == "run"
            && tokens.get(2).is_some_and(|script| {
                matches!(
                    script.to_ascii_lowercase().as_str(),
                    "test" | "build" | "lint" | "typecheck" | "check"
                )
            })
            && tokens.len() == 3;
    }
    program == "make"
        && matches!(
            subcommand.as_str(),
            "test" | "check" | "build" | "lint" | "typecheck"
        )
        && tokens.len() == 2
}

fn is_workspace_task(tokens: &[String], program: &str) -> bool {
    let subcommand = tokens
        .get(1)
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if program == "cargo" {
        return matches!(subcommand.as_str(), "test" | "check" | "build" | "clippy");
    }
    if matches!(program, "npm" | "pnpm" | "yarn" | "bun") {
        return matches!(
            subcommand.as_str(),
            "test" | "build" | "lint" | "typecheck" | "check"
        ) || subcommand == "run"
            && tokens.get(2).is_some_and(|script| {
                matches!(
                    script.to_ascii_lowercase().as_str(),
                    "test" | "build" | "lint" | "typecheck" | "check"
                )
            });
    }
    program == "make"
        && matches!(
            subcommand.as_str(),
            "test" | "check" | "build" | "lint" | "typecheck"
        )
}

fn has_workspace_task_boundary_override(tokens: &[String], program: &str) -> bool {
    if program == "cargo" {
        return option_value_targets(tokens, &["--target"]).any(is_obvious_external_path)
            || tokens.iter().skip(2).any(|token| {
                matches!(
                    token.as_str(),
                    "--manifest-path" | "--config" | "--target-dir"
                ) || token.starts_with("--manifest-path=")
                    || token.starts_with("--config=")
                    || token.starts_with("--target-dir=")
                    || token == "-Z"
                    || token.starts_with("-Z") && token.len() > 2
            });
    }
    if matches!(program, "npm" | "pnpm" | "yarn" | "bun") {
        return tokens.iter().skip(2).any(|token| {
            matches!(
                token.as_str(),
                "--prefix" | "--cwd" | "--dir" | "--script-shell" | "-C"
            ) || token.starts_with("--prefix=")
                || token.starts_with("--cwd=")
                || token.starts_with("--dir=")
                || token.starts_with("--script-shell=")
                || token.starts_with("-C") && token.len() > 2
        });
    }
    program == "make"
        && tokens.iter().skip(2).any(|token| {
            is_env_assignment(token)
                || matches!(
                    token.as_str(),
                    "-f" | "--file" | "--makefile" | "--eval" | "-C" | "--directory"
                )
                || token.starts_with("-f") && token.len() > 2
                || token.starts_with("--file=")
                || token.starts_with("--makefile=")
                || token.starts_with("--eval=")
                || token.starts_with("-C") && token.len() > 2
                || token.starts_with("--directory=")
        })
}

fn is_known_read_only(tokens: &[String], program: &str) -> bool {
    match program {
        "pwd" => matches_closed_cli_grammar(tokens, 1, &[], &[], "LP", ""),
        "ls" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--all",
                "--almost-all",
                "--directory",
                "--classify",
                "--human-readable",
                "--inode",
                "--reverse",
                "--recursive",
                "--size",
            ],
            &["--color", "--ignore", "--sort", "--time"],
            "aAldhiF1RrSst",
            "",
        ),
        "rg" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--line-number",
                "--with-filename",
                "--no-filename",
                "--ignore-case",
                "--case-sensitive",
                "--smart-case",
                "--word-regexp",
                "--fixed-strings",
                "--files-with-matches",
                "--files-without-match",
                "--count",
                "--count-matches",
                "--stats",
                "--json",
                "--no-heading",
                "--heading",
                "--hidden",
                "--no-ignore",
                "--no-ignore-vcs",
                "--files",
                "--type-list",
                "--version",
                "--help",
            ],
            &[
                "--glob",
                "--type",
                "--type-not",
                "--regexp",
                "--file",
                "--max-count",
                "--after-context",
                "--before-context",
                "--context",
                "--max-depth",
                "--max-filesize",
                "--sort",
                "--sortr",
                "--replace",
                "--engine",
                "--threads",
                "--encoding",
            ],
            "nHhisSwFclqvU",
            "egtfmABC",
        ),
        "grep" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--line-number",
                "--ignore-case",
                "--invert-match",
                "--extended-regexp",
                "--fixed-strings",
                "--recursive",
                "--files-with-matches",
                "--files-without-match",
                "--count",
                "--quiet",
                "--silent",
                "--word-regexp",
                "--line-regexp",
            ],
            &[
                "--regexp",
                "--file",
                "--max-count",
                "--after-context",
                "--before-context",
                "--context",
                "--include",
                "--exclude",
                "--exclude-from",
            ],
            "nivEFrlcqswx",
            "efmABC",
        ),
        "cat" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--number",
                "--number-nonblank",
                "--squeeze-blank",
                "--show-all",
                "--show-ends",
                "--show-tabs",
                "--show-nonprinting",
            ],
            &[],
            "nbsAETv",
            "",
        ),
        "head" => matches_closed_cli_grammar(
            tokens,
            1,
            &["--quiet", "--verbose"],
            &["--lines", "--bytes"],
            "qv",
            "nc",
        ),
        "tail" => matches_closed_cli_grammar(
            tokens,
            1,
            &["--quiet", "--verbose"],
            &["--lines", "--bytes"],
            "qv",
            "nc",
        ),
        "wc" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--lines",
                "--words",
                "--bytes",
                "--chars",
                "--max-line-length",
            ],
            &[],
            "lwcmL",
            "",
        ),
        "stat" | "file" | "which" | "whereis" | "type" => {
            matches_closed_cli_grammar(tokens, 1, &[], &[], "", "")
        }
        "du" => matches_closed_cli_grammar(
            tokens,
            1,
            &["--human-readable", "--summarize", "--kilobytes"],
            &["--max-depth"],
            "hsk",
            "d",
        ),
        "df" => matches_closed_cli_grammar(
            tokens,
            1,
            &["--human-readable", "--portability", "--kilobytes"],
            &[],
            "hPk",
            "",
        ),
        "printf" => tokens.get(1).is_none_or(|token| token != "-v"),
        "echo" => matches_closed_cli_grammar(tokens, 1, &[], &[], "neE", ""),
        "true" | "false" => tokens.len() == 1,
        "uname" => matches_closed_cli_grammar(tokens, 1, &[], &[], "asnrvmopi", ""),
        "date" => is_read_only_date(tokens),
        "sort" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--ignore-leading-blanks",
                "--dictionary-order",
                "--ignore-case",
                "--general-numeric-sort",
                "--numeric-sort",
                "--reverse",
                "--stable",
                "--unique",
            ],
            &["--key", "--field-separator", "--buffer-size"],
            "bdfgnrsu",
            "ktS",
        ),
        "cut" => matches_closed_cli_grammar(
            tokens,
            1,
            &["--complement", "--only-delimited", "--zero-terminated"],
            &["--bytes", "--characters", "--delimiter", "--fields"],
            "sz",
            "bcdf",
        ),
        "jq" => matches_closed_cli_grammar(
            tokens,
            1,
            &[
                "--raw-output",
                "--compact-output",
                "--exit-status",
                "--slurp",
                "--null-input",
                "--sort-keys",
                "--monochrome-output",
            ],
            &["--arg", "--argjson", "--slurpfile", "--rawfile"],
            "rcesnSM",
            "",
        ),
        "find" => is_read_only_find(tokens),
        "git" => is_read_only_git(tokens),
        "pip" | "pip3" => is_read_only_pip(tokens),
        "cargo" => {
            matches!(
                tokens.get(1).map(String::as_str),
                Some("--version" | "-V" | "version")
            ) && tokens.len() == 2
        }
        _ => false,
    }
}

fn is_read_only_git(tokens: &[String]) -> bool {
    let Some(subcommand) = tokens.get(1).map(|value| value.to_ascii_lowercase()) else {
        return false;
    };
    if matches!(subcommand.as_str(), "--version" | "version") {
        return tokens.len() == 2;
    }
    if subcommand == "remote" {
        return match tokens.get(2).map(String::as_str) {
            None => true,
            Some("-v" | "--verbose") => tokens.len() == 3,
            Some("get-url") => {
                let mut names = 0;
                for token in tokens.iter().skip(3) {
                    if matches!(token.as_str(), "--all" | "--push") {
                        continue;
                    }
                    if token.starts_with('-') {
                        return false;
                    }
                    names += 1;
                }
                names == 1
            }
            Some(_) => false,
        };
    }
    if subcommand != "branch" {
        return false;
    }
    match tokens.get(2).map(String::as_str) {
        None => true,
        Some("--show-current" | "-a" | "--all" | "-r" | "--remotes") => tokens.len() == 3,
        Some("--list") => tokens.iter().skip(3).all(|token| !token.starts_with('-')),
        Some(_) => false,
    }
}

fn matches_closed_cli_grammar(
    tokens: &[String],
    start: usize,
    flag_options: &[&str],
    value_options: &[&str],
    short_flags: &str,
    short_value_options: &str,
) -> bool {
    let mut index = start;
    let mut options_ended = false;
    while let Some(token) = tokens.get(index) {
        if options_ended || token == "-" || !token.starts_with('-') {
            index += 1;
            continue;
        }
        if token == "--" {
            options_ended = true;
            index += 1;
            continue;
        }
        if token.starts_with("--") {
            if flag_options.contains(&token.as_str()) {
                index += 1;
                continue;
            }
            if let Some((name, value)) = token.split_once('=') {
                if value_options.contains(&name) && !value.is_empty() {
                    index += 1;
                    continue;
                }
                return false;
            }
            if value_options.contains(&token.as_str()) && tokens.get(index + 1).is_some() {
                index += 2;
                continue;
            }
            return false;
        }

        let short = &token[1..];
        let Some(first) = short.chars().next() else {
            return false;
        };
        if short_value_options.contains(first) {
            if short.chars().count() == 1 {
                if tokens.get(index + 1).is_none() {
                    return false;
                }
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        if short
            .chars()
            .all(|character| short_flags.contains(character))
        {
            index += 1;
            continue;
        }
        return false;
    }
    true
}

fn is_read_only_date(tokens: &[String]) -> bool {
    match tokens.get(1).map(String::as_str) {
        None => true,
        Some("-u" | "--utc" | "--universal") => {
            tokens.len() == 2
                || tokens.len() == 3 && tokens.get(2).is_some_and(|format| format.starts_with('+'))
        }
        Some(format) => tokens.len() == 2 && format.starts_with('+'),
    }
}

fn is_read_only_find(tokens: &[String]) -> bool {
    let mut index = 1;
    while let Some(token) = tokens.get(index) {
        if matches!(
            token.as_str(),
            "-print"
                | "-print0"
                | "-ls"
                | "-empty"
                | "-readable"
                | "-writable"
                | "-executable"
                | "-true"
                | "-false"
                | "-prune"
                | "-a"
                | "-and"
                | "-o"
                | "-or"
                | "!"
                | "-P"
        ) {
            index += 1;
            continue;
        }
        if matches!(
            token.as_str(),
            "-maxdepth"
                | "-mindepth"
                | "-name"
                | "-iname"
                | "-path"
                | "-ipath"
                | "-type"
                | "-size"
                | "-mtime"
                | "-mmin"
                | "-newer"
                | "-user"
                | "-group"
                | "-perm"
                | "-printf"
        ) {
            if tokens.get(index + 1).is_none() {
                return false;
            }
            index += 2;
            continue;
        }
        if token.starts_with('-') {
            return false;
        }
        index += 1;
    }
    true
}

fn is_read_only_pip(tokens: &[String]) -> bool {
    match tokens.get(1).map(String::as_str) {
        Some("--version" | "-V") => tokens.len() == 2,
        Some("check") => tokens.len() == 2,
        Some("show") => matches_closed_cli_grammar(tokens, 2, &["--files"], &[], "f", ""),
        Some("freeze") => matches_closed_cli_grammar(
            tokens,
            2,
            &["--all", "--exclude-editable", "--local", "--user"],
            &["--exclude"],
            "l",
            "",
        ),
        Some("list") => matches_closed_cli_grammar(
            tokens,
            2,
            &[
                "--local",
                "--user",
                "--editable",
                "--exclude-editable",
                "--include-editable",
                "--not-required",
                "--disable-pip-version-check",
                "--no-color",
            ],
            &["--format", "--path"],
            "lue",
            "",
        ),
        _ => false,
    }
}

fn is_git_worktree_read(tokens: &[String], program: &str) -> bool {
    program == "git"
        && tokens.get(1).is_some_and(|subcommand| {
            matches!(
                subcommand.to_ascii_lowercase().as_str(),
                "status" | "diff" | "log" | "show"
            )
        })
}

fn is_jq_environment_read(tokens: &[String], program: &str) -> bool {
    program == "jq"
        && tokens.iter().skip(1).any(|token| {
            let lower = token.to_ascii_lowercase();
            lower.contains("$env")
                || lower
                    .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .any(|word| word == "env")
        })
}

fn is_jq_module_load(tokens: &[String], program: &str) -> bool {
    program == "jq"
        && tokens.iter().skip(1).any(|token| {
            token
                .to_ascii_lowercase()
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|word| matches!(word, "include" | "import" | "module"))
        })
}

fn run_shell_command(
    cwd: &Path,
    root: Option<&Path>,
    request: &AgentCommandRequest,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
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
            error: None,
            policy_evaluation: None,
        });
    }
    let timeout_ms = request
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let started = Instant::now();
    let mut command = shell_command(&request.command);
    configure_command_process_group(&mut command);
    let mut child = command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TERM", "dumb")
        .env("CI", "1")
        .spawn()
        .map_err(|error| format!("启动命令失败：{error}"))?;
    let stdout_reader = spawn_bounded_output_reader(
        child
            .stdout
            .take()
            .ok_or_else(|| "无法读取命令 stdout。".to_string())?,
    );
    let stderr_reader = spawn_bounded_output_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| "无法读取命令 stderr。".to_string())?,
    );

    let deadline = Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let mut cancelled = false;
    let exit_status = loop {
        if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
            cancelled = true;
            terminate_command_process_group(&mut child);
            break child
                .wait()
                .map_err(|error| format!("等待已取消命令失败：{error}"))?;
        }

        match child
            .try_wait()
            .map_err(|error| format!("等待命令失败：{error}"))?
        {
            Some(status) => {
                // A command may spawn descendants which keep stdout/stderr pipes open after the
                // shell exits. Clean the dedicated group before joining reader threads.
                terminate_command_process_group(&mut child);
                break status;
            }
            None if started.elapsed() >= deadline => {
                timed_out = true;
                terminate_command_process_group(&mut child);
                break child
                    .wait()
                    .map_err(|error| format!("等待超时命令失败：{error}"))?;
            }
            None => thread::sleep(Duration::from_millis(50)),
        }
    };

    let (stdout, stdout_truncated) = join_output_reader(stdout_reader, "stdout")?;
    let (stderr, stderr_truncated) = join_output_reader(stderr_reader, "stderr")?;

    Ok(AgentCommandExecutionResult {
        command: request.command.clone(),
        cwd: relative_cwd(root, cwd),
        exit_code: exit_status.code(),
        stdout,
        stderr,
        timed_out,
        cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated,
        stderr_truncated,
        error: None,
        policy_evaluation: None,
    })
}

fn command_cancel_requested(
    cancellation_token: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> bool {
    cancellation_token.is_cancelled()
        || action_cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn spawn_bounded_output_reader<R>(
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

fn join_output_reader(
    reader: thread::JoinHandle<std::io::Result<(String, bool)>>,
    stream: &str,
) -> Result<(String, bool), String> {
    reader
        .join()
        .map_err(|_| format!("读取命令 {stream} 的线程异常退出。"))?
        .map_err(|error| format!("读取命令 {stream} 失败：{error}"))
}

#[cfg(unix)]
fn configure_command_process_group(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(not(unix))]
fn configure_command_process_group(_command: &mut Command) {}

#[cfg(unix)]
fn terminate_command_process_group(child: &mut Child) {
    let Ok(process_group) = i32::try_from(child.id()) else {
        let _ = child.kill();
        return;
    };
    // The shell is created as the leader of a fresh process group. A negative pid targets that
    // entire group, so timeout/cancellation cannot leave ordinary descendants running.
    // SAFETY: `kill` does not dereference memory. The pid is derived from the live `Child` and is
    // negated intentionally to address only the process group created for that child.
    let killed = unsafe { libc::kill(-process_group, libc::SIGKILL) } == 0;
    if !killed {
        let _ = child.kill();
    }
}

#[cfg(not(unix))]
fn terminate_command_process_group(child: &mut Child) {
    // Defensive fallback for internal plumbing only. The authorized public entry point fails
    // closed on Windows until a Job Object can provide equivalent descendant cleanup.
    let _ = child.kill();
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
    // Login profiles can redefine a statically evaluated command between inspection and spawn.
    shell.arg("-c").arg(command);
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

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;
    use crate::{AgentApprovalStatus, AgentCommandPermission, AgentReadPermission};
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn external_read_risk_uses_the_protocol_wire_value() {
        assert_eq!(
            serde_json::to_value(CommandRiskClass::ExternalRead).unwrap(),
            serde_json::json!("external_read")
        );
    }

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
    fn guarded_automatic_policy_routes_risky_commands_to_explicit_approval() {
        for command in [
            "pip install openpyxl",
            "pip3 install openpyxl 2>&1",
            "printf hello > result.txt",
            "git reset --hard",
            "python3 -c 'print(1)'",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
        }
    }

    #[test]
    fn full_access_automatic_and_explicit_user_allow_non_catastrophic_risk() {
        for command in [
            "pip install openpyxl",
            "pip3 install openpyxl 2>&1",
            "printf hello > result.txt",
            "git reset --hard",
            "python3 -c 'print(1)'",
        ] {
            for (policy, source) in [
                (
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                ),
                (
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::ExplicitUser,
                ),
            ] {
                assert_eq!(
                    evaluate_command_policy(command, policy, source).decision,
                    CommandPolicyDecision::Allow,
                    "{command}"
                );
            }
        }
    }

    #[test]
    fn catastrophic_and_unsupported_commands_are_denied_for_every_authorization() {
        for command in [
            "rm -rf /",
            "sudo whoami",
            "pkexec whoami",
            "run0 whoami",
            "mkfs.ext4 /dev/disk9",
            "dd if=/dev/zero of=/dev/disk9",
            "vim file.txt",
            "echo 'unterminated",
        ] {
            for (policy, source) in [
                (
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                ),
                (
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                ),
                (
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::ExplicitUser,
                ),
            ] {
                assert_eq!(
                    evaluate_command_policy(command, policy, source).decision,
                    CommandPolicyDecision::Deny,
                    "{command}"
                );
            }
        }
    }

    #[test]
    fn guarded_automatic_keeps_only_statically_read_only_commands_automatic() {
        for command in [
            "git remote -v",
            "pip list",
            "cargo --version",
            "true && rg policy crates",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn shell_reserved_function_and_group_syntax_is_always_denied() {
        for command in [
            "! sudo whoami",
            "{ sudo whoami; }",
            "if true; then sudo whoami; fi",
            "function elevate { sudo whoami; }",
            "elevate() { sudo whoami; }",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
            assert!(
                evaluation.findings.iter().any(|finding| {
                    finding.code == "command.unsupported.shell_reserved_syntax"
                        || finding.code == "command.unsupported.shell_group"
                }),
                "{command}: {:?}",
                evaluation.findings
            );
        }
    }

    #[test]
    fn guarded_automatic_does_not_trust_explicit_program_paths_or_environment_overrides() {
        for command in [
            "./ls",
            "/bin/ls",
            "tools/ls",
            "./busybox ls",
            "/tmp/busybox ls",
            "./command ls",
            "/tmp/builtin ls",
            "PATH=/tmp ls",
            "LD_PRELOAD=./shim.so ls",
            "DYLD_INSERT_LIBRARIES=./shim.dylib ls",
            "GIT_EXTERNAL_DIFF=./diff-helper git status",
            "env -u PATH ls",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                evaluation.findings
            );
            assert!(evaluation.findings.iter().any(|finding| {
                matches!(
                    finding.code.as_str(),
                    "command.risk.explicit_program_path" | "command.risk.environment_override"
                )
            }));
        }

        assert_eq!(
            evaluate_command_policy(
                "ls",
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow
        );

        let busybox_install = evaluate_command_policy(
            "busybox --install ls",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            busybox_install.decision,
            CommandPolicyDecision::RequireExplicitApproval
        );
        assert!(busybox_install
            .findings
            .iter()
            .any(|finding| finding.risk == CommandRiskClass::Unknown));
    }

    #[test]
    fn guarded_automatic_routes_dynamic_arguments_and_environment_reads_to_approval() {
        for command in [
            "echo $SECRET",
            "cat $HOME/.config/token",
            "cat ${HOME}/.config/token",
            "cat $(printf /etc/passwd)",
            "cat `printf /etc/passwd`",
            "env",
            "printenv",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                evaluation.findings
            );
        }
    }

    #[test]
    fn read_only_commands_with_execution_hooks_are_not_automatically_trusted() {
        for command in [
            "rg --pre helper pattern .",
            "rg --pre=helper pattern .",
            "rg --pre-glob '*.md' pattern .",
            "sort --compress-program=gzip input.txt",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }

        for command in [
            "git diff",
            "git show HEAD",
            "git log -1",
            "git show --ext-diff HEAD",
            "git log --textconv",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                evaluation.findings
            );
            assert!(evaluation
                .findings
                .iter()
                .any(|finding| finding.code == "command.risk.git_external_diff"));
        }
    }

    #[test]
    fn workspace_only_reads_route_obvious_external_paths_to_approval() {
        let workspace_only = policy_permissions(
            AgentReadPermission::WorkspaceOnly,
            AgentCommandSafetyPolicy::Guarded,
        );
        for command in [
            "cat /etc/passwd",
            "head ~/.ssh/id_ed25519",
            "tail ../outside.log",
            "rg token @home",
            "grep -f/etc/passwd pattern local.txt",
            "rg -f/etc/patterns pattern .",
            "stat file:///etc/passwd",
        ] {
            let evaluation = evaluate_command_policy_with_permissions(
                command,
                workspace_only,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                evaluation.findings
            );
            assert!(evaluation
                .findings
                .iter()
                .any(|finding| finding.code == "command.scope.external_read"));
        }

        assert_eq!(
            evaluate_command_policy_with_permissions(
                "cat local.txt",
                workspace_only,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow
        );
        assert_eq!(
            evaluate_command_policy_with_permissions(
                "cat /etc/passwd",
                policy_permissions(AgentReadPermission::All, AgentCommandSafetyPolicy::Guarded),
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow
        );
    }

    #[test]
    fn recursive_destruction_of_platform_sensitive_roots_is_always_denied() {
        for target in [
            "/Volumes",
            "/Volumes/Backup",
            "/private/etc",
            "/dev",
            "/boot",
            "/lib",
            "/root",
            "/proc",
            "/sys",
            "/mnt/data",
            "/media/user/drive",
        ] {
            let command = format!("rm -rf {target}");
            let evaluation = evaluate_command_policy(
                &command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::Deny,
                "{command}: {:?}",
                evaluation.findings
            );
            assert_eq!(
                evaluation.code, "command.catastrophic.filesystem_root",
                "{command}"
            );
        }
    }

    #[test]
    fn guarded_automatic_allows_only_explicitly_allowlisted_workspace_tasks() {
        for command in ["cargo test", "pnpm lint", "npm run build", "make test"] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }

        for command in [
            "cargo run",
            "pnpm deploy",
            "npm run arbitrary-script",
            "make deploy",
            "npm publish",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
        }
    }

    #[test]
    fn guarded_automatic_workspace_tasks_use_a_closed_runner_grammar() {
        for command in [
            "cargo test --manifest-path /tmp/evil/Cargo.toml",
            "cargo test --config build.rustc-wrapper=evil",
            "cargo test --target /tmp/evil.json",
            "cargo test --target=../evil.json",
            "make test -f /tmp/evil.mk",
            "make test --eval='test:; @echo pwn'",
            "npm test --prefix /tmp/evil",
            "yarn test --cwd /tmp/evil",
            "pnpm test --dir /tmp/evil",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }

        for command in [
            "cargo test -p mycopilot-core --locked -- --nocapture",
            "npm run build",
            "pnpm lint",
            "make check",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn read_only_closed_grammar_blocks_execution_and_write_option_bypasses() {
        for command in [
            "rg --hostname-bin=/tmp/evil --hyperlink-format='file://{host}{path}' pattern .",
            "rg -z pattern .",
            "rg --search-zip pattern .",
            "find . -ok sh -c 'echo pwn' \\;",
            "find . -okdir sh -c 'echo pwn' \\;",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }

        for command in [
            "sort --output /tmp/output input",
            "sort -o/tmp/output input",
            "sort --temporary-directory /tmp input",
            "sort -T/tmp input",
            "find . -fprint0 /tmp/output",
            "file -C -m ./magic",
            "file --compile --magic-file ./magic",
            "file -z archive.gz",
            "date -f /etc/passwd",
            "cargo metadata",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
        }

        assert_eq!(
            evaluate_command_policy(
                "pip list --outdated",
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .risk_level,
            AgentCommandRiskLevel::Network
        );
    }

    #[test]
    fn git_read_grammar_does_not_auto_run_hooks_or_accept_mutating_suffixes() {
        for command in [
            "git status --short",
            "git diff",
            "git log -p",
            "git show HEAD",
            "git log --output=/tmp/log",
            "git remote -v add origin https://example.invalid/repo",
            "git branch -a -D victim",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
        }
        for command in [
            "git remote",
            "git remote -v",
            "git remote get-url origin",
            "git branch",
            "git branch --show-current",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn environment_equivalents_and_shell_path_expansion_require_approval() {
        for command in [
            "jq -n env",
            "jq -n '$ENV'",
            "jq 'include \"helpers\"; run' data.json",
            "cat {../secret,local}",
            "cat .?/.ssh/id_rsa",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
        }
    }

    #[test]
    fn direct_system_control_and_recursive_symlink_traversal_are_always_denied() {
        for command in [
            "dd if=/dev/zero of=/dev/mem",
            "tee /dev/kmem",
            "echo b > /proc/sysrq-trigger",
            "printf 1 > /sys/kernel/control",
            "systemctl reboot",
            "systemctl isolate reboot.target",
            "systemctl --no-block start poweroff.target",
            "systemctl isolate runlevel6.target",
            "systemctl isolate runlevel0.target",
            "systemctl --no-block isolate multi-user.target",
            "systemctl restart runlevel6.target",
            "systemctl start ctrl-alt-del.target",
            "systemctl enable --now reboot.target",
            "systemctl --now reenable runlevel6.target",
            "systemctl preset runlevel0.target --now",
            "systemctl --now preset ctrl-alt-del.target",
            "systemctl link --now reboot.target",
            "systemctl reload-or-restart poweroff.target",
            "systemctl try-reload-or-restart reboot.target",
            "systemctl reload-or-try-restart ctrl-alt-del.target",
            "systemctl start 'reboot.*'",
            "systemctl start '*.target'",
            "systemctl restart 'runlevel?.target'",
            "systemctl soft-reboot",
            "init 0",
            "telinit 6",
            "launchctl reboot system",
            "kill -9 1",
            "kill -9 0001",
            "kill -9 -1",
            "kill -9 -0001",
            "kill -9 '1'</dev/null",
            "kill -9 \"0001\"</dev/null",
            "kill -9 \\1</dev/null",
            "chmod -RL 000 link",
            "chown -RH root link",
            "rm -rf /var/lib",
            "rm -rf /private/var/db",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn every_compound_segment_is_evaluated() {
        for command in [
            "true && pip install openpyxl",
            "true; /usr/local/bin/pip3 install openpyxl",
            "printf ok | sh -c 'pip install openpyxl'",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
            assert!(evaluation
                .findings
                .iter()
                .any(|finding| finding.risk == CommandRiskClass::PackageManagement));
        }

        let wrapped_redirect = evaluate_command_policy(
            "sh -c 'printf ok' > output.txt",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            wrapped_redirect.decision,
            CommandPolicyDecision::RequireExplicitApproval
        );
        assert!(wrapped_redirect
            .findings
            .iter()
            .any(|finding| finding.risk == CommandRiskClass::DirectWrite));
    }

    #[test]
    fn nested_catastrophic_commands_cannot_hide_in_shell_wrappers_or_substitutions() {
        for command in [
            "sh -c 'rm -rf /'",
            "bash -lc \"sudo whoami\"",
            "env -u TOKEN /usr/bin/sudo whoami",
            "command env -S 'sudo whoami'",
            "busybox env -S 'sudo whoami'",
            "echo $(rm -rf /)",
            "echo `sudo whoami`",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn opaque_wrappers_dynamic_programs_and_background_execution_are_always_denied() {
        for command in [
            "env -S 'sh -c \"sudo whoami\"'",
            "env -S\"sh -c 'sudo whoami'\" true",
            "$DIR/env -S 'printf ok'",
            "$DIR/busybox ls",
            "$DIR/command ls",
            "command $DIR/env -S 'printf ok'",
            "xargs sh -c 'sudo whoami'",
            "source ./script.sh",
            "time -o timing.txt sudo whoami",
            "timeout 5 sh -c 'echo ok'",
            "X=sudo $X whoami",
            "$(printf sudo) whoami",
            "make test & true",
        ] {
            let evaluation = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
        assert_eq!(
            evaluate_command_policy(
                "busybox rm -rf /",
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::ExplicitUser,
            )
            .decision,
            CommandPolicyDecision::Deny
        );
    }

    #[test]
    fn explicit_shell_scripts_require_guarded_approval_but_are_trusted_after_authorization() {
        for command in [
            "sh ./skill-script.sh --check",
            "bash -e scripts/install.sh",
            "zsh -- ./scripts/verify.zsh",
            "fish -C 'echo preparing' ./scripts/verify.fish",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }

        for command in [
            "bash --rcfile ./startup.bash -c 'echo ok'",
            "bash --init-file ./startup.bash -c 'echo ok'",
            "bash -lc 'echo ok'",
            "bash --debugger -c 'echo ok'",
            "bash -O extglob -c 'echo ok'",
            "zsh -c 'echo ok'",
            "fish -c 'echo ok'",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::Guarded,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}"
            );
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }

        for command in [
            "sh $SCRIPT",
            "bash ~/script.sh",
            "zsh scripts/*.zsh",
            "bash -i",
            "bash -ic 'echo interactive'",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn raw_devices_and_dynamic_redirection_targets_are_always_denied() {
        for command in [
            "cat > /dev/disk9",
            "cat > /tmp/../dev/disk9",
            "tee /dev/nvme0n1",
            "dd if=/dev/zero of=/tmp/../dev/disk9",
            "sort -o /dev/rdisk4 input.txt",
            "find . -fprint /dev/sda",
            "printf value > $TARGET",
            "printf value >&/dev/disk9",
            "printf value >>&/tmp/../dev/disk9",
            "printf value > /dev/d[i]sk9",
            "printf value > ~/output.txt",
            "printf value > /dev/{disk,rdisk}9",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }

        for command in [
            "printf value >&1",
            "printf value 2>&1",
            "dd if=/dev/zero of=/dev/null count=0",
        ] {
            assert_ne!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }

        let ordinary_posix_redirect = evaluate_command_policy(
            "printf value >&output.txt",
            AgentCommandSafetyPolicy::Guarded,
            CommandAuthorizationSource::Automatic,
        );
        assert_eq!(
            ordinary_posix_redirect.decision,
            CommandPolicyDecision::RequireExplicitApproval
        );
        assert!(ordinary_posix_redirect
            .findings
            .iter()
            .any(|finding| finding.risk == CommandRiskClass::DirectWrite));
    }

    #[test]
    fn device_tcp_and_udp_input_redirections_are_never_automatic() {
        for command in [
            "cat </dev/tcp/example.com/80",
            "head < /dev/udp/example.com/53",
            "cat <local.txt</dev/tcp/example.com/80",
        ] {
            let automatic = evaluate_command_policy(
                command,
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                automatic.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                automatic.findings
            );
            assert_eq!(automatic.risk_level, AgentCommandRiskLevel::Network);
            assert!(automatic
                .findings
                .iter()
                .any(|finding| finding.code == "command.risk.network_redirection"));

            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::Automatic,
                )
                .decision,
                CommandPolicyDecision::Allow,
                "{command}"
            );
        }
    }

    #[test]
    fn input_redirection_targets_participate_in_read_scope_and_syntax_policy() {
        let workspace_only = policy_permissions(
            AgentReadPermission::WorkspaceOnly,
            AgentCommandSafetyPolicy::Guarded,
        );
        for command in [
            "cat </etc/passwd",
            "cat <local.txt</etc/passwd",
            "grep x <../secret",
            "head <\"$HOME/.ssh/id_rsa\"",
        ] {
            let evaluation = evaluate_command_policy_with_permissions(
                command,
                workspace_only,
                CommandAuthorizationSource::Automatic,
            );
            assert_eq!(
                evaluation.decision,
                CommandPolicyDecision::RequireExplicitApproval,
                "{command}: {:?}",
                evaluation.findings
            );
            assert!(evaluation.findings.iter().any(|finding| {
                matches!(
                    finding.code.as_str(),
                    "command.scope.external_input_redirection"
                        | "command.risk.dynamic_input_redirection"
                )
            }));
        }

        assert_eq!(
            evaluate_command_policy_with_permissions(
                "cat <local.txt",
                workspace_only,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow
        );
        assert_eq!(
            evaluate_command_policy(
                "cat </etc/passwd",
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::Allow
        );
        assert_eq!(
            evaluate_command_policy(
                "cat <>local.txt",
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            )
            .decision,
            CommandPolicyDecision::RequireExplicitApproval
        );

        for command in [
            "cat <<EOF",
            "cat <<<secret",
            "cat <(printf secret)",
            "cat <&3",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn find_exec_and_opaque_recursive_targets_are_always_denied() {
        for command in [
            "find . -exec sh -c 'sudo whoami' \\;",
            "find . -execdir sudo whoami \\;",
            "find /[e]tc -delete",
            "find -H / -delete",
            "find -L /Users/example -delete",
            "rm -rf /[e]tc",
            "rm -rf target/*",
            "chmod -R 000 ~root",
            "chown -R user /{etc,var}",
        ] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn destructive_targets_are_lexically_resolved_against_cwd() {
        for command in ["rm -rf /tmp/../etc", "rm -rf /Users/example/*"] {
            assert_eq!(
                evaluate_command_policy(
                    command,
                    AgentCommandSafetyPolicy::FullAccess,
                    CommandAuthorizationSource::ExplicitUser,
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }

        for command in ["rm -rf ..", "rm -rf ../*", "chmod -R 755 .."] {
            assert_eq!(
                evaluate_command_policy_at(
                    command,
                    policy_permissions(
                        AgentReadPermission::All,
                        AgentCommandSafetyPolicy::FullAccess,
                    ),
                    CommandAuthorizationSource::ExplicitUser,
                    None,
                    Some(Path::new("/Users/example/project")),
                )
                .decision,
                CommandPolicyDecision::Deny,
                "{command}"
            );
        }
    }

    #[test]
    fn runs_simple_command_in_workspace() {
        let workspace = TestWorkspace::new();
        let result = run_authorized_command(
            Some(&workspace.path),
            &request("printf hello", Some(5_000)),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            CommandAuthorizationSource::ExplicitUser,
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

        let result = run_authorized_command(
            Some(&workspace.path),
            &request,
            AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout, "outside");
        assert_eq!(result.cwd, outside.path.to_string_lossy());
    }

    #[test]
    fn executor_rechecks_workspace_only_read_scope_against_canonical_cwd() {
        let workspace = TestWorkspace::new();
        let outside = TestWorkspace::new();
        std::fs::write(outside.path.join("local.txt"), "outside-secret").unwrap();
        let mut command = request("cat local.txt", Some(5_000));
        command.cwd = Some(outside.path.to_string_lossy().to_string());
        let permissions = AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..Default::default()
        };

        let error = run_authorized_command(
            Some(&workspace.path),
            &command,
            permissions,
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
        assert_eq!(
            error
                .policy_evaluation()
                .map(|evaluation| evaluation.decision),
            Some(CommandPolicyDecision::RequireExplicitApproval)
        );
        assert!(error
            .policy_evaluation()
            .is_some_and(|evaluation| evaluation
                .findings
                .iter()
                .any(|finding| finding.code == "command.scope.external_cwd")));

        let result = run_authorized_command(
            Some(&workspace.path),
            &command,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();
        assert_eq!(result.stdout, "outside-secret");
    }

    #[test]
    fn times_out_long_command() {
        let workspace = TestWorkspace::new();
        let result = run_authorized_command(
            Some(&workspace.path),
            &request("sleep 2", Some(50)),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                ..Default::default()
            },
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert!(result.timed_out);
    }

    #[test]
    fn pre_start_action_cancellation_never_spawns_the_command() {
        let workspace = TestWorkspace::new();
        let cancel_flag = Arc::new(AtomicBool::new(true));
        let result = run_shell_command(
            &workspace.path,
            Some(&workspace.path),
            &request("mkdir must-not-exist", Some(5_000)),
            AgentCancellationToken::new(),
            Some(cancel_flag),
        )
        .unwrap();

        assert!(result.cancelled);
        assert_eq!(result.exit_code, None);
        assert!(!workspace.path.join("must-not-exist").exists());
    }

    #[cfg(unix)]
    #[test]
    fn timeout_terminates_the_shell_process_group() {
        let workspace = TestWorkspace::new();
        let started = Instant::now();
        let result = run_shell_command(
            &workspace.path,
            Some(&workspace.path),
            &request("sleep 30 & wait", Some(50)),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert!(result.timed_out);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn completed_shell_does_not_leave_a_descendant_holding_output_pipes() {
        let workspace = TestWorkspace::new();
        let started = Instant::now();
        let result = run_shell_command(
            &workspace.path,
            Some(&workspace.path),
            &request("sleep 30 &", Some(5_000)),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[test]
    fn drains_and_bounds_large_stdout_and_stderr_without_deadlock() {
        let workspace = TestWorkspace::new();
        let result = run_shell_command(
            &workspace.path,
            Some(&workspace.path),
            &request(
                "head -c 200000 /dev/zero; head -c 200000 /dev/zero >&2",
                Some(5_000),
            ),
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout_truncated);
        assert!(result.stderr_truncated);
        assert!(result.stdout.len() <= MAX_OUTPUT_BYTES + 32);
        assert!(result.stderr.len() <= MAX_OUTPUT_BYTES + 32);
    }

    #[test]
    fn execution_recomputes_policy_instead_of_trusting_declared_risk() {
        let workspace = TestWorkspace::new();
        let mut command = request("printf trusted", Some(5_000));
        command.risk_level = Some(AgentCommandRiskLevel::Destructive);

        let result = run_authorized_command(
            Some(&workspace.path),
            &command,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                ..Default::default()
            },
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap();

        assert_eq!(result.stdout, "trusted");

        let mut disguised_catastrophe = request("rm -rf /", Some(5_000));
        disguised_catastrophe.risk_level = Some(AgentCommandRiskLevel::ReadOnly);
        let error = run_authorized_command(
            Some(&workspace.path),
            &disguised_catastrophe,
            AgentPermissions {
                read: AgentReadPermission::All,
                write: AgentWritePermission::All,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::FullAccess,
                ..Default::default()
            },
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("command.catastrophic.filesystem_root"));
        assert_eq!(
            error
                .policy_evaluation()
                .map(|evaluation| evaluation.code.as_str()),
            Some("command.catastrophic.filesystem_root")
        );
    }

    #[test]
    fn executor_refuses_guarded_automatic_command_that_needs_approval() {
        let workspace = TestWorkspace::new();
        let error = run_authorized_command(
            Some(&workspace.path),
            &request("printf blocked > output.txt", Some(5_000)),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::AutoApprove,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                ..Default::default()
            },
            CommandAuthorizationSource::Automatic,
            AgentCancellationToken::new(),
            None,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("command.explicit_approval_required"));
        assert_eq!(
            error
                .policy_evaluation()
                .map(|evaluation| evaluation.decision),
            Some(CommandPolicyDecision::RequireExplicitApproval)
        );
        assert!(!workspace.path.join("output.txt").exists());
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

    fn policy_permissions(
        read: AgentReadPermission,
        command_safety: AgentCommandSafetyPolicy,
    ) -> AgentPermissions {
        AgentPermissions {
            read,
            command_safety,
            ..AgentPermissions::default()
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

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    #[test]
    fn windows_execution_fails_closed_until_native_containment_exists() {
        for (safety, source) in [
            (
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::Automatic,
            ),
            (
                AgentCommandSafetyPolicy::FullAccess,
                CommandAuthorizationSource::Automatic,
            ),
            (
                AgentCommandSafetyPolicy::Guarded,
                CommandAuthorizationSource::ExplicitUser,
            ),
        ] {
            let evaluation = evaluate_command_policy("echo hello", safety, source);
            assert_eq!(evaluation.decision, CommandPolicyDecision::Deny);
            assert_eq!(evaluation.code, "command.unsupported.windows_execution");
        }
    }
}
