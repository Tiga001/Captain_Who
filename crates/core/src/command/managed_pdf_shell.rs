//! Authoritative parser and Host-only compiler for the trusted bundled PDF Skill shell surface.
//!
//! This is intentionally not a general shell policy. It accepts only the small command language
//! needed by the exact `bundled:application:pdf` workflow, records source spans for every file
//! operand, and compiles those operands to approval-frozen private input files immediately before
//! launch. The canonical user/model command remains unchanged in durable state.

use super::*;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::ops::Range;

const MANAGED_PROGRAMS: &[&str] = &[
    "pdfinfo",
    "pdftotext",
    "pdftoppm",
    "python",
    "python3",
    "rg",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedPdfShellPlan {
    command: String,
    operands: Vec<ManagedPdfInputOperand>,
    programs: Vec<ManagedPdfProgram>,
    private_redirection_paths: Vec<String>,
}

pub(crate) struct ManagedPdfShellCompileTools<'a> {
    pub(crate) python: &'a Path,
    pub(crate) pdf_cli: &'a Path,
    pub(crate) ripgrep: &'a Path,
    pub(crate) input_root: &'a Path,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedPdfInputOperand {
    mount_path: String,
    source_range: Range<usize>,
    source: ManagedPdfInputSource,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedPdfProgram {
    name: String,
    source_range: Range<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedPdfInputSource {
    Declared,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedOutputRedirection {
    target: String,
    target_range: Range<usize>,
}

impl ManagedPdfShellPlan {
    pub(crate) fn implicit_workspace_inputs(&self) -> Vec<String> {
        self.operands
            .iter()
            .filter(|operand| operand.source == ManagedPdfInputSource::Workspace)
            .map(|operand| operand.mount_path.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn compile(
        &self,
        prepared_inputs: Option<&PreparedAgentFileInputs>,
        tools: &ManagedPdfShellCompileTools<'_>,
    ) -> Result<String, String> {
        let input_root = tools
            .input_root
            .canonicalize()
            .map_err(|error| format!("无法规范化受管命令输入根：{error}"))?;
        let mut replacements = Vec::with_capacity(self.operands.len() + self.programs.len());
        if !self.operands.is_empty() {
            let prepared = prepared_inputs.ok_or_else(|| {
                "Managed PDF Shell 引用了文件输入，但审批冻结的输入集合为空。".to_string()
            })?;
            let declared = prepared
                .evidence()
                .iter()
                .map(|evidence| evidence.mount_path.as_str())
                .collect::<BTreeSet<_>>();
            for operand in &self.operands {
                if !declared.contains(operand.mount_path.as_str()) {
                    return Err(format!(
                        "Managed PDF Shell 输入中不存在 mountPath `{}`。",
                        operand.mount_path
                    ));
                }
                let path = input_root.join(&operand.mount_path);
                let metadata = fs::symlink_metadata(&path)
                    .map_err(|error| format!("无法读取 Managed PDF Shell 输入文件：{error}"))?;
                if metadata.file_type().is_symlink() || !metadata.is_file() {
                    return Err("Managed PDF Shell 输入必须是非符号链接普通文件。".to_string());
                }
                let canonical = path
                    .canonicalize()
                    .map_err(|error| format!("无法规范化 Managed PDF Shell 输入文件：{error}"))?;
                if !canonical.starts_with(&input_root) {
                    return Err("Managed PDF Shell 输入文件逃离了冻结输入根。".to_string());
                }
                let canonical = canonical.to_str().ok_or_else(|| {
                    "Managed PDF Shell 不支持非 UTF-8 的私有输入路径。".to_string()
                })?;
                replacements.push((operand.source_range.clone(), shell_single_quote(canonical)));
            }
        }
        let input_root = input_root
            .to_str()
            .ok_or_else(|| "Managed PDF Shell 不支持非 UTF-8 的私有输入根。".to_string())?;
        for program in &self.programs {
            let replacement = match program.name.as_str() {
                "pdfinfo" | "pdftotext" | "pdftoppm" => format!(
                    "{} -I -B {} --managed-input-root {} {}",
                    shell_quote_path(tools.python)?,
                    shell_quote_path(tools.pdf_cli)?,
                    shell_single_quote(input_root),
                    program.name
                ),
                "python" | "python3" => {
                    format!("{} -I -B", shell_quote_path(tools.python)?)
                }
                "rg" => format!("{} --no-config", shell_quote_path(tools.ripgrep)?),
                _ => return Err("Managed PDF Shell 编译时遇到未授权程序。".to_string()),
            };
            replacements.push((program.source_range.clone(), replacement));
        }
        compile_source_replacements(&self.command, replacements)
    }

    /// Revalidates every static shell redirection against the private execution root immediately
    /// before launch. Dispatcher processes cannot create symlinks under the Seatbelt profile, and
    /// this check also rejects links left by a crashed or pre-upgrade run before unsandboxed Bash
    /// is allowed to open a redirection target.
    pub(crate) fn validate_private_redirections(
        &self,
        execution_root: &Path,
    ) -> Result<(), String> {
        let execution_root = execution_root
            .canonicalize()
            .map_err(|_| "Managed PDF Shell 无法验证 Run 私有执行根。".to_string())?;
        for relative in &self.private_redirection_paths {
            if relative == "-" {
                continue;
            }
            let normalized = normalize_workspace_relative(relative)?.ok_or_else(|| {
                "Managed PDF Shell 重定向目标不属于 Run 私有执行空间。".to_string()
            })?;
            let mut cursor = execution_root.clone();
            let components = Path::new(&normalized).components().collect::<Vec<_>>();
            for (index, component) in components.iter().enumerate() {
                let std::path::Component::Normal(component) = component else {
                    return Err("Managed PDF Shell 重定向路径包含不安全组件。".to_string());
                };
                cursor.push(component);
                match fs::symlink_metadata(&cursor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => {
                        return Err(format!(
                            "Managed PDF Shell 重定向路径 `{relative}` 包含符号链接，已拒绝执行。"
                        ));
                    }
                    Ok(metadata) if index + 1 < components.len() && !metadata.is_dir() => {
                        return Err(format!(
                            "Managed PDF Shell 重定向路径 `{relative}` 的祖先不是目录。"
                        ));
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                    Err(error) => {
                        return Err(format!(
                            "Managed PDF Shell 无法复验重定向路径 `{relative}`：{error}"
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn parse_managed_pdf_shell(
    command: &str,
) -> Result<Option<ManagedPdfShellPlan>, String> {
    let command = normalize_command_text(command).map_err(format_pdf_syntax_error)?;
    let lexed = lex_command(&command).map_err(format_pdf_syntax_error)?;
    if !lexed.embedded_commands.is_empty() {
        return Err("Managed PDF Shell 不允许命令替换、反引号或动态生成的命令。".to_string());
    }
    let output_redirections = collect_output_redirections(&command, &lexed.ignored_policy_ranges)?;
    let mut private_redirection_paths = output_redirections
        .iter()
        .map(|redirection| redirection.target.clone())
        .collect::<Vec<_>>();
    let output_ranges = output_redirections
        .iter()
        .map(|redirection| redirection.target_range.clone())
        .collect::<Vec<_>>();
    for redirection in &output_redirections {
        validate_private_path(&redirection.target, ManagedPathUse::Write)?;
    }

    let mut operands = Vec::new();
    let mut programs = Vec::new();
    let mut variables = BTreeMap::new();
    let mut saw_managed_program = false;
    for segment in &lexed.segments {
        if segment.tokens.len() != segment.token_ranges.len() {
            return Err("Managed PDF Shell 词法来源范围不完整，已拒绝执行。".to_string());
        }
        let words = segment
            .tokens
            .iter()
            .cloned()
            .zip(segment.token_ranges.iter().cloned())
            .filter(|(_, range)| !output_ranges.iter().any(|output| output == range))
            .collect::<Vec<_>>();
        for (word, range) in &words {
            validate_shell_word_surface(word, range, &command)?;
        }
        let (program_index, command_variables) =
            validate_assignments_and_find_program(&words, &mut variables)?;
        let Some(program_index) = program_index else {
            if segment.has_heredoc
                || segment.has_write_redirection
                || !segment.input_redirections.is_empty()
            {
                return Err("Managed PDF Shell 的变量赋值不能携带重定向或 heredoc。".to_string());
            }
            continue;
        };
        let program = words[program_index].0.as_str();
        if !MANAGED_PROGRAMS.contains(&program) {
            if saw_managed_program || script_contains_managed_intent(&lexed) {
                return Err(unsupported_managed_program_error(program));
            }
            return Ok(None);
        }
        saw_managed_program = true;
        validate_program_token(program, &words[program_index].1, &command)?;
        programs.push(ManagedPdfProgram {
            name: program.to_string(),
            source_range: words[program_index].1.clone(),
        });
        let args = &words[program_index + 1..];
        match program {
            "pdfinfo" => parse_pdfinfo(args, segment, &command_variables, &mut operands)?,
            "pdftotext" => parse_pdftotext(args, segment, &command_variables, &mut operands)?,
            "pdftoppm" => parse_pdftoppm(args, segment, &command_variables, &mut operands)?,
            "python" | "python3" => parse_python(args, segment, &command_variables, &mut operands)?,
            "rg" => parse_rg(args, segment, &command_variables, &mut operands)?,
            _ => unreachable!("program was checked against MANAGED_PROGRAMS"),
        }
        for redirection in &segment.input_redirections {
            if redirection.dynamic {
                return Err("Managed PDF Shell 不允许动态输入重定向目标。".to_string());
            }
            if let Some(input) = parse_input_operand(
                &redirection.target,
                redirection.target_range.clone(),
                true,
                &command_variables,
            )? {
                if redirection.read_write {
                    return Err(
                        "Managed PDF Shell 不允许通过 `<>` 修改冻结的只读输入。".to_string()
                    );
                }
                operands.push(input);
            } else {
                validate_private_path(
                    &redirection.target,
                    if redirection.read_write {
                        ManagedPathUse::Write
                    } else {
                        ManagedPathUse::Read
                    },
                )?;
                private_redirection_paths.push(redirection.target.clone());
            }
        }
    }
    if !saw_managed_program {
        return Ok(None);
    }
    deduplicate_operands(&mut operands)?;
    private_redirection_paths.sort();
    private_redirection_paths.dedup();
    Ok(Some(ManagedPdfShellPlan {
        command,
        operands,
        programs,
        private_redirection_paths,
    }))
}

fn unsupported_managed_program_error(program: &str) -> String {
    const COMMON_FILTERS: &[&str] = &[
        "head", "tail", "grep", "egrep", "fgrep", "sed", "awk", "cut", "sort", "uniq", "wc",
        "less", "more",
    ];
    let executable_set = "`pdfinfo`, `pdftotext`, `pdftoppm`, `python`, `python3`, `rg`";
    if COMMON_FILTERS.contains(&program) {
        return format!(
            "Managed PDF Shell 不提供常见过滤器 `{program}`；完整可执行集合只有 {executable_set}。读取特定 PDF 页请使用 `pdftotext -f FIRST -l LAST`，限制搜索命中请使用 `rg --max-count N PATTERN`，只取前 N 行请使用 `rg --max-count N '^'`。如果已有超限结果，请通过其 `historyOpen` 调用 `conversation_history`，不要重新执行全文提取。"
        );
    }
    format!(
        "Managed PDF Shell 不允许执行 `{program}`；完整可执行集合只有 {executable_set}。复杂但有界的处理请使用受管 Python quoted heredoc。"
    )
}

fn script_contains_managed_intent(lexed: &LexedCommand) -> bool {
    lexed.segments.iter().any(|segment| {
        segment
            .tokens
            .iter()
            .find(|token| !is_env_assignment(token))
            .is_some_and(|token| MANAGED_PROGRAMS.contains(&token.as_str()))
    })
}

fn validate_assignments_and_find_program(
    words: &[(String, Range<usize>)],
    persistent: &mut BTreeMap<String, String>,
) -> Result<(Option<usize>, BTreeMap<String, String>), String> {
    let mut command_variables = persistent.clone();
    let mut index = 0;
    while words
        .get(index)
        .is_some_and(|(word, _)| is_env_assignment(word))
    {
        let (name, value) = validate_simple_assignment(&words[index].0)?;
        command_variables.insert(name, value);
        index += 1;
    }
    if index == words.len() {
        *persistent = command_variables.clone();
        return Ok((None, command_variables));
    }
    Ok((Some(index), command_variables))
}

fn validate_simple_assignment(assignment: &str) -> Result<(String, String), String> {
    let (name, value) = assignment
        .split_once('=')
        .ok_or_else(|| "Managed PDF Shell 变量赋值格式无效。".to_string())?;
    let upper = name.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "PATH"
            | "HOME"
            | "TMP"
            | "TMPDIR"
            | "TEMP"
            | "USERPROFILE"
            | "IFS"
            | "CDPATH"
            | "ENV"
            | "BASH_ENV"
            | "SHELLOPTS"
            | "BASHOPTS"
    ) || [
        "MYCOPILOT_",
        "PYTHON",
        "PIP_",
        "RIPGREP_",
        "LD_",
        "DYLD_",
        "BASH_",
        "ARTIFACT_",
    ]
    .iter()
    .any(|prefix| upper.starts_with(prefix))
    {
        return Err(format!(
            "Managed PDF Shell 不允许覆盖 Host 保留变量 `{name}`。"
        ));
    }
    if value.len() > 4_096
        || value.chars().any(char::is_control)
        || value.contains(['$', '`'])
        || value.chars().any(|character| {
            matches!(
                character,
                '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '~' | '!'
            )
        })
    {
        return Err(format!(
            "Managed PDF Shell 变量 `{name}` 必须是静态、可打印且不超过 4096 字节的值。"
        ));
    }
    Ok((name.to_string(), value.to_string()))
}

fn validate_shell_word_surface(
    word: &str,
    range: &Range<usize>,
    command: &str,
) -> Result<(), String> {
    let raw = command
        .chars()
        .skip(range.start)
        .take(range.end.saturating_sub(range.start))
        .collect::<String>();
    let mut quote = None;
    let mut escaped = false;
    for character in raw.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if character == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if let Some(active) = quote {
            if character == active {
                quote = None;
            }
            continue;
        }
        if matches!(character, '\'' | '"') {
            quote = Some(character);
            continue;
        }
        if matches!(
            character,
            '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '~' | '!'
        ) {
            return Err(format!(
                "Managed PDF Shell 参数 `{word}` 包含未引用的 Shell 展开语法。"
            ));
        }
    }
    Ok(())
}

fn validate_program_token(
    program: &str,
    range: &Range<usize>,
    command: &str,
) -> Result<(), String> {
    let raw = command
        .chars()
        .skip(range.start)
        .take(range.end - range.start)
        .collect::<String>();
    if raw.contains('/') || raw.contains('\\') || program != raw.trim_matches(['\'', '"']) {
        return Err("Managed PDF Shell 的程序名必须是 allowlist 中的简单命令名。".to_string());
    }
    Ok(())
}

fn parse_pdfinfo(
    args: &[(String, Range<usize>)],
    segment: &ShellSegment,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    require_no_heredoc("pdfinfo", segment)?;
    let positional = parse_positionals(args, &["-f", "-l"], &[], variables)?;
    if positional.len() != 1 {
        return Err("Managed PDF Shell 的 pdfinfo 需要且只能指定一个 PDF 输入。".to_string());
    }
    push_required_pdf_input(positional[0], variables, operands)
}

fn parse_pdftotext(
    args: &[(String, Range<usize>)],
    segment: &ShellSegment,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    require_no_heredoc("pdftotext", segment)?;
    let positional = parse_positionals(args, &["-f", "-l"], &["-layout", "-nopgbrk"], variables)?;
    if !matches!(positional.len(), 1 | 2) {
        return Err(
            "Managed PDF Shell 的 pdftotext 需要 PDF 输入和可选的私有输出路径。".to_string(),
        );
    }
    push_required_pdf_input(positional[0], variables, operands)?;
    if let Some((output, _)) = positional.get(1) {
        let output = resolve_user_variable(output, variables)?;
        if output != "-" {
            validate_private_path(output, ManagedPathUse::WriteIntermediate)?;
        }
    }
    Ok(())
}

fn parse_pdftoppm(
    args: &[(String, Range<usize>)],
    segment: &ShellSegment,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    require_no_heredoc("pdftoppm", segment)?;
    let positional = parse_positionals(
        args,
        &["-f", "-l", "-r"],
        &["-png", "-jpeg", "-jpg", "-singlefile"],
        variables,
    )?;
    if positional.len() != 2 {
        return Err(
            "Managed PDF Shell 的 pdftoppm 需要 PDF 输入和 outputs/ 下的输出前缀。".to_string(),
        );
    }
    push_required_pdf_input(positional[0], variables, operands)?;
    validate_publishable_output_prefix(resolve_user_variable(&positional[1].0, variables)?)
}

fn parse_python(
    args: &[(String, Range<usize>)],
    segment: &ShellSegment,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    let Some((mode, mode_range)) = args.first() else {
        return Err("Managed PDF Shell 的 python 调用缺少 `-`、`-c` 或冻结脚本。".to_string());
    };
    let argument_start = match mode.as_str() {
        "-" => {
            if !segment.has_heredoc {
                return Err(
                    "Managed PDF Shell 的 `python -` 必须使用安全引用的 heredoc。".to_string(),
                );
            }
            1
        }
        "-c" => {
            if segment.has_heredoc || args.get(1).is_none_or(|(code, _)| code.is_empty()) {
                return Err(
                    "Managed PDF Shell 的 `python -c` 必须提供静态代码且不能同时使用 heredoc。"
                        .to_string(),
                );
            }
            2
        }
        _ => {
            if segment.has_heredoc {
                return Err("保存的 Python 脚本不能同时使用 heredoc。".to_string());
            }
            let Some(input) = parse_input_operand(mode, mode_range.clone(), false, variables)?
            else {
                return Err(
                    "Managed PDF Shell 的保存脚本必须来自 `$MYCOPILOT_INPUT_ROOT/<mount>.py`。"
                        .to_string(),
                );
            };
            if Path::new(&input.mount_path)
                .extension()
                .and_then(OsStr::to_str)
                != Some("py")
            {
                return Err("Managed PDF Shell 的输入脚本必须是 .py 文件。".to_string());
            }
            operands.push(input);
            1
        }
    };
    for (argument, range) in &args[argument_start..] {
        validate_static_argument(argument, variables)?;
        if let Some(input) = parse_input_operand(argument, range.clone(), false, variables)? {
            operands.push(input);
        }
    }
    Ok(())
}

fn parse_rg(
    args: &[(String, Range<usize>)],
    segment: &ShellSegment,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    require_no_heredoc("rg", segment)?;
    if args.is_empty() {
        return Err("Managed PDF Shell 的 rg 调用缺少搜索参数。".to_string());
    }
    let mut index = 0;
    let mut options_open = true;
    let mut has_pattern = false;
    let mut uses_explicit_patterns = false;
    while index < args.len() {
        let (argument, range) = &args[index];
        let resolved = resolve_user_variable(argument, variables)?;
        if options_open && resolved == "--" {
            reject_indirect_rg_option(argument)?;
            options_open = false;
            index += 1;
            continue;
        }
        if options_open && resolved.starts_with('-') && resolved != "-" {
            reject_indirect_rg_option(argument)?;
            match classify_rg_option(resolved)? {
                RgOption::Switch => {
                    index += 1;
                }
                RgOption::NumberInline { option, value } => {
                    validate_rg_number(option, value)?;
                    index += 1;
                }
                RgOption::NumberNext { option } => {
                    let Some((value, _)) = args.get(index + 1) else {
                        return Err(format!("Managed PDF Shell 的 rg 选项 `{option}` 缺少值。"));
                    };
                    if value.contains('$') {
                        return Err(format!(
                            "Managed PDF Shell 的 rg 选项 `{option}` 必须直接给出静态整数值。"
                        ));
                    }
                    validate_rg_number(option, value)?;
                    index += 2;
                }
                RgOption::PatternInline(pattern) => {
                    if has_pattern && !uses_explicit_patterns {
                        return Err(
                            "Managed PDF Shell 的 rg 不能混用位置正则与 `-e/--regexp`；请把每个正则都写成 `-e PATTERN`。"
                                .to_string(),
                        );
                    }
                    validate_rg_pattern(pattern, variables)?;
                    has_pattern = true;
                    uses_explicit_patterns = true;
                    index += 1;
                }
                RgOption::PatternNext { option } => {
                    if has_pattern && !uses_explicit_patterns {
                        return Err(
                            "Managed PDF Shell 的 rg 不能混用位置正则与 `-e/--regexp`；请把每个正则都写成 `-e PATTERN`。"
                                .to_string(),
                        );
                    }
                    let Some((pattern, _)) = args.get(index + 1) else {
                        return Err(format!(
                            "Managed PDF Shell 的 rg 选项 `{option}` 缺少正则。"
                        ));
                    };
                    validate_rg_pattern(pattern, variables)?;
                    has_pattern = true;
                    uses_explicit_patterns = true;
                    index += 2;
                }
            }
            continue;
        }
        if !has_pattern {
            validate_rg_pattern(argument, variables)?;
            has_pattern = true;
        } else {
            push_rg_path(argument, range.clone(), variables, operands)?;
        }
        index += 1;
    }
    if !has_pattern {
        return Err("Managed PDF Shell 的 rg 调用缺少搜索正则。".to_string());
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum RgOption<'a> {
    Switch,
    NumberInline { option: &'a str, value: &'a str },
    NumberNext { option: &'a str },
    PatternInline(&'a str),
    PatternNext { option: &'a str },
}

fn classify_rg_option(argument: &str) -> Result<RgOption<'_>, String> {
    const SWITCHES: &[&str] = &[
        "-n",
        "--line-number",
        "-i",
        "--ignore-case",
        "-s",
        "--case-sensitive",
        "-S",
        "--smart-case",
        "-w",
        "--word-regexp",
        "-F",
        "--fixed-strings",
        "-H",
        "--with-filename",
        "-I",
        "--no-filename",
        "--heading",
        "--no-heading",
    ];
    if SWITCHES.contains(&argument)
        || argument.strip_prefix('-').is_some_and(|cluster| {
            cluster.len() > 1
                && cluster
                    .chars()
                    .all(|flag| matches!(flag, 'n' | 'i' | 's' | 'S' | 'w' | 'F' | 'H' | 'I'))
        })
    {
        return Ok(RgOption::Switch);
    }
    for (short, long) in [
        ("-A", "--after-context"),
        ("-B", "--before-context"),
        ("-C", "--context"),
        ("-m", "--max-count"),
    ] {
        if argument == short || argument == long {
            return Ok(RgOption::NumberNext { option: argument });
        }
        if let Some(value) = argument
            .strip_prefix(short)
            .filter(|value| !value.is_empty())
        {
            return Ok(RgOption::NumberInline {
                option: short,
                value,
            });
        }
        if let Some(value) = argument
            .strip_prefix(long)
            .and_then(|value| value.strip_prefix('='))
        {
            return Ok(RgOption::NumberInline {
                option: long,
                value,
            });
        }
    }
    if argument == "-e" || argument == "--regexp" {
        return Ok(RgOption::PatternNext { option: argument });
    }
    if let Some(pattern) = argument
        .strip_prefix("-e")
        .filter(|value| !value.is_empty())
    {
        return Ok(RgOption::PatternInline(pattern));
    }
    if let Some(pattern) = argument.strip_prefix("--regexp=") {
        return Ok(RgOption::PatternInline(pattern));
    }
    if argument == "-f"
        || argument.starts_with("-f")
        || argument == "--file"
        || argument.starts_with("--file=")
    {
        return Err(
            "Managed PDF Shell 不支持 rg 的 pattern-file 选项；请使用 `-e PATTERN` 直接提供有界正则。"
                .to_string(),
        );
    }
    Err(format!(
        "Managed PDF Shell 不支持 rg 选项 `{argument}`；请使用行号、大小写、上下文、max-count 或 `-e/--regexp` 等受管搜索选项。"
    ))
}

fn reject_indirect_rg_option(argument: &str) -> Result<(), String> {
    if argument.contains('$') {
        return Err(
            "Managed PDF Shell 不允许通过变量间接提供 rg 选项；请直接写出受支持的选项。"
                .to_string(),
        );
    }
    Ok(())
}

fn validate_rg_number(option: &str, value: &str) -> Result<(), String> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| format!("Managed PDF Shell 的 rg 选项 `{option}` 必须使用非负整数值。"))?;
    let maximum = if matches!(option, "-m" | "--max-count") {
        10_000
    } else {
        1_000
    };
    if parsed > maximum {
        return Err(format!(
            "Managed PDF Shell 的 rg 选项 `{option}` 不能超过 {maximum}。"
        ));
    }
    Ok(())
}

fn validate_rg_pattern(pattern: &str, variables: &BTreeMap<String, String>) -> Result<(), String> {
    if pdf_input_mount(pattern).is_some() {
        return Err("Managed PDF Shell 的 rg 搜索正则不能引用文件输入路径。".to_string());
    }
    // Shell surface validation already rejected unquoted expansion syntax. Resolve only a static
    // user variable here; a regex is data, not a filesystem path, so dots, slashes and glob-like
    // regex metacharacters must never enter path validation.
    let _ = resolve_user_variable(pattern, variables)?;
    Ok(())
}

fn push_rg_path(
    argument: &str,
    range: Range<usize>,
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    validate_static_argument(argument, variables)?;
    if let Some(input) = parse_input_operand(argument, range, false, variables)? {
        operands.push(input);
        return Ok(());
    }
    validate_private_path(
        resolve_user_variable(argument, variables)?,
        ManagedPathUse::Read,
    )
}

fn parse_positionals<'a>(
    args: &'a [(String, Range<usize>)],
    valued_options: &[&str],
    switches: &[&str],
    variables: &BTreeMap<String, String>,
) -> Result<Vec<&'a (String, Range<usize>)>, String> {
    let mut positional = Vec::new();
    let mut index = 0;
    while index < args.len() {
        let argument = args[index].0.as_str();
        let resolved = if pdf_input_mount(argument).is_some() {
            argument
        } else {
            resolve_user_variable(argument, variables)?
        };
        if simple_variable_name(argument).is_some() && resolved.starts_with('-') {
            return Err(
                "Managed PDF Shell 不允许通过变量间接提供命令选项；请直接写出受支持的选项。"
                    .to_string(),
            );
        }
        if valued_options.contains(&argument) {
            let Some(value) = args.get(index + 1) else {
                return Err(format!("Managed PDF Shell 选项 `{argument}` 缺少值。"));
            };
            validate_static_argument(&value.0, variables)?;
            index += 2;
        } else if switches.contains(&argument) {
            index += 1;
        } else if argument.starts_with('-') && argument != "-" {
            return Err(format!("Managed PDF Shell 不支持选项 `{argument}`。"));
        } else {
            validate_static_argument(argument, variables)?;
            positional.push(&args[index]);
            index += 1;
        }
    }
    Ok(positional)
}

fn require_no_heredoc(program: &str, segment: &ShellSegment) -> Result<(), String> {
    if segment.has_heredoc {
        return Err(format!(
            "Managed PDF Shell 的 `{program}` 不接受 heredoc；heredoc 仅用于 `python -`。"
        ));
    }
    Ok(())
}

fn push_required_pdf_input(
    word: &(String, Range<usize>),
    variables: &BTreeMap<String, String>,
    operands: &mut Vec<ManagedPdfInputOperand>,
) -> Result<(), String> {
    if pdf_input_mount(&word.0).is_none() {
        let resolved = resolve_user_variable(&word.0, variables)?;
        if resolved.starts_with("outputs/") && has_pdf_extension(resolved) {
            return validate_private_path(resolved, ManagedPathUse::Read);
        }
    }
    let Some(input) = parse_input_operand(&word.0, word.1.clone(), true, variables)? else {
        return Err(
            "Managed PDF Shell 的 PDF 输入必须是 workspace 相对 .pdf、outputs/ 下已有 PDF，或 `$MYCOPILOT_INPUT_ROOT/<mount>.pdf`。"
                .to_string(),
        );
    };
    if input.source == ManagedPdfInputSource::Declared || !input.mount_path.starts_with("outputs/")
    {
        operands.push(input);
    }
    Ok(())
}

fn parse_input_operand(
    value: &str,
    source_range: Range<usize>,
    require_pdf: bool,
    variables: &BTreeMap<String, String>,
) -> Result<Option<ManagedPdfInputOperand>, String> {
    if let Some(relative) = pdf_input_mount(value) {
        let relative = relative?;
        if require_pdf && !has_pdf_extension(&relative) {
            return Err("Managed PDF Shell 的输入 mount 必须是 .pdf 文件。".to_string());
        }
        return Ok(Some(ManagedPdfInputOperand {
            mount_path: relative,
            source_range,
            source: ManagedPdfInputSource::Declared,
        }));
    }
    let value = resolve_user_variable(value, variables)?;
    if !has_pdf_extension(value) {
        return Ok(None);
    }
    let Some(relative) = normalize_workspace_relative(value)? else {
        return Ok(None);
    };
    if relative.starts_with("outputs/") {
        validate_private_path(&relative, ManagedPathUse::Read)?;
        return Ok(None);
    }
    Ok(Some(ManagedPdfInputOperand {
        mount_path: relative,
        source_range,
        source: ManagedPdfInputSource::Workspace,
    }))
}

fn pdf_input_mount(value: &str) -> Option<Result<String, String>> {
    let relative = value
        .strip_prefix("$MYCOPILOT_INPUT_ROOT/")
        .or_else(|| value.strip_prefix("${MYCOPILOT_INPUT_ROOT}/"))?;
    Some(normalize_mount_path(relative))
}

fn normalize_mount_path(value: &str) -> Result<String, String> {
    let path = Path::new(value);
    if value.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err("Managed PDF Shell 的 input root 路径必须是不含 `..` 的相对路径。".to_string());
    }
    Ok(value.replace('\\', "/"))
}

fn normalize_workspace_relative(value: &str) -> Result<Option<String>, String> {
    if value.is_empty()
        || value.starts_with('@')
        || value.starts_with('~')
        || value.contains('\\')
        || value.contains('$')
        || value.contains("://")
    {
        return Ok(None);
    }
    let path = Path::new(value);
    if path.is_absolute() {
        return Ok(None);
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(
                part.to_str()
                    .ok_or_else(|| {
                        "Managed PDF Shell 的 workspace 路径必须是有效 UTF-8。".to_string()
                    })?
                    .to_string(),
            ),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(
                    "Managed PDF Shell 的 workspace 路径不能包含 `..`、根目录或平台前缀。"
                        .to_string(),
                );
            }
        }
    }
    Ok((!parts.is_empty()).then(|| parts.join("/")))
}

fn validate_static_argument(
    argument: &str,
    variables: &BTreeMap<String, String>,
) -> Result<(), String> {
    if argument.contains('`') || argument.contains("$()") || argument.contains("``") {
        return Err("Managed PDF Shell 不允许命令替换或反引号。".to_string());
    }
    let resolved = if argument.contains('$') && pdf_input_mount(argument).is_none() {
        resolve_user_variable(argument, variables)?
    } else {
        argument
    };
    if is_obvious_external_path(resolved) {
        return Err(format!(
            "Managed PDF Shell 不允许 workspace/私有运行空间之外的路径 `{resolved}`。"
        ));
    }
    Ok(())
}

fn resolve_user_variable<'a>(
    value: &'a str,
    variables: &'a BTreeMap<String, String>,
) -> Result<&'a str, String> {
    let Some(name) = simple_variable_name(value) else {
        if value.contains('$') {
            return Err(format!(
                "Managed PDF Shell 只允许完整参数形式的简单变量引用；无法核验 `{value}`。"
            ));
        }
        return Ok(value);
    };
    variables
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("Managed PDF Shell 变量 `{name}` 未在当前脚本中静态声明。"))
}

fn simple_variable_name(value: &str) -> Option<&str> {
    value
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
        .or_else(|| value.strip_prefix('$'))
        .filter(|name| {
            !name.is_empty()
                && name
                    .chars()
                    .all(|character| character == '_' || character.is_ascii_alphanumeric())
                && name
                    .chars()
                    .next()
                    .is_some_and(|character| character == '_' || character.is_ascii_alphabetic())
        })
}

#[derive(Clone, Copy)]
enum ManagedPathUse {
    Read,
    Write,
    WriteIntermediate,
}

fn validate_private_path(value: &str, usage: ManagedPathUse) -> Result<(), String> {
    if value == "-"
        && matches!(
            usage,
            ManagedPathUse::Read | ManagedPathUse::WriteIntermediate
        )
    {
        return Ok(());
    }
    if value.contains('$')
        || value.contains('`')
        || value.chars().any(|character| {
            matches!(
                character,
                '*' | '?' | '[' | ']' | '{' | '}' | '(' | ')' | '~' | '!'
            )
        })
    {
        return Err("Managed PDF Shell 的重定向和文件路径必须是静态字面值。".to_string());
    }
    let Some(relative) = normalize_workspace_relative(value)? else {
        return Err(format!(
            "Managed PDF Shell 文件路径 `{value}` 必须位于 Run 私有执行空间。"
        ));
    };
    if [".home", ".tmp", ".runtime"]
        .iter()
        .any(|reserved| relative == *reserved || relative.starts_with(&format!("{reserved}/")))
    {
        return Err("Managed PDF Shell 不允许访问 Host 保留的私有目录。".to_string());
    }
    if matches!(usage, ManagedPathUse::Write) && relative.starts_with("$MYCOPILOT_INPUT_ROOT/") {
        return Err("Managed PDF Shell 不允许写入只读输入根。".to_string());
    }
    Ok(())
}

fn validate_publishable_output_prefix(value: &str) -> Result<(), String> {
    validate_private_path(value, ManagedPathUse::Write)?;
    if !value.starts_with("outputs/") {
        return Err("PDF 页面渲染输出前缀必须位于 `outputs/`。".to_string());
    }
    Ok(())
}

fn has_pdf_extension(value: &str) -> bool {
    Path::new(value)
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
}

fn deduplicate_operands(operands: &mut Vec<ManagedPdfInputOperand>) -> Result<(), String> {
    let mut spans = BTreeMap::<(usize, usize), (String, ManagedPdfInputSource)>::new();
    let mut deduplicated = Vec::with_capacity(operands.len());
    for operand in std::mem::take(operands) {
        let key = (operand.source_range.start, operand.source_range.end);
        match spans.get(&key) {
            Some(existing) if existing == &(operand.mount_path.clone(), operand.source) => {}
            Some(_) => {
                return Err("Managed PDF Shell 对同一来源范围产生了冲突的输入解释。".to_string());
            }
            None => {
                spans.insert(key, (operand.mount_path.clone(), operand.source));
                deduplicated.push(operand);
            }
        }
    }
    *operands = deduplicated;
    Ok(())
}

fn collect_output_redirections(
    command: &str,
    ignored_ranges: &[Range<usize>],
) -> Result<Vec<ManagedOutputRedirection>, String> {
    let chars = command.chars().collect::<Vec<_>>();
    let mut results = Vec::new();
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
        if let Some(active) = quote {
            if character == active {
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
        let output = character == '>' || character == '&' && chars.get(index + 1) == Some(&'>');
        if !output {
            index += 1;
            continue;
        }
        if character == '&' {
            index += 2;
        } else {
            index += 1;
            if chars.get(index) == Some(&'>') {
                index += 1;
            }
        }
        if chars.get(index) == Some(&'&') {
            if let Some(next) = consume_fd_duplication(&chars, index + 1) {
                index = next;
                continue;
            }
            index += 1;
        }
        while chars
            .get(index)
            .is_some_and(|character| character.is_whitespace())
        {
            index += 1;
        }
        let start = index;
        let (target, dynamic, next) =
            read_redirection_target(&chars, start).map_err(format_pdf_syntax_error)?;
        if dynamic {
            return Err("Managed PDF Shell 不允许动态输出重定向目标。".to_string());
        }
        results.push(ManagedOutputRedirection {
            target,
            target_range: start..next,
        });
        index = next;
    }
    Ok(results)
}

fn compile_source_replacements(
    command: &str,
    mut replacements: Vec<(Range<usize>, String)>,
) -> Result<String, String> {
    replacements.sort_by_key(|replacement| std::cmp::Reverse(replacement.0.start));
    let mut chars = command.chars().collect::<Vec<_>>();
    let mut previous_start = chars.len();
    for (range, replacement) in replacements {
        if range.start > range.end || range.end > chars.len() || range.end > previous_start {
            return Err("Managed PDF Shell 输入来源范围重叠或越界，已拒绝编译。".to_string());
        }
        chars.splice(range.clone(), replacement.chars());
        previous_start = range.start;
    }
    Ok(chars.into_iter().collect())
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn shell_quote_path(path: &Path) -> Result<String, String> {
    path.to_str()
        .map(shell_single_quote)
        .ok_or_else(|| "Managed PDF Shell 不支持非 UTF-8 的受管工具路径。".to_string())
}

fn format_pdf_syntax_error(error: CommandSyntaxError) -> String {
    format!(
        "Managed PDF Shell 命令语法无效（{}）：{}",
        error.code, error.reason
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    #[test]
    fn accepts_multistep_pdf_shell_and_collects_all_workspace_inputs() {
        let command = "PAGE=307\npdfinfo 'manual one.pdf'; pdftotext -f $PAGE -l \"$PAGE\" second.pdf - | rg --max-count 3 'unit op'";
        let plan = parse_managed_pdf_shell(command).unwrap().unwrap();
        assert_eq!(
            plan.implicit_workspace_inputs(),
            vec!["manual one.pdf".to_string(), "second.pdf".to_string()]
        );
    }

    #[test]
    fn word_qa_read_path_is_one_direct_input_for_info_and_page_render() {
        let input = "word-work/静夜思-visual-qa.pdf";
        for command in [
            format!("pdfinfo '{input}'"),
            format!("pdftoppm -f 1 -l 1 -png '{input}' outputs/page"),
        ] {
            let plan = parse_managed_pdf_shell(&command).unwrap().unwrap();
            assert_eq!(plan.implicit_workspace_inputs(), vec![input.to_string()]);
        }
    }

    #[test]
    fn rg_regex_roles_do_not_misclassify_patterns_as_paths() {
        for command in [
            r#"pdftotext -layout "$MYCOPILOT_INPUT_ROOT/AspenPolymer-Unit Operations and Reaction Models.pdf" - | rg -n -i -C 4 --max-count 30 "defining polymer|polymer component|define.*polymer""#,
            r#"python -c 'print(1)' | rg -e 'foo/bar\\.pdf' -e 'other.*pattern'"#,
            r#"python -c 'print(1)' | rg --regexp='(polymer|oligomer)[- /].*'"#,
            r#"python -c 'print(1)' | rg -- '-leading-pattern'"#,
            r#"python -c 'print(1)' | rg -ni -C4 --max-count=30 'define.*polymer'"#,
            r#"rg -e 'needle.*value' extracted.txt"#,
            r#"rg needle -- file-without-extension"#,
        ] {
            assert!(
                parse_managed_pdf_shell(command).unwrap().is_some(),
                "{command}"
            );
        }
    }

    #[test]
    fn rg_compilation_preserves_regex_bytes() {
        let input_root = TempDir::new().unwrap();
        let command = r#"python -c 'print(1)' | rg -n -e 'define.*polymer|foo/bar\\.pdf'"#;
        let plan = parse_managed_pdf_shell(command).unwrap().unwrap();
        let compiled = plan
            .compile(
                None,
                &ManagedPdfShellCompileTools {
                    python: Path::new("/managed/python"),
                    pdf_cli: Path::new("/managed/pdf-runtime-cli.py"),
                    ripgrep: Path::new("/managed/rg"),
                    input_root: input_root.path(),
                },
            )
            .unwrap();
        assert!(compiled.contains("'define.*polymer|foo/bar\\\\.pdf'"));
    }

    #[test]
    fn rg_role_parser_rejects_ambiguous_options_and_unsafe_paths() {
        for command in [
            "rg -n",
            "rg --max-count 30",
            "rg -e",
            "rg -f patterns.txt extracted.txt",
            "rg --file=patterns.txt extracted.txt",
            "rg --pre helper needle extracted.txt",
            "rg --search-zip needle extracted.txt",
            "rg -niC4 needle extracted.txt",
            "rg -C -1 needle extracted.txt",
            "rg --context=1001 needle extracted.txt",
            "rg possible-path -e needle extracted.txt",
            "rg /etc/passwd -e needle",
            "rg ../outside.txt -e needle",
            "rg manual.pdf -e needle",
            "rg needle /etc/passwd",
            "rg needle ../outside.txt",
            "OPTION=-n; rg \"$OPTION\" needle extracted.txt",
        ] {
            assert!(parse_managed_pdf_shell(command).is_err(), "{command}");
        }
    }

    #[test]
    fn accepts_quoted_python_heredoc_and_private_redirection() {
        let command = "python - <<'PY' > extracted.txt\nfrom pathlib import Path\nprint(Path('x'))\nPY\nrg x extracted.txt";
        assert!(parse_managed_pdf_shell(command).unwrap().is_some());
    }

    #[test]
    fn common_filter_rejection_returns_bounded_recovery_commands() {
        for program in ["head", "tail", "grep", "sed", "awk"] {
            let command = format!("pdftotext manual.pdf - | {program} -n 20");
            let error = parse_managed_pdf_shell(&command).unwrap_err();
            for expected in [
                &format!("`{program}`"),
                "`pdftotext -f FIRST -l LAST`",
                "`rg --max-count N PATTERN`",
                "`rg --max-count N '^'`",
                "`historyOpen`",
                "`conversation_history`",
            ] {
                assert!(
                    error.contains(expected),
                    "missing `{expected}` in `{error}`"
                );
            }
        }
    }

    #[test]
    fn rejects_mixed_unmanaged_commands_even_with_full_shell_shape() {
        for command in [
            "pdfinfo manual.pdf; curl https://example.com",
            "curl https://example.com | pdftotext manual.pdf -",
            "python -c 'print(1)' && pip install pypdf",
            "pdfinfo manual.pdf; source environment.sh",
            "pdfinfo manual.pdf; eval 'rg marker extracted.txt'",
            "pdfinfo manual.pdf; exec rg marker extracted.txt",
            "pdfinfo manual.pdf; read secret",
            "pdfinfo manual.pdf; printf '%s' secret",
            "pdfinfo manual.pdf; for file in *.pdf; do rg marker \"$file\"; done",
            "pdfinfo manual.pdf; while true; do rg marker extracted.txt; done",
        ] {
            assert!(parse_managed_pdf_shell(command).is_err(), "{command}");
        }
    }

    #[test]
    fn rejects_substitution_background_and_path_escape() {
        for command in [
            "pdfinfo $(printf manual.pdf)",
            "pdfinfo `printf manual.pdf`",
            "pdfinfo manual.pdf &",
            "pdftotext manual.pdf ../outside.txt",
            "pdftoppm manual.pdf elsewhere/page",
            "python -c 'print(1)' > /tmp/leak",
            "pdfinfo *.pdf",
            "pdfinfo manual.pdf > outputs/*.txt",
            "PDF='*.pdf'; pdfinfo \"$PDF\"",
            "pdfinfo manual.pdf > \"$OUTPUT\"",
        ] {
            assert!(parse_managed_pdf_shell(command).is_err(), "{command}");
        }
    }

    #[test]
    fn rejects_host_environment_overrides() {
        for name in [
            "PATH",
            "HOME",
            "TMPDIR",
            "MYCOPILOT_INPUT_ROOT",
            "PYTHONPATH",
            "PIP_INDEX_URL",
            "RIPGREP_CONFIG_PATH",
            "DYLD_INSERT_LIBRARIES",
            "BASH_ENV",
        ] {
            let command = format!("{name}=evil pdfinfo manual.pdf");
            assert!(parse_managed_pdf_shell(&command).is_err(), "{name}");
        }
    }

    #[test]
    fn outputs_pdf_is_private_not_an_implicit_workspace_input() {
        let plan = parse_managed_pdf_shell("pdfinfo outputs/draft.pdf")
            .unwrap()
            .unwrap();
        assert!(plan.implicit_workspace_inputs().is_empty());
        assert!(
            parse_managed_pdf_shell("pdfinfo \"$MYCOPILOT_INPUT_ROOT/manual.pdf\"")
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn static_pdf_variable_freezes_unicode_path_and_compiles_only_operand_span() {
        let workspace = TempDir::new().unwrap();
        let filename = "资料 手册.pdf";
        let bytes = b"%PDF-1.4\nfixture\n";
        fs::write(workspace.path().join(filename), bytes).unwrap();
        let plan = parse_managed_pdf_shell(&format!(
            "PDF='{filename}'\npdfinfo \"$PDF\"; rg 'PDF' extracted.txt"
        ))
        .unwrap()
        .unwrap();
        assert_eq!(plan.implicit_workspace_inputs(), vec![filename.to_string()]);
        let prepared = materialize_agent_file_inputs(
            Some(workspace.path()),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            &AgentFileInputExecutionContext::default(),
            &[crate::AgentFileInputBinding {
                schema_version: crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
                mount_path: filename.to_string(),
                source: crate::AgentFileInputRef::Workspace {
                    path: filename.to_string(),
                },
                size_bytes: bytes.len() as u64,
                sha256: format!("{:x}", Sha256::digest(bytes)),
            }],
            None,
        )
        .unwrap()
        .unwrap();
        let compiled = plan
            .compile(
                Some(&prepared),
                &ManagedPdfShellCompileTools {
                    python: Path::new("/managed/python"),
                    pdf_cli: Path::new("/managed/pdf-runtime-cli.py"),
                    ripgrep: Path::new("/managed/rg"),
                    input_root: prepared.root(),
                },
            )
            .unwrap();
        let frozen_path = prepared
            .root()
            .canonicalize()
            .unwrap()
            .join(filename)
            .canonicalize()
            .unwrap();
        assert!(compiled.contains(&shell_single_quote(frozen_path.to_str().unwrap())));
        assert!(compiled.starts_with(&format!("PDF='{filename}'")));
        assert!(compiled.contains("'/managed/python' -I -B '/managed/pdf-runtime-cli.py'"));
        assert!(compiled.ends_with("'/managed/rg' --no-config 'PDF' extracted.txt"));
        assert!(!plan
            .command
            .contains(prepared.root().to_string_lossy().as_ref()));
    }

    #[test]
    fn host_only_or_undeclared_variable_references_fail_closed() {
        for command in [
            "pdfinfo \"$MYCOPILOT_PDF_RUNTIME_ROOT\"",
            "rg pattern \"$PATH\"",
            "PDF=manual.pdf; pdfinfo \"$OTHER\"",
            "OPTION=--pre; rg \"$OPTION\" marker extracted.txt",
            "OPTION=-layout; pdftotext \"$OPTION\" manual.pdf -",
            "TARGET=/etc/passwd; rg secret \"$TARGET\"",
        ] {
            assert!(parse_managed_pdf_shell(command).is_err(), "{command}");
        }
        assert!(
            parse_managed_pdf_shell("PATTERN=needle; rg \"$PATTERN\" extracted.txt")
                .unwrap()
                .is_some()
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_redirection_revalidation_rejects_existing_symlinks() {
        use std::os::unix::fs::symlink;

        let execution = TempDir::new().unwrap();
        fs::create_dir(execution.path().join("outputs")).unwrap();
        let external = TempDir::new().unwrap();
        symlink(external.path(), execution.path().join("outputs/link")).unwrap();

        let plan =
            parse_managed_pdf_shell("pdftotext outputs/source.pdf - > outputs/link/leak.txt")
                .unwrap()
                .unwrap();
        assert!(plan
            .validate_private_redirections(execution.path())
            .unwrap_err()
            .contains("符号链接"));
    }
}
