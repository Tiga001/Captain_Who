use super::*;

pub(super) fn classify_segment(
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

pub(super) fn is_catastrophic_system_control(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_pid_one(token: &str) -> bool {
    let digits = token.strip_prefix('+').unwrap_or(token);
    !digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
        && digits.trim_start_matches('0') == "1"
}

pub(super) fn is_negative_pid_one(token: &str) -> bool {
    token.strip_prefix('-').is_some_and(is_pid_one)
}

pub(super) fn has_disk_mutation(tokens: &[String]) -> bool {
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

pub(super) fn has_raw_device_output(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        token
            .to_ascii_lowercase()
            .strip_prefix("of=")
            .is_some_and(is_catastrophic_write_target)
    })
}

pub(super) fn writes_raw_device(tokens: &[String], program: &str) -> bool {
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

pub(super) fn sort_output_targets(tokens: &[String]) -> impl Iterator<Item = &str> {
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

pub(super) fn sort_uses_explicit_temp_storage(tokens: &[String]) -> bool {
    tokens.iter().skip(1).any(|token| {
        matches!(token.as_str(), "-T" | "--temporary-directory")
            || token.starts_with("-T") && token.len() > 2
            || token.starts_with("--temporary-directory=")
    })
}

pub(super) fn option_value_targets<'a>(
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

pub(super) fn is_raw_device_path(target: &str) -> bool {
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

pub(super) fn is_catastrophic_write_target(target: &str) -> bool {
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

pub(super) fn is_catastrophic_filesystem_operation(
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

pub(super) fn has_opaque_recursive_target(tokens: &[String], program: &str) -> bool {
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

pub(super) fn has_unsafe_recursive_symlink_traversal(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_opaque_destructive_target(target: &str) -> bool {
    target.contains(['$', '`', '*', '?', '[', ']', '{', '}']) || target.starts_with('~')
}

pub(super) fn has_recursive_flag(tokens: &[String]) -> bool {
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

pub(super) fn command_targets(tokens: &[String]) -> impl Iterator<Item = &str> {
    tokens
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|token| !token.starts_with('-') && *token != "--")
}

pub(super) fn is_catastrophic_target(target: &str, cwd: Option<&Path>) -> bool {
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

pub(super) fn normalize_lexical_path(path: &Path) -> PathBuf {
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

pub(super) fn is_sensitive_absolute_path(path: &Path) -> bool {
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

pub(super) fn is_unsupported_interactive(tokens: &[String], program: &str) -> bool {
    matches!(
        program,
        "vim" | "vi" | "nano" | "less" | "more" | "top" | "htop" | "watch" | "read" | "passwd"
    ) || matches!(program, "python" | "python3" | "node") && tokens.len() == 1
        || program == "ssh" && tokens.len() <= 2
}

pub(super) fn is_package_management(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_network_command(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_high_impact_command(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_direct_write_command(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_safe_workspace_task(tokens: &[String], program: &str) -> bool {
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
