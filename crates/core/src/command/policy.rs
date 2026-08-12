use super::*;

pub(super) const MAX_POLICY_NESTING: usize = 8;

#[derive(Debug)]
pub(super) struct ShellSegment {
    pub(super) tokens: Vec<String>,
    /// Character ranges for the source words corresponding one-for-one with [`Self::tokens`].
    ///
    /// These ranges are deliberately retained by the authoritative lexer so trusted managed
    /// runtimes can compile only already-validated operands (for example a frozen PDF input)
    /// without regex replacement or reparsing dequoted strings. They are Host-only metadata and
    /// never enter the durable command/request projection.
    pub(super) token_ranges: Vec<std::ops::Range<usize>>,
    pub(super) has_write_redirection: bool,
    pub(super) input_redirections: Vec<ShellInputRedirection>,
    /// A safely quoted here-document feeds data to this segment. The body is deliberately absent:
    /// it is shell input, not another command. Shell interpreters remain forbidden below because
    /// they would execute that input as a script.
    pub(super) has_heredoc: bool,
}

#[derive(Debug)]
pub(super) struct ShellInputRedirection {
    pub(super) target: String,
    pub(super) dynamic: bool,
    pub(super) target_range: std::ops::Range<usize>,
    pub(super) read_write: bool,
}

#[derive(Debug)]
pub(super) struct LexedCommand {
    pub(super) segments: Vec<ShellSegment>,
    pub(super) embedded_commands: Vec<String>,
    /// Character ranges occupied by quoted here-document bodies and terminators. Independent
    /// policy scans must skip these ranges so data containing `>` or command-looking text is not
    /// reinterpreted as shell syntax.
    pub(super) ignored_policy_ranges: Vec<std::ops::Range<usize>>,
}

#[derive(Debug)]
pub(crate) struct CommandSyntaxError {
    pub(crate) code: &'static str,
    pub(crate) reason: &'static str,
}

/// Evaluates a command using the same classifier used to populate proposal risk metadata.
///
/// `RequireExplicitApproval` is a routing decision, not an execution failure: callers should
/// surface an approval request and start the exact frozen action through the Host command Session
/// with [`CommandAuthorizationSource::ExplicitUser`] only after approval.
///
/// This context-free classifier uses unrestricted read scope.
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
pub(super) fn evaluate_command_policy_at(
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
pub(super) fn evaluate_command_policy_at(
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

pub(super) fn requires_explicit_approval(risk: CommandRiskClass) -> bool {
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

pub(super) fn analyze_command(
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
    let command = normalize_command_text(command)?;
    let lexed = lex_command(&command)?;
    validate_write_redirection_targets(&command, &lexed.ignored_policy_ranges)?;
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

/// Returns the one canonical command representation used by model ingestion, policy, approval,
/// checkpoints, and process launch.
///
/// JSON providers commonly emit CRLF (and occasionally lone CR) even on Unix. Normalize both
/// transport spellings to LF before any durable or executable consumer sees the command. Leading
/// and trailing whitespace is semantically meaningful for scripts and here-documents, so it is
/// preserved; trimming is used only to reject an all-whitespace command.
pub(crate) fn normalize_command_text(command: &str) -> Result<String, CommandSyntaxError> {
    let normalized = command.replace("\r\n", "\n").replace('\r', "\n");
    if normalized.trim().is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.empty",
            reason: "命令不能为空。",
        });
    }
    if normalized.chars().count() > MAX_COMMAND_CHARS {
        return Err(CommandSyntaxError {
            code: "command.malformed.too_long",
            reason: "命令超过允许的长度上限。",
        });
    }
    if normalized.contains('\0') {
        return Err(CommandSyntaxError {
            code: "command.malformed.nul",
            reason: "命令不能包含空字符。",
        });
    }
    Ok(normalized)
}

pub(super) fn validate_write_redirection_targets(
    command: &str,
    ignored_ranges: &[std::ops::Range<usize>],
) -> Result<(), CommandSyntaxError> {
    let chars = command.chars().collect::<Vec<_>>();
    let mut quote = None;
    let mut escaped = false;
    let mut index = 0;
    while index < chars.len() {
        if let Some(range) = ignored_ranges.iter().find(|range| range.contains(&index)) {
            index = range.end;
            continue;
        }
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

pub(super) fn consume_fd_duplication(chars: &[char], start: usize) -> Option<usize> {
    if chars.get(start) == Some(&'-') && is_shell_boundary(chars.get(start + 1).copied()) {
        return Some(start + 1);
    }
    let mut index = start;
    while chars.get(index).is_some_and(|value| value.is_ascii_digit()) {
        index += 1;
    }
    (index > start && is_shell_boundary(chars.get(index).copied())).then_some(index)
}

pub(super) fn is_shell_boundary(character: Option<char>) -> bool {
    character.is_none_or(|value| value.is_whitespace() || matches!(value, ';' | '|' | '&' | '>'))
}

pub(super) fn read_redirection_target(
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
