//! Revision-bound, dependency-aware execution for Python Skill scripts.
//!
//! This module deliberately does not expose a shell command. A caller supplies
//! a canonical `skill://` URI, structured argv, and declarative requirements.
//! The resource bytes are read through the activated [`SkillResourceSession`],
//! copied to a private temporary snapshot, and started with a resolved Python
//! interpreter as a structured process invocation.

use super::{
    SkillResourceError, SkillResourceKind, SkillResourceSession, SkillResourceUri,
    SkillResourceUriError,
};
use crate::command::{
    configure_command_process_group, join_process_output_capture, spawn_process_output_capture,
    terminate_command_process_group, try_wait_command_process_group, ProcessOutputCaptureBudget,
    ProcessOutputCaptureMetadata, ProcessOutputCapturePolicy, ProcessOutputSpool,
};
use crate::{
    AgentCancellationToken, AgentSkillDependencyCheck, AgentSkillDependencyKind,
    AgentSkillDependencyStatus, AgentSkillScriptInterpreter, AgentSkillScriptPreflightReport,
    AgentSkillScriptPreflightStatus, AgentSkillScriptRequest, AgentSkillScriptRequirements,
    AgentSkillScriptResult,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, UNIX_EPOCH};

pub const DEFAULT_SKILL_SCRIPT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_SKILL_SCRIPT_TIMEOUT_MS: u64 = 600_000;
pub const MAX_SKILL_SCRIPT_ARGUMENTS: usize = 128;
pub const MAX_SKILL_SCRIPT_ARGUMENT_BYTES: usize = 64 * 1024;
pub const MAX_SKILL_SCRIPT_REQUIREMENTS: usize = 128;
const MAX_REQUIREMENT_NAME_BYTES: usize = 128;
const MAX_PYTHON_LIBRARY_DIRECTORIES: usize = 64;
const MAX_PYTHON_DISTRIBUTION_ENTRIES: usize = 4_096;
const MAX_PYTHON_DISTRIBUTION_METADATA_BYTES: u64 = 64 * 1024;
const MAX_PYTHON_INTERPRETER_BYTES: u64 = 256 * 1024 * 1024;
const RUNTIME_FINGERPRINT_PREFIX: &str = "skill-python-runtime-sha256-v1:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillScriptRuntimeErrorCode {
    InvalidRequest,
    InvalidResourceUri,
    ResourceUnavailable,
    UnsupportedPlatform,
    UnsupportedScript,
    RuntimeConflict,
    Io,
    ProcessFailure,
}

impl SkillScriptRuntimeErrorCode {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalidRequest",
            Self::InvalidResourceUri => "invalidResourceUri",
            Self::ResourceUnavailable => "resourceUnavailable",
            Self::UnsupportedPlatform => "unsupportedPlatform",
            Self::UnsupportedScript => "unsupportedScript",
            Self::RuntimeConflict => "runtimeConflict",
            Self::Io => "io",
            Self::ProcessFailure => "processFailure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkillScriptRuntimeRecovery {
    ChangeRequest,
    ReactivateSkill,
    InstallDependency,
    Retry,
}

impl SkillScriptRuntimeRecovery {
    pub fn stable_name(self) -> &'static str {
        match self {
            Self::ChangeRequest => "changeRequest",
            Self::ReactivateSkill => "reactivateSkill",
            Self::InstallDependency => "installDependency",
            Self::Retry => "retry",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillScriptRuntimeError {
    code: SkillScriptRuntimeErrorCode,
    recovery: SkillScriptRuntimeRecovery,
    message: String,
}

impl SkillScriptRuntimeError {
    fn new(
        code: SkillScriptRuntimeErrorCode,
        recovery: SkillScriptRuntimeRecovery,
        message: impl Into<String>,
    ) -> Self {
        Self {
            code,
            recovery,
            message: message.into(),
        }
    }

    pub fn code(&self) -> SkillScriptRuntimeErrorCode {
        self.code
    }

    pub fn recovery(&self) -> SkillScriptRuntimeRecovery {
        self.recovery
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SkillScriptRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.message.fmt(formatter)
    }
}

impl Error for SkillScriptRuntimeError {}

impl From<SkillResourceUriError> for SkillScriptRuntimeError {
    fn from(error: SkillResourceUriError) -> Self {
        Self::new(
            SkillScriptRuntimeErrorCode::InvalidResourceUri,
            SkillScriptRuntimeRecovery::ChangeRequest,
            error.to_string(),
        )
    }
}

impl From<SkillResourceError> for SkillScriptRuntimeError {
    fn from(error: SkillResourceError) -> Self {
        Self::new(
            SkillScriptRuntimeErrorCode::ResourceUnavailable,
            SkillScriptRuntimeRecovery::ReactivateSkill,
            error.to_string(),
        )
    }
}

/// A private, host-side execution capability created by a successful
/// preflight. Filesystem interpreter details intentionally never need to be
/// serialized into model-visible requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillScriptReadyPlan {
    script_uri: SkillResourceUri,
    resource_digest: String,
    interpreter_path: PathBuf,
    interpreter_version: String,
    requirements: AgentSkillScriptRequirements,
    runtime_fingerprint: String,
}

impl SkillScriptReadyPlan {
    pub fn script_uri(&self) -> &SkillResourceUri {
        &self.script_uri
    }

    pub fn resource_digest(&self) -> &str {
        &self.resource_digest
    }

    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillScriptPreflightOutcome {
    Ready {
        report: AgentSkillScriptPreflightReport,
        plan: Box<SkillScriptReadyPlan>,
    },
    MissingDependencies {
        report: AgentSkillScriptPreflightReport,
    },
    Unsupported {
        report: AgentSkillScriptPreflightReport,
    },
}

impl SkillScriptPreflightOutcome {
    pub fn report(&self) -> &AgentSkillScriptPreflightReport {
        match self {
            Self::Ready { report, .. }
            | Self::MissingDependencies { report }
            | Self::Unsupported { report } => report,
        }
    }

    pub fn ready_plan(&self) -> Option<&SkillScriptReadyPlan> {
        match self {
            Self::Ready { plan, .. } => Some(plan),
            Self::MissingDependencies { .. } | Self::Unsupported { .. } => None,
        }
    }
}

/// Resolves and checks a Python runtime without starting the interpreter,
/// importing requested modules, or installing anything. Distribution checks
/// read bounded `*.dist-info/METADATA` files below the interpreter prefix.
pub fn preflight_skill_python_script(
    resources: &SkillResourceSession,
    workspace_root: &Path,
    script_uri: &SkillResourceUri,
    interpreter: AgentSkillScriptInterpreter,
    requirements: &AgentSkillScriptRequirements,
) -> Result<SkillScriptPreflightOutcome, SkillScriptRuntimeError> {
    validate_platform()?;
    validate_requirements(requirements)?;
    let workspace_root = canonical_workspace(workspace_root)?;
    let snapshot = resources.read_verified_bytes(script_uri)?;
    validate_script_descriptor(script_uri, snapshot.descriptor.kind())?;
    if interpreter != AgentSkillScriptInterpreter::Python3 {
        return Ok(unsupported_report(
            interpreter,
            "skill_script.interpreter_unsupported",
            "Only the logical Python 3 interpreter is supported.",
        ));
    }

    let Some(interpreter_path) = resolve_python3(&workspace_root) else {
        return Ok(missing_report(
            interpreter,
            Vec::new(),
            "skill_script.python_interpreter_missing",
            "No eligible Python 3 interpreter was found outside the workspace on PATH.",
        ));
    };
    let interpreter_version = static_python_version(&interpreter_path);
    let dependencies = dependency_checks(requirements, &interpreter_path)?;
    let missing = dependencies
        .iter()
        .any(|check| check.status == AgentSkillDependencyStatus::Missing);
    if missing {
        return Ok(missing_report(
            interpreter,
            dependencies,
            "skill_script.dependencies_missing",
            "One or more declared Skill script dependencies are unavailable.",
        ));
    }
    let runtime_fingerprint = runtime_fingerprint(
        &interpreter_path,
        &interpreter_version,
        snapshot.descriptor.content_digest(),
        requirements,
        &dependencies,
    )?;
    let report = AgentSkillScriptPreflightReport {
        status: AgentSkillScriptPreflightStatus::Ready,
        interpreter,
        interpreter_version: Some(interpreter_version.clone()),
        dependencies,
        runtime_fingerprint: runtime_fingerprint.clone(),
        error_code: None,
        message: None,
    };
    let plan = SkillScriptReadyPlan {
        script_uri: script_uri.clone(),
        resource_digest: snapshot.descriptor.content_digest().to_string(),
        interpreter_path,
        interpreter_version,
        requirements: requirements.clone(),
        runtime_fingerprint,
    };
    Ok(SkillScriptPreflightOutcome::Ready {
        report,
        plan: Box::new(plan),
    })
}

/// Executes a frozen request after repeating preflight and comparing the
/// revision, content digest, requirements, and runtime fingerprint.
pub fn execute_skill_python_script(
    resources: &SkillResourceSession,
    workspace_root: &Path,
    request: &AgentSkillScriptRequest,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<AgentSkillScriptResult, SkillScriptRuntimeError> {
    validate_arguments(&request.args)?;
    let uri = SkillResourceUri::parse(&request.script_uri)?;
    if uri.package().skill_id().as_str() != request.skill_id
        || uri.package().revision().as_str() != request.skill_revision
        || uri.path().as_str() != request.resource_path
    {
        return Err(runtime_conflict(
            "The frozen Skill script identity does not match its canonical URI.",
        ));
    }

    let outcome = preflight_skill_python_script(
        resources,
        workspace_root,
        &uri,
        request.interpreter,
        &request.requirements,
    )?;
    let (report, plan) = match outcome {
        SkillScriptPreflightOutcome::Ready { report, plan } => (report, plan),
        SkillScriptPreflightOutcome::MissingDependencies { report }
        | SkillScriptPreflightOutcome::Unsupported { report } => {
            return Err(SkillScriptRuntimeError::new(
                SkillScriptRuntimeErrorCode::RuntimeConflict,
                SkillScriptRuntimeRecovery::InstallDependency,
                report
                    .message
                    .unwrap_or_else(|| "Skill script preflight is no longer ready.".to_string()),
            ));
        }
    };
    if request.resource_digest != plan.resource_digest
        || request.preflight.runtime_fingerprint != plan.runtime_fingerprint
        || request.preflight.status != AgentSkillScriptPreflightStatus::Ready
    {
        return Err(runtime_conflict(
            "The Skill script resource or runtime changed after approval; run preflight again.",
        ));
    }
    let snapshot = resources.read_verified_bytes(&uri)?;
    if snapshot.descriptor.content_digest() != plan.resource_digest {
        return Err(runtime_conflict(
            "The Skill script resource changed after the execution plan was validated.",
        ));
    }
    let workspace_root = canonical_workspace(workspace_root)?;
    let started = Instant::now();
    if cancellation_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return Ok(cancelled_result(request, report, started));
    }

    let temporary = tempfile::Builder::new()
        .prefix("mycopilot-skill-script-")
        .tempdir()
        .map_err(|error| io_error("create private script directory", error))?;
    set_private_directory_permissions(temporary.path())?;
    let script_path = temporary.path().join("script.py");
    write_private_script(&script_path, &snapshot.bytes)?;

    let timeout_ms = request
        .timeout_ms
        .unwrap_or(DEFAULT_SKILL_SCRIPT_TIMEOUT_MS)
        .clamp(1, MAX_SKILL_SCRIPT_TIMEOUT_MS);
    let execution = run_python_process(
        &plan,
        &script_path,
        &workspace_root,
        &request.args,
        Duration::from_millis(timeout_ms),
        &cancellation_token,
        action_cancel_flag.as_ref(),
    )?;
    let (error_code, error) = execution_result_error(&execution);
    Ok(AgentSkillScriptResult {
        script_uri: request.script_uri.clone(),
        skill_id: request.skill_id.clone(),
        skill_revision: request.skill_revision.clone(),
        resource_digest: request.resource_digest.clone(),
        preflight: report,
        exit_code: execution.exit_code,
        stdout: execution.stdout,
        stderr: execution.stderr,
        timed_out: execution.timed_out,
        cancelled: execution.cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated: execution.stdout_truncated,
        stderr_truncated: execution.stderr_truncated,
        output_capture: execution.output_capture,
        stdout_spool: execution.stdout_spool,
        stderr_spool: execution.stderr_spool,
        error_code,
        error,
    })
}

fn validate_platform() -> Result<(), SkillScriptRuntimeError> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(SkillScriptRuntimeError::new(
            SkillScriptRuntimeErrorCode::UnsupportedPlatform,
            SkillScriptRuntimeRecovery::ChangeRequest,
            "Skill script execution is unavailable until this platform supports bounded process-tree termination.",
        ))
    }
}

fn validate_script_descriptor(
    uri: &SkillResourceUri,
    kind: SkillResourceKind,
) -> Result<(), SkillScriptRuntimeError> {
    if kind != SkillResourceKind::Script {
        return Err(SkillScriptRuntimeError::new(
            SkillScriptRuntimeErrorCode::UnsupportedScript,
            SkillScriptRuntimeRecovery::ChangeRequest,
            format!("Skill resource `{uri}` is not classified as a script."),
        ));
    }
    if Path::new(uri.path().as_str())
        .extension()
        .and_then(OsStr::to_str)
        != Some("py")
    {
        return Err(SkillScriptRuntimeError::new(
            SkillScriptRuntimeErrorCode::UnsupportedScript,
            SkillScriptRuntimeRecovery::ChangeRequest,
            "Only `.py` Skill scripts are supported.",
        ));
    }
    Ok(())
}

fn validate_requirements(
    requirements: &AgentSkillScriptRequirements,
) -> Result<(), SkillScriptRuntimeError> {
    let total = requirements
        .python_distributions
        .len()
        .saturating_add(requirements.commands.len());
    if total > MAX_SKILL_SCRIPT_REQUIREMENTS {
        return Err(invalid_request(format!(
            "Skill script requirements exceed the {MAX_SKILL_SCRIPT_REQUIREMENTS}-entry limit."
        )));
    }
    for name in &requirements.python_distributions {
        validate_requirement_name(name, "Python distribution", |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
        })?;
    }
    for name in &requirements.commands {
        validate_requirement_name(name, "command", |byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+')
        })?;
        if Path::new(name).components().count() != 1 {
            return Err(invalid_request(
                "Command requirements must be portable basenames, not paths.",
            ));
        }
    }
    Ok(())
}

fn validate_requirement_name(
    name: &str,
    label: &str,
    allowed: impl Fn(u8) -> bool,
) -> Result<(), SkillScriptRuntimeError> {
    if name.is_empty()
        || name.len() > MAX_REQUIREMENT_NAME_BYTES
        || !name.bytes().all(allowed)
        || name.contains(['/', '\\'])
    {
        Err(invalid_request(format!(
            "{label} requirement `{name}` is invalid."
        )))
    } else {
        Ok(())
    }
}

fn validate_arguments(args: &[String]) -> Result<(), SkillScriptRuntimeError> {
    if args.len() > MAX_SKILL_SCRIPT_ARGUMENTS {
        return Err(invalid_request(format!(
            "Skill script argv exceeds the {MAX_SKILL_SCRIPT_ARGUMENTS}-argument limit."
        )));
    }
    let bytes = args
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(value.len()))
        .ok_or_else(|| invalid_request("Skill script argv size overflowed."))?;
    if bytes > MAX_SKILL_SCRIPT_ARGUMENT_BYTES {
        return Err(invalid_request(format!(
            "Skill script argv exceeds the {MAX_SKILL_SCRIPT_ARGUMENT_BYTES}-byte limit."
        )));
    }
    if args.iter().any(|argument| argument.contains('\0')) {
        return Err(invalid_request(
            "Skill script argv cannot contain NUL bytes.",
        ));
    }
    Ok(())
}

fn canonical_workspace(workspace_root: &Path) -> Result<PathBuf, SkillScriptRuntimeError> {
    let root = workspace_root
        .canonicalize()
        .map_err(|error| io_error("resolve workspace", error))?;
    if !root.is_dir() {
        return Err(invalid_request(
            "The selected workspace is not a directory.",
        ));
    }
    Ok(root)
}

fn resolve_python3(workspace_root: &Path) -> Option<PathBuf> {
    resolve_python3_from_candidates(
        workspace_root,
        path_candidates("python3").chain(path_candidates("python")),
    )
}

fn resolve_python3_from_candidates(
    workspace_root: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    candidates
        .into_iter()
        .filter_map(python_executable_canonical_path)
        .find(|path| !path.starts_with(workspace_root))
}

fn path_candidates(program: &str) -> std::vec::IntoIter<PathBuf> {
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .map(move |directory| directory.join(program))
        .collect::<Vec<_>>()
        .into_iter()
}

fn executable_canonical_path(path: PathBuf) -> Option<PathBuf> {
    let canonical = path.canonicalize().ok()?;
    let metadata = canonical.metadata().ok()?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return None;
    }
    Some(canonical)
}

fn python_executable_canonical_path(path: PathBuf) -> Option<PathBuf> {
    let canonical = executable_canonical_path(path)?;
    let name = canonical.file_name()?.to_str()?.to_ascii_lowercase();
    if !name.starts_with("python") || !has_native_executable_header(&canonical) {
        return None;
    }
    Some(canonical)
}

#[cfg(unix)]
fn has_native_executable_header(path: &Path) -> bool {
    let mut options = OpenOptions::new();
    options.read(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    let Ok(mut file) = options.open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    if file.read_exact(&mut magic).is_err() {
        return false;
    }
    matches!(
        magic,
        // ELF plus 32/64-bit, native/reversed, and fat Mach-O headers.
        [0x7f, b'E', b'L', b'F']
            | [0xfe, 0xed, 0xfa, 0xce]
            | [0xfe, 0xed, 0xfa, 0xcf]
            | [0xce, 0xfa, 0xed, 0xfe]
            | [0xcf, 0xfa, 0xed, 0xfe]
            | [0xca, 0xfe, 0xba, 0xbe]
            | [0xbe, 0xba, 0xfe, 0xca]
            | [0xca, 0xfe, 0xba, 0xbf]
            | [0xbf, 0xba, 0xfe, 0xca]
    )
}

#[cfg(not(unix))]
fn has_native_executable_header(_path: &Path) -> bool {
    false
}

#[cfg(unix)]
fn is_executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &fs::Metadata) -> bool {
    false
}

fn static_python_version(interpreter: &Path) -> String {
    let executable_name = interpreter
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("python3");
    let suffix = executable_name.strip_prefix("python").unwrap_or_default();
    if suffix.starts_with("3.")
        && suffix[2..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        format!("Python {suffix}")
    } else {
        // No process is started during preflight. Some system launchers do not
        // encode a patch version in their filename, so the static identity is
        // intentionally honest rather than guessed.
        "Python 3 (static identity)".to_string()
    }
}

fn dependency_checks(
    requirements: &AgentSkillScriptRequirements,
    interpreter: &Path,
) -> Result<Vec<AgentSkillDependencyCheck>, SkillScriptRuntimeError> {
    let distributions = python_distributions(interpreter, &requirements.python_distributions)?;
    let mut checks =
        Vec::with_capacity(requirements.python_distributions.len() + requirements.commands.len());
    checks.extend(requirements.python_distributions.iter().map(|name| {
        let version = distributions.get(name).cloned().flatten();
        AgentSkillDependencyCheck {
            kind: AgentSkillDependencyKind::PythonDistribution,
            name: name.clone(),
            status: if version.is_some() {
                AgentSkillDependencyStatus::Available
            } else {
                AgentSkillDependencyStatus::Missing
            },
            version,
        }
    }));
    checks.extend(requirements.commands.iter().map(|name| {
        let resolved = path_candidates(name).find_map(executable_canonical_path);
        AgentSkillDependencyCheck {
            kind: AgentSkillDependencyKind::Command,
            name: name.clone(),
            status: if resolved.is_some() {
                AgentSkillDependencyStatus::Available
            } else {
                AgentSkillDependencyStatus::Missing
            },
            version: None,
        }
    }));
    Ok(checks)
}

fn python_distributions(
    interpreter: &Path,
    names: &[String],
) -> Result<BTreeMap<String, Option<String>>, SkillScriptRuntimeError> {
    if names.is_empty() {
        return Ok(BTreeMap::new());
    }
    let requested = names
        .iter()
        .map(|name| (normalize_distribution_name(name), name.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut found = BTreeMap::<String, String>::new();
    let mut remaining_entries = MAX_PYTHON_DISTRIBUTION_ENTRIES;
    for site_packages in static_site_package_directories(interpreter) {
        let Ok(entries) = fs::read_dir(site_packages) else {
            continue;
        };
        for entry in entries.flatten() {
            if remaining_entries == 0 {
                break;
            }
            remaining_entries -= 1;
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
                continue;
            };
            if !file_name.ends_with(".dist-info") {
                continue;
            }
            let Some((name, version)) = read_distribution_metadata(&path) else {
                continue;
            };
            let normalized = normalize_distribution_name(&name);
            if requested.contains_key(&normalized) {
                found.entry(normalized).or_insert(version);
            }
            if found.len() == requested.len() {
                break;
            }
        }
        if remaining_entries == 0 || found.len() == requested.len() {
            break;
        }
    }
    Ok(names
        .iter()
        .map(|name| {
            (
                name.clone(),
                found.get(&normalize_distribution_name(name)).cloned(),
            )
        })
        .collect())
}

fn static_site_package_directories(interpreter: &Path) -> Vec<PathBuf> {
    let Some(prefix) = interpreter
        .parent()
        .and_then(Path::parent)
        .and_then(|path| path.canonicalize().ok())
    else {
        return Vec::new();
    };
    let mut directories = Vec::new();
    for root in [prefix.join("lib"), prefix.join("Lib")] {
        let Ok(root) = root.canonicalize() else {
            continue;
        };
        if !root.starts_with(&prefix) || !root.is_dir() {
            continue;
        }
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let remaining = MAX_PYTHON_LIBRARY_DIRECTORIES.saturating_sub(directories.len());
        directories.extend(entries.flatten().take(remaining).filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("python") {
                return None;
            }
            let site_packages = path.join("site-packages").canonicalize().ok()?;
            (site_packages.starts_with(&prefix) && site_packages.is_dir()).then_some(site_packages)
        }));
        if directories.len() >= MAX_PYTHON_LIBRARY_DIRECTORIES {
            break;
        }
    }
    directories.sort();
    directories.dedup();
    directories
}

fn read_distribution_metadata(dist_info: &Path) -> Option<(String, String)> {
    let directory_metadata = fs::symlink_metadata(dist_info).ok()?;
    if !directory_metadata.file_type().is_dir() || directory_metadata.file_type().is_symlink() {
        return None;
    }
    let metadata_path = dist_info.join("METADATA");
    let metadata = fs::symlink_metadata(&metadata_path).ok()?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(metadata_path).ok()?;
    let read_limit = MAX_PYTHON_DISTRIBUTION_METADATA_BYTES.checked_add(1)?;
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut file)
        .take(read_limit)
        .read_to_end(&mut bytes)
        .ok()?;
    if u64::try_from(bytes.len()).ok()? > MAX_PYTHON_DISTRIBUTION_METADATA_BYTES {
        return None;
    }
    let contents = String::from_utf8(bytes).ok()?;
    let mut name = None;
    let mut version = None;
    for line in contents.lines() {
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Name:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("Version:") {
            version = Some(value.trim().to_string());
        }
        if name.is_some() && version.is_some() {
            break;
        }
    }
    Some((name?, version?))
}

fn normalize_distribution_name(name: &str) -> String {
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

fn runtime_fingerprint(
    interpreter: &Path,
    interpreter_version: &str,
    resource_digest: &str,
    requirements: &AgentSkillScriptRequirements,
    dependencies: &[AgentSkillDependencyCheck],
) -> Result<String, SkillScriptRuntimeError> {
    let metadata = interpreter
        .metadata()
        .map_err(|error| io_error("inspect Python interpreter", error))?;
    if metadata.len() > MAX_PYTHON_INTERPRETER_BYTES {
        return Err(invalid_request(format!(
            "Python interpreter exceeds the {MAX_PYTHON_INTERPRETER_BYTES}-byte identity limit."
        )));
    }
    let interpreter_digest = hash_interpreter(interpreter, metadata.len())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok());
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.skill.python-runtime\0");
    digest.update(1_u32.to_be_bytes());
    update_fingerprint(&mut digest, interpreter.as_os_str().as_encoded_bytes());
    update_fingerprint(&mut digest, interpreter_version.as_bytes());
    digest.update(metadata.len().to_be_bytes());
    update_fingerprint(&mut digest, &interpreter_digest);
    digest.update(
        modified
            .map_or(0, |duration| duration.as_secs())
            .to_be_bytes(),
    );
    digest.update(
        modified
            .map_or(0, |duration| u64::from(duration.subsec_nanos()))
            .to_be_bytes(),
    );
    update_fingerprint(&mut digest, resource_digest.as_bytes());
    update_fingerprint(
        &mut digest,
        serde_json::to_string(requirements)
            .map_err(|error| invalid_request(error.to_string()))?
            .as_bytes(),
    );
    update_fingerprint(
        &mut digest,
        serde_json::to_string(dependencies)
            .map_err(|error| invalid_request(error.to_string()))?
            .as_bytes(),
    );
    Ok(format!(
        "{RUNTIME_FINGERPRINT_PREFIX}{}",
        hex_lower(&digest.finalize())
    ))
}

fn hash_interpreter(path: &Path, expected_len: u64) -> Result<Vec<u8>, SkillScriptRuntimeError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options
        .open(path)
        .map_err(|error| io_error("open Python interpreter for identity verification", error))?;
    let before = file
        .metadata()
        .map_err(|error| io_error("inspect Python interpreter", error))?;
    if !before.is_file() || before.len() != expected_len {
        return Err(runtime_conflict(
            "The Python interpreter changed during dependency preflight.",
        ));
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| io_error("hash Python interpreter", error))?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if total > MAX_PYTHON_INTERPRETER_BYTES {
            return Err(invalid_request(format!(
                "Python interpreter exceeds the {MAX_PYTHON_INTERPRETER_BYTES}-byte identity limit."
            )));
        }
        hasher.update(&buffer[..read]);
    }
    let after = file
        .metadata()
        .map_err(|error| io_error("reinspect Python interpreter", error))?;
    if total != expected_len || after.len() != expected_len {
        return Err(runtime_conflict(
            "The Python interpreter changed during dependency preflight.",
        ));
    }
    Ok(hasher.finalize().to_vec())
}

fn update_fingerprint(digest: &mut Sha256, bytes: &[u8]) {
    digest.update((bytes.len() as u64).to_be_bytes());
    digest.update(bytes);
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn run_python_process(
    plan: &SkillScriptReadyPlan,
    script_path: &Path,
    workspace_root: &Path,
    args: &[String],
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<ProcessOutput, SkillScriptRuntimeError> {
    let mut command = Command::new(&plan.interpreter_path);
    command
        .arg("-I")
        .arg(script_path)
        .args(args)
        .current_dir(workspace_root);
    configure_private_environment(
        &mut command,
        Some(script_path.parent().expect("private script has a parent")),
    );
    run_bounded_process(command, timeout, cancellation, action_cancel_flag)
}

fn configure_private_environment(command: &mut Command, private_temp: Option<&Path>) {
    command.env_clear();
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    command.env("LANG", "C.UTF-8");
    command.env("LC_ALL", "C.UTF-8");
    command.env("TERM", "dumb");
    command.env("CI", "1");
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    if let Some(private_temp) = private_temp {
        command.env("TMPDIR", private_temp);
    }
}

struct ProcessOutput {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
    cancelled: bool,
    stdout_truncated: bool,
    stderr_truncated: bool,
    output_capture: ProcessOutputCaptureMetadata,
    stdout_spool: ProcessOutputSpool,
    stderr_spool: ProcessOutputSpool,
}

fn run_bounded_process(
    mut command: Command,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<ProcessOutput, SkillScriptRuntimeError> {
    if cancellation_requested(cancellation, action_cancel_flag) {
        return Ok(ProcessOutput {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled: true,
            stdout_truncated: false,
            stderr_truncated: false,
            output_capture: ProcessOutputCaptureMetadata::default(),
            stdout_spool: ProcessOutputSpool::default(),
            stderr_spool: ProcessOutputSpool::default(),
        });
    }
    configure_command_process_group(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| process_error(format!("Cannot start Skill script process: {error}")))?;
    let capture_policy = ProcessOutputCapturePolicy::process_default();
    let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
    let stdout_reader = spawn_process_output_capture(
        child
            .stdout
            .take()
            .ok_or_else(|| process_error("Cannot capture Skill script stdout."))?,
        capture_budget.clone(),
        capture_policy,
    );
    let stderr_reader = spawn_process_output_capture(
        child
            .stderr
            .take()
            .ok_or_else(|| process_error("Cannot capture Skill script stderr."))?,
        capture_budget,
        capture_policy,
    );
    let started = Instant::now();
    let mut timed_out = false;
    let mut cancelled = false;
    let status = loop {
        if cancellation_requested(cancellation, action_cancel_flag) {
            cancelled = true;
            terminate_command_process_group(&mut child);
            break wait_for_child(&mut child, "cancelled")?;
        }
        match try_wait_command_process_group(&mut child)
            .map_err(|error| process_error(format!("Cannot wait for Skill script: {error}")))?
        {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                timed_out = true;
                terminate_command_process_group(&mut child);
                break wait_for_child(&mut child, "timed out")?;
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    let stdout_capture =
        join_process_output_capture(stdout_reader, "Skill script stdout").map_err(process_error)?;
    let stderr_capture =
        join_process_output_capture(stderr_reader, "Skill script stderr").map_err(process_error)?;
    let output_capture =
        ProcessOutputCaptureMetadata::from_streams(&stdout_capture, &stderr_capture);
    Ok(ProcessOutput {
        exit_code: status.code(),
        stdout: stdout_capture.preview().to_string(),
        stderr: stderr_capture.preview().to_string(),
        timed_out,
        cancelled,
        stdout_truncated: stdout_capture.preview_truncated(),
        stderr_truncated: stderr_capture.preview_truncated(),
        output_capture,
        stdout_spool: stdout_capture.spool(),
        stderr_spool: stderr_capture.spool(),
    })
}

fn wait_for_child(
    child: &mut Child,
    state: &str,
) -> Result<std::process::ExitStatus, SkillScriptRuntimeError> {
    child
        .wait()
        .map_err(|error| process_error(format!("Cannot wait for {state} Skill script: {error}")))
}

fn cancellation_requested(
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> bool {
    cancellation.is_cancelled()
        || action_cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

fn write_private_script(path: &Path, bytes: &[u8]) -> Result<(), SkillScriptRuntimeError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .map_err(|error| io_error("create private script snapshot", error))?;
    file.write_all(bytes)
        .map_err(|error| io_error("write private script snapshot", error))?;
    file.sync_all()
        .map_err(|error| io_error("sync private script snapshot", error))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), SkillScriptRuntimeError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("protect private script directory", error))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), SkillScriptRuntimeError> {
    Err(SkillScriptRuntimeError::new(
        SkillScriptRuntimeErrorCode::UnsupportedPlatform,
        SkillScriptRuntimeRecovery::ChangeRequest,
        "Private Skill script snapshots are unavailable on this platform.",
    ))
}

fn missing_report(
    interpreter: AgentSkillScriptInterpreter,
    dependencies: Vec<AgentSkillDependencyCheck>,
    code: &str,
    message: &str,
) -> SkillScriptPreflightOutcome {
    SkillScriptPreflightOutcome::MissingDependencies {
        report: AgentSkillScriptPreflightReport {
            status: AgentSkillScriptPreflightStatus::MissingDependencies,
            interpreter,
            interpreter_version: None,
            dependencies,
            runtime_fingerprint: String::new(),
            error_code: Some(code.to_string()),
            message: Some(message.to_string()),
        },
    }
}

fn unsupported_report(
    interpreter: AgentSkillScriptInterpreter,
    code: &str,
    message: &str,
) -> SkillScriptPreflightOutcome {
    SkillScriptPreflightOutcome::Unsupported {
        report: AgentSkillScriptPreflightReport {
            status: AgentSkillScriptPreflightStatus::Unsupported,
            interpreter,
            interpreter_version: None,
            dependencies: Vec::new(),
            runtime_fingerprint: String::new(),
            error_code: Some(code.to_string()),
            message: Some(message.to_string()),
        },
    }
}

fn cancelled_result(
    request: &AgentSkillScriptRequest,
    report: AgentSkillScriptPreflightReport,
    started: Instant,
) -> AgentSkillScriptResult {
    AgentSkillScriptResult {
        script_uri: request.script_uri.clone(),
        skill_id: request.skill_id.clone(),
        skill_revision: request.skill_revision.clone(),
        resource_digest: request.resource_digest.clone(),
        preflight: report,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        error_code: Some("skill_script.cancelled".to_string()),
        error: Some("Skill script execution was cancelled before launch.".to_string()),
    }
}

fn execution_result_error(output: &ProcessOutput) -> (Option<String>, Option<String>) {
    if output.cancelled {
        return (
            Some("skill_script.cancelled".to_string()),
            Some("Skill script execution was cancelled.".to_string()),
        );
    }
    if output.timed_out {
        return (
            Some("skill_script.timeout".to_string()),
            Some("Skill script execution exceeded its timeout.".to_string()),
        );
    }
    match output.exit_code {
        Some(0) => (None, None),
        Some(exit_code) => (
            Some("skill_script.nonzero_exit".to_string()),
            Some(format!(
                "Skill script exited with non-zero status {exit_code}."
            )),
        ),
        None => (
            Some("skill_script.terminated_without_exit_code".to_string()),
            Some("Skill script terminated without an exit code.".to_string()),
        ),
    }
}

fn invalid_request(message: impl Into<String>) -> SkillScriptRuntimeError {
    SkillScriptRuntimeError::new(
        SkillScriptRuntimeErrorCode::InvalidRequest,
        SkillScriptRuntimeRecovery::ChangeRequest,
        message,
    )
}

fn runtime_conflict(message: impl Into<String>) -> SkillScriptRuntimeError {
    SkillScriptRuntimeError::new(
        SkillScriptRuntimeErrorCode::RuntimeConflict,
        SkillScriptRuntimeRecovery::Retry,
        message,
    )
}

fn io_error(operation: &str, error: std::io::Error) -> SkillScriptRuntimeError {
    SkillScriptRuntimeError::new(
        SkillScriptRuntimeErrorCode::Io,
        SkillScriptRuntimeRecovery::Retry,
        format!("Cannot {operation}: {error}"),
    )
}

fn process_error(message: impl Into<String>) -> SkillScriptRuntimeError {
    SkillScriptRuntimeError::new(
        SkillScriptRuntimeErrorCode::ProcessFailure,
        SkillScriptRuntimeRecovery::Retry,
        message,
    )
}

// Avoid accepting future path-bearing cwd extensions without an explicit
// containment review.
#[allow(dead_code)]
fn clean_workspace_relative_path(value: &str) -> Result<PathBuf, SkillScriptRuntimeError> {
    let mut output = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(invalid_request(
                    "Skill script cwd must stay inside the workspace.",
                ));
            }
        }
    }
    Ok(output)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::skills::digest::package_file_digest;
    use crate::skills::model::{
        SkillId, SkillResourceDescriptor, SkillResourceIndex, SkillRevision, SkillSourceId,
    };
    use crate::skills::resource_runtime::{
        SkillResourceReader, SkillResourceSessionBinding, SkillResourceSourceError,
    };
    use crate::AgentApprovalStatus;
    use std::os::unix::fs::PermissionsExt;

    struct ScriptReader {
        path: String,
        bytes: Vec<u8>,
    }

    impl SkillResourceReader for ScriptReader {
        fn read(
            &self,
            expected: &SkillResourceDescriptor,
        ) -> Result<Vec<u8>, SkillResourceSourceError> {
            if expected.path() == self.path {
                Ok(self.bytes.clone())
            } else {
                Err(SkillResourceSourceError::Unavailable(
                    "missing test script".to_string(),
                ))
            }
        }
    }

    struct Fixture {
        session: SkillResourceSession,
        uri: SkillResourceUri,
        digest: String,
        workspace: tempfile::TempDir,
    }

    fn fixture(source: &str) -> Fixture {
        let bytes = source.as_bytes().to_vec();
        let digest = package_file_digest(&bytes);
        let source_id = SkillSourceId::parse("installed:user").unwrap();
        let skill_id =
            SkillId::parse("installed:user:01234567-89ab-4def-8123-456789abcdef").unwrap();
        let revision =
            SkillRevision::parse(format!("skill-package-sha256-v3:{}", "a".repeat(64))).unwrap();
        let path = "scripts/test.py".to_string();
        let descriptor = SkillResourceDescriptor::new(
            path.clone(),
            SkillResourceKind::Script,
            bytes.len() as u64,
            digest.clone(),
        );
        let session = SkillResourceSession::from_bindings([SkillResourceSessionBinding {
            skill_id: skill_id.clone(),
            revision: revision.clone(),
            source_id,
            resources: SkillResourceIndex::new(vec![descriptor]),
            reader: Some(Arc::new(ScriptReader {
                path: path.clone(),
                bytes,
            })),
        }])
        .unwrap();
        let package = super::super::SkillPackageUri::new(skill_id, revision);
        let uri = package.resource(super::super::SkillResourcePath::parse(path).unwrap());
        Fixture {
            session,
            uri,
            digest,
            workspace: tempfile::tempdir().unwrap(),
        }
    }

    fn ready_request(fixture: &Fixture, args: Vec<String>) -> Option<AgentSkillScriptRequest> {
        let outcome = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &AgentSkillScriptRequirements::default(),
        )
        .unwrap();
        let SkillScriptPreflightOutcome::Ready { report, .. } = outcome else {
            // The core reports the missing host dependency correctly; machines
            // without Python cannot exercise execution-specific assertions.
            return None;
        };
        Some(AgentSkillScriptRequest {
            id: "script-test".to_string(),
            script_uri: fixture.uri.to_string(),
            skill_id: fixture.uri.package().skill_id().to_string(),
            skill_revision: fixture.uri.package().revision().to_string(),
            resource_path: fixture.uri.path().to_string(),
            resource_digest: fixture.digest.clone(),
            interpreter: AgentSkillScriptInterpreter::Python3,
            args,
            requirements: AgentSkillScriptRequirements::default(),
            preflight: report,
            timeout_ms: Some(2_000),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        })
    }

    fn execute(
        fixture: &Fixture,
        request: &AgentSkillScriptRequest,
    ) -> Result<AgentSkillScriptResult, SkillScriptRuntimeError> {
        execute_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            request,
            AgentCancellationToken::new(),
            None,
        )
    }

    #[test]
    fn preflight_reports_missing_distribution_without_importing_it() {
        let fixture = fixture("print('unused')\n");
        let requirements = AgentSkillScriptRequirements {
            python_distributions: vec![
                "mycopilot-test-package-that-cannot-exist-7d22bb68".to_string()
            ],
            commands: Vec::new(),
        };
        let outcome = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &requirements,
        )
        .unwrap();
        if resolve_python3(fixture.workspace.path()).is_none() {
            assert_eq!(
                outcome.report().status,
                AgentSkillScriptPreflightStatus::MissingDependencies
            );
            return;
        }
        let report = outcome.report();
        assert_eq!(
            report.status,
            AgentSkillScriptPreflightStatus::MissingDependencies
        );
        assert_eq!(report.dependencies.len(), 1);
        assert_eq!(
            report.dependencies[0].status,
            AgentSkillDependencyStatus::Missing
        );
    }

    #[test]
    fn preflight_never_launches_a_workspace_interpreter() {
        let fixture = fixture("print('unused')\n");
        let bin = fixture.workspace.path().join(".venv/bin");
        fs::create_dir_all(&bin).unwrap();
        fs::write(
            fixture.workspace.path().join(".venv/pyvenv.cfg"),
            "version = 3.12.1\n",
        )
        .unwrap();
        let marker = fixture.workspace.path().join("interpreter-was-run");
        let interpreter = bin.join("python3");
        fs::write(
            &interpreter,
            format!("#!/bin/sh\ntouch '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&interpreter, fs::Permissions::from_mode(0o755)).unwrap();

        let outcome = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &AgentSkillScriptRequirements::default(),
        )
        .unwrap();

        if resolve_python3(fixture.workspace.path()).is_some() {
            assert!(matches!(outcome, SkillScriptPreflightOutcome::Ready { .. }));
        } else {
            assert_eq!(
                outcome.report().status,
                AgentSkillScriptPreflightStatus::MissingDependencies
            );
        }
        assert!(!marker.exists());
    }

    #[test]
    fn workspace_interpreter_candidate_does_not_hide_later_host_python() {
        let fixture = tempfile::tempdir().unwrap();
        let workspace_root = fixture.path().canonicalize().unwrap();
        let workspace_python = workspace_root.join("python3");
        fs::write(&workspace_python, "#!/bin/sh\nexit 99\n").unwrap();
        fs::set_permissions(&workspace_python, fs::Permissions::from_mode(0o755)).unwrap();
        let Some(host_python) = path_candidates("python3")
            .chain(path_candidates("python"))
            .filter_map(python_executable_canonical_path)
            .find(|path| !path.starts_with(&workspace_root))
        else {
            return;
        };

        let resolved = resolve_python3_from_candidates(
            &workspace_root,
            [workspace_python, host_python.clone()],
        );
        assert_eq!(resolved.as_deref(), Some(host_python.as_path()));
    }

    #[test]
    fn shell_shim_named_python_is_not_treated_as_an_interpreter() {
        let fixture = tempfile::tempdir().unwrap();
        let shim = fixture.path().join("python3");
        fs::write(&shim, "#!/bin/sh\necho 'not python'\n").unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(python_executable_canonical_path(shim).is_none());
    }

    #[test]
    fn workspace_metadata_cannot_spoof_external_interpreter_identity() {
        let fixture = fixture("print('unused')\n");
        let first = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &AgentSkillScriptRequirements::default(),
        )
        .unwrap();
        let SkillScriptPreflightOutcome::Ready {
            report: first_report,
            ..
        } = first
        else {
            return;
        };

        fs::create_dir_all(fixture.workspace.path().join(".venv")).unwrap();
        fs::write(
            fixture.workspace.path().join(".venv/pyvenv.cfg"),
            "version = 3.99.999-spoofed\n",
        )
        .unwrap();
        let second = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &AgentSkillScriptRequirements::default(),
        )
        .unwrap();
        let SkillScriptPreflightOutcome::Ready {
            report: second_report,
            ..
        } = second
        else {
            panic!("external interpreter disappeared during fixture")
        };
        assert_eq!(
            second_report.interpreter_version,
            first_report.interpreter_version
        );
        assert_eq!(
            second_report.runtime_fingerprint,
            first_report.runtime_fingerprint
        );
    }

    #[test]
    fn command_preflight_accepts_non_python_executables() {
        let Some(command_path) = path_candidates("sh").find_map(executable_canonical_path) else {
            return;
        };
        assert!(!command_path.as_os_str().is_empty());
        let fixture = fixture("print('unused')\n");
        let outcome = preflight_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &fixture.uri,
            AgentSkillScriptInterpreter::Python3,
            &AgentSkillScriptRequirements {
                python_distributions: Vec::new(),
                commands: vec!["sh".to_string()],
            },
        )
        .unwrap();
        if resolve_python3(fixture.workspace.path()).is_none() {
            return;
        }
        assert_eq!(outcome.report().dependencies.len(), 1);
        assert_eq!(
            outcome.report().dependencies[0].status,
            AgentSkillDependencyStatus::Available
        );
    }

    #[test]
    fn oversized_distribution_metadata_is_not_trusted() {
        let fixture = tempfile::tempdir().unwrap();
        let dist_info = fixture.path().join("example-1.0.dist-info");
        fs::create_dir(&dist_info).unwrap();
        let mut metadata = b"Name: example\nVersion: 1.0\n".to_vec();
        metadata.resize(
            usize::try_from(MAX_PYTHON_DISTRIBUTION_METADATA_BYTES).unwrap() + 1,
            b'x',
        );
        fs::write(dist_info.join("METADATA"), metadata).unwrap();
        assert!(read_distribution_metadata(&dist_info).is_none());
    }

    #[test]
    fn raw_host_environment_capabilities_are_not_part_of_the_protocol() {
        let error = serde_json::from_value::<AgentSkillScriptRequirements>(serde_json::json!({
            "environment": ["OPENAI_API_KEY"]
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field `environment`"));
    }

    #[test]
    fn argv_shell_metacharacters_are_passed_literally() {
        let fixture = fixture("import sys\nprint(sys.argv[1])\n");
        let literal = "$(printf injected); echo nope".to_string();
        let Some(request) = ready_request(&fixture, vec![literal.clone()]) else {
            return;
        };
        let result = execute(&fixture, &request).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.trim(), literal);
        assert!(result.stderr.is_empty());
    }

    #[test]
    fn undeclared_host_environment_is_not_inherited() {
        let fixture = fixture("import os\nprint(os.environ.get('HOME', '<absent>'))\n");
        let Some(request) = ready_request(&fixture, Vec::new()) else {
            return;
        };
        let result = execute(&fixture, &request).unwrap();
        assert_eq!(result.exit_code, Some(0));
        assert_eq!(result.stdout.trim(), "<absent>");
    }

    #[test]
    fn nonzero_exit_preserves_stdout_and_stderr() {
        let fixture = fixture(
            "import sys\nprint('before failure')\nprint('diagnostic', file=sys.stderr)\nsys.exit(23)\n",
        );
        let Some(request) = ready_request(&fixture, Vec::new()) else {
            return;
        };
        let result = execute(&fixture, &request).unwrap();
        assert_eq!(result.exit_code, Some(23));
        assert!(result.stdout.contains("before failure"));
        assert!(result.stderr.contains("diagnostic"));
        assert!(!result.timed_out);
        assert_eq!(
            result.error_code.as_deref(),
            Some("skill_script.nonzero_exit")
        );
    }

    #[test]
    fn timeout_terminates_the_script_process_group() {
        let fixture = fixture("import time\ntime.sleep(30)\n");
        let Some(mut request) = ready_request(&fixture, Vec::new()) else {
            return;
        };
        request.timeout_ms = Some(50);
        let result = execute(&fixture, &request).unwrap();
        assert!(result.timed_out);
        assert!(!result.cancelled);
        assert_ne!(result.exit_code, Some(0));
        assert_eq!(result.error_code.as_deref(), Some("skill_script.timeout"));
    }

    #[test]
    fn prelaunch_cancellation_is_reported_without_starting_the_script() {
        let fixture = fixture("print('must not run')\n");
        let Some(request) = ready_request(&fixture, Vec::new()) else {
            return;
        };
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
        let result = execute_skill_python_script(
            &fixture.session,
            fixture.workspace.path(),
            &request,
            cancellation,
            None,
        )
        .unwrap();
        assert!(result.cancelled);
        assert_eq!(result.exit_code, None);
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
        assert_eq!(result.error_code.as_deref(), Some("skill_script.cancelled"));
    }

    #[test]
    fn running_cancellation_preserves_partial_output_and_prevents_late_effects() {
        let fixture = fixture(concat!(
            "import pathlib, sys, time\n",
            "print('stdout before cancellation', flush=True)\n",
            "print('stderr before cancellation', file=sys.stderr, flush=True)\n",
            "pathlib.Path(sys.argv[1]).write_text('started')\n",
            "time.sleep(30)\n",
            "pathlib.Path(sys.argv[2]).write_text('late effect')\n",
        ));
        let started = fixture.workspace.path().join("script-started");
        let late_effect = fixture.workspace.path().join("late-effect");
        let Some(request) = ready_request(
            &fixture,
            vec![
                started.to_string_lossy().into_owned(),
                late_effect.to_string_lossy().into_owned(),
            ],
        ) else {
            return;
        };
        let cancel_flag = Arc::new(AtomicBool::new(false));

        let result = thread::scope(|scope| {
            let execution_flag = Arc::clone(&cancel_flag);
            let handle = scope.spawn(|| {
                execute_skill_python_script(
                    &fixture.session,
                    fixture.workspace.path(),
                    &request,
                    AgentCancellationToken::new(),
                    Some(execution_flag),
                )
                .unwrap()
            });
            let deadline = Instant::now() + Duration::from_secs(3);
            while !started.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(10));
            }
            assert!(
                started.exists(),
                "script did not reach its cancellation point"
            );
            cancel_flag.store(true, Ordering::SeqCst);
            handle.join().unwrap()
        });

        assert!(result.cancelled);
        assert!(!result.timed_out);
        assert!(result.stdout.contains("stdout before cancellation"));
        assert!(result.stderr.contains("stderr before cancellation"));
        assert!(!late_effect.exists());
        assert_eq!(result.error_code.as_deref(), Some("skill_script.cancelled"));
    }

    #[test]
    fn frozen_digest_and_revision_conflicts_fail_closed() {
        let fixture = fixture("print('must not run')\n");
        let Some(mut request) = ready_request(&fixture, Vec::new()) else {
            return;
        };
        request.resource_digest = format!("skill-file-sha256-v1:{}", "f".repeat(64));
        let error = execute(&fixture, &request).unwrap_err();
        assert_eq!(error.code(), SkillScriptRuntimeErrorCode::RuntimeConflict);

        request.resource_digest = fixture.digest.clone();
        request.skill_revision = format!("skill-package-sha256-v3:{}", "b".repeat(64));
        let error = execute(&fixture, &request).unwrap_err();
        assert_eq!(error.code(), SkillScriptRuntimeErrorCode::RuntimeConflict);
    }
}
