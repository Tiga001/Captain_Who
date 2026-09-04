use super::*;

#[derive(Deserialize)]
pub(super) struct RepositoryApiResponse {
    pub(super) default_branch: String,
}

#[derive(Deserialize)]
pub(super) struct CommitApiResponse {
    pub(super) sha: String,
}

pub(super) fn read_success_response(
    mut response: Response,
    max_bytes: usize,
    json_response: bool,
) -> Result<Vec<u8>, GitHubTransportError> {
    if !response.status().is_success() {
        return Err(map_response_status(&response));
    }
    if let Some(length) = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
    {
        if length > max_bytes as u64 {
            return Err(GitHubTransportError::ResponseTooLarge);
        }
    }
    if json_response {
        let content_type_is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| {
                value.split(';').next().is_some_and(|mime| {
                    mime.trim().ends_with("/json") || mime.trim().ends_with("+json")
                })
            });
        if !content_type_is_json {
            return Err(GitHubTransportError::InvalidResponse);
        }
    }
    let mut body = Vec::new();
    response
        .by_ref()
        .take(max_bytes.saturating_add(1) as u64)
        .read_to_end(&mut body)
        .map_err(map_io_transport_error)?;
    if body.len() > max_bytes {
        return Err(GitHubTransportError::ResponseTooLarge);
    }
    Ok(body)
}

pub(super) fn spool_archive_response(
    mut response: Response,
) -> Result<GitHubArchive, GitHubTransportError> {
    if !response.status().is_success() {
        return Err(map_response_status(&response));
    }
    let expected_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if expected_length.is_some_and(|length| length > MAX_GITHUB_ZIP_BYTES as u64) {
        return Err(GitHubTransportError::ResponseTooLarge);
    }

    spool_archive_reader(&mut response, expected_length)
}

pub(super) fn spool_archive_reader(
    reader: &mut dyn Read,
    expected_length: Option<u64>,
) -> Result<GitHubArchive, GitHubTransportError> {
    if expected_length.is_some_and(|length| length > MAX_GITHUB_ZIP_BYTES as u64) {
        return Err(GitHubTransportError::ResponseTooLarge);
    }
    let mut file = NamedTempFile::new().map_err(|_| GitHubTransportError::Unavailable)?;
    let mut hasher = Sha256::new();
    let mut total = 0usize;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer).map_err(map_io_transport_error)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read)
            .ok_or(GitHubTransportError::ResponseTooLarge)?;
        if total > MAX_GITHUB_ZIP_BYTES {
            return Err(GitHubTransportError::ResponseTooLarge);
        }
        file.write_all(&buffer[..read])
            .map_err(|_| GitHubTransportError::Unavailable)?;
        hasher.update(&buffer[..read]);
    }
    if expected_length.is_some_and(|length| length != total as u64) {
        return Err(GitHubTransportError::InvalidResponse);
    }
    file.as_file_mut()
        .flush()
        .map_err(|_| GitHubTransportError::Unavailable)?;
    let digest: [u8; 32] = hasher.finalize().into();
    Ok(GitHubArchive::from_spool(file, total, digest))
}

pub(super) fn map_response_status(response: &Response) -> GitHubTransportError {
    map_status_and_headers(response.status(), response.headers())
}

pub(super) fn map_status_and_headers(
    status: StatusCode,
    headers: &HeaderMap,
) -> GitHubTransportError {
    match status {
        StatusCode::NOT_FOUND => GitHubTransportError::NotFound,
        StatusCode::TOO_MANY_REQUESTS => GitHubTransportError::RateLimited {
            retry_after: retry_after_from_headers(headers).or(Some(GITHUB_RATE_LIMIT_FALLBACK)),
            secondary: headers.contains_key(RETRY_AFTER),
        },
        StatusCode::FORBIDDEN if is_rate_limit_response(headers) => {
            GitHubTransportError::RateLimited {
                retry_after: retry_after_from_headers(headers).or(Some(GITHUB_RATE_LIMIT_FALLBACK)),
                secondary: headers.contains_key(RETRY_AFTER),
            }
        }
        status if status.is_server_error() => GitHubTransportError::Unavailable,
        _ => GitHubTransportError::Rejected,
    }
}

fn is_rate_limit_response(headers: &HeaderMap) -> bool {
    headers.contains_key(RETRY_AFTER)
        || headers
            .get("x-ratelimit-remaining")
            .and_then(|value| value.to_str().ok())
            == Some("0")
}

fn retry_after_from_headers(headers: &HeaderMap) -> Option<Duration> {
    if let Some(seconds) = headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
    {
        return Some(Duration::from_secs(seconds.clamp(1, 24 * 60 * 60)));
    }
    let reset = headers
        .get("x-ratelimit-reset")?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
    Some(Duration::from_secs(
        reset.saturating_sub(now).clamp(1, 24 * 60 * 60),
    ))
}

pub(super) fn map_reqwest_error(error: reqwest::Error) -> GitHubTransportError {
    if error.is_timeout() {
        GitHubTransportError::Timeout
    } else if error.is_connect() {
        GitHubTransportError::NetworkUnavailable
    } else {
        GitHubTransportError::Unavailable
    }
}

fn map_io_transport_error(error: std::io::Error) -> GitHubTransportError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    ) {
        GitHubTransportError::Timeout
    } else {
        GitHubTransportError::NetworkUnavailable
    }
}

pub(super) fn retryable_transport_error(error: &GitHubTransportError) -> bool {
    matches!(
        error,
        GitHubTransportError::Timeout
            | GitHubTransportError::NetworkUnavailable
            | GitHubTransportError::Unavailable
    )
}

pub(super) fn resolve_with_git_ls_remote(
    request: &GitHubResolveRequest,
) -> Result<GitHubCommit, GitHubTransportError> {
    if let GitHubReference::Commit(commit) = request.reference() {
        return Ok(commit.clone());
    }
    let repository_url = format!(
        "https://github.com/{}/{}.git",
        request.repository().owner(),
        request.repository().name()
    );
    let mut command = Command::new("git");
    command
        .arg("-c")
        .arg("credential.helper=")
        .arg("-c")
        .arg("core.askPass=")
        .arg("ls-remote")
        .arg("--symref")
        .arg("--exit-code")
        .arg(&repository_url);
    match request.reference() {
        GitHubReference::DefaultBranch => {
            command.arg("HEAD");
        }
        GitHubReference::Named(reference) => {
            command
                .arg(format!("refs/heads/{}", reference.as_str()))
                .arg(format!("refs/tags/{}", reference.as_str()))
                .arg(format!("refs/tags/{}^{{}}", reference.as_str()));
        }
        GitHubReference::Commit(_) => unreachable!("commit references return before spawning git"),
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "Never")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| GitHubTransportError::Unavailable)?;
    let deadline = Instant::now()
        .checked_add(GITHUB_LS_REMOTE_TIMEOUT)
        .ok_or(GitHubTransportError::Unavailable)?;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GitHubTransportError::Timeout);
            }
            Err(_) => return Err(GitHubTransportError::Unavailable),
        }
    };
    if !status.success() {
        return Err(GitHubTransportError::NotFound);
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or(GitHubTransportError::InvalidResponse)?;
    let mut bytes = Vec::new();
    stdout
        .by_ref()
        .take(GITHUB_API_BODY_BYTES.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| GitHubTransportError::InvalidResponse)?;
    if bytes.len() > GITHUB_API_BODY_BYTES {
        return Err(GitHubTransportError::InvalidResponse);
    }
    parse_ls_remote_output(request.reference(), &bytes)
}

pub(super) fn parse_ls_remote_output(
    reference: &GitHubReference,
    output: &[u8],
) -> Result<GitHubCommit, GitHubTransportError> {
    let output = std::str::from_utf8(output).map_err(|_| GitHubTransportError::InvalidResponse)?;
    let mut refs = BTreeMap::<&str, &str>::new();
    for line in output.lines() {
        if line.starts_with("ref: ") {
            continue;
        }
        let Some((sha, name)) = line.split_once('\t') else {
            return Err(GitHubTransportError::InvalidResponse);
        };
        refs.insert(name, sha);
    }
    let sha = match reference {
        GitHubReference::DefaultBranch => refs.get("HEAD").copied(),
        GitHubReference::Named(reference) => {
            let branch = format!("refs/heads/{}", reference.as_str());
            let peeled_tag = format!("refs/tags/{}^{{}}", reference.as_str());
            let tag = format!("refs/tags/{}", reference.as_str());
            refs.get(branch.as_str())
                .or_else(|| refs.get(peeled_tag.as_str()))
                .or_else(|| refs.get(tag.as_str()))
                .copied()
        }
        GitHubReference::Commit(commit) => return Ok(commit.clone()),
    }
    .ok_or(GitHubTransportError::NotFound)?;
    GitHubCommit::parse(sha).map_err(|_| GitHubTransportError::InvalidResponse)
}

pub(super) fn github_api_repository_url(
    repository: &GitHubRepository,
) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://api.github.com/",
        &["repos", repository.owner(), repository.name()],
    )
}

pub(super) fn github_api_commit_url(
    repository: &GitHubRepository,
    reference: &str,
) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://api.github.com/",
        &[
            "repos",
            repository.owner(),
            repository.name(),
            "commits",
            reference,
        ],
    )
}

pub(super) fn github_codeload_url(
    repository: &GitHubRepository,
    commit: &GitHubCommit,
) -> Result<Url, GitHubTransportError> {
    github_url(
        "https://codeload.github.com/",
        &[
            repository.owner(),
            repository.name(),
            "zip",
            commit.as_str(),
        ],
    )
}

fn github_url(base: &str, segments: &[&str]) -> Result<Url, GitHubTransportError> {
    let mut url = Url::parse(base).map_err(|_| GitHubTransportError::InvalidResponse)?;
    let mut path = url
        .path_segments_mut()
        .map_err(|_| GitHubTransportError::InvalidResponse)?;
    path.pop_if_empty();
    for segment in segments {
        path.push(segment);
    }
    drop(path);
    Ok(url)
}
