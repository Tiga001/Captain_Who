use super::*;

#[derive(Debug)]
struct PendingHeredoc {
    delimiter: String,
}

pub(super) fn lex_command(command: &str) -> Result<LexedCommand, CommandSyntaxError> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Quote {
        None,
        Single,
        Double,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum SeparatorState {
        None,
        Complete,
        RequiresCommand,
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
    let mut segment_has_heredoc = false;
    let mut pending_heredoc: Option<PendingHeredoc> = None;
    let mut ignored_policy_ranges = Vec::new();
    let mut quote = Quote::None;
    let mut index = 0;
    let mut separator_state = SeparatorState::None;

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
                    // POSIX removes a backslash-newline pair before tokenization, including
                    // inside double quotes. Mirroring that behavior is security-significant:
                    // `r\\\nm` must be classified as `rm`, not an unrelated opaque program.
                    if *next != '\n' {
                        token.push(*next);
                    }
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
                if character == '\n' {
                    push_token(&mut tokens, &mut token);
                    token_has_quoted_or_escaped_content = false;
                    // A newline following `|`, `&&`, or `||` is a shell continuation, not the
                    // beginning of a here-document body or another empty command.
                    if separator_state == SeparatorState::RequiresCommand {
                        index += 1;
                        continue;
                    }
                    if !tokens.is_empty() {
                        segments.push(ShellSegment {
                            tokens: std::mem::take(&mut tokens),
                            has_write_redirection: write_redirection,
                            input_redirections: std::mem::take(&mut input_redirections),
                            has_heredoc: segment_has_heredoc,
                        });
                        write_redirection = false;
                        segment_has_heredoc = false;
                    }
                    separator_state = SeparatorState::None;
                    index += 1;
                    if let Some(heredoc) = pending_heredoc.take() {
                        let (next, ignored) = consume_heredoc_body(&chars, index, &heredoc)?;
                        ignored_policy_ranges.push(ignored);
                        index = next;
                    }
                    continue;
                }
                if character.is_whitespace() {
                    push_token(&mut tokens, &mut token);
                    token_has_quoted_or_escaped_content = false;
                    index += 1;
                    continue;
                }
                if character == '#' && token.is_empty() && !token_has_quoted_or_escaped_content {
                    // Shell comments end at the physical newline. Stopping analysis for the
                    // entire string here would let a later line bypass policy.
                    while chars.get(index).is_some_and(|character| *character != '\n') {
                        index += 1;
                    }
                    continue;
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
                    if *next != '\n' {
                        token.push(*next);
                    }
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
                    if chars.get(index + 2) == Some(&'<') {
                        return Err(CommandSyntaxError {
                            code: "command.unsupported.here_string",
                            reason: "run_command 不支持 here-string；请使用普通参数或安全引用的 heredoc。",
                        });
                    }
                    if pending_heredoc.is_some() {
                        return Err(CommandSyntaxError {
                            code: "command.unsupported.multiple_heredoc",
                            reason:
                                "一个复合命令行最多允许一个 heredoc。请拆成多个 run_command 调用。",
                        });
                    }
                    if token.is_empty()
                        || token_has_quoted_or_escaped_content
                        || !token.chars().all(|character| character.is_ascii_digit())
                    {
                        push_token(&mut tokens, &mut token);
                    } else {
                        // `0<<'EOF'` selects stdin; the IO number is not an argv token.
                        token.clear();
                    }
                    token_has_quoted_or_escaped_content = false;
                    let (heredoc, next) = read_heredoc_delimiter(&chars, index + 2)?;
                    pending_heredoc = Some(heredoc);
                    segment_has_heredoc = true;
                    index = next;
                    continue;
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
                        has_heredoc: segment_has_heredoc,
                    });
                    write_redirection = false;
                    segment_has_heredoc = false;
                    index += 1;
                    if chars.get(index) == Some(&character) && matches!(character, '|' | '&') {
                        index += 1;
                    }
                    separator_state = if character == ';' {
                        SeparatorState::Complete
                    } else {
                        SeparatorState::RequiresCommand
                    };
                    continue;
                }

                separator_state = SeparatorState::None;
                token.push(character);
                index += 1;
            }
        }
    }
    if pending_heredoc.is_some() {
        return Err(CommandSyntaxError {
            code: "command.malformed.heredoc_header_newline_missing",
            reason: "heredoc 声明后缺少正文换行和精确终止符。",
        });
    }

    if quote != Quote::None {
        return Err(CommandSyntaxError {
            code: "command.malformed.unclosed_quote",
            reason: "命令包含未闭合的引号。",
        });
    }
    push_token(&mut tokens, &mut token);
    if separator_state != SeparatorState::None && tokens.is_empty() {
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
            has_heredoc: segment_has_heredoc,
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
        ignored_policy_ranges,
    })
}

fn read_heredoc_delimiter(
    chars: &[char],
    mut index: usize,
) -> Result<(PendingHeredoc, usize), CommandSyntaxError> {
    while chars
        .get(index)
        .is_some_and(|character| matches!(character, ' ' | '\t'))
    {
        index += 1;
    }
    let mut delimiter = String::new();
    let mut quote = None;
    let mut quoted = false;
    let mut escaped = false;
    while let Some(character) = chars.get(index).copied() {
        if escaped {
            if character == '\n' {
                return Err(CommandSyntaxError {
                    code: "command.malformed.heredoc_delimiter",
                    reason: "heredoc delimiter 不能跨行。",
                });
            }
            quoted = true;
            delimiter.push(character);
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
                delimiter.push(character);
            }
            index += 1;
            continue;
        }
        if matches!(character, '\'' | '"') {
            quoted = true;
            quote = Some(character);
            index += 1;
        } else if character.is_whitespace() || matches!(character, ';' | '|' | '&' | '<' | '>') {
            break;
        } else {
            delimiter.push(character);
            index += 1;
        }
    }
    if escaped || quote.is_some() || delimiter.is_empty() {
        return Err(CommandSyntaxError {
            code: "command.malformed.heredoc_delimiter",
            reason: "heredoc delimiter 为空、未闭合或格式无效。",
        });
    }
    if !quoted {
        return Err(CommandSyntaxError {
            code: "command.unsupported.unquoted_heredoc",
            reason:
                "未引用的 heredoc 会执行 shell 展开，无法安全核验；请将 delimiter 写成 <<'NAME'。",
        });
    }
    Ok((PendingHeredoc { delimiter }, index))
}

fn consume_heredoc_body(
    chars: &[char],
    start: usize,
    heredoc: &PendingHeredoc,
) -> Result<(usize, std::ops::Range<usize>), CommandSyntaxError> {
    let mut line_start = start;
    while line_start <= chars.len() {
        let mut line_end = line_start;
        while chars
            .get(line_end)
            .is_some_and(|character| *character != '\n')
        {
            line_end += 1;
        }
        let line = chars[line_start..line_end].iter().collect::<String>();
        if line == heredoc.delimiter {
            let next = if line_end < chars.len() {
                line_end + 1
            } else {
                line_end
            };
            return Ok((next, start..next));
        }
        if line_end == chars.len() {
            break;
        }
        line_start = line_end + 1;
    }
    Err(CommandSyntaxError {
        code: "command.malformed.heredoc_terminator_missing",
        reason: "heredoc 缺少独占一行且精确匹配的终止符。",
    })
}

pub(super) fn push_token(tokens: &mut Vec<String>, token: &mut String) {
    if !token.is_empty() {
        tokens.push(std::mem::take(token));
    }
}

pub(super) fn extract_parenthesized_command(
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

pub(super) fn extract_backtick_command(
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
pub(super) struct EffectiveProgram {
    pub(super) index: usize,
    /// Every token that the policy parser treated as an executable wrapper or final program.
    /// Keeping this chain is security-significant: provenance checks must not be lost when a
    /// wrapper such as `env`, `busybox`, or `command` is unwrapped to classify its applet.
    pub(super) executable_indices: Vec<usize>,
}

pub(super) fn effective_program(tokens: &[String]) -> Option<EffectiveProgram> {
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

pub(super) fn is_env_assignment(token: &str) -> bool {
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

pub(super) fn program_basename(program: &str) -> String {
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_ascii_lowercase()
}

pub(super) fn is_shell_program(program: &str) -> bool {
    matches!(
        program,
        "sh" | "bash" | "zsh" | "dash" | "ksh" | "fish" | "cmd" | "cmd.exe"
    )
}

pub(super) fn shell_command_argument_index(tokens: &[String]) -> Option<usize> {
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

pub(super) fn shell_script_argument(tokens: &[String]) -> Option<&str> {
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

pub(super) fn shell_option_is_interactive(option: &str) -> bool {
    option == "--interactive"
        || option.starts_with('-')
            && !option.starts_with("--")
            && option.chars().skip(1).any(|character| character == 'i')
}

pub(super) fn shell_option_takes_value(option: &str) -> bool {
    matches!(
        option,
        "-O" | "+O" | "-o" | "+o" | "--rcfile" | "--init-file"
    )
}

pub(super) fn shell_loads_startup_code(tokens: &[String], command_index: usize) -> bool {
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

pub(super) fn shell_command_options_are_closed(
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

pub(super) fn is_opaque_shell_script_path(script: &str) -> bool {
    script.starts_with('~')
        || script.contains(['$', '`', '*', '?', '[', ']', '{', '}'])
        || script == "-"
}

pub(super) fn lower_tokens(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .map(|token| token.to_ascii_lowercase())
        .collect()
}
