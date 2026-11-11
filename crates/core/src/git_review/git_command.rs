use super::*;
use std::io::Seek;

const MAX_FILTER_CONFIG_BYTES: usize = 16 * 1024;
const MAX_FILTER_DRIVERS: usize = 128;
const MAX_GIT_INPUT_BYTES: usize = 2 * MAX_STATUS_BYTES;

pub(super) struct GitOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
    pub(super) stdout_truncated: bool,
}

pub(super) fn run_git(
    cwd: &Path,
    args: &[String],
    stdout_limit: usize,
) -> Result<GitOutput, String> {
    run_git_with_input(cwd, args, stdout_limit, None)
}

pub(super) fn run_git_with_input(
    cwd: &Path,
    args: &[String],
    stdout_limit: usize,
    stdin: Option<&[u8]>,
) -> Result<GitOutput, String> {
    let mut command = git_command(cwd);
    if matches!(
        args.first().map(String::as_str),
        Some("status" | "diff" | "add" | "restore" | "reset" | "rm" | "log")
    ) {
        // --no-ext-diff/--no-textconv do not disable clean or long-running filters.
        // Even `status` can invoke those while refreshing tracked worktree content.
        let mut configuration = git_command(cwd);
        configuration.args([
            "config",
            "--includes",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ]);
        let output = run_git_command(configuration, MAX_FILTER_CONFIG_BYTES, None)?;
        if output.stdout_truncated || (!output.status.success() && output.status.code() != Some(1))
        {
            return Err("Unable to verify the Git filter configuration safely.".into());
        }
        let drivers = filter_drivers(&output.stdout)?;
        let mutation = matches!(
            args.first().map(String::as_str),
            Some("add" | "restore" | "reset" | "rm")
        );
        for driver in drivers {
            for operation in ["clean", "smudge", "process"] {
                command
                    .arg("-c")
                    .arg(format!("filter.{driver}.{operation}="));
            }
            // Reads use raw bytes only for discovery; filtered diffs/content are rejected by
            // the service. A mutation must fail if a filter appears after attribute validation.
            command
                .arg("-c")
                .arg(format!("filter.{driver}.required={mutation}"));
        }
    }
    command.args(args);
    run_git_command(command, stdout_limit, stdin)
}

fn filter_drivers(bytes: &[u8]) -> Result<HashSet<&str>, String> {
    let invalid = || "Unable to verify the Git filter configuration safely.".to_string();
    if !bytes.is_empty() && bytes.last() != Some(&0) {
        return Err(invalid());
    }
    let mut drivers = HashSet::new();
    for key in bytes.split(|byte| *byte == 0).filter(|key| !key.is_empty()) {
        let key = std::str::from_utf8(key).map_err(|_| invalid())?;
        let (prefix, operation) = key.rsplit_once('.').ok_or_else(invalid)?;
        let driver = prefix.strip_prefix("filter.").ok_or_else(invalid)?;
        if driver.is_empty()
            || driver.contains('=')
            || driver.chars().any(char::is_control)
            || !matches!(operation, "clean" | "smudge" | "process" | "required")
        {
            return Err(invalid());
        }
        drivers.insert(driver);
        if drivers.len() > MAX_FILTER_DRIVERS {
            return Err(invalid());
        }
    }
    Ok(drivers)
}

fn git_command(cwd: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-c")
        .arg("core.fsmonitor=false")
        // Inline submodule diffs can invoke a second repository's filters, which are not part
        // of this repository's configuration probe. Review exposes gitlinks, not nested tools.
        .arg("-c")
        .arg("diff.submodule=short")
        .arg("-c")
        .arg("submodule.recurse=false")
        .arg("-c")
        .arg(if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        })
        .arg("-C")
        .arg(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
        .env("GIT_TERMINAL_PROMPT", "0")
        // A repository-controlled replacement ref must not redirect a validated object ID.
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_PAGER", "cat")
        .env("PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_REPLACE_REF_BASE")
        .env_remove("GIT_CEILING_DIRECTORIES")
        .env_remove("GIT_DISCOVERY_ACROSS_FILESYSTEM")
        .env_remove("GIT_EXTERNAL_DIFF")
        .env_remove("GIT_DIFF_OPTS")
        // GIT_CONFIG redirects only `git config`, not status/diff. Inheriting it would make
        // the filter probe inspect a different configuration from the actual operation.
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_NOSYSTEM")
        .env_remove("GIT_CONFIG_COUNT");

    for (key, _) in std::env::vars_os() {
        let key_text = key.to_string_lossy();
        if key_text.starts_with("GIT_CONFIG_KEY_") || key_text.starts_with("GIT_CONFIG_VALUE_") {
            command.env_remove(key);
        }
    }
    command
}

fn run_git_command(
    mut command: Command,
    stdout_limit: usize,
    stdin: Option<&[u8]>,
) -> Result<GitOutput, String> {
    if let Some(input) = stdin.filter(|input| !input.is_empty()) {
        if input.len() > MAX_GIT_INPUT_BYTES {
            return Err("The Git input is too large to process safely.".into());
        }
        // Writing a pipe before starting stdout/stderr readers can deadlock check-attr on
        // large path batches. An anonymous bounded input file needs no detached writer task.
        let mut file =
            tempfile::tempfile().map_err(|_| "Unable to prepare Git input.".to_string())?;
        file.write_all(input)
            .and_then(|()| file.rewind())
            .map_err(|_| "Unable to prepare Git input.".to_string())?;
        command.stdin(Stdio::from(file));
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Unable to start Git: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Git stdout was not captured.".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "Git stderr was not captured.".to_string())?;
    let stdout_reader = thread::spawn(move || read_bounded(stdout, stdout_limit));
    let stderr_reader = thread::spawn(move || read_bounded(stderr, MAX_STDERR_BYTES));
    let deadline = Instant::now() + GIT_TIMEOUT;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err("Git operation timed out.".to_string());
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(format!("Unable to wait for Git: {error}"));
            }
        }
    };
    let (stdout, stdout_truncated) = stdout_reader
        .join()
        .map_err(|_| "Git stdout reader failed.".to_string())??;
    let (stderr, _) = stderr_reader
        .join()
        .map_err(|_| "Git stderr reader failed.".to_string())??;

    Ok(GitOutput {
        status,
        stdout,
        stderr,
        stdout_truncated,
    })
}

pub(super) fn read_bounded<R: Read>(
    mut reader: R,
    limit: usize,
) -> Result<(Vec<u8>, bool), String> {
    let mut output = Vec::with_capacity(limit.min(64 * 1024));
    let mut buffer = [0_u8; 16 * 1024];
    let mut truncated = false;
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Unable to read Git output: {error}"))?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(output.len());
        let retained = remaining.min(read);
        output.extend_from_slice(&buffer[..retained]);
        truncated |= retained < read;
    }
    Ok((output, truncated))
}

pub(super) fn git_failure(fallback: &str, output: &GitOutput) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let message = stderr.lines().next().unwrap_or_default().trim();
    if message.is_empty() {
        fallback.to_string()
    } else {
        format!("{fallback} {message}")
    }
}

pub(super) fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map(|index| index + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

pub(super) fn path_to_git_string(path: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

pub(super) fn literal_pathspec(path: &str) -> String {
    format!(":(top,literal){path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_driver_config_preserves_case_and_rejects_ambiguous_overrides() {
        let drivers =
            filter_drivers(b"filter.Driver.With.Dots.clean\0filter.Driver.With.Dots.process\0")
                .unwrap();
        assert_eq!(drivers, HashSet::from(["Driver.With.Dots"]));
        for invalid in [
            b"filter.foo=bar.clean\0".as_slice(),
            b"filter.foo\nbar.clean\0".as_slice(),
            b"filter.foo.clean".as_slice(),
        ] {
            assert!(filter_drivers(invalid).is_err());
        }
    }

    #[test]
    fn configuration_probe_cannot_be_redirected_by_git_config_environment() {
        let command = git_command(Path::new("."));
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "GIT_CONFIG" && value.is_none()));
    }
}
