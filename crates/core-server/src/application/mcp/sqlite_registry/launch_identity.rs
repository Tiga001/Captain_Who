//! Launch authorization digests and filesystem identity validation.

use super::*;

pub(crate) fn launch_authorization_is_valid(record: &McpPersistedRegistryRecord) -> bool {
    launch_authorization_identity_is_valid(record)
        && record
            .launch_authorization
            .as_ref()
            .is_some_and(|authorization| {
                compute_launch_file_identity_digest(
                    &record.entry.config,
                    &record.launch_spec_digest,
                )
                .is_ok_and(|digest| digest == authorization.file_identity_digest)
            })
}

pub(crate) fn launch_authorization_identity_is_valid(record: &McpPersistedRegistryRecord) -> bool {
    record.entry.config.trust == McpTrustLevel::UserApproved
        && record
            .launch_authorization
            .as_ref()
            .is_some_and(|authorization| {
                authorization.server_id == record.entry.config.id
                    && authorization.launch_spec_digest == record.launch_spec_digest
                    && authorization.authored_config_epoch == record.entry.config_epoch
                    && authorization.authored_config_digest == record.entry.config_digest
                    && authorization.authorization_format_version
                        == MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION
                    && authorization.authorization_policy_version
                        == MCP_LAUNCH_AUTHORIZATION_POLICY_VERSION
            })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LaunchSpec<'a> {
    authorization_format_version: u32,
    server_id: String,
    scope: &'static str,
    source: &'static str,
    transport: &'static str,
    executable: &'a str,
    arguments: &'a [String],
    cwd: &'a str,
    environment: &'a [String],
}

pub(crate) fn compute_launch_spec_digest(
    config: &McpServerConfig,
) -> Result<McpLaunchSpecDigest, McpRegistryPersistenceError> {
    let config = normalize_config(config.clone())?;
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    if !stdio.environment.is_empty() || config.scope != McpServerScope::User {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    let empty_environment: &[String] = &[];
    let launch = LaunchSpec {
        authorization_format_version: MCP_LAUNCH_AUTHORIZATION_FORMAT_VERSION,
        server_id: config.id.to_string(),
        scope: "user",
        source: SOURCE_USER_MANUAL,
        transport: "stdio",
        executable: path_text(&stdio.program)?,
        arguments: &stdio.arguments,
        cwd: path_text(&stdio.cwd)?,
        environment: empty_environment,
    };
    let encoded =
        serde_json::to_vec(&launch).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let mut hasher = Sha256::new();
    hasher.update(LAUNCH_DIGEST_DOMAIN);
    hasher.update(encoded);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    use fmt::Write as _;
    for byte in digest {
        write!(output, "{byte:02x}")
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    }
    Ok(McpLaunchSpecDigest(output))
}

pub(crate) fn compute_launch_file_identity_digest(
    config: &McpServerConfig,
    launch_spec_digest: &McpLaunchSpecDigest,
) -> Result<McpLaunchSpecDigest, McpRegistryPersistenceError> {
    prepare_launch_file_identity(config, launch_spec_digest).map(|(digest, _)| digest)
}

pub(crate) fn prepare_launch_file_identity(
    config: &McpServerConfig,
    launch_spec_digest: &McpLaunchSpecDigest,
) -> Result<(McpLaunchSpecDigest, McpServerConfig), McpRegistryPersistenceError> {
    let config = normalize_config(config.clone())?;
    let McpTransportConfig::Stdio(stdio) = &config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    // Keep the normalized logical executable path for process creation. In
    // particular, Python virtual environments intentionally expose `bin/python`
    // as a symlink; replacing it with the canonical base interpreter changes
    // Python's environment discovery and loses the venv site-packages. The
    // identity below still binds both this logical path and its canonical
    // target before the authorized connector reaches spawn.
    let launch_program = stdio.program.clone();

    let mut hasher = Sha256::new();
    hasher.update(LAUNCH_FILE_IDENTITY_DOMAIN);
    hash_identity_field(&mut hasher, launch_spec_digest.as_str().as_bytes());
    let mut remaining_content_hash_bytes = MAX_LAUNCH_CONTENT_HASH_TOTAL_BYTES;
    hash_required_launch_path(
        &mut hasher,
        b"executable",
        &stdio.program,
        true,
        &mut remaining_content_hash_bytes,
    )?;
    let canonical_cwd = hash_required_launch_path(
        &mut hasher,
        b"cwd",
        &stdio.cwd,
        false,
        &mut remaining_content_hash_bytes,
    )?;

    let mut code_inputs = 0_usize;
    let mut canonical_arguments = stdio.arguments.clone();
    for (index, argument) in stdio.arguments.iter().enumerate() {
        let Some(code_input) = launch_code_input(argument) else {
            continue;
        };
        code_inputs = code_inputs
            .checked_add(1)
            .filter(|count| *count <= MAX_LAUNCH_CODE_INPUTS)
            .ok_or(McpRegistryPersistenceError::InvalidConfig)?;
        hash_identity_field(&mut hasher, b"code-input");
        hasher.update((index as u64).to_le_bytes());
        let candidate = if code_input.path.is_absolute() {
            code_input.path.to_path_buf()
        } else {
            stdio.cwd.join(code_input.path)
        };
        let canonical = hash_required_launch_path(
            &mut hasher,
            b"code-input-path",
            &candidate,
            true,
            &mut remaining_content_hash_bytes,
        )?;
        let canonical = path_text(&canonical)?;
        canonical_arguments[index] = match code_input.inline_prefix {
            Some(prefix) => format!("{prefix}{canonical}"),
            None => canonical.to_string(),
        };
    }

    let digest = hasher.finalize();
    let mut output = String::with_capacity(64);
    use fmt::Write as _;
    for byte in digest {
        write!(output, "{byte:02x}")
            .map_err(|_| McpRegistryPersistenceError::StorageUnavailable)?;
    }
    let mut launch_config = config;
    let McpTransportConfig::Stdio(stdio) = &mut launch_config.transport else {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    };
    stdio.program = launch_program;
    stdio.cwd = canonical_cwd;
    stdio.arguments = canonical_arguments;
    Ok((McpLaunchSpecDigest(output), launch_config))
}

fn hash_required_launch_path(
    hasher: &mut Sha256,
    role: &[u8],
    path: &Path,
    require_file: bool,
    remaining_content_hash_bytes: &mut u64,
) -> Result<PathBuf, McpRegistryPersistenceError> {
    hash_identity_field(hasher, role);
    hash_identity_field(hasher, path_text(path)?.as_bytes());
    let canonical =
        std::fs::canonicalize(path).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let metadata =
        std::fs::metadata(&canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if (require_file && !metadata.is_file()) || (!require_file && !metadata.is_dir()) {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    hash_canonical_launch_object(hasher, &canonical, metadata, remaining_content_hash_bytes)?;
    Ok(canonical)
}

#[derive(Clone, Copy)]
struct LaunchCodeInput<'a> {
    path: &'a Path,
    inline_prefix: Option<&'static str>,
}

fn launch_code_input(argument: &str) -> Option<LaunchCodeInput<'_>> {
    const INLINE_CODE_PATH_FLAGS: [&str; 5] = [
        "--require=",
        "--import=",
        "--loader=",
        "--experimental-loader=",
        "--module=",
    ];
    if let Some((prefix, value)) = INLINE_CODE_PATH_FLAGS
        .iter()
        .find_map(|prefix| argument.strip_prefix(prefix).map(|value| (*prefix, value)))
    {
        let path = Path::new(value);
        let explicit_filesystem_path = path.is_absolute()
            || matches!(
                path.components().next(),
                Some(Component::CurDir | Component::ParentDir)
            );
        if !explicit_filesystem_path {
            // URL imports and package specifiers are resolved by the runtime;
            // guessing them as cwd-relative files would reject valid launches.
            return None;
        }
        return has_code_extension(path).then_some(LaunchCodeInput {
            path,
            inline_prefix: Some(prefix),
        });
    }
    let path = Path::new(argument);
    if argument.is_empty()
        || argument.starts_with('-')
        || (!path.is_absolute() && has_uri_scheme(argument))
    {
        return None;
    }
    has_code_extension(path).then_some(LaunchCodeInput {
        path,
        inline_prefix: None,
    })
}

fn has_uri_scheme(value: &str) -> bool {
    let Some(separator) = value.find(':') else {
        return false;
    };
    let scheme = &value[..separator];
    !scheme.is_empty()
        && scheme
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'-' | b'.'))
}

fn has_code_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .is_some_and(|extension| {
            matches!(
                extension.as_str(),
                "js" | "mjs"
                    | "cjs"
                    | "ts"
                    | "mts"
                    | "cts"
                    | "py"
                    | "pyw"
                    | "rb"
                    | "pl"
                    | "php"
                    | "sh"
                    | "bash"
                    | "zsh"
                    | "fish"
                    | "ps1"
                    | "bat"
                    | "cmd"
                    | "jar"
                    | "wasm"
            )
        })
}

fn hash_canonical_launch_object(
    hasher: &mut Sha256,
    canonical: &Path,
    metadata_before: std::fs::Metadata,
    remaining_content_hash_bytes: &mut u64,
) -> Result<(), McpRegistryPersistenceError> {
    hash_identity_field(hasher, path_text(canonical)?.as_bytes());
    let before = LaunchMetadataSnapshot::from_metadata(&metadata_before)?;
    let hash_contents = metadata_before.is_file()
        && metadata_before.len() <= MAX_LAUNCH_CONTENT_HASH_FILE_BYTES
        && metadata_before.len() <= *remaining_content_hash_bytes;
    before.hash_into(hasher, !hash_contents);

    if hash_contents {
        *remaining_content_hash_bytes -= metadata_before.len();
        hash_launch_file_contents(hasher, canonical, &before)?;
    }

    let metadata_after =
        std::fs::metadata(canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if before != LaunchMetadataSnapshot::from_metadata(&metadata_after)? {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    Ok(())
}

fn hash_launch_file_contents(
    hasher: &mut Sha256,
    canonical: &Path,
    expected: &LaunchMetadataSnapshot,
) -> Result<(), McpRegistryPersistenceError> {
    let mut file =
        std::fs::File::open(canonical).map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    let opened = file
        .metadata()
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if LaunchMetadataSnapshot::from_metadata(&opened)? != *expected {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }

    let mut content_hasher = Sha256::new();
    let mut buffer = [0_u8; LAUNCH_IDENTITY_READ_BUFFER_BYTES];
    let mut bytes_read = 0_u64;
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
        if count == 0 {
            break;
        }
        bytes_read = bytes_read
            .checked_add(count as u64)
            .filter(|total| *total <= expected.len)
            .ok_or(McpRegistryPersistenceError::InvalidConfig)?;
        content_hasher.update(&buffer[..count]);
    }
    if bytes_read != expected.len {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }

    let closed_over = file
        .metadata()
        .map_err(|_| McpRegistryPersistenceError::InvalidConfig)?;
    if LaunchMetadataSnapshot::from_metadata(&closed_over)? != *expected {
        return Err(McpRegistryPersistenceError::InvalidConfig);
    }
    hash_identity_field(hasher, b"content-sha256");
    hash_identity_field(hasher, content_hasher.finalize().as_slice());
    Ok(())
}

fn hash_identity_field(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

#[derive(PartialEq, Eq)]
struct LaunchMetadataSnapshot {
    kind: u8,
    len: u64,
    modified_nanos: Option<u128>,
    readonly: bool,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    changed_seconds: i64,
    #[cfg(unix)]
    changed_nanos: i64,
    #[cfg(unix)]
    mode: u32,
    #[cfg(unix)]
    owner: u32,
    #[cfg(unix)]
    group: u32,
}

impl LaunchMetadataSnapshot {
    fn from_metadata(metadata: &std::fs::Metadata) -> Result<Self, McpRegistryPersistenceError> {
        let is_file = metadata.is_file();
        let kind = if is_file {
            1
        } else if metadata.is_dir() {
            2
        } else {
            return Err(McpRegistryPersistenceError::InvalidConfig);
        };
        let modified_nanos = is_file
            .then(|| {
                metadata.modified().ok().and_then(|modified| {
                    modified
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|duration| duration.as_nanos())
                })
            })
            .flatten();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                kind,
                // Directory size, mtime, permissions, and ownership legitimately
                // change as applications create files under an authorized cwd.
                // Its canonical target plus device/inode are the stable identity.
                len: if is_file { metadata.len() } else { 0 },
                modified_nanos,
                readonly: is_file && metadata.permissions().readonly(),
                device: metadata.dev(),
                inode: metadata.ino(),
                changed_seconds: if is_file { metadata.ctime() } else { 0 },
                changed_nanos: if is_file { metadata.ctime_nsec() } else { 0 },
                mode: if is_file { metadata.mode() } else { 0 },
                owner: if is_file { metadata.uid() } else { 0 },
                group: if is_file { metadata.gid() } else { 0 },
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                kind,
                len: if is_file { metadata.len() } else { 0 },
                modified_nanos,
                readonly: is_file && metadata.permissions().readonly(),
            })
        }
    }

    fn hash_into(&self, hasher: &mut Sha256, include_ctime: bool) {
        hasher.update([self.kind]);
        hasher.update(self.len.to_le_bytes());
        match self.modified_nanos {
            Some(value) => {
                hasher.update([1]);
                hasher.update(value.to_le_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update([u8::from(self.readonly)]);
        #[cfg(unix)]
        {
            hasher.update(self.device.to_le_bytes());
            hasher.update(self.inode.to_le_bytes());
            if include_ctime {
                // Large runtime binaries keep the cheap ctime tamper signal;
                // complete content hashing is reserved for bounded files.
                hasher.update(self.changed_seconds.to_le_bytes());
                hasher.update(self.changed_nanos.to_le_bytes());
            }
            // For content-hashed files, ctime remains part of the before/after
            // equality check but not the persisted identity. macOS may update
            // it for quarantine/xattr bookkeeping without changing authority.
            hasher.update(self.mode.to_le_bytes());
            hasher.update(self.owner.to_le_bytes());
            hasher.update(self.group.to_le_bytes());
        }
    }
}
