use super::*;

pub(super) fn is_workspace_task(tokens: &[String], program: &str) -> bool {
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

pub(super) fn has_workspace_task_boundary_override(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_known_read_only(tokens: &[String], program: &str) -> bool {
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

pub(super) fn is_read_only_git(tokens: &[String]) -> bool {
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

pub(super) fn matches_closed_cli_grammar(
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

pub(super) fn is_read_only_date(tokens: &[String]) -> bool {
    match tokens.get(1).map(String::as_str) {
        None => true,
        Some("-u" | "--utc" | "--universal") => {
            tokens.len() == 2
                || tokens.len() == 3 && tokens.get(2).is_some_and(|format| format.starts_with('+'))
        }
        Some(format) => tokens.len() == 2 && format.starts_with('+'),
    }
}

pub(super) fn is_read_only_find(tokens: &[String]) -> bool {
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

pub(super) fn is_read_only_pip(tokens: &[String]) -> bool {
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

pub(super) fn is_git_worktree_read(tokens: &[String], program: &str) -> bool {
    program == "git"
        && tokens.get(1).is_some_and(|subcommand| {
            matches!(
                subcommand.to_ascii_lowercase().as_str(),
                "status" | "diff" | "log" | "show"
            )
        })
}

pub(super) fn is_jq_environment_read(tokens: &[String], program: &str) -> bool {
    program == "jq"
        && tokens.iter().skip(1).any(|token| {
            let lower = token.to_ascii_lowercase();
            lower.contains("$env")
                || lower
                    .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                    .any(|word| word == "env")
        })
}

pub(super) fn is_jq_module_load(tokens: &[String], program: &str) -> bool {
    program == "jq"
        && tokens.iter().skip(1).any(|token| {
            token
                .to_ascii_lowercase()
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .any(|word| matches!(word, "include" | "import" | "module"))
        })
}
