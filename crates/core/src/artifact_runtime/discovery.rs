use super::types::{
    ArtifactRuntimeAvailability, ArtifactRuntimeBundleStatus, ArtifactRuntimeDependency,
    ArtifactRuntimeError, ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeKind,
    ArtifactRuntimePreflight, ArtifactRuntimeRecovery, ArtifactRuntimeRequirement,
    ArtifactRuntimeSource, ArtifactRuntimeStatus, ARTIFACT_RUNTIME_BUNDLE_STATUS_SCHEMA_VERSION,
    ARTIFACT_RUNTIME_PROVIDER_ID,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const COMPONENT_DIRECTORY: &str = "artifact-runtime";
const COMPONENT_RECEIPT: &str = "component-receipt.json";
const BUNDLE_REVISION_PREFIX: &str = "artifact-runtime-bundle-sha256-v1:";
const BUILD_INPUTS_REVISION_PREFIX: &str = "artifact-runtime-build-inputs-sha256-v1:";
const RUNTIME_FINGERPRINT_PREFIX: &str = "artifact-runtime-sha256-v1:";
const MAX_RECEIPT_BYTES: u64 = 16 * 1024 * 1024;
const MAX_COMPONENT_FILES: usize = 100_000;
const MAX_COMPONENT_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const SHA256_HEX_LENGTH: usize = 64;

pub const ARTIFACT_RUNTIME_BUNDLE_VERSION: &str = "2026.08.2";
pub const ARTIFACT_RUNTIME_NODE_VERSION: &str = "22.23.1";
pub const ARTIFACT_RUNTIME_PYTHON_VERSION: &str = "3.12.13";
pub const ARTIFACT_RUNTIME_RIPGREP_VERSION: &str = "15.1.0";

const EXPECTED_NODE_DEPENDENCIES: &[(&str, &str)] = &[
    ("docx", "9.6.1"),
    ("exceljs", "4.4.0"),
    ("pptxgenjs", "4.0.1"),
];
const EXPECTED_PYTHON_DEPENDENCIES: &[(&str, &str)] = &[
    ("openpyxl", "3.1.5"),
    ("pdfplumber", "0.11.9"),
    ("pypdf", "6.15.0"),
    ("pypdfium2", "5.12.1"),
    ("python-docx", "1.2.0"),
    ("python-pptx", "1.0.2"),
    ("reportlab", "4.4.9"),
    ("xlsxwriter", "3.2.9"),
];
const EXPECTED_PDF_CLI_PATH: &str = "runtime/pdf-runtime-cli.py";
const EXPECTED_RIPGREP_LICENSES: &[&str] = &[
    "legal/ripgrep/COPYING",
    "legal/ripgrep/LICENSE-MIT",
    "legal/ripgrep/UNLICENSE",
];

#[derive(Debug, Clone, Default)]
pub struct ArtifactRuntimeDiscoveryOptions {
    configured_component_dir: Option<PathBuf>,
    application_resources_dir: Option<PathBuf>,
}

impl ArtifactRuntimeDiscoveryOptions {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_configured_component_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        // This is an explicit development/integration trust override. Packaged
        // desktop builds must use `with_application_resources_dir` so the
        // application signature and resource boundary remain authoritative.
        self.configured_component_dir = Some(directory.into());
        self
    }

    pub fn with_application_resources_dir(mut self, directory: impl Into<PathBuf>) -> Self {
        self.application_resources_dir = Some(directory.into());
        self
    }
}

#[derive(Debug, Clone)]
pub struct ArtifactRuntimeProvider {
    root: PathBuf,
    source: ArtifactRuntimeSource,
    receipt: ComponentReceipt,
    files: BTreeMap<String, VerifiedFile>,
}

impl ArtifactRuntimeProvider {
    pub fn discover(
        options: &ArtifactRuntimeDiscoveryOptions,
    ) -> Result<Self, ArtifactRuntimeError> {
        let (root, source) = resolve_component_root(options)?;
        let receipt = read_receipt(&root)?;
        validate_receipt_contract(&receipt)?;
        let files = verify_component_tree(&root, &receipt)?;
        validate_tool_executable_files(&receipt, &files)?;
        Ok(Self {
            root,
            source,
            receipt,
            files,
        })
    }

    pub fn inspect(options: &ArtifactRuntimeDiscoveryOptions) -> ArtifactRuntimeBundleStatus {
        match Self::discover(options) {
            Ok(provider) => provider.status(),
            Err(error) => ArtifactRuntimeBundleStatus {
                schema_version: ARTIFACT_RUNTIME_BUNDLE_STATUS_SCHEMA_VERSION,
                provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
                availability: ArtifactRuntimeAvailability::Unavailable,
                source: None,
                bundle_version: None,
                bundle_revision: None,
                runtimes: [ArtifactRuntimeKind::Node, ArtifactRuntimeKind::Python]
                    .into_iter()
                    .map(|kind| unavailable_runtime_status(kind, &error))
                    .collect(),
                error_code: Some(error.code().stable_name().to_string()),
                recovery: Some(error.recovery().stable_name().to_string()),
                message: Some(error.message().to_string()),
            },
        }
    }

    pub fn component_root(&self) -> &Path {
        &self.root
    }

    pub fn source(&self) -> ArtifactRuntimeSource {
        self.source
    }

    pub fn bundle_version(&self) -> &str {
        &self.receipt.bundle_version
    }

    pub fn bundle_revision(&self) -> &str {
        &self.receipt.bundle_revision
    }

    pub fn status(&self) -> ArtifactRuntimeBundleStatus {
        ArtifactRuntimeBundleStatus {
            schema_version: ARTIFACT_RUNTIME_BUNDLE_STATUS_SCHEMA_VERSION,
            provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            availability: ArtifactRuntimeAvailability::Available,
            source: Some(self.source),
            bundle_version: Some(self.receipt.bundle_version.clone()),
            bundle_revision: Some(self.receipt.bundle_revision.clone()),
            runtimes: [ArtifactRuntimeKind::Node, ArtifactRuntimeKind::Python]
                .into_iter()
                .map(|kind| self.runtime_status(kind))
                .collect(),
            error_code: None,
            recovery: None,
            message: None,
        }
    }

    pub fn runtime_status(&self, kind: ArtifactRuntimeKind) -> ArtifactRuntimeStatus {
        let runtime = self.receipt.runtime(kind);
        ArtifactRuntimeStatus {
            kind,
            availability: ArtifactRuntimeAvailability::Available,
            version: Some(runtime.version.clone()),
            dependencies: runtime
                .dependencies
                .iter()
                .map(|dependency| ArtifactRuntimeDependency {
                    name: dependency.name.clone(),
                    version: dependency.version.clone(),
                })
                .collect(),
            runtime_fingerprint: Some(runtime_fingerprint(
                &self.receipt.bundle_revision,
                kind,
                runtime,
            )),
            error_code: None,
            recovery: None,
            message: None,
        }
    }

    /// Returns the receipt-verified ripgrep executable. This never consults ambient PATH and
    /// revalidates the complete component immediately before handing the path to a command host.
    pub fn ripgrep_executable(&self) -> Result<PathBuf, ArtifactRuntimeError> {
        self.reverify_bundle_integrity()?;
        self.file_path(&self.receipt.tools.ripgrep.executable)
    }

    /// Returns the receipt-verified PDF compatibility CLI consumed by managed Python.
    pub fn pdf_cli_path(&self) -> Result<PathBuf, ArtifactRuntimeError> {
        self.reverify_bundle_integrity()?;
        self.file_path(&self.receipt.tools.pdf_cli.path)
    }

    /// Revalidates the complete component file set and the immutable identities captured by the
    /// receipt-backed discovery pass. Command hosts should call this immediately before spawn and
    /// again after process completion so an integrity loss is recorded even when the child exits
    /// successfully.
    ///
    /// Discovery performs the full SHA-256 pass. Rechecking thousands of package files by hash on
    /// every command would add tens of seconds of latency, so Unix pre/postflight uses kernel-owned
    /// device, inode, size, and nanosecond ctime identities plus an exact file-set walk. An
    /// unprivileged writer cannot restore ctime after changing bytes. Platforms without an
    /// equivalent identity retain the conservative full-hash fallback.
    pub fn verify_integrity(&self) -> Result<(), ArtifactRuntimeError> {
        self.reverify_bundle_integrity()
    }

    /// Resolves a revision-bound invocation without starting the runtime or
    /// installing dependencies. A missing package is a normal preflight
    /// outcome; component corruption remains an error.
    pub fn preflight(
        &self,
        kind: ArtifactRuntimeKind,
        requirements: &[ArtifactRuntimeRequirement],
    ) -> Result<ArtifactRuntimePreflight, ArtifactRuntimeError> {
        if requirements.len() > 128 {
            return Err(ArtifactRuntimeError::new(
                ArtifactRuntimeErrorCode::InvalidRequest,
                ArtifactRuntimeRecovery::ChangeRequest,
                "Artifact runtime preflight accepts at most 128 dependency requirements.",
            ));
        }
        let runtime = self.receipt.runtime(kind);
        // The provider was cryptographically verified at discovery. Revalidate the complete file
        // set and every captured immutable identity immediately before issuing an invocation.
        self.reverify_bundle_integrity()?;

        let installed = runtime
            .dependencies
            .iter()
            .map(|dependency| {
                (
                    normalize_dependency_name(kind, &dependency.name),
                    dependency.version.as_str(),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let missing = requirements
            .iter()
            .filter(|requirement| {
                installed
                    .get(&normalize_dependency_name(kind, requirement.name()))
                    .is_none_or(|installed_version| {
                        requirement
                            .version()
                            .is_some_and(|required| required != *installed_version)
                    })
            })
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Ok(ArtifactRuntimePreflight::MissingDependencies {
                status: self.runtime_status(kind),
                missing,
            });
        }

        Ok(ArtifactRuntimePreflight::Ready(
            self.build_invocation(kind, runtime)?,
        ))
    }

    fn build_invocation(
        &self,
        kind: ArtifactRuntimeKind,
        runtime: &RuntimeReceipt,
    ) -> Result<ArtifactRuntimeInvocation, ArtifactRuntimeError> {
        let executable = self.file_path(&runtime.executable)?;
        let mut arguments_prefix = Vec::new();
        let mut environment = BTreeMap::new();
        let fingerprint = runtime_fingerprint(&self.receipt.bundle_revision, kind, runtime);

        environment.insert(
            OsString::from("MYCOPILOT_ARTIFACT_RUNTIME_PROVIDER"),
            OsString::from(ARTIFACT_RUNTIME_PROVIDER_ID),
        );
        environment.insert(
            OsString::from("MYCOPILOT_ARTIFACT_RUNTIME_REVISION"),
            OsString::from(&self.receipt.bundle_revision),
        );
        environment.insert(
            OsString::from("MYCOPILOT_ARTIFACT_RUNTIME_FINGERPRINT"),
            OsString::from(&fingerprint),
        );

        match kind {
            ArtifactRuntimeKind::Node => {
                let package_root =
                    self.directory_path(runtime.package_root.as_deref().ok_or_else(|| {
                        invalid_component("Node runtime is missing packageRoot.")
                    })?)?;
                let bootstrap =
                    self.file_path(runtime.bootstrap.as_deref().ok_or_else(|| {
                        invalid_component("Node runtime is missing its module bootstrap.")
                    })?)?;
                arguments_prefix.push(OsString::from("--import"));
                arguments_prefix.push(bootstrap.into_os_string());
                environment.insert(OsString::from("NODE_NO_WARNINGS"), OsString::from("1"));
                environment.insert(
                    OsString::from("MYCOPILOT_ARTIFACT_NODE_MODULES"),
                    package_root.into_os_string(),
                );
            }
            ArtifactRuntimeKind::Python => {
                let runtime_home =
                    self.directory_path(runtime.runtime_home.as_deref().ok_or_else(|| {
                        invalid_component("Python runtime is missing runtimeHome.")
                    })?)?;
                arguments_prefix.push(OsString::from("-I"));
                arguments_prefix.push(OsString::from("-B"));
                environment.insert(OsString::from("PYTHONNOUSERSITE"), OsString::from("1"));
                environment.insert(
                    OsString::from("PYTHONDONTWRITEBYTECODE"),
                    OsString::from("1"),
                );
                environment.insert(OsString::from("PYTHONUTF8"), OsString::from("1"));
                environment.insert(OsString::from("PIP_NO_INDEX"), OsString::from("1"));
                environment.insert(
                    OsString::from("MYCOPILOT_ARTIFACT_PYTHON_HOME"),
                    runtime_home.into_os_string(),
                );
            }
        }

        Ok(ArtifactRuntimeInvocation::new(
            self.receipt.bundle_version.clone(),
            self.receipt.bundle_revision.clone(),
            fingerprint,
            kind,
            runtime.version.clone(),
            executable,
            arguments_prefix,
            environment,
        ))
    }

    fn reverify_bundle_integrity(&self) -> Result<(), ArtifactRuntimeError> {
        // Re-enumerate on every execution boundary on every platform. Verifying only the files
        // remembered at discovery would miss a newly-added JavaScript or Python module that the
        // runtime could import even though it was never covered by the signed receipt.
        let actual = collect_component_files(&self.root)?;
        if actual.len() != self.files.len() || actual.keys().ne(self.files.keys()) {
            return Err(ArtifactRuntimeError::new(
                ArtifactRuntimeErrorCode::IntegrityMismatch,
                ArtifactRuntimeRecovery::RepairComponent,
                "Artifact runtime component file set changed after discovery.",
            ));
        }
        for (relative, expected) in &self.files {
            let path = actual
                .get(relative)
                .ok_or_else(|| integrity_error(&expected.path))?;
            #[cfg(unix)]
            {
                expected.identity.verify(path)?;
            }
            #[cfg(not(unix))]
            {
                // Modified/created timestamps are not a sufficiently strong immutable identity on
                // all supported non-Unix filesystems. Preserve the cryptographic fallback until a
                // native volume/file-id + change-journal implementation is available.
                expected.identity.verify(path)?;
                verify_file(path, expected.size, &expected.sha256)?;
            }
        }
        Ok(())
    }

    fn file_path(&self, relative: &str) -> Result<PathBuf, ArtifactRuntimeError> {
        self.files
            .get(relative)
            .map(|file| file.path.clone())
            .ok_or_else(|| {
                invalid_component(format!(
                    "Artifact runtime file `{relative}` is absent from the verified receipt."
                ))
            })
    }

    fn directory_path(&self, relative: &str) -> Result<PathBuf, ArtifactRuntimeError> {
        validate_relative_path(relative)?;
        let path = resolve_without_symlinks(&self.root, relative, ExpectedEntry::Directory)?;
        Ok(path)
    }
}

pub fn artifact_runtime_component_relative_path() -> PathBuf {
    PathBuf::from(COMPONENT_DIRECTORY)
}

fn resolve_component_root(
    options: &ArtifactRuntimeDiscoveryOptions,
) -> Result<(PathBuf, ArtifactRuntimeSource), ArtifactRuntimeError> {
    if let Some(configured) = options.configured_component_dir.as_deref() {
        if !configured.is_absolute() {
            return Err(invalid_component(
                "The configured artifact runtime component directory must be absolute.",
            ));
        }
        return canonical_component_root(configured)
            .map(|root| (root, ArtifactRuntimeSource::ConfiguredComponent));
    }
    if let Some(resources) = options.application_resources_dir.as_deref() {
        if !resources.is_absolute() {
            return Err(invalid_component(
                "The application resources directory must be absolute.",
            ));
        }
        let resources = canonical_component_root(resources)?;
        let candidate = resources.join(COMPONENT_DIRECTORY);
        if !candidate.exists() {
            return Err(unavailable_error(format!(
                "Managed artifact runtime component is missing at `{}`.",
                candidate.display()
            )));
        }
        let root = canonical_component_root(&candidate)?;
        if !root.starts_with(&resources) {
            return Err(invalid_component(
                "The packaged artifact runtime resolves outside application resources.",
            ));
        }
        return Ok((root, ArtifactRuntimeSource::PackagedComponent));
    }
    Err(unavailable_error(
        "Managed artifact runtime is unavailable. Package the component or configure its absolute directory.",
    ))
}

fn canonical_component_root(path: &Path) -> Result<PathBuf, ArtifactRuntimeError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            unavailable_error(format!(
                "Artifact runtime component directory `{}` does not exist.",
                path.display()
            ))
        } else {
            io_error("inspect artifact runtime component directory", error)
        }
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid_component(
            "Artifact runtime component root must be a real directory, not a symlink.",
        ));
    }
    path.canonicalize()
        .map_err(|error| io_error("canonicalize artifact runtime component directory", error))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComponentReceipt {
    schema_version: u32,
    provider_id: String,
    bundle_version: String,
    build_inputs_revision: String,
    platform: String,
    arch: String,
    runtimes: RuntimeSet,
    tools: ToolSet,
    files: Vec<FileReceipt>,
    bundle_revision: String,
}

impl ComponentReceipt {
    fn runtime(&self, kind: ArtifactRuntimeKind) -> &RuntimeReceipt {
        match kind {
            ArtifactRuntimeKind::Node => &self.runtimes.node,
            ArtifactRuntimeKind::Python => &self.runtimes.python,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeSet {
    node: RuntimeReceipt,
    python: RuntimeReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ToolSet {
    pdf_cli: PdfCliReceipt,
    ripgrep: ExecutableToolReceipt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PdfCliReceipt {
    version: String,
    path: String,
    identity_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutableToolReceipt {
    version: String,
    executable: String,
    identity_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeReceipt {
    version: String,
    executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    package_root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_home: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bootstrap: Option<String>,
    dependencies: Vec<DependencyReceipt>,
    identity_files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DependencyReceipt {
    name: String,
    version: String,
    identity_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FileReceipt {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptRevisionPayload<'a> {
    schema_version: u32,
    provider_id: &'a str,
    bundle_version: &'a str,
    build_inputs_revision: &'a str,
    platform: &'a str,
    arch: &'a str,
    runtimes: &'a RuntimeSet,
    tools: &'a ToolSet,
    files: &'a [FileReceipt],
}

fn read_receipt(root: &Path) -> Result<ComponentReceipt, ArtifactRuntimeError> {
    let path = root.join(COMPONENT_RECEIPT);
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            unavailable_error("Artifact runtime component receipt is missing.")
        } else {
            io_error("inspect artifact runtime component receipt", error)
        }
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err(invalid_component(format!(
            "Artifact runtime receipt must be a regular file no larger than {MAX_RECEIPT_BYTES} bytes."
        )));
    }
    let bytes =
        fs::read(&path).map_err(|error| io_error("read artifact runtime receipt", error))?;
    if bytes.contains(&0) {
        return Err(invalid_component(
            "Artifact runtime component receipt contains a NUL byte.",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        invalid_component(format!(
            "Artifact runtime component receipt is not valid strict JSON: {error}"
        ))
    })
}

fn validate_receipt_contract(receipt: &ComponentReceipt) -> Result<(), ArtifactRuntimeError> {
    if receipt.schema_version != 3 {
        return Err(invalid_component(
            "Artifact runtime receipt schemaVersion must be 3.",
        ));
    }
    if receipt.provider_id != ARTIFACT_RUNTIME_PROVIDER_ID {
        return Err(invalid_component(format!(
            "Artifact runtime provider must be `{ARTIFACT_RUNTIME_PROVIDER_ID}`."
        )));
    }
    if receipt.bundle_version != ARTIFACT_RUNTIME_BUNDLE_VERSION {
        return Err(invalid_component(format!(
            "Artifact runtime bundle must be pinned to version {ARTIFACT_RUNTIME_BUNDLE_VERSION}."
        )));
    }
    let Some(build_inputs_digest) = receipt
        .build_inputs_revision
        .strip_prefix(BUILD_INPUTS_REVISION_PREFIX)
    else {
        return Err(invalid_component(format!(
            "Artifact runtime buildInputsRevision must use the `{BUILD_INPUTS_REVISION_PREFIX}` prefix."
        )));
    };
    validate_sha256(build_inputs_digest, "build inputs revision")?;
    if receipt.platform != current_platform() || receipt.arch != current_arch() {
        return Err(ArtifactRuntimeError::new(
            ArtifactRuntimeErrorCode::UnsupportedTarget,
            ArtifactRuntimeRecovery::InstallComponent,
            format!(
                "Artifact runtime component targets {}-{}, but this host is {}-{}.",
                receipt.platform,
                receipt.arch,
                current_platform(),
                current_arch()
            ),
        ));
    }
    validate_runtime_receipt(
        ArtifactRuntimeKind::Node,
        &receipt.runtimes.node,
        ARTIFACT_RUNTIME_NODE_VERSION,
        EXPECTED_NODE_DEPENDENCIES,
    )?;
    validate_runtime_receipt(
        ArtifactRuntimeKind::Python,
        &receipt.runtimes.python,
        ARTIFACT_RUNTIME_PYTHON_VERSION,
        EXPECTED_PYTHON_DEPENDENCIES,
    )?;
    validate_tool_receipts(&receipt.tools)?;
    if receipt.files.is_empty() || receipt.files.len() > MAX_COMPONENT_FILES {
        return Err(invalid_component(format!(
            "Artifact runtime receipt must contain between 1 and {MAX_COMPONENT_FILES} files."
        )));
    }
    let mut previous = None;
    let mut total = 0_u64;
    for file in &receipt.files {
        validate_relative_path(&file.path)?;
        if file.path == COMPONENT_RECEIPT {
            return Err(invalid_component(
                "Artifact runtime receipt cannot include itself in the component file set.",
            ));
        }
        if previous.is_some_and(|value: &str| value >= file.path.as_str()) {
            return Err(invalid_component(
                "Artifact runtime component files must use unique, canonical sorted paths.",
            ));
        }
        previous = Some(&file.path);
        validate_sha256(&file.sha256, "component file digest")?;
        if file.size > MAX_COMPONENT_FILE_BYTES {
            return Err(invalid_component(format!(
                "Artifact runtime file `{}` exceeds the per-file size limit.",
                file.path
            )));
        }
        total = total
            .checked_add(file.size)
            .ok_or_else(|| invalid_component("Artifact runtime component size overflowed."))?;
        if total > MAX_COMPONENT_BYTES {
            return Err(invalid_component(
                "Artifact runtime component exceeds the total size limit.",
            ));
        }
    }
    let expected_revision = compute_bundle_revision(receipt)?;
    if receipt.bundle_revision != expected_revision {
        return Err(ArtifactRuntimeError::new(
            ArtifactRuntimeErrorCode::IntegrityMismatch,
            ArtifactRuntimeRecovery::RepairComponent,
            "Artifact runtime bundle revision does not match its frozen receipt.",
        ));
    }
    Ok(())
}

fn validate_tool_receipts(tools: &ToolSet) -> Result<(), ArtifactRuntimeError> {
    if tools.pdf_cli.version != "1" {
        return Err(invalid_component(
            "Managed PDF CLI must be pinned to version 1.",
        ));
    }
    validate_relative_path(&tools.pdf_cli.path)?;
    if tools.pdf_cli.path != EXPECTED_PDF_CLI_PATH {
        return Err(invalid_component(
            "Managed PDF tool paths do not match the frozen bundle contract.",
        ));
    }
    validate_identity_files(
        &tools.pdf_cli.identity_files,
        std::iter::once(tools.pdf_cli.path.as_str()),
        "Managed PDF CLI",
    )?;

    if tools.ripgrep.version != ARTIFACT_RUNTIME_RIPGREP_VERSION {
        return Err(invalid_component(format!(
            "Managed ripgrep must be pinned to version {ARTIFACT_RUNTIME_RIPGREP_VERSION}."
        )));
    }
    validate_relative_path(&tools.ripgrep.executable)?;
    let expected_ripgrep = if current_platform() == "win32" {
        "dependencies/tools/rg.exe"
    } else {
        "dependencies/tools/rg"
    };
    if tools.ripgrep.executable != expected_ripgrep {
        return Err(invalid_component(
            "Managed ripgrep executable path does not match the frozen bundle contract.",
        ));
    }
    validate_identity_files(
        &tools.ripgrep.identity_files,
        std::iter::once(tools.ripgrep.executable.as_str())
            .chain(EXPECTED_RIPGREP_LICENSES.iter().copied()),
        "Managed ripgrep",
    )
}

fn validate_identity_files<'a>(
    identity_files: &[String],
    required: impl IntoIterator<Item = &'a str>,
    label: &str,
) -> Result<(), ArtifactRuntimeError> {
    if identity_files.is_empty() || identity_files.len() > 64 {
        return Err(invalid_component(format!(
            "{label} must declare between 1 and 64 identity files."
        )));
    }
    let mut identities = BTreeSet::new();
    for path in identity_files {
        validate_relative_path(path)?;
        if !identities.insert(path.as_str()) {
            return Err(invalid_component(format!(
                "{label} identity files must be unique."
            )));
        }
    }
    if required
        .into_iter()
        .any(|required| !identities.contains(required))
    {
        return Err(invalid_component(format!(
            "{label} identity files do not include every executable or resource entry."
        )));
    }
    Ok(())
}

fn validate_runtime_receipt(
    kind: ArtifactRuntimeKind,
    runtime: &RuntimeReceipt,
    expected_version: &str,
    expected_dependencies: &[(&str, &str)],
) -> Result<(), ArtifactRuntimeError> {
    if runtime.version != expected_version {
        return Err(invalid_component(format!(
            "Managed {} runtime must be pinned to version {expected_version}.",
            kind.stable_name()
        )));
    }
    validate_relative_path(&runtime.executable)?;
    match kind {
        ArtifactRuntimeKind::Node => {
            if runtime.runtime_home.is_some()
                || runtime.package_root.is_none()
                || runtime.bootstrap.is_none()
            {
                return Err(invalid_component(
                    "Node runtime requires packageRoot and bootstrap, and cannot declare runtimeHome.",
                ));
            }
            validate_relative_path(runtime.package_root.as_deref().unwrap())?;
            validate_relative_path(runtime.bootstrap.as_deref().unwrap())?;
        }
        ArtifactRuntimeKind::Python => {
            if runtime.runtime_home.is_none()
                || runtime.package_root.is_some()
                || runtime.bootstrap.is_some()
            {
                return Err(invalid_component(
                    "Python runtime requires runtimeHome and cannot declare packageRoot or bootstrap.",
                ));
            }
            validate_relative_path(runtime.runtime_home.as_deref().unwrap())?;
        }
    }
    if runtime.dependencies.len() != expected_dependencies.len() {
        return Err(invalid_component(format!(
            "Managed {} runtime dependency set is incomplete or contains unexpected packages.",
            kind.stable_name()
        )));
    }
    let expected = expected_dependencies
        .iter()
        .map(|(name, version)| (normalize_dependency_name(kind, name), *version))
        .collect::<BTreeMap<_, _>>();
    let mut actual = BTreeMap::new();
    for dependency in &runtime.dependencies {
        validate_dependency_text(&dependency.name, "name")?;
        validate_dependency_text(&dependency.version, "version")?;
        validate_relative_path(&dependency.identity_file)?;
        let normalized = normalize_dependency_name(kind, &dependency.name);
        if actual
            .insert(normalized, dependency.version.as_str())
            .is_some()
        {
            return Err(invalid_component(format!(
                "Managed {} runtime contains duplicate dependency names.",
                kind.stable_name()
            )));
        }
    }
    if actual != expected {
        return Err(invalid_component(format!(
            "Managed {} runtime dependencies do not match the pinned Office dependency set.",
            kind.stable_name()
        )));
    }
    if runtime.identity_files.is_empty() || runtime.identity_files.len() > 64 {
        return Err(invalid_component(
            "Each managed runtime must declare between 1 and 64 identity files.",
        ));
    }
    let mut identities = BTreeSet::new();
    for path in &runtime.identity_files {
        validate_relative_path(path)?;
        if !identities.insert(path.as_str()) {
            return Err(invalid_component(
                "Managed runtime identity files must be unique.",
            ));
        }
    }
    if !identities.contains(runtime.executable.as_str())
        || runtime
            .bootstrap
            .as_deref()
            .is_some_and(|path| !identities.contains(path))
        || runtime
            .dependencies
            .iter()
            .any(|dependency| !identities.contains(dependency.identity_file.as_str()))
    {
        return Err(invalid_component(
            "Runtime identity files must include the executable, bootstrap, and every dependency marker.",
        ));
    }
    Ok(())
}

fn validate_dependency_text(value: &str, label: &str) -> Result<(), ArtifactRuntimeError> {
    if value.is_empty()
        || value.len() > 128
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err(invalid_component(format!(
            "Artifact runtime dependency {label} is invalid."
        )));
    }
    Ok(())
}

fn compute_bundle_revision(receipt: &ComponentReceipt) -> Result<String, ArtifactRuntimeError> {
    let payload = ReceiptRevisionPayload {
        schema_version: receipt.schema_version,
        provider_id: &receipt.provider_id,
        bundle_version: &receipt.bundle_version,
        build_inputs_revision: &receipt.build_inputs_revision,
        platform: &receipt.platform,
        arch: &receipt.arch,
        runtimes: &receipt.runtimes,
        tools: &receipt.tools,
        files: &receipt.files,
    };
    let bytes = serde_json::to_vec(&payload).map_err(|error| {
        invalid_component(format!(
            "Cannot canonicalize artifact runtime component receipt: {error}"
        ))
    })?;
    Ok(format!(
        "{BUNDLE_REVISION_PREFIX}{}",
        hex_lower(&Sha256::digest(bytes))
    ))
}

#[derive(Debug, Clone)]
struct VerifiedFile {
    path: PathBuf,
    #[cfg(not(unix))]
    size: u64,
    #[cfg(not(unix))]
    sha256: String,
    identity: FileIdentity,
}

fn verify_component_tree(
    root: &Path,
    receipt: &ComponentReceipt,
) -> Result<BTreeMap<String, VerifiedFile>, ArtifactRuntimeError> {
    let expected = receipt
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let actual_paths = collect_component_files(root)?;
    let actual_names = actual_paths
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let expected_names = expected.keys().copied().collect::<BTreeSet<_>>();
    if actual_names != expected_names {
        let missing = expected_names.difference(&actual_names).next().copied();
        let unexpected = actual_names.difference(&expected_names).next().copied();
        return Err(ArtifactRuntimeError::new(
            ArtifactRuntimeErrorCode::IntegrityMismatch,
            ArtifactRuntimeRecovery::RepairComponent,
            format!(
                "Artifact runtime component file set differs from its receipt{}{}.",
                missing.map_or_else(String::new, |path| format!("; missing `{path}`")),
                unexpected.map_or_else(String::new, |path| format!("; unexpected `{path}`"))
            ),
        ));
    }
    let mut verified = BTreeMap::new();
    for (relative, path) in actual_paths {
        let descriptor = expected[relative.as_str()];
        verify_file(&path, descriptor.size, &descriptor.sha256)?;
        verified.insert(
            relative,
            VerifiedFile {
                identity: FileIdentity::capture(&path)?,
                path,
                #[cfg(not(unix))]
                size: descriptor.size,
                #[cfg(not(unix))]
                sha256: descriptor.sha256.clone(),
            },
        );
    }
    for runtime in [&receipt.runtimes.node, &receipt.runtimes.python] {
        if !verified.contains_key(&runtime.executable) {
            return Err(invalid_component(format!(
                "Runtime executable `{}` is absent from the component file set.",
                runtime.executable
            )));
        }
    }
    for path in receipt
        .tools
        .pdf_cli
        .identity_files
        .iter()
        .chain(receipt.tools.ripgrep.identity_files.iter())
    {
        if !verified.contains_key(path) {
            return Err(invalid_component(format!(
                "Managed tool identity file `{path}` is absent from the component file set."
            )));
        }
    }
    Ok(verified)
}

fn validate_tool_executable_files(
    receipt: &ComponentReceipt,
    files: &BTreeMap<String, VerifiedFile>,
) -> Result<(), ArtifactRuntimeError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let relative = receipt.tools.ripgrep.executable.as_str();
        let file = files.get(relative).ok_or_else(|| {
            invalid_component(format!(
                "Managed tool executable `{relative}` is absent from the component file set."
            ))
        })?;
        let metadata = fs::symlink_metadata(&file.path)
            .map_err(|error| io_error("inspect managed tool executable", error))?;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(invalid_component(format!(
                "Managed tool executable `{relative}` is not executable."
            )));
        }
    }
    #[cfg(not(unix))]
    let _ = (receipt, files);
    Ok(())
}

#[derive(Debug, Clone)]
struct FileIdentity {
    size: u64,
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(unix)]
    change_seconds: i64,
    #[cfg(unix)]
    change_nanoseconds: i64,
}

impl FileIdentity {
    fn capture(path: &Path) -> Result<Self, ArtifactRuntimeError> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| io_error("capture artifact runtime file identity", error))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(integrity_error(path));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                size: metadata.len(),
                device: metadata.dev(),
                inode: metadata.ino(),
                change_seconds: metadata.ctime(),
                change_nanoseconds: metadata.ctime_nsec(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                size: metadata.len(),
                modified: metadata.modified().ok(),
                created: metadata.created().ok(),
            })
        }
    }

    fn verify(&self, path: &Path) -> Result<(), ArtifactRuntimeError> {
        let current = Self::capture(path)?;
        #[cfg(unix)]
        let matches = self.size == current.size
            && self.device == current.device
            && self.inode == current.inode
            && self.change_seconds == current.change_seconds
            && self.change_nanoseconds == current.change_nanoseconds;
        #[cfg(not(unix))]
        let matches = self.size == current.size
            && self.modified == current.modified
            && self.created == current.created;
        if matches {
            Ok(())
        } else {
            Err(integrity_error(path))
        }
    }
}

fn collect_component_files(root: &Path) -> Result<BTreeMap<String, PathBuf>, ArtifactRuntimeError> {
    let mut pending = vec![(String::new(), root.to_path_buf())];
    let mut files = BTreeMap::new();
    while let Some((prefix, directory)) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .map_err(|error| io_error("read artifact runtime component directory", error))?;
        for entry in entries {
            let entry =
                entry.map_err(|error| io_error("read artifact runtime directory entry", error))?;
            let name = entry.file_name().into_string().map_err(|_| {
                invalid_component("Artifact runtime component paths must be valid UTF-8.")
            })?;
            let relative = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            validate_relative_path(&relative)?;
            let metadata = fs::symlink_metadata(entry.path())
                .map_err(|error| io_error("inspect artifact runtime component entry", error))?;
            if metadata.file_type().is_symlink() {
                return Err(invalid_component(format!(
                    "Artifact runtime component cannot contain symlink `{relative}`."
                )));
            }
            if metadata.is_dir() {
                pending.push((relative, entry.path()));
            } else if metadata.is_file() {
                if relative != COMPONENT_RECEIPT {
                    if files.len() == MAX_COMPONENT_FILES {
                        return Err(invalid_component(
                            "Artifact runtime component exceeds the file-count limit.",
                        ));
                    }
                    files.insert(relative, entry.path());
                }
            } else {
                return Err(invalid_component(format!(
                    "Artifact runtime component entry `{relative}` is not a regular file or directory."
                )));
            }
        }
    }
    Ok(files)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), ArtifactRuntimeError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|error| {
        ArtifactRuntimeError::new(
            ArtifactRuntimeErrorCode::IntegrityMismatch,
            ArtifactRuntimeRecovery::RepairComponent,
            format!(
                "Cannot open artifact runtime file `{}`: {error}",
                path.display()
            ),
        )
    })?;
    let before = file
        .metadata()
        .map_err(|error| io_error("inspect artifact runtime file", error))?;
    if !before.is_file() || before.len() != expected_size {
        return Err(integrity_error(path));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("hash artifact runtime file", error))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > expected_size || total > MAX_COMPONENT_FILE_BYTES {
            return Err(integrity_error(path));
        }
        digest.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| io_error("reinspect artifact runtime file", error))?;
    let path_after = fs::symlink_metadata(path)
        .map_err(|error| io_error("reinspect artifact runtime file path", error))?;
    if total != expected_size
        || after.len() != expected_size
        || path_after.file_type().is_symlink()
        || !path_after.is_file()
        || path_after.len() != expected_size
        || !same_file_identity(&before, &after, &path_after)
        || hex_lower(&digest.finalize()) != expected_sha256
    {
        return Err(integrity_error(path));
    }
    Ok(())
}

#[cfg(unix)]
fn same_file_identity(
    before: &fs::Metadata,
    after: &fs::Metadata,
    path_after: &fs::Metadata,
) -> bool {
    use std::os::unix::fs::MetadataExt;
    before.dev() == after.dev()
        && before.ino() == after.ino()
        && before.dev() == path_after.dev()
        && before.ino() == path_after.ino()
}

#[cfg(not(unix))]
fn same_file_identity(
    before: &fs::Metadata,
    after: &fs::Metadata,
    path_after: &fs::Metadata,
) -> bool {
    // Opening without following the final symlink plus a post-read path check
    // closes the common replacement window. Platforms without stable file-id
    // metadata still compare all portable timestamps and sizes.
    before.len() == after.len()
        && before.len() == path_after.len()
        && before.modified().ok() == after.modified().ok()
        && before.modified().ok() == path_after.modified().ok()
        && before.created().ok() == path_after.created().ok()
}

fn runtime_fingerprint(
    bundle_revision: &str,
    kind: ArtifactRuntimeKind,
    runtime: &RuntimeReceipt,
) -> String {
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.artifact-runtime\0");
    digest.update(1_u32.to_be_bytes());
    update_fingerprint(&mut digest, bundle_revision.as_bytes());
    update_fingerprint(&mut digest, kind.stable_name().as_bytes());
    update_fingerprint(&mut digest, runtime.version.as_bytes());
    for dependency in &runtime.dependencies {
        update_fingerprint(&mut digest, dependency.name.as_bytes());
        update_fingerprint(&mut digest, dependency.version.as_bytes());
        update_fingerprint(&mut digest, dependency.identity_file.as_bytes());
    }
    format!(
        "{RUNTIME_FINGERPRINT_PREFIX}{}",
        hex_lower(&digest.finalize())
    )
}

fn update_fingerprint(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

fn normalize_dependency_name(kind: ArtifactRuntimeKind, name: &str) -> String {
    match kind {
        ArtifactRuntimeKind::Node => name.to_ascii_lowercase(),
        ArtifactRuntimeKind::Python => {
            let mut normalized = String::with_capacity(name.len());
            let mut separator = false;
            for character in name.chars().flat_map(char::to_lowercase) {
                if matches!(character, '-' | '_' | '.') {
                    if !separator {
                        normalized.push('-');
                        separator = true;
                    }
                } else {
                    normalized.push(character);
                    separator = false;
                }
            }
            normalized
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExpectedEntry {
    Directory,
}

fn resolve_without_symlinks(
    root: &Path,
    relative: &str,
    expected: ExpectedEntry,
) -> Result<PathBuf, ArtifactRuntimeError> {
    validate_relative_path(relative)?;
    let mut current = root.to_path_buf();
    for component in Path::new(relative).components() {
        let Component::Normal(segment) = component else {
            return Err(invalid_component("Artifact runtime path is not canonical."));
        };
        current.push(segment);
        let metadata = fs::symlink_metadata(&current)
            .map_err(|error| io_error("inspect artifact runtime path", error))?;
        if metadata.file_type().is_symlink() {
            return Err(invalid_component(format!(
                "Artifact runtime path `{relative}` traverses a symlink."
            )));
        }
    }
    match expected {
        ExpectedEntry::Directory if !current.is_dir() => Err(invalid_component(format!(
            "Artifact runtime path `{relative}` is not a directory."
        ))),
        ExpectedEntry::Directory => Ok(current),
    }
}

fn validate_relative_path(path: &str) -> Result<(), ArtifactRuntimeError> {
    if path.is_empty() || path.len() > 1024 || path.contains('\\') || path.contains('\0') {
        return Err(invalid_component(
            "Artifact runtime component paths must be non-empty canonical UTF-8 paths.",
        ));
    }
    let parsed = Path::new(path);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_component(format!(
            "Artifact runtime path `{path}` is not a canonical relative path."
        )));
    }
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<(), ArtifactRuntimeError> {
    if value.len() != SHA256_HEX_LENGTH
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid_component(format!(
            "Artifact runtime {label} must be a lowercase SHA-256 digest."
        )));
    }
    Ok(())
}

fn current_platform() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        "windows" => "win32",
        other => other,
    }
}

fn current_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => other,
    }
}

fn unavailable_runtime_status(
    kind: ArtifactRuntimeKind,
    error: &ArtifactRuntimeError,
) -> ArtifactRuntimeStatus {
    ArtifactRuntimeStatus {
        kind,
        availability: ArtifactRuntimeAvailability::Unavailable,
        version: None,
        dependencies: Vec::new(),
        runtime_fingerprint: None,
        error_code: Some(error.code().stable_name().to_string()),
        recovery: Some(error.recovery().stable_name().to_string()),
        message: Some(error.message().to_string()),
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    use fmt::Write as _;
    use std::fmt;
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn invalid_component(message: impl Into<String>) -> ArtifactRuntimeError {
    ArtifactRuntimeError::new(
        ArtifactRuntimeErrorCode::InvalidComponent,
        ArtifactRuntimeRecovery::RepairComponent,
        message,
    )
}

fn unavailable_error(message: impl Into<String>) -> ArtifactRuntimeError {
    ArtifactRuntimeError::new(
        ArtifactRuntimeErrorCode::Unavailable,
        ArtifactRuntimeRecovery::InstallComponent,
        message,
    )
}

fn integrity_error(path: &Path) -> ArtifactRuntimeError {
    ArtifactRuntimeError::new(
        ArtifactRuntimeErrorCode::IntegrityMismatch,
        ArtifactRuntimeRecovery::RepairComponent,
        format!(
            "Artifact runtime file `{}` no longer matches its frozen receipt.",
            path.display()
        ),
    )
}

fn io_error(operation: &str, error: std::io::Error) -> ArtifactRuntimeError {
    ArtifactRuntimeError::new(
        ArtifactRuntimeErrorCode::Io,
        ArtifactRuntimeRecovery::Retry,
        format!("Cannot {operation}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::CommandRuntimeProfileResolver;
    use crate::command::{
        run_authorized_command_with_artifact_runtime, CommandAuthorizationSource,
        COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
    };
    use crate::{
        AgentApprovalStatus, AgentCancellationToken, AgentCommandPermission, AgentCommandRequest,
        AgentCommandRuntimeKind, AgentCommandRuntimeProfile, AgentCommandSafetyPolicy,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentWritePermission,
    };
    use std::fs::File;
    use std::io::Write;

    struct Fixture {
        directory: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let files = [
                ("runtime-manifest.json", b"{}".as_slice()),
                ("runtime/node-bootstrap.mjs", b"// fixture".as_slice()),
                ("dependencies/node/bin/node", b"node fixture".as_slice()),
                (
                    "dependencies/node/node_modules/docx/package.json",
                    br#"{"name":"docx","version":"9.6.1"}"#.as_slice(),
                ),
                (
                    "dependencies/node/node_modules/exceljs/package.json",
                    br#"{"name":"exceljs","version":"4.4.0"}"#.as_slice(),
                ),
                (
                    "dependencies/node/node_modules/pptxgenjs/package.json",
                    br#"{"name":"pptxgenjs","version":"4.0.1"}"#.as_slice(),
                ),
                (
                    "dependencies/python/bin/python3",
                    b"python fixture".as_slice(),
                ),
                ("dependencies/tools/rg", b"ripgrep fixture".as_slice()),
                (
                    "runtime/pdf-runtime-cli.py",
                    b"# managed PDF CLI fixture\n".as_slice(),
                ),
                ("legal/ripgrep/COPYING", b"fixture copyright\n".as_slice()),
                (
                    "legal/ripgrep/LICENSE-MIT",
                    b"fixture MIT license\n".as_slice(),
                ),
                (
                    "legal/ripgrep/UNLICENSE",
                    b"fixture unlicense\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA",
                    b"Name: openpyxl\nVersion: 3.1.5\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA",
                    b"Name: pdfplumber\nVersion: 0.11.9\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA",
                    b"Name: pypdf\nVersion: 6.15.0\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA",
                    b"Name: pypdfium2\nVersion: 5.12.1\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA",
                    b"Name: python-docx\nVersion: 1.2.0\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA",
                    b"Name: python-pptx\nVersion: 1.0.2\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA",
                    b"Name: reportlab\nVersion: 4.4.9\n".as_slice(),
                ),
                (
                    "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA",
                    b"Name: XlsxWriter\nVersion: 3.2.9\n".as_slice(),
                ),
            ];
            for (relative, bytes) in files {
                let path = directory.path().join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                let mut file = File::create(&path).unwrap();
                file.write_all(bytes).unwrap();
                #[cfg(unix)]
                if relative.ends_with("/node")
                    || relative.ends_with("/python3")
                    || relative.ends_with("/rg")
                {
                    use std::os::unix::fs::PermissionsExt;
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
                }
            }
            let mut receipt = receipt_for(directory.path());
            receipt.bundle_revision = compute_bundle_revision(&receipt).unwrap();
            fs::write(
                directory.path().join(COMPONENT_RECEIPT),
                serde_json::to_vec_pretty(&receipt).unwrap(),
            )
            .unwrap();
            Self { directory }
        }

        fn options(&self) -> ArtifactRuntimeDiscoveryOptions {
            ArtifactRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(self.directory.path())
        }
    }

    fn receipt_for(root: &Path) -> ComponentReceipt {
        let paths = collect_component_files(root).unwrap();
        let files = paths
            .into_iter()
            .map(|(path, absolute)| {
                let bytes = fs::read(absolute).unwrap();
                FileReceipt {
                    path,
                    size: bytes.len() as u64,
                    sha256: hex_lower(&Sha256::digest(bytes)),
                }
            })
            .collect();
        ComponentReceipt {
            schema_version: 3,
            provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            bundle_version: ARTIFACT_RUNTIME_BUNDLE_VERSION.to_string(),
            build_inputs_revision: format!(
                "{BUILD_INPUTS_REVISION_PREFIX}{}",
                "a".repeat(SHA256_HEX_LENGTH)
            ),
            platform: current_platform().to_string(),
            arch: current_arch().to_string(),
            runtimes: RuntimeSet {
                node: RuntimeReceipt {
                    version: ARTIFACT_RUNTIME_NODE_VERSION.to_string(),
                    executable: "dependencies/node/bin/node".to_string(),
                    package_root: Some("dependencies/node/node_modules".to_string()),
                    runtime_home: None,
                    bootstrap: Some("runtime/node-bootstrap.mjs".to_string()),
                    dependencies: vec![
                        DependencyReceipt {
                            name: "docx".to_string(),
                            version: "9.6.1".to_string(),
                            identity_file:
                                "dependencies/node/node_modules/docx/package.json".to_string(),
                        },
                        DependencyReceipt {
                            name: "exceljs".to_string(),
                            version: "4.4.0".to_string(),
                            identity_file:
                                "dependencies/node/node_modules/exceljs/package.json".to_string(),
                        },
                        DependencyReceipt {
                            name: "pptxgenjs".to_string(),
                            version: "4.0.1".to_string(),
                            identity_file:
                                "dependencies/node/node_modules/pptxgenjs/package.json".to_string(),
                        },
                    ],
                    identity_files: vec![
                        "dependencies/node/bin/node".to_string(),
                        "runtime/node-bootstrap.mjs".to_string(),
                        "dependencies/node/node_modules/docx/package.json".to_string(),
                        "dependencies/node/node_modules/exceljs/package.json".to_string(),
                        "dependencies/node/node_modules/pptxgenjs/package.json".to_string(),
                    ],
                },
                python: RuntimeReceipt {
                    version: ARTIFACT_RUNTIME_PYTHON_VERSION.to_string(),
                    executable: "dependencies/python/bin/python3".to_string(),
                    package_root: None,
                    runtime_home: Some("dependencies/python".to_string()),
                    bootstrap: None,
                    dependencies: vec![
                        DependencyReceipt {
                            name: "openpyxl".to_string(),
                            version: "3.1.5".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "pdfplumber".to_string(),
                            version: "0.11.9".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "pypdf".to_string(),
                            version: "6.15.0".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "pypdfium2".to_string(),
                            version: "5.12.1".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "python-docx".to_string(),
                            version: "1.2.0".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "python-pptx".to_string(),
                            version: "1.0.2".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "reportlab".to_string(),
                            version: "4.4.9".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA".to_string(),
                        },
                        DependencyReceipt {
                            name: "xlsxwriter".to_string(),
                            version: "3.2.9".to_string(),
                            identity_file: "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA".to_string(),
                        },
                    ],
                    identity_files: vec![
                        "dependencies/python/bin/python3".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA".to_string(),
                        "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA".to_string(),
                    ],
                },
            },
            tools: ToolSet {
                pdf_cli: PdfCliReceipt {
                    version: "1".to_string(),
                    path: "runtime/pdf-runtime-cli.py".to_string(),
                    identity_files: vec!["runtime/pdf-runtime-cli.py".to_string()],
                },
                ripgrep: ExecutableToolReceipt {
                    version: ARTIFACT_RUNTIME_RIPGREP_VERSION.to_string(),
                    executable: "dependencies/tools/rg".to_string(),
                    identity_files: vec![
                        "dependencies/tools/rg".to_string(),
                        "legal/ripgrep/COPYING".to_string(),
                        "legal/ripgrep/LICENSE-MIT".to_string(),
                        "legal/ripgrep/UNLICENSE".to_string(),
                    ],
                },
            },
            files,
            bundle_revision: String::new(),
        }
    }

    #[test]
    fn discovers_a_complete_revision_bound_bundle_without_path_fallback() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        let status = provider.status();
        assert_eq!(status.availability, ArtifactRuntimeAvailability::Available);
        assert_eq!(
            status.bundle_version.as_deref(),
            Some(ARTIFACT_RUNTIME_BUNDLE_VERSION)
        );
        assert!(status
            .bundle_revision
            .as_deref()
            .unwrap()
            .starts_with(BUNDLE_REVISION_PREFIX));
        assert_eq!(status.runtimes.len(), 2);
    }

    #[test]
    fn host_profiles_resolve_exact_versions_from_the_verified_receipt() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();

        let presentations = provider
            .resolve_profile(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
            )
            .unwrap();
        assert_eq!(presentations.runtime_version, ARTIFACT_RUNTIME_NODE_VERSION);
        assert_eq!(presentations.bundle_revision, provider.bundle_revision());
        assert_eq!(presentations.resolved_packages.len(), 1);
        assert_eq!(presentations.resolved_packages[0].name, "pptxgenjs");
        assert_eq!(presentations.resolved_packages[0].version, "4.0.1");
        assert!(presentations
            .profile_revision
            .starts_with("artifact-runtime-profile-sha256-v1:"));

        let spreadsheets = provider
            .resolve_profile(
                AgentCommandRuntimeProfile::Spreadsheets,
                AgentCommandRuntimeKind::Python,
            )
            .unwrap();
        assert_eq!(
            spreadsheets.runtime_version,
            ARTIFACT_RUNTIME_PYTHON_VERSION
        );
        assert_eq!(
            spreadsheets
                .resolved_packages
                .iter()
                .map(|package| (package.name.as_str(), package.version.as_str()))
                .collect::<Vec<_>>(),
            vec![("openpyxl", "3.1.5"), ("xlsxwriter", "3.2.9")]
        );
    }

    #[test]
    fn verified_python_runtime_exposes_the_frozen_pdf_toolchain() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        let ArtifactRuntimePreflight::Ready(invocation) = provider
            .preflight(
                ArtifactRuntimeKind::Python,
                &[
                    ArtifactRuntimeRequirement::exact("pdfplumber", "0.11.9").unwrap(),
                    ArtifactRuntimeRequirement::exact("pypdf", "6.15.0").unwrap(),
                    ArtifactRuntimeRequirement::exact("pypdfium2", "5.12.1").unwrap(),
                    ArtifactRuntimeRequirement::exact("reportlab", "4.4.9").unwrap(),
                ],
            )
            .unwrap()
        else {
            panic!("PDF dependencies should be available without installation")
        };
        assert_eq!(invocation.kind(), ArtifactRuntimeKind::Python);
        assert_eq!(invocation.version(), ARTIFACT_RUNTIME_PYTHON_VERSION);
        assert_eq!(
            invocation.arguments_prefix(),
            &[OsString::from("-I"), OsString::from("-B")]
        );
    }

    #[test]
    fn exposes_only_receipt_verified_pdf_cli_and_ripgrep_paths() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();

        assert_eq!(
            provider.pdf_cli_path().unwrap(),
            fixture
                .directory
                .path()
                .canonicalize()
                .unwrap()
                .join("runtime/pdf-runtime-cli.py")
        );
        assert_eq!(
            provider.ripgrep_executable().unwrap(),
            fixture
                .directory
                .path()
                .canonicalize()
                .unwrap()
                .join("dependencies/tools/rg")
        );
        fs::write(
            fixture.directory.path().join("dependencies/tools/rg"),
            b"replaced ripgrep fixture",
        )
        .unwrap();
        let error = provider.ripgrep_executable().unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::IntegrityMismatch);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_a_receipted_ripgrep_without_execute_permission() {
        use std::os::unix::fs::PermissionsExt;

        let fixture = Fixture::new();
        let launcher = fixture.directory.path().join("dependencies/tools/rg");
        fs::set_permissions(&launcher, fs::Permissions::from_mode(0o644)).unwrap();

        let error = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::InvalidComponent);
        assert!(error.message().contains("is not executable"));
    }

    #[test]
    fn execution_refuses_a_profile_binding_that_no_longer_matches_the_provider() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        let workspace = tempfile::tempdir().unwrap();
        fs::write(workspace.path().join("build.mjs"), "// must never run\n").unwrap();
        let mut binding = provider
            .resolve_profile(
                AgentCommandRuntimeProfile::Presentations,
                AgentCommandRuntimeKind::Node,
            )
            .unwrap();
        binding.bundle_revision.push_str("-replaced");
        let request = AgentCommandRequest {
            id: "runtime-binding-conflict".to_string(),
            command: "node build.mjs".to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: AgentApprovalStatus::Approved,
            risk_level: None,
            reason: Some("verify frozen profile conflict".to_string()),
            observe: None,
            inputs: Vec::new(),
            runtime: None,
            runtime_binding: Some(Box::new(binding)),
        };
        let result = run_authorized_command_with_artifact_runtime(
            Some(workspace.path()),
            &request,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
            },
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
            Some(&provider),
        )
        .unwrap();

        assert_eq!(result.exit_code, None);
        assert_eq!(
            result.runtime.unwrap().error_code.as_deref(),
            Some(COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH)
        );
    }

    #[test]
    fn resolves_isolated_node_and_python_invocations() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        let ArtifactRuntimePreflight::Ready(node) = provider
            .preflight(
                ArtifactRuntimeKind::Node,
                &[ArtifactRuntimeRequirement::exact("exceljs", "4.4.0").unwrap()],
            )
            .unwrap()
        else {
            panic!("Node runtime should be ready")
        };
        assert!(node.executable().is_absolute());
        assert_eq!(node.arguments_prefix()[0], "--import");
        assert!(!node
            .environment()
            .contains_key(std::ffi::OsStr::new("NODE_PATH")));
        assert!(node
            .environment()
            .contains_key(std::ffi::OsStr::new("MYCOPILOT_ARTIFACT_NODE_MODULES")));

        let ArtifactRuntimePreflight::Ready(python) = provider
            .preflight(
                ArtifactRuntimeKind::Python,
                &[ArtifactRuntimeRequirement::any("python_docx").unwrap()],
            )
            .unwrap()
        else {
            panic!("Python runtime should be ready")
        };
        assert_eq!(
            python.arguments_prefix(),
            &[OsString::from("-I"), OsString::from("-B")]
        );
        assert!(python
            .environment()
            .contains_key(std::ffi::OsStr::new("PYTHONNOUSERSITE")));
    }

    #[test]
    fn missing_or_wrong_dependency_is_a_structured_non_installing_outcome() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        let outcome = provider
            .preflight(
                ArtifactRuntimeKind::Python,
                &[
                    ArtifactRuntimeRequirement::any("pandas").unwrap(),
                    ArtifactRuntimeRequirement::exact("openpyxl", "0.0.1").unwrap(),
                ],
            )
            .unwrap();
        let ArtifactRuntimePreflight::MissingDependencies { missing, status } = outcome else {
            panic!("requirements must not be installed automatically")
        };
        assert_eq!(missing.len(), 2);
        assert_eq!(status.availability, ArtifactRuntimeAvailability::Available);
    }

    #[test]
    fn modified_component_file_fails_discovery_and_post_discovery_preflight() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        fs::write(
            fixture.directory.path().join("dependencies/node/bin/node"),
            // Keep the original byte length. Unix fast revalidation must detect the kernel-owned
            // ctime change rather than relying on size alone.
            b"tampered!!!!",
        )
        .unwrap();
        let error = provider
            .preflight(ArtifactRuntimeKind::Node, &[])
            .unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::IntegrityMismatch);

        let error = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::IntegrityMismatch);
    }

    #[test]
    fn post_discovery_integrity_rejects_an_added_component_file() {
        let fixture = Fixture::new();
        let provider = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap();
        fs::write(
            fixture
                .directory
                .path()
                .join("dependencies/node/node_modules/injected.js"),
            b"module.exports = 'unexpected';\n",
        )
        .unwrap();

        let error = provider.verify_integrity().unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::IntegrityMismatch);
    }

    #[test]
    fn missing_component_has_stable_status_and_never_searches_path() {
        let missing = tempfile::tempdir().unwrap().path().join("missing");
        let status = ArtifactRuntimeProvider::inspect(
            &ArtifactRuntimeDiscoveryOptions::new().with_configured_component_dir(missing),
        );
        assert_eq!(
            status.availability,
            ArtifactRuntimeAvailability::Unavailable
        );
        assert_eq!(status.error_code.as_deref(), Some("unavailable"));
        assert_eq!(status.runtimes.len(), 2);
        assert!(status
            .runtimes
            .iter()
            .all(|runtime| runtime.availability == ArtifactRuntimeAvailability::Unavailable));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_component_entries_are_rejected() {
        use std::os::unix::fs::symlink;
        let fixture = Fixture::new();
        let executable = fixture.directory.path().join("dependencies/node/bin/node");
        fs::remove_file(&executable).unwrap();
        symlink("/bin/sh", &executable).unwrap();
        let error = ArtifactRuntimeProvider::discover(&fixture.options()).unwrap_err();
        assert_eq!(error.code(), ArtifactRuntimeErrorCode::InvalidComponent);
    }
}
