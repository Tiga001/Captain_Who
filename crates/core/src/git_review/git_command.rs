use super::*;

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
    let mut command = Command::new("git");
    command
        .arg("--no-pager")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-c")
        .arg(if cfg!(windows) {
            "core.hooksPath=NUL"
        } else {
            "core.hooksPath=/dev/null"
        })
        .arg("-C")
        .arg(cwd)
        .args(args)
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
