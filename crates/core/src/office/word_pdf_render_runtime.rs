use super::types::{OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const WORD_PDF_RENDER_RUNTIME_PROVIDER_ID: &str = "mycopilot.word-pdf-render-runtime";
pub const WORD_PDF_RENDER_RUNTIME_BUNDLE_VERSION: &str = "2026.08.1";
pub const WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION: &str = "26.2.4.2";

const COMPONENT_DIRECTORY: &str = "word-pdf-renderer";
const COMPONENT_RECEIPT: &str = "component-receipt.json";
const RUNTIME_REVISION_PREFIX: &str = "word-pdf-render-runtime-sha256-v1:";
const MAX_RECEIPT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_COMPONENT_FILES: usize = 20_000;
const MAX_COMPONENT_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct WordPdfRenderRuntimeDiscoveryOptions {
    configured_component_dir: Option<PathBuf>,
    application_resources_dir: Option<PathBuf>,
    workspace_roots: Vec<PathBuf>,
}

impl WordPdfRenderRuntimeDiscoveryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_configured_component_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.configured_component_dir = Some(directory.into());
        self
    }

    pub fn with_application_resources_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.application_resources_dir = Some(directory.into());
        self
    }

    pub fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_roots = vec![workspace_root.into()];
        self
    }

    pub fn with_workspace_roots(mut self, roots: impl IntoIterator<Item = PathBuf>) -> Self {
        self.workspace_roots = roots.into_iter().collect();
        self
    }
}

#[derive(Debug, Clone)]
pub struct WordPdfRenderRuntime {
    root: PathBuf,
    executable: PathBuf,
    receipt: ComponentReceipt,
    files: BTreeMap<String, VerifiedFile>,
    links: BTreeMap<String, VerifiedLink>,
}

#[derive(Debug, Clone)]
struct VerifiedFile {
    #[cfg(not(unix))]
    size: u64,
    #[cfg(not(unix))]
    sha256: String,
    identity: FileIdentity,
}

#[derive(Debug, Clone)]
struct VerifiedLink {
    target: String,
    identity: FileIdentity,
    resolved_identity: FileIdentity,
}

type VerifiedComponentTree = (
    BTreeMap<String, VerifiedFile>,
    BTreeMap<String, VerifiedLink>,
);

#[derive(Debug)]
struct CollectedTree {
    files: BTreeMap<String, PathBuf>,
    links: BTreeMap<String, CollectedLink>,
}

#[derive(Debug)]
struct CollectedLink {
    target: String,
    identity: FileIdentity,
    resolved_identity: FileIdentity,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    ctime_seconds: i64,
    #[cfg(unix)]
    ctime_nanoseconds: i64,
    size: u64,
}

impl WordPdfRenderRuntime {
    pub fn discover(
        options: &WordPdfRenderRuntimeDiscoveryOptions,
    ) -> Result<Self, OfficeEngineError> {
        if !word_pdf_render_runtime_supported() {
            return Err(unavailable_error());
        }

        let workspaces = options
            .workspace_roots
            .iter()
            .map(|root| root.canonicalize().unwrap_or_else(|_| root.clone()))
            .collect::<Vec<_>>();
        let root = resolve_component_root(options)?;
        if workspaces
            .iter()
            .any(|workspace| root.starts_with(workspace) || workspace.starts_with(&root))
        {
            return Err(invalid_component(
                "The Word PDF render runtime must not be loaded from the agent-writable workspace.",
            ));
        }
        let receipt = read_receipt(&root)?;
        validate_receipt(&receipt)?;
        let (files, links) = verify_component_tree(&root, &receipt)?;
        let executable = resolve_component_file(&root, &receipt.runtime.executable)?;
        let metadata = fs::metadata(&executable).map_err(|error| {
            invalid_component(format!(
                "Cannot inspect the Word PDF render runtime executable: {error}"
            ))
        })?;
        if !metadata.is_file() || !is_executable(&metadata) {
            return Err(invalid_component(
                "The Word PDF render runtime executable is not an executable regular file.",
            ));
        }
        for required in [&receipt.runtime.license, &receipt.runtime.notice] {
            let path = resolve_component_file(&root, required)?;
            let metadata = fs::metadata(&path).map_err(|error| {
                invalid_component(format!(
                    "Cannot inspect required Word PDF render runtime notice: {error}"
                ))
            })?;
            if !metadata.is_file() || metadata.len() == 0 {
                return Err(invalid_component(
                    "The Word PDF render runtime license and notice must be non-empty regular files.",
                ));
            }
        }
        Ok(Self {
            root,
            executable,
            receipt,
            files,
            links,
        })
    }

    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    pub fn runtime_revision(&self) -> &str {
        &self.receipt.bundle_revision
    }

    pub(crate) fn component_root(&self) -> &Path {
        &self.root
    }

    pub fn libreoffice_version(&self) -> &str {
        &self.receipt.runtime.version
    }

    pub fn verify_integrity(&self) -> Result<(), OfficeEngineError> {
        let current = read_receipt(&self.root)?;
        if current != self.receipt {
            return Err(integrity_error(
                "The Word PDF render runtime receipt changed after discovery.",
            ));
        }
        let actual = collect_component_tree(&self.root)?;
        if actual.files.len() != self.files.len() || actual.files.keys().ne(self.files.keys()) {
            return Err(integrity_error(
                "The Word PDF render runtime file set changed after discovery.",
            ));
        }
        for (relative, expected) in &self.files {
            let path = actual.files.get(relative).ok_or_else(|| {
                integrity_error("A Word PDF render runtime file disappeared after discovery.")
            })?;
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                integrity_error(format!(
                    "Cannot reinspect Word PDF render runtime file: {error}"
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(integrity_error(
                    "A Word PDF render runtime file changed type after discovery.",
                ));
            }
            if file_identity(&metadata) != expected.identity {
                return Err(integrity_error(
                    "A Word PDF render runtime file identity changed after discovery.",
                ));
            }
            #[cfg(not(unix))]
            verify_file(path, expected.size, &expected.sha256)?;
        }
        if actual.links.len() != self.links.len() || actual.links.keys().ne(self.links.keys()) {
            return Err(integrity_error(
                "The Word PDF render runtime symlink set changed after discovery.",
            ));
        }
        for (relative, expected) in &self.links {
            let actual = actual.links.get(relative).ok_or_else(|| {
                integrity_error("A Word PDF render runtime symlink disappeared after discovery.")
            })?;
            if actual.target != expected.target {
                return Err(integrity_error(
                    "A Word PDF render runtime symlink target changed after discovery.",
                ));
            }
            if actual.identity != expected.identity
                || actual.resolved_identity != expected.resolved_identity
            {
                return Err(integrity_error(
                    "A Word PDF render runtime symlink identity changed after discovery.",
                ));
            }
        }
        Ok(())
    }
}

pub fn word_pdf_render_component_relative_path() -> PathBuf {
    PathBuf::from(COMPONENT_DIRECTORY)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComponentReceipt {
    schema_version: u32,
    provider_id: String,
    bundle_version: String,
    platform: String,
    arch: String,
    runtime: RuntimeReceipt,
    archive: ArchiveReceipt,
    files: Vec<FileReceipt>,
    links: Vec<LinkReceipt>,
    bundle_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeReceipt {
    family: String,
    version: String,
    executable: String,
    license: String,
    notice: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveReceipt {
    url: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileReceipt {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LinkReceipt {
    path: String,
    target: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptIdentity<'a> {
    schema_version: u32,
    provider_id: &'a str,
    bundle_version: &'a str,
    platform: &'a str,
    arch: &'a str,
    runtime: &'a RuntimeReceipt,
    archive: &'a ArchiveReceipt,
    files: &'a [FileReceipt],
    links: &'a [LinkReceipt],
}

fn resolve_component_root(
    options: &WordPdfRenderRuntimeDiscoveryOptions,
) -> Result<PathBuf, OfficeEngineError> {
    let candidate = if let Some(configured) = options.configured_component_dir.as_deref() {
        configured.to_path_buf()
    } else if let Some(resources) = options.application_resources_dir.as_deref() {
        if !resources.is_absolute() {
            return Err(invalid_component(
                "The Word PDF render runtime application resources directory must be absolute.",
            ));
        }
        let resources = canonical_directory(resources)?;
        let candidate = resources.join(word_pdf_render_component_relative_path());
        let root = canonical_directory(&candidate).map_err(|_| unavailable_error())?;
        if !root.starts_with(&resources) {
            return Err(invalid_component(
                "The packaged Word PDF render runtime resolves outside application resources.",
            ));
        }
        return Ok(root);
    } else {
        return Err(unavailable_error());
    };
    if !candidate.is_absolute() {
        return Err(invalid_component(
            "The configured Word PDF render runtime directory must be absolute.",
        ));
    }
    canonical_directory(&candidate).map_err(|_| unavailable_error())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, OfficeEngineError> {
    let canonical = path.canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot resolve Word PDF render runtime directory `{}`: {error}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(invalid_component(format!(
            "Word PDF render runtime path `{}` is not a directory.",
            canonical.display()
        )));
    }
    Ok(canonical)
}

fn read_receipt(root: &Path) -> Result<ComponentReceipt, OfficeEngineError> {
    let path = root.join(COMPONENT_RECEIPT);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            unavailable_error()
        } else {
            invalid_component(format!(
                "Cannot inspect Word PDF render runtime receipt: {error}"
            ))
        }
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err(invalid_component(
            "The Word PDF render runtime receipt must be a small regular non-symlink file.",
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        invalid_component(format!(
            "Cannot read Word PDF render runtime receipt: {error}"
        ))
    })?;
    if bytes.contains(&0) {
        return Err(invalid_component(
            "The Word PDF render runtime receipt contains a NUL byte.",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        invalid_component(format!(
            "The Word PDF render runtime receipt is not valid strict JSON: {error}"
        ))
    })
}

fn validate_receipt(receipt: &ComponentReceipt) -> Result<(), OfficeEngineError> {
    if receipt.schema_version != 1
        || receipt.provider_id != WORD_PDF_RENDER_RUNTIME_PROVIDER_ID
        || receipt.bundle_version != WORD_PDF_RENDER_RUNTIME_BUNDLE_VERSION
    {
        return Err(invalid_component(
            "The Word PDF render runtime receipt uses an unsupported schema, provider, or bundle version.",
        ));
    }
    if receipt.platform != current_platform() || receipt.arch != current_arch() {
        return Err(invalid_component(format!(
            "The Word PDF render runtime targets {}-{}, not {}-{}.",
            receipt.platform,
            receipt.arch,
            current_platform(),
            current_arch()
        )));
    }
    if receipt.runtime.family != "libreoffice"
        || receipt.runtime.version != WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION
        || receipt.runtime.executable != expected_executable()
        || receipt.runtime.license != expected_license()
        || receipt.runtime.notice != expected_notice()
    {
        return Err(invalid_component(
            "The Word PDF render runtime identity does not match the application-pinned LibreOffice release.",
        ));
    }
    if receipt.archive != expected_archive_receipt() {
        return Err(invalid_component(
            "The Word PDF render runtime archive identity does not match the application-pinned LibreOffice release.",
        ));
    }
    for path in [
        &receipt.runtime.executable,
        &receipt.runtime.license,
        &receipt.runtime.notice,
    ] {
        validate_relative_path(path)?;
    }
    if receipt.files.len() < 3 || receipt.files.len() > MAX_COMPONENT_FILES {
        return Err(invalid_component(format!(
            "The Word PDF render runtime receipt must contain between 3 and {MAX_COMPONENT_FILES} files."
        )));
    }
    let mut previous: Option<&str> = None;
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for file in &receipt.files {
        validate_relative_path(&file.path)?;
        if !file.path.starts_with("libreoffice/") || file.path == COMPONENT_RECEIPT {
            return Err(invalid_component(
                "Word PDF render runtime files must remain inside libreoffice/.",
            ));
        }
        if previous.is_some_and(|value| value >= file.path.as_str()) || !paths.insert(&file.path) {
            return Err(invalid_component(
                "The Word PDF render runtime files must be unique and sorted by path.",
            ));
        }
        previous = Some(&file.path);
        if file.size > MAX_COMPONENT_FILE_BYTES || !valid_sha256(&file.sha256) {
            return Err(invalid_component(
                "The Word PDF render runtime receipt contains an invalid file descriptor.",
            ));
        }
        total = total.checked_add(file.size).ok_or_else(|| {
            invalid_component("The Word PDF render runtime component size overflowed.")
        })?;
        if total > MAX_COMPONENT_BYTES {
            return Err(invalid_component(
                "The Word PDF render runtime component exceeds its 2 GiB safety limit.",
            ));
        }
    }
    let mut previous_link: Option<&str> = None;
    for link in &receipt.links {
        validate_relative_path(&link.path)?;
        validate_safe_link(&link.path, &link.target)?;
        if !link.path.starts_with("libreoffice/")
            || previous_link.is_some_and(|value| value >= link.path.as_str())
            || paths.contains(&link.path)
            || !paths.insert(&link.path)
        {
            return Err(invalid_component(
                "The Word PDF render runtime symlinks must be disjoint, unique, sorted, and inside libreoffice/.",
            ));
        }
        previous_link = Some(&link.path);
    }
    if receipt.files.len().saturating_add(receipt.links.len()) > MAX_COMPONENT_FILES {
        return Err(invalid_component(
            "The Word PDF render runtime receipt exceeds its entry-count limit.",
        ));
    }
    for required in [
        &receipt.runtime.executable,
        &receipt.runtime.license,
        &receipt.runtime.notice,
    ] {
        if !receipt.files.iter().any(|file| &file.path == required) {
            return Err(invalid_component(
                "The Word PDF render runtime receipt omits a required executable or legal notice.",
            ));
        }
    }
    if receipt.bundle_revision != compute_bundle_revision(receipt)? {
        return Err(integrity_error(
            "The Word PDF render runtime bundle revision does not match its frozen receipt.",
        ));
    }
    Ok(())
}

fn compute_bundle_revision(receipt: &ComponentReceipt) -> Result<String, OfficeEngineError> {
    let identity = ReceiptIdentity {
        schema_version: receipt.schema_version,
        provider_id: &receipt.provider_id,
        bundle_version: &receipt.bundle_version,
        platform: &receipt.platform,
        arch: &receipt.arch,
        runtime: &receipt.runtime,
        archive: &receipt.archive,
        files: &receipt.files,
        links: &receipt.links,
    };
    let value = serde_json::to_value(&identity).map_err(|error| {
        invalid_component(format!(
            "Cannot canonicalize Word PDF render runtime receipt: {error}"
        ))
    })?;
    let mut canonical = Vec::new();
    write_canonical_json(&value, &mut canonical)?;
    Ok(format!(
        "{RUNTIME_REVISION_PREFIX}{}",
        hex_lower(&Sha256::digest(canonical))
    ))
}

fn write_canonical_json(
    value: &serde_json::Value,
    output: &mut Vec<u8>,
) -> Result<(), OfficeEngineError> {
    match value {
        serde_json::Value::Null => output.extend_from_slice(b"null"),
        serde_json::Value::Bool(value) => {
            output.extend_from_slice(if *value { b"true" } else { b"false" })
        }
        serde_json::Value::Number(value) => output.extend_from_slice(value.to_string().as_bytes()),
        serde_json::Value::String(value) => {
            serde_json::to_writer(output, value).map_err(|error| {
                invalid_component(format!(
                    "Cannot encode canonical Word PDF render runtime string: {error}"
                ))
            })?
        }
        serde_json::Value::Array(values) => {
            output.push(b'[');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                write_canonical_json(value, output)?;
            }
            output.push(b']');
        }
        serde_json::Value::Object(values) => {
            output.push(b'{');
            let mut keys = values.keys().collect::<Vec<_>>();
            keys.sort_unstable();
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key).map_err(|error| {
                    invalid_component(format!(
                        "Cannot encode canonical Word PDF render runtime key: {error}"
                    ))
                })?;
                output.push(b':');
                write_canonical_json(&values[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

fn verify_component_tree(
    root: &Path,
    receipt: &ComponentReceipt,
) -> Result<VerifiedComponentTree, OfficeEngineError> {
    let actual = collect_component_tree(root)?;
    let expected = receipt
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    if actual.files.len() != expected.len()
        || actual
            .files
            .keys()
            .map(String::as_str)
            .ne(expected.keys().copied())
    {
        return Err(integrity_error(
            "The Word PDF render runtime file set differs from its frozen receipt.",
        ));
    }
    let mut verified = BTreeMap::new();
    for (relative, path) in actual.files {
        let frozen = expected.get(relative.as_str()).ok_or_else(|| {
            integrity_error("The Word PDF render runtime contains an unrecognized file.")
        })?;
        let identity = verify_file(&path, frozen.size, &frozen.sha256)?;
        verified.insert(
            relative,
            VerifiedFile {
                #[cfg(not(unix))]
                size: frozen.size,
                #[cfg(not(unix))]
                sha256: frozen.sha256.clone(),
                identity,
            },
        );
    }
    let expected_links = receipt
        .links
        .iter()
        .map(|link| (link.path.as_str(), link))
        .collect::<BTreeMap<_, _>>();
    if actual.links.len() != expected_links.len()
        || actual
            .links
            .keys()
            .map(String::as_str)
            .ne(expected_links.keys().copied())
    {
        return Err(integrity_error(
            "The Word PDF render runtime symlink set differs from its frozen receipt.",
        ));
    }
    let mut verified_links = BTreeMap::new();
    for (relative, link) in actual.links {
        let frozen = expected_links.get(relative.as_str()).ok_or_else(|| {
            integrity_error("The Word PDF render runtime contains an unrecognized symlink.")
        })?;
        if link.target != frozen.target {
            return Err(integrity_error(
                "A Word PDF render runtime symlink target differs from its frozen receipt.",
            ));
        }
        verified_links.insert(
            relative,
            VerifiedLink {
                target: link.target,
                identity: link.identity,
                resolved_identity: link.resolved_identity,
            },
        );
    }
    Ok((verified, verified_links))
}

fn collect_component_tree(root: &Path) -> Result<CollectedTree, OfficeEngineError> {
    let mut files = BTreeMap::new();
    let mut links = BTreeMap::new();
    let canonical_root = root.canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot canonicalize Word PDF render runtime root: {error}"
        ))
    })?;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            invalid_component(format!("Cannot enumerate Word PDF render runtime: {error}"))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                invalid_component(format!("Cannot enumerate Word PDF render runtime: {error}"))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                invalid_component(format!(
                    "Cannot inspect Word PDF render runtime entry: {error}"
                ))
            })?;
            if metadata.file_type().is_symlink() {
                let relative = path.strip_prefix(root).map_err(|_| {
                    invalid_component("Word PDF render runtime symlink escaped its component root.")
                })?;
                let relative = normalized_relative_path(relative)?;
                let frozen = freeze_safe_link_with_hook(&canonical_root, &path, &relative, || {})?;
                if links.insert(relative, frozen).is_some()
                    || files.len().saturating_add(links.len()) > MAX_COMPONENT_FILES
                {
                    return Err(invalid_component(
                        "The Word PDF render runtime contains too many or duplicate entries.",
                    ));
                }
                continue;
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(invalid_component(
                    "The Word PDF render runtime can contain only regular files and directories.",
                ));
            }
            let relative = path.strip_prefix(root).map_err(|_| {
                invalid_component("Word PDF render runtime entry escaped its component root.")
            })?;
            let relative = normalized_relative_path(relative)?;
            if relative == COMPONENT_RECEIPT {
                continue;
            }
            if files.insert(relative, path).is_some()
                || files.len().saturating_add(links.len()) > MAX_COMPONENT_FILES
            {
                return Err(invalid_component(
                    "The Word PDF render runtime contains too many or duplicate files.",
                ));
            }
        }
    }
    Ok(CollectedTree { files, links })
}

fn resolve_component_file(root: &Path, relative: &str) -> Result<PathBuf, OfficeEngineError> {
    validate_relative_path(relative)?;
    let canonical = root.join(relative).canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot resolve Word PDF render runtime file: {error}"
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(invalid_component(
            "Word PDF render runtime file resolves outside its component root.",
        ));
    }
    Ok(canonical)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<FileIdentity, OfficeEngineError> {
    verify_file_with_hook(path, expected_size, expected_sha256, || {})
}

fn verify_file_with_hook<F: FnOnce()>(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
    after_hash: F,
) -> Result<FileIdentity, OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        integrity_error(format!(
            "Cannot inspect Word PDF render runtime file: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected_size
        || metadata.len() > MAX_COMPONENT_FILE_BYTES
    {
        return Err(integrity_error(
            "A Word PDF render runtime file no longer matches its frozen size or type.",
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        integrity_error(format!("Cannot open Word PDF render runtime file: {error}"))
    })?;
    let opened_before = file.metadata().map_err(|error| {
        integrity_error(format!(
            "Cannot inspect opened Word PDF render runtime file: {error}"
        ))
    })?;
    let identity = file_identity(&metadata);
    if file_identity(&opened_before) != identity {
        return Err(integrity_error(
            "A Word PDF render runtime file changed while it was opened for verification.",
        ));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            integrity_error(format!("Cannot hash Word PDF render runtime file: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_COMPONENT_FILE_BYTES {
            return Err(integrity_error(
                "A Word PDF render runtime file exceeds its per-file safety limit.",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if total != expected_size || hex_lower(&digest.finalize()) != expected_sha256 {
        return Err(integrity_error(
            "A Word PDF render runtime file failed SHA-256 verification.",
        ));
    }
    after_hash();
    let opened_after = file.metadata().map_err(|error| {
        integrity_error(format!(
            "Cannot reinspect opened Word PDF render runtime file: {error}"
        ))
    })?;
    let path_after = fs::symlink_metadata(path).map_err(|error| {
        integrity_error(format!(
            "Cannot reinspect Word PDF render runtime path: {error}"
        ))
    })?;
    if file_identity(&opened_after) != identity
        || path_after.file_type().is_symlink()
        || !path_after.is_file()
        || file_identity(&path_after) != identity
    {
        return Err(integrity_error(
            "A Word PDF render runtime file changed during identity verification.",
        ));
    }
    Ok(identity)
}

fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        FileIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
            ctime_seconds: metadata.ctime(),
            ctime_nanoseconds: metadata.ctime_nsec(),
            size: metadata.len(),
        }
    }
    #[cfg(not(unix))]
    {
        FileIdentity {
            size: metadata.len(),
        }
    }
}

fn validate_relative_path(value: &str) -> Result<(), OfficeEngineError> {
    if value.is_empty() || value.contains('\\') || value.as_bytes().contains(&0) {
        return Err(invalid_component(
            "Word PDF render runtime paths must be normalized POSIX-relative paths.",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_component(
            "Word PDF render runtime receipt contains an unsafe relative path.",
        ));
    }
    Ok(())
}

fn validate_safe_link(path: &str, target: &str) -> Result<(), OfficeEngineError> {
    if target.is_empty()
        || target.trim() != target
        || target.contains('\\')
        || target.as_bytes().contains(&0)
        || target.starts_with('/')
    {
        return Err(invalid_component(format!(
            "Word PDF render runtime symlink `{path}` must use a relative normalized target."
        )));
    }
    let mut stack = path.split('/').collect::<Vec<_>>();
    stack.pop();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return Err(invalid_component(format!(
                        "Word PDF render runtime symlink `{path}` escapes its component root."
                    )));
                }
            }
            value => stack.push(value),
        }
    }
    if stack.is_empty() {
        return Err(invalid_component(format!(
            "Word PDF render runtime symlink `{path}` resolves to its component root."
        )));
    }
    Ok(())
}

fn freeze_safe_link_with_hook<F: FnOnce()>(
    canonical_root: &Path,
    path: &Path,
    relative: &str,
    after_first_snapshot: F,
) -> Result<CollectedLink, OfficeEngineError> {
    let before = fs::symlink_metadata(path).map_err(|error| {
        invalid_component(format!(
            "Cannot inspect Word PDF render runtime symlink: {error}"
        ))
    })?;
    if !before.file_type().is_symlink() {
        return Err(invalid_component(
            "A Word PDF render runtime symlink changed type during verification.",
        ));
    }
    let identity = file_identity(&before);
    let target = fs::read_link(path).map_err(|error| {
        invalid_component(format!(
            "Cannot read Word PDF render runtime symlink: {error}"
        ))
    })?;
    let target = target.to_str().ok_or_else(|| {
        invalid_component("Word PDF render runtime symlink targets must be UTF-8.")
    })?;
    validate_safe_link(relative, target)?;
    let resolved = path.canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot resolve Word PDF render runtime symlink: {error}"
        ))
    })?;
    if !resolved.starts_with(canonical_root) {
        return Err(invalid_component(
            "A Word PDF render runtime symlink resolves outside its component root.",
        ));
    }
    let resolved_identity = file_identity(&fs::metadata(&resolved).map_err(|error| {
        invalid_component(format!(
            "Cannot inspect Word PDF render runtime symlink target: {error}"
        ))
    })?);

    after_first_snapshot();

    let after = fs::symlink_metadata(path).map_err(|error| {
        integrity_error(format!(
            "Cannot reinspect Word PDF render runtime symlink: {error}"
        ))
    })?;
    let target_after = fs::read_link(path).map_err(|error| {
        integrity_error(format!(
            "Cannot reread Word PDF render runtime symlink: {error}"
        ))
    })?;
    let resolved_after = path.canonicalize().map_err(|error| {
        integrity_error(format!(
            "Cannot reresolve Word PDF render runtime symlink: {error}"
        ))
    })?;
    let resolved_identity_after =
        file_identity(&fs::metadata(&resolved_after).map_err(|error| {
            integrity_error(format!(
                "Cannot reinspect Word PDF render runtime symlink target: {error}"
            ))
        })?);
    if !after.file_type().is_symlink()
        || file_identity(&after) != identity
        || target_after != *target
        || resolved_after != resolved
        || !resolved_after.starts_with(canonical_root)
        || resolved_identity_after != resolved_identity
    {
        return Err(integrity_error(
            "A Word PDF render runtime symlink or its resolved target changed during verification.",
        ));
    }
    Ok(CollectedLink {
        target: target.to_string(),
        identity,
        resolved_identity,
    })
}

fn normalized_relative_path(path: &Path) -> Result<String, OfficeEngineError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(invalid_component(
                "Word PDF render runtime contains a non-normalized path.",
            ));
        };
        parts.push(value.to_str().ok_or_else(|| {
            invalid_component("Word PDF render runtime paths must be valid UTF-8.")
        })?);
    }
    if parts.is_empty() {
        return Err(invalid_component(
            "Word PDF render runtime contains an empty path.",
        ));
    }
    Ok(parts.join("/"))
}

fn current_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    }
}

fn word_pdf_render_runtime_supported() -> bool {
    cfg!(target_os = "macos")
        || cfg!(target_os = "linux")
        || cfg!(target_os = "windows")
        || cfg!(test)
}

fn current_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn expected_executable() -> &'static str {
    match current_platform() {
        "darwin" => "libreoffice/LibreOffice.app/Contents/MacOS/soffice",
        "win32" => "libreoffice/program/soffice.exe",
        _ => "libreoffice/program/soffice",
    }
}

fn expected_license() -> &'static str {
    match current_platform() {
        "darwin" => "libreoffice/LibreOffice.app/Contents/Resources/LICENSE",
        _ => "libreoffice/LICENSE",
    }
}

fn expected_notice() -> &'static str {
    match current_platform() {
        "darwin" => "libreoffice/LibreOffice.app/Contents/Resources/NOTICE",
        _ => "libreoffice/NOTICE",
    }
}

fn expected_archive_receipt() -> ArchiveReceipt {
    expected_archive_receipt_for(current_platform(), current_arch())
}

fn expected_archive_receipt_for(platform: &str, arch: &str) -> ArchiveReceipt {
    let (url, size, sha256) = match (platform, arch) {
        ("darwin", "arm64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/mac/aarch64/LibreOffice_26.2.4.2_MacOS_aarch64.dmg",
            295_019_039,
            "64e0ad05564554eeee639d49b08b20908a38d4722ec95f1620d05c99bcbe9fb1",
        ),
        ("darwin", "x64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/mac/x86_64/LibreOffice_26.2.4.2_MacOS_x86-64.dmg",
            305_573_151,
            "f92ba40fdada173232fe929bf77973a1ffcccec55ae7971957a6de84d33f0f1e",
        ),
        ("linux", "arm64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/deb/aarch64/LibreOffice_26.2.4.2_Linux_aarch64_deb.tar.gz",
            206_721_044,
            "038d9d6c9045094f90d26b443c5f76f7ef09ffd8f81de6a2b89c09a37a7bc6b9",
        ),
        ("linux", "x64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/deb/x86_64/LibreOffice_26.2.4.2_Linux_x86-64_deb.tar.gz",
            217_982_126,
            "810ef197e190d7804a60e0016052c46ff33792303a200fddda9d5216a64b9900",
        ),
        ("win32", "arm64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/win/aarch64/LibreOffice_26.2.4.2_Win_aarch64.msi",
            358_891_520,
            "1cd35d4d2821f6b6e7e65a2fc7c0faa2b5074ecd0ad90c5eb30af8a4f86d3b0d",
        ),
        ("win32", "x64") => (
            "https://downloadarchive.documentfoundation.org/libreoffice/old/26.2.4.2/win/x86_64/LibreOffice_26.2.4.2_Win_x86-64.msi",
            372_539_392,
            "202f26cda071c5aa4996a5a28412fddceb3891dceb0366982c62650456c0730f",
        ),
        _ => panic!("Word PDF renderer supports only fixed desktop targets: {platform}-{arch}"),
    };
    ArchiveReceipt {
        url: url.to_string(),
        size,
        sha256: sha256.to_string(),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(windows)]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    true
}

fn unavailable_error() -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::RenderBackendUnavailable,
        OfficeEngineRecovery::InstallComponent,
        "The application-managed Word PDF render runtime is unavailable.",
    )
}

fn invalid_component(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::RenderBackendInvalid,
        OfficeEngineRecovery::InstallComponent,
        message,
    )
}

fn integrity_error(message: impl Into<String>) -> OfficeEngineError {
    invalid_component(message)
}

#[cfg(test)]
pub(super) fn write_test_word_pdf_render_runtime(root: &Path) -> PathBuf {
    write_test_word_pdf_render_runtime_with_executable(
        root,
        b"#!/bin/sh\necho 'LibreOffice 26.2.4.2'\n",
    )
}

#[cfg(test)]
pub(super) fn write_test_word_pdf_render_runtime_with_executable(
    root: &Path,
    executable_bytes: &[u8],
) -> PathBuf {
    let executable = expected_executable();
    let executable_path = root.join(executable);
    fs::create_dir_all(executable_path.parent().expect("test executable parent")).unwrap();
    fs::write(&executable_path, executable_bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&executable_path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    for (relative, bytes) in [
        (expected_license(), b"MPL-2.0\n".as_slice()),
        (expected_notice(), b"LibreOffice notices\n".as_slice()),
    ] {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().expect("test notice parent")).unwrap();
        fs::write(path, bytes).unwrap();
    }
    let CollectedTree {
        files: collected_files,
        links: collected_links,
    } = collect_component_tree(root).unwrap();
    let mut files = collected_files
        .into_iter()
        .map(|(path, absolute)| {
            let bytes = fs::read(absolute).unwrap();
            FileReceipt {
                path,
                size: bytes.len() as u64,
                sha256: hex_lower(&Sha256::digest(&bytes)),
            }
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let mut receipt = ComponentReceipt {
        schema_version: 1,
        provider_id: WORD_PDF_RENDER_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: WORD_PDF_RENDER_RUNTIME_BUNDLE_VERSION.to_string(),
        platform: current_platform().to_string(),
        arch: current_arch().to_string(),
        runtime: RuntimeReceipt {
            family: "libreoffice".to_string(),
            version: WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION.to_string(),
            executable: executable.to_string(),
            license: expected_license().to_string(),
            notice: expected_notice().to_string(),
        },
        archive: expected_archive_receipt(),
        files,
        links: collected_links
            .into_iter()
            .map(|(path, link)| LinkReceipt {
                path,
                target: link.target,
            })
            .collect(),
        bundle_revision: String::new(),
    };
    receipt.bundle_revision = compute_bundle_revision(&receipt).unwrap();
    fs::write(
        root.join(COMPONENT_RECEIPT),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    executable_path.canonicalize().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_archive_contract_matches_every_cross_platform_manifest_target() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../resources/word-pdf-renderer-manifest.json"
        ))
        .unwrap();
        for (platform, arch) in [
            ("darwin", "arm64"),
            ("darwin", "x64"),
            ("linux", "arm64"),
            ("linux", "x64"),
            ("win32", "arm64"),
            ("win32", "x64"),
        ] {
            let target = &manifest["targets"][format!("{platform}-{arch}")]["archive"];
            let expected = expected_archive_receipt_for(platform, arch);
            assert_eq!(target["url"], expected.url);
            assert_eq!(target["size"], expected.size);
            assert_eq!(target["sha256"], expected.sha256);
        }
    }

    #[test]
    fn discovers_and_reverifies_a_frozen_word_pdf_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_test_word_pdf_render_runtime(directory.path());
        let runtime = WordPdfRenderRuntime::discover(
            &WordPdfRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap();
        assert_eq!(runtime.executable_path(), executable);
        assert_eq!(
            runtime.libreoffice_version(),
            WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION
        );
        assert!(runtime
            .runtime_revision()
            .starts_with(RUNTIME_REVISION_PREFIX));
        runtime.verify_integrity().unwrap();
    }

    #[test]
    fn rejects_mutated_or_workspace_owned_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let executable = write_test_word_pdf_render_runtime(directory.path());
        let runtime = WordPdfRenderRuntime::discover(
            &WordPdfRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap();
        fs::write(executable, b"changed").unwrap();
        assert_eq!(
            runtime.verify_integrity().unwrap_err().code(),
            OfficeEngineErrorCode::RenderBackendInvalid
        );

        let error = WordPdfRenderRuntime::discover(
            &WordPdfRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path())
                .with_workspace_root(directory.path()),
        )
        .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::RenderBackendInvalid);
    }

    #[cfg(unix)]
    #[test]
    fn freezes_safe_relative_symlinks_and_rejects_retargeting_outside_the_component() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let link_directory = directory.path().join("libreoffice/share");
        fs::create_dir_all(&link_directory).unwrap();
        fs::write(link_directory.join("target.dat"), b"target").unwrap();
        let link = link_directory.join("current.dat");
        symlink("target.dat", &link).unwrap();
        write_test_word_pdf_render_runtime(directory.path());
        let runtime = WordPdfRenderRuntime::discover(
            &WordPdfRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap();
        runtime.verify_integrity().unwrap();

        fs::remove_file(&link).unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        symlink(outside.path(), &link).unwrap();
        assert_eq!(
            runtime.verify_integrity().unwrap_err().code(),
            OfficeEngineErrorCode::RenderBackendInvalid
        );
    }

    #[test]
    fn rejects_absolute_and_parent_escape_symlink_targets() {
        assert!(validate_safe_link("libreoffice/share/link", "target.dat").is_ok());
        assert!(validate_safe_link("libreoffice/share/link", "../program/resource.dat").is_ok());
        assert!(validate_safe_link("libreoffice/share/link", "/tmp/escape").is_err());
        assert!(validate_safe_link("libreoffice/share/link", "../../../escape").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_path_replacement_between_hashing_and_identity_freeze() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("runtime-file");
        let replacement = directory.path().join("replacement");
        fs::write(&path, b"trusted").unwrap();
        fs::write(&replacement, b"hostile").unwrap();
        let expected = hex_lower(&Sha256::digest(b"trusted"));
        let error = verify_file_with_hook(&path, 7, &expected, || {
            fs::rename(&replacement, &path).unwrap();
        })
        .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::RenderBackendInvalid);
    }

    #[test]
    #[ignore = "requires MYCOPILOT_WORD_PDF_RENDERER_DIR to point to a prepared component"]
    fn discovers_the_real_prepared_component() {
        let directory = std::env::var_os("MYCOPILOT_WORD_PDF_RENDERER_DIR")
            .expect("MYCOPILOT_WORD_PDF_RENDERER_DIR is required");
        let runtime = WordPdfRenderRuntime::discover(
            &WordPdfRenderRuntimeDiscoveryOptions::new().with_configured_component_dir(directory),
        )
        .unwrap();
        assert_eq!(
            runtime.libreoffice_version(),
            WORD_PDF_RENDER_RUNTIME_LIBREOFFICE_VERSION
        );
        runtime.verify_integrity().unwrap();
    }
}
