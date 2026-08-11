use super::*;

pub(super) fn assess_segment(
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

    if segment.has_heredoc && is_shell_program(&program) {
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::Unsupported,
            "command.unsupported.shell_heredoc",
            "shell 解释器会把 heredoc 正文作为脚本执行，当前安全分析器拒绝这种组合。",
        ));
        return Ok(());
    }

    // A quoted here-document is an explicit, finite program body for these interpreters rather
    // than interactive terminal input. Its contents remain opaque code: guarded automatic mode
    // must route it to approval, while an explicit/full-access authorization may run it.
    if segment.has_heredoc && matches!(program.as_str(), "python" | "python3" | "node") {
        findings.push(finding(
            segment_index,
            &program,
            CommandRiskClass::Unknown,
            "command.risk.heredoc_interpreter",
            "解释器将执行 quoted heredoc 中的不透明代码；自动执行需要明确批准。",
        ));
        append_redirection_findings(segment, segment_index, &program, read_permission, findings);
        return Ok(());
    }

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

pub(super) fn append_shell_script_finding(
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

pub(super) fn append_invocation_provenance_findings(
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

pub(super) fn is_explicit_program_path(program: &str) -> bool {
    program.contains(['/', '\\']) || program.starts_with(['.', '~'])
}

pub(super) fn has_environment_override(tokens: &[String], program_index: usize) -> bool {
    tokens
        .iter()
        .take(program_index)
        .any(|token| is_env_assignment(token) || program_basename(token) == "env")
}

pub(super) fn has_dynamic_argument(argument: &str) -> bool {
    argument.contains(['$', '`'])
}

pub(super) fn has_shell_path_expansion(argument: &str) -> bool {
    argument.starts_with('~') || argument.contains(['*', '?', '[', ']', '{', '}'])
}

pub(super) fn is_shell_reserved_program(program: &str) -> bool {
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

pub(super) fn has_obvious_external_read_argument(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_obvious_external_path(argument: &str) -> bool {
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

pub(super) fn is_network_redirection_target(target: &str) -> bool {
    let lower = target
        .trim_matches(['"', '\''])
        .to_ascii_lowercase()
        .replace('\\', "/");
    lower.starts_with("/dev/tcp/") || lower.starts_with("/dev/udp/")
}

pub(super) fn append_redirection_findings(
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

pub(super) fn is_dynamic_program_name(program: &str) -> bool {
    program.contains('$')
        || program.contains('`')
        || program.contains("$()")
        || program.contains("``")
        || program.contains(['*', '?', '[', '{', '}'])
}

pub(super) fn env_split_command(tokens: &[String]) -> Result<Option<&str>, CommandSyntaxError> {
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

pub(super) fn finding(
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

pub(super) fn aggregate_risk_level(findings: &[CommandPolicyFinding]) -> AgentCommandRiskLevel {
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
