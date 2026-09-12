use super::types::{OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery};
use crate::command::{join_output_reader, spawn_bounded_output_reader};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub const OFFICE_RENDER_RUNTIME_PROVIDER_ID: &str = "mycopilot.office-render-runtime";
pub const OFFICE_RENDER_RUNTIME_BUNDLE_VERSION: &str = "2026.07.2";

const COMPONENT_DIRECTORY: &str = "office-renderer";
const COMPONENT_RECEIPT: &str = "component-receipt.json";
const RUNTIME_REVISION_PREFIX: &str = "office-render-runtime-sha256-v1:";
const EXPECTED_BROWSER_FAMILY: &str = "chromium";
const EXPECTED_BROWSER_VERSION: &str = "149.0.7827.55";
const EXPECTED_PLAYWRIGHT_VERSION: &str = "1.61.1";
const EXPECTED_PLAYWRIGHT_REVISION: &str = "1228";
const MAX_RECEIPT_BYTES: u64 = 1024 * 1024;
const MAX_COMPONENT_FILES: usize = 256;
const MAX_COMPONENT_FILE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_COMPONENT_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_RENDER_MARKER_BYTES: u64 = 8 * 1024;
const MAX_RENDER_FAILURE_MESSAGE_BYTES: usize = 2 * 1024;
const BROWSER_PROXY_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const BROWSER_PROXY_SELF_TEST_TIMEOUT: Duration = Duration::from_secs(5);
const BROWSER_MARKER_LOCK_TIMEOUT: Duration = Duration::from_secs(1);
const BROWSER_MARKER_SCHEMA_VERSION: u32 = 1;
pub(crate) const BROWSER_PROXY_SELF_TEST_ARGUMENT: &str =
    "--mycopilot-office-browser-proxy-self-test";
pub(crate) const BROWSER_PROXY_SELF_TEST_RESPONSE: &str = "mycopilot-office-browser-proxy-v1\n";

pub(crate) const BROWSER_PROXY_MODE_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_PROXY_MODE";
pub(crate) const BROWSER_EXECUTABLE_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_EXECUTABLE";
pub(crate) const BROWSER_PROFILE_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_PROFILE";
pub(crate) const BROWSER_FAILURE_MARKER_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_FAILURE_MARKER";
pub(crate) const BROWSER_MARKER_NONCE_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_MARKER_NONCE";
pub(crate) const BROWSER_MAX_INVOCATIONS_ENV: &str = "MYCOPILOT_OFFICE_BROWSER_MAX_INVOCATIONS";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum OfficeRenderMarkerStatus {
    Pending,
    Running,
    Success,
    Failure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OfficeRenderMarker {
    schema_version: u32,
    nonce: String,
    status: OfficeRenderMarkerStatus,
    invocation_count: u32,
    max_invocations: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    timed_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OfficeRenderFailureMarker {
    pub error_code: String,
    pub message: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Default)]
pub struct OfficeRenderRuntimeDiscoveryOptions {
    configured_component_dir: Option<PathBuf>,
    application_resources_dir: Option<PathBuf>,
    workspace_roots: Vec<PathBuf>,
}

impl OfficeRenderRuntimeDiscoveryOptions {
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
pub struct OfficeRenderRuntime {
    root: PathBuf,
    executable: PathBuf,
    receipt: ComponentReceipt,
    files: BTreeMap<String, VerifiedFile>,
}

#[derive(Debug, Clone)]
struct VerifiedFile {
    #[cfg(not(unix))]
    size: u64,
    #[cfg(not(unix))]
    sha256: String,
    identity: FileIdentity,
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

impl OfficeRenderRuntime {
    pub fn discover(
        options: &OfficeRenderRuntimeDiscoveryOptions,
    ) -> Result<Self, OfficeEngineError> {
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
                "The Office render runtime must not be loaded from the agent-writable workspace.",
            ));
        }
        let receipt = read_receipt(&root)?;
        validate_receipt(&receipt)?;
        let files = verify_component_tree(&root, &receipt)?;
        let executable = resolve_component_file(&root, &receipt.browser.executable)?;
        let metadata = fs::metadata(&executable).map_err(|error| {
            invalid_component(format!(
                "Cannot inspect the Office render runtime executable: {error}"
            ))
        })?;
        if !metadata.is_file() || !is_executable(&metadata) {
            return Err(invalid_component(
                "The Office render runtime browser is not an executable regular file.",
            ));
        }
        Ok(Self {
            root,
            executable,
            receipt,
            files,
        })
    }

    pub fn executable_path(&self) -> &Path {
        &self.executable
    }

    pub(crate) fn component_root(&self) -> &Path {
        &self.root
    }

    pub fn runtime_revision(&self) -> &str {
        &self.receipt.bundle_revision
    }

    pub fn browser_version(&self) -> &str {
        &self.receipt.browser.version
    }

    pub fn verify_integrity(&self) -> Result<(), OfficeEngineError> {
        let current = read_receipt(&self.root)?;
        if current != self.receipt {
            return Err(integrity_error(
                "The Office render runtime receipt changed after discovery.",
            ));
        }
        let actual = collect_component_files(&self.root)?;
        if actual.len() != self.files.len() || actual.keys().ne(self.files.keys()) {
            return Err(integrity_error(
                "The Office render runtime file set changed after discovery.",
            ));
        }
        for (relative, expected) in &self.files {
            let path = actual.get(relative).ok_or_else(|| {
                integrity_error("An Office render runtime file disappeared after discovery.")
            })?;
            let metadata = fs::symlink_metadata(path).map_err(|error| {
                integrity_error(format!(
                    "Cannot reinspect Office render runtime file: {error}"
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(integrity_error(
                    "An Office render runtime file changed type after discovery.",
                ));
            }
            let identity = file_identity(&metadata);
            if identity != expected.identity {
                return Err(integrity_error(
                    "An Office render runtime file identity changed after discovery.",
                ));
            }
            #[cfg(not(unix))]
            verify_file(path, expected.size, &expected.sha256)?;
        }
        Ok(())
    }
}

pub fn office_render_component_relative_path() -> PathBuf {
    PathBuf::from(COMPONENT_DIRECTORY)
}

pub fn office_browser_proxy_mode_requested() -> bool {
    std::env::var_os(BROWSER_PROXY_MODE_ENV).as_deref() == Some(OsStr::new("1"))
}

/// Entry point used when the trusted core-server executable is invoked through
/// the private `google-chrome` alias created for OfficeCLI. It accepts only the
/// fixed Chrome argument surface emitted by OfficeCLI v1.0.139, appends the
/// application-owned isolation policy, and never invokes a shell.
pub fn run_office_browser_proxy() -> i32 {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.as_slice() == [OsString::from(BROWSER_PROXY_SELF_TEST_ARGUMENT)] {
        let _ = std::io::stdout().write_all(BROWSER_PROXY_SELF_TEST_RESPONSE.as_bytes());
        return 0;
    }
    let marker_context = proxy_marker_context();
    let invocation = marker_context
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|(path, nonce)| begin_proxy_invocation(path, nonce));
    let result = invocation
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|_| run_browser_proxy_inner(&args));
    match result {
        Ok(output) => {
            let marker_written = marker_context
                .as_ref()
                .ok()
                .zip(invocation.as_ref().ok())
                .is_some_and(|((path, nonce), count)| {
                    commit_proxy_success(path, nonce, *count).is_ok()
                });
            if !marker_written {
                let _ = synthesize_failure_output(&args);
                let _ = writeln!(
                    std::io::stderr(),
                    "Cannot commit the private Office render success marker."
                );
                return 0;
            }
            let _ = std::io::stdout().write_all(&output.stdout);
            let _ = std::io::stderr().write_all(&output.stderr);
            0
        }
        Err(failure) => {
            if let (Ok((path, nonce)), Ok(count)) = (&marker_context, &invocation) {
                let _ = commit_proxy_failure(path, nonce, *count, &failure);
            }
            let _ = synthesize_failure_output(&args);
            let _ = writeln!(std::io::stderr(), "{}", failure.message);
            // OfficeCLI otherwise falls through to user-installed Chrome or
            // Firefox. A synthetic provider result keeps its fallback closed;
            // the parent reads the authenticated private marker and replaces
            // this sentinel with the structured failure.
            0
        }
    }
}

fn proxy_marker_context() -> Result<(PathBuf, String), OfficeRenderFailureMarker> {
    let path = required_absolute_env_path(BROWSER_FAILURE_MARKER_ENV)?;
    let nonce = std::env::var(BROWSER_MARKER_NONCE_ENV).map_err(|_| {
        render_failure(
            "office.render_backend_failed",
            "The private Office render marker nonce is missing.",
            false,
        )
    })?;
    validate_marker_nonce(&nonce)?;
    Ok((path, nonce))
}

#[derive(Debug)]
struct BrowserProxyOutput {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

fn run_browser_proxy_inner(
    args: &[OsString],
) -> Result<BrowserProxyOutput, OfficeRenderFailureMarker> {
    let executable = required_absolute_env_path(BROWSER_EXECUTABLE_ENV)?;
    let profile = required_absolute_env_path(BROWSER_PROFILE_ENV)?;
    run_browser_proxy_with_paths(args, &executable, &profile)
}

fn run_browser_proxy_with_paths(
    args: &[OsString],
    executable: &Path,
    profile: &Path,
) -> Result<BrowserProxyOutput, OfficeRenderFailureMarker> {
    validate_browser_arguments(args)?;
    let metadata = fs::metadata(executable).map_err(|error| {
        render_failure(
            "office.render_backend_unavailable",
            format!("Cannot inspect the application-managed Chromium executable: {error}"),
            false,
        )
    })?;
    if !metadata.is_file() || !is_executable(&metadata) {
        return Err(render_failure(
            "office.render_backend_unavailable",
            "The application-managed Chromium executable is unavailable.",
            false,
        ));
    }
    fs::create_dir_all(profile).map_err(|error| {
        render_failure(
            "office.render_backend_failed",
            format!("Cannot create the private Office browser profile: {error}"),
            false,
        )
    })?;
    set_private_directory_permissions(profile).map_err(|error| {
        render_failure(
            "office.render_backend_failed",
            format!("Cannot secure the private Office browser profile: {error}"),
            false,
        )
    })?;

    let mut command = Command::new(executable);
    command
        .arg(format!("--user-data-dir={}", profile.to_string_lossy()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-component-update")
        .arg("--disable-background-networking")
        // Browser-backed Office rendering is an offline operation. Route every
        // page request to a closed loopback proxy and disable Chromium's
        // implicit loopback bypass so local HTML cannot fetch remote or local
        // network resources while it is being rendered.
        .arg("--proxy-server=http://127.0.0.1:9")
        .arg("--proxy-bypass-list=<-loopback>")
        .arg("--host-resolver-rules=MAP * ~NOTFOUND")
        .arg("--disable-quic")
        .arg("--force-webrtc-ip-handling-policy=disable_non_proxied_udp")
        .arg("--disable-sync")
        .arg("--disable-default-apps")
        .arg("--disable-extensions")
        .arg("--disable-breakpad")
        .arg("--disable-crash-reporter");
    #[cfg(target_os = "macos")]
    command.arg("--use-mock-keychain");
    #[cfg(target_os = "linux")]
    command.arg("--password-store=basic");
    // `--no-sandbox` is intentionally consumed rather than forwarded. The
    // managed browser keeps its native sandbox even though upstream OfficeCLI
    // includes that switch for generic CI environments.
    command.args(
        args.iter()
            .filter(|arg| arg.as_os_str() != OsStr::new("--no-sandbox")),
    );
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Chromium deliberately inherits the OfficeCLI process group. The outer
    // executor can therefore terminate OfficeCLI, this proxy, Chromium, and
    // Chromium's children as one lifecycle unit on cancellation or timeout.
    // Giving Chromium its own group would let it survive if this proxy were
    // killed before it could run cleanup code.
    let mut child = command.spawn().map_err(|error| {
        render_failure(
            "office.render_backend_unavailable",
            format!("Cannot start the application-managed Chromium runtime: {error}"),
            false,
        )
    })?;
    let stdout_reader = spawn_bounded_output_reader(child.stdout.take().ok_or_else(|| {
        render_failure(
            "office.render_backend_failed",
            "Cannot capture Chromium stdout.",
            false,
        )
    })?);
    let stderr_reader = spawn_bounded_output_reader(child.stderr.take().ok_or_else(|| {
        render_failure(
            "office.render_backend_failed",
            "Cannot capture Chromium stderr.",
            false,
        )
    })?);
    let started = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|error| {
            render_failure(
                "office.render_backend_failed",
                format!("Cannot wait for the managed Chromium runtime: {error}"),
                false,
            )
        })? {
            Some(status) => break status,
            None if started.elapsed() >= BROWSER_PROXY_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                // Do not join the readers on timeout: a Chromium descendant
                // could still hold an inherited pipe briefly. The outer
                // OfficeCLI group owns the full process-tree cleanup.
                drop(stdout_reader);
                drop(stderr_reader);
                return Err(render_failure(
                    "office.render_backend_timeout",
                    "The application-managed Chromium render exceeded its 30-second deadline.",
                    true,
                ));
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };
    let (stdout, _) = join_output_reader(stdout_reader, "managed Chromium stdout")
        .map_err(|error| render_failure("office.render_backend_failed", error, false))?;
    let (stderr, _) = join_output_reader(stderr_reader, "managed Chromium stderr")
        .map_err(|error| render_failure("office.render_backend_failed", error, false))?;
    if !status.success() {
        let detail = stderr
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        return Err(render_failure(
            "office.render_backend_failed",
            if detail.is_empty() {
                format!("The application-managed Chromium runtime exited with status {status}.")
            } else {
                format!("The application-managed Chromium runtime failed: {detail}")
            },
            false,
        ));
    }
    if let Some(path) = screenshot_output_path(args) {
        let valid =
            fs::metadata(&path).is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0);
        if !valid {
            return Err(render_failure(
                "office.render_backend_failed",
                "The application-managed Chromium runtime produced no screenshot.",
                false,
            ));
        }
    } else if args.iter().any(|arg| arg == OsStr::new("--dump-dom")) && stdout.trim().is_empty() {
        return Err(render_failure(
            "office.render_backend_failed",
            "The application-managed Chromium runtime produced no DOM output.",
            false,
        ));
    }
    Ok(BrowserProxyOutput {
        stdout: stdout.into_bytes(),
        stderr: stderr.into_bytes(),
    })
}

fn validate_browser_arguments(args: &[OsString]) -> Result<(), OfficeRenderFailureMarker> {
    if args.is_empty() || args.len() > 32 {
        return Err(render_failure(
            "office.render_backend_failed",
            "OfficeCLI supplied an invalid managed browser argument count.",
            false,
        ));
    }
    let mut has_operation = false;
    let mut has_file_url = false;
    for argument in args {
        let value = argument.to_str().ok_or_else(|| {
            render_failure(
                "office.render_backend_failed",
                "OfficeCLI supplied a non-UTF-8 managed browser argument.",
                false,
            )
        })?;
        let allowed = matches!(
            value,
            "--headless=new"
                | "--disable-gpu"
                | "--no-sandbox"
                | "--hide-scrollbars"
                | "--enable-logging=stderr"
                | "--v=0"
                | "--dump-dom"
        ) || [
            "--force-device-scale-factor=",
            "--window-size=",
            "--virtual-time-budget=",
            "--timeout=",
            "--default-background-color=",
            "--screenshot=",
        ]
        .iter()
        .any(|prefix| value.starts_with(prefix))
            || value.starts_with("file://");
        if !allowed {
            return Err(render_failure(
                "office.render_backend_failed",
                format!("OfficeCLI supplied an unsupported managed browser option `{value}`."),
                false,
            ));
        }
        has_operation |= value == "--dump-dom" || value.starts_with("--screenshot=");
        has_file_url |= value.starts_with("file://");
    }
    if !has_operation || !has_file_url {
        return Err(render_failure(
            "office.render_backend_failed",
            "The managed browser request must contain one render operation and a local file URL.",
            false,
        ));
    }
    Ok(())
}

fn required_absolute_env_path(name: &str) -> Result<PathBuf, OfficeRenderFailureMarker> {
    let value = std::env::var_os(name).ok_or_else(|| {
        render_failure(
            "office.render_backend_unavailable",
            format!("Managed browser configuration `{name}` is missing."),
            false,
        )
    })?;
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(render_failure(
            "office.render_backend_unavailable",
            format!("Managed browser configuration `{name}` must be absolute."),
            false,
        ));
    }
    Ok(path)
}

fn screenshot_output_path(args: &[OsString]) -> Option<PathBuf> {
    args.iter().find_map(|argument| {
        argument
            .to_str()
            .and_then(|value| value.strip_prefix("--screenshot="))
            .map(PathBuf::from)
    })
}

fn synthesize_failure_output(args: &[OsString]) -> std::io::Result<()> {
    if let Some(path) = screenshot_output_path(args) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Valid transparent 1x1 PNG. The parent process never publishes it:
        // the private failure marker has precedence over provider exit status.
        const SENTINEL_PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 6, 0, 0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 8, 215, 99, 248, 207,
            192, 240, 31, 0, 5, 0, 1, 255, 137, 153, 61, 29, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
            96, 130,
        ];
        fs::write(path, SENTINEL_PNG)?;
    }
    if args.iter().any(|arg| arg == OsStr::new("--dump-dom")) {
        std::io::stdout().write_all(
            b"<!doctype html><html><head><title>PAGES:0</title></head><body></body></html>\n",
        )?;
    }
    Ok(())
}

pub(crate) fn initialize_render_marker(
    path: &Path,
    nonce: &str,
    max_invocations: u32,
) -> Result<(), OfficeEngineError> {
    validate_marker_nonce(nonce).map_err(marker_engine_error)?;
    if !(1..=2).contains(&max_invocations) {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::RenderBackendFailed,
            OfficeEngineRecovery::Retry,
            "The private Office render invocation limit is invalid.",
        ));
    }
    write_render_marker(path, &pending_marker(nonce, max_invocations)).map_err(|error| {
        OfficeEngineError::new(
            OfficeEngineErrorCode::RenderBackendFailed,
            OfficeEngineRecovery::Retry,
            format!("Cannot initialize the private Office render marker: {error}"),
        )
    })
}

pub(crate) fn read_render_failure(
    path: &Path,
    expected_nonce: &str,
    expected_max_invocations: u32,
    required: bool,
) -> Result<Option<OfficeRenderFailureMarker>, OfficeEngineError> {
    let marker = match read_render_marker(path) {
        Ok(Some(marker)) => marker,
        Ok(None) if !required => return Ok(None),
        Ok(None) => {
            return Ok(Some(render_failure(
                "office.render_backend_failed",
                "The trusted Office browser proxy produced no completion marker.",
                false,
            )))
        }
        Err(error) => {
            return Ok(Some(render_failure(
                "office.render_backend_failed",
                format!("The private Office render completion marker is invalid: {error}"),
                false,
            )))
        }
    };
    if marker.schema_version != BROWSER_MARKER_SCHEMA_VERSION
        || marker.nonce != expected_nonce
        || marker.max_invocations != expected_max_invocations
        || marker.invocation_count > marker.max_invocations
        || validate_marker_nonce(&marker.nonce).is_err()
    {
        return Ok(Some(render_failure(
            "office.render_backend_failed",
            "The private Office render completion marker has the wrong identity.",
            false,
        )));
    }
    match marker.status {
        OfficeRenderMarkerStatus::Pending
            if !required
                && marker.invocation_count == 0
                && marker.error_code.is_none()
                && marker.message.is_none()
                && !marker.timed_out =>
        {
            Ok(None)
        }
        OfficeRenderMarkerStatus::Pending | OfficeRenderMarkerStatus::Running => {
            Ok(Some(render_failure(
                "office.render_backend_failed",
                "The trusted Office browser proxy did not complete the render request.",
                false,
            )))
        }
        OfficeRenderMarkerStatus::Success
            if marker.invocation_count > 0
                && marker.error_code.is_none()
                && marker.message.is_none()
                && !marker.timed_out =>
        {
            Ok(None)
        }
        OfficeRenderMarkerStatus::Failure => {
            let Some(error_code) = marker.error_code else {
                return Ok(Some(invalid_terminal_marker()));
            };
            let Some(message) = marker.message else {
                return Ok(Some(invalid_terminal_marker()));
            };
            if !valid_render_failure_code(&error_code)
                || message.is_empty()
                || message.len() > MAX_RENDER_FAILURE_MESSAGE_BYTES
                || message.chars().any(char::is_control)
                || marker.timed_out != (error_code == "office.render_backend_timeout")
            {
                return Ok(Some(invalid_terminal_marker()));
            }
            Ok(Some(OfficeRenderFailureMarker {
                error_code,
                message,
                timed_out: marker.timed_out,
            }))
        }
        OfficeRenderMarkerStatus::Success => Ok(Some(invalid_terminal_marker())),
    }
}

fn pending_marker(nonce: &str, max_invocations: u32) -> OfficeRenderMarker {
    OfficeRenderMarker {
        schema_version: BROWSER_MARKER_SCHEMA_VERSION,
        nonce: nonce.to_string(),
        status: OfficeRenderMarkerStatus::Pending,
        invocation_count: 0,
        max_invocations,
        error_code: None,
        message: None,
        timed_out: false,
    }
}

fn failure_marker(
    nonce: &str,
    invocation_count: u32,
    max_invocations: u32,
    failure: &OfficeRenderFailureMarker,
) -> OfficeRenderMarker {
    OfficeRenderMarker {
        schema_version: BROWSER_MARKER_SCHEMA_VERSION,
        nonce: nonce.to_string(),
        status: OfficeRenderMarkerStatus::Failure,
        invocation_count,
        max_invocations,
        error_code: Some(failure.error_code.clone()),
        message: Some(bounded_marker_message(&failure.message)),
        timed_out: failure.timed_out,
    }
}

fn bounded_marker_message(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars().filter(|character| !character.is_control()) {
        if output.len() + character.len_utf8() > MAX_RENDER_FAILURE_MESSAGE_BYTES {
            break;
        }
        output.push(character);
    }
    output
}

fn invalid_terminal_marker() -> OfficeRenderFailureMarker {
    render_failure(
        "office.render_backend_failed",
        "The private Office render completion marker has an invalid terminal state.",
        false,
    )
}

fn read_render_marker(path: &Path) -> Result<Option<OfficeRenderMarker>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("cannot inspect it: {error}")),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RENDER_MARKER_BYTES
    {
        return Err("it is not a small regular file".to_string());
    }
    let bytes = fs::read(path).map_err(|error| format!("cannot read it: {error}"))?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| format!("it does not contain a strict marker document: {error}"))
}

fn write_render_marker(path: &Path, marker: &OfficeRenderMarker) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(marker).map_err(std::io::Error::other)?;
    if bytes.len() as u64 > MAX_RENDER_MARKER_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Office render marker exceeds its size limit",
        ));
    }
    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::other(
                "Office render marker changed file type",
            ));
        }
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()
}

struct OfficeRenderMarkerLock {
    path: PathBuf,
    file: Option<File>,
}

impl Drop for OfficeRenderMarkerLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

fn acquire_render_marker_lock(path: &Path) -> Result<OfficeRenderMarkerLock, String> {
    let file_name = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| "the private Office render marker path is invalid".to_string())?;
    let lock_path = path.with_file_name(format!(".{file_name}.lock"));
    let started = Instant::now();
    loop {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&lock_path) {
            Ok(file) => {
                return Ok(OfficeRenderMarkerLock {
                    path: lock_path,
                    file: Some(file),
                });
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && started.elapsed() < BROWSER_MARKER_LOCK_TIMEOUT =>
            {
                thread::sleep(Duration::from_millis(2));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Err("the private Office render marker lock timed out".to_string());
            }
            Err(error) => {
                return Err(format!(
                    "cannot acquire the private Office render marker lock: {error}"
                ));
            }
        }
    }
}

fn begin_proxy_invocation(path: &Path, nonce: &str) -> Result<u32, OfficeRenderFailureMarker> {
    let _lock = acquire_render_marker_lock(path)
        .map_err(|error| render_failure("office.render_backend_failed", error, false))?;
    let mut marker = read_render_marker(path)
        .map_err(|error| render_failure("office.render_backend_failed", error, false))?
        .ok_or_else(|| {
            render_failure(
                "office.render_backend_failed",
                "The private Office render marker is missing.",
                false,
            )
        })?;
    let identity_valid = marker.schema_version == BROWSER_MARKER_SCHEMA_VERSION
        && marker.nonce == nonce
        && validate_marker_nonce(&marker.nonce).is_ok()
        && (1..=2).contains(&marker.max_invocations)
        && marker.invocation_count <= marker.max_invocations;
    if !identity_valid {
        return Err(render_failure(
            "office.render_backend_failed",
            "The private Office render marker has an invalid invocation identity.",
            false,
        ));
    }
    let admissible = marker.invocation_count < marker.max_invocations
        && matches!(
            marker.status,
            OfficeRenderMarkerStatus::Pending | OfficeRenderMarkerStatus::Success
        )
        && marker.error_code.is_none()
        && marker.message.is_none()
        && !marker.timed_out;
    if !admissible {
        let violation = render_failure(
            "office.render_backend_failed",
            "The private Office render marker cannot admit another browser invocation.",
            false,
        );
        // Admission violations are terminal and sticky. In particular, a
        // concurrent or over-limit proxy must not leave an older Success that
        // could authenticate the synthetic fallback output.
        write_render_marker(
            path,
            &failure_marker(
                nonce,
                marker.invocation_count,
                marker.max_invocations,
                &violation,
            ),
        )
        .map_err(|error| {
            render_failure(
                "office.render_backend_failed",
                format!("Cannot persist the private Office browser admission failure: {error}"),
                false,
            )
        })?;
        return Err(violation);
    }
    marker.invocation_count += 1;
    marker.status = OfficeRenderMarkerStatus::Running;
    write_render_marker(path, &marker).map_err(|error| {
        render_failure(
            "office.render_backend_failed",
            format!("Cannot begin the private Office browser invocation: {error}"),
            false,
        )
    })?;
    Ok(marker.invocation_count)
}

fn commit_proxy_success(path: &Path, nonce: &str, count: u32) -> Result<(), String> {
    let _lock = acquire_render_marker_lock(path)?;
    let mut marker = read_render_marker(path)?
        .ok_or_else(|| "the private Office render marker disappeared".to_string())?;
    if marker.schema_version != BROWSER_MARKER_SCHEMA_VERSION
        || marker.nonce != nonce
        || marker.status != OfficeRenderMarkerStatus::Running
        || marker.invocation_count != count
        || count == 0
        || count > marker.max_invocations
    {
        return Err("the private Office render marker changed during execution".to_string());
    }
    marker.status = OfficeRenderMarkerStatus::Success;
    marker.error_code = None;
    marker.message = None;
    marker.timed_out = false;
    write_render_marker(path, &marker).map_err(|error| error.to_string())
}

fn commit_proxy_failure(
    path: &Path,
    nonce: &str,
    count: u32,
    failure: &OfficeRenderFailureMarker,
) -> Result<(), String> {
    let _lock = acquire_render_marker_lock(path)?;
    let marker = read_render_marker(path)?
        .ok_or_else(|| "the private Office render marker disappeared".to_string())?;
    if marker.schema_version != BROWSER_MARKER_SCHEMA_VERSION
        || marker.nonce != nonce
        || validate_marker_nonce(&marker.nonce).is_err()
        || marker.status != OfficeRenderMarkerStatus::Running
        || marker.invocation_count != count
        || count == 0
        || count > marker.max_invocations
    {
        return Err("the private Office render marker identity changed".to_string());
    }
    write_render_marker(
        path,
        &failure_marker(
            nonce,
            marker.invocation_count,
            marker.max_invocations,
            failure,
        ),
    )
    .map_err(|error| error.to_string())
}

fn validate_marker_nonce(nonce: &str) -> Result<(), OfficeRenderFailureMarker> {
    let valid = nonce.len() == 32
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        return Err(render_failure(
            "office.render_backend_failed",
            "The private Office render marker nonce is invalid.",
            false,
        ));
    }
    Ok(())
}

fn valid_render_failure_code(value: &str) -> bool {
    matches!(
        value,
        "office.render_backend_unavailable"
            | "office.render_backend_invalid"
            | "office.render_backend_timeout"
            | "office.render_backend_failed"
    )
}

fn marker_engine_error(marker: OfficeRenderFailureMarker) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::RenderBackendFailed,
        OfficeEngineRecovery::Retry,
        marker.message,
    )
}

fn render_failure(
    error_code: impl Into<String>,
    message: impl Into<String>,
    timed_out: bool,
) -> OfficeRenderFailureMarker {
    OfficeRenderFailureMarker {
        error_code: error_code.into(),
        message: message.into(),
        timed_out,
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(windows)]
fn set_private_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ComponentReceipt {
    schema_version: u32,
    provider_id: String,
    bundle_version: String,
    platform: String,
    arch: String,
    browser: BrowserReceipt,
    archive: ArchiveReceipt,
    files: Vec<FileReceipt>,
    bundle_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserReceipt {
    family: String,
    version: String,
    playwright_version: String,
    revision: String,
    executable: String,
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ReceiptIdentity<'a> {
    schema_version: u32,
    provider_id: &'a str,
    bundle_version: &'a str,
    platform: &'a str,
    arch: &'a str,
    browser: &'a BrowserReceipt,
    archive: &'a ArchiveReceipt,
    files: &'a [FileReceipt],
}

fn resolve_component_root(
    options: &OfficeRenderRuntimeDiscoveryOptions,
) -> Result<PathBuf, OfficeEngineError> {
    let candidate = if let Some(configured) = options.configured_component_dir.as_deref() {
        configured.to_path_buf()
    } else if let Some(resources) = options.application_resources_dir.as_deref() {
        if !resources.is_absolute() {
            return Err(invalid_component(
                "The Office render runtime application resources directory must be absolute.",
            ));
        }
        let resources = canonical_directory(resources)?;
        let candidate = resources.join(office_render_component_relative_path());
        let root = canonical_directory(&candidate).map_err(|_| unavailable_error())?;
        if !root.starts_with(&resources) {
            return Err(invalid_component(
                "The packaged Office render runtime resolves outside application resources.",
            ));
        }
        return Ok(root);
    } else {
        return Err(unavailable_error());
    };

    if !candidate.is_absolute() {
        return Err(invalid_component(
            "The configured Office render runtime directory must be absolute.",
        ));
    }
    canonical_directory(&candidate).map_err(|_| unavailable_error())
}

fn canonical_directory(path: &Path) -> Result<PathBuf, OfficeEngineError> {
    let canonical = path.canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot resolve Office render runtime directory `{}`: {error}",
            path.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(invalid_component(format!(
            "Office render runtime path `{}` is not a directory.",
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
                "Cannot inspect Office render runtime receipt: {error}"
            ))
        }
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_RECEIPT_BYTES
    {
        return Err(invalid_component(
            "The Office render runtime receipt must be a small regular file, not a symlink.",
        ));
    }
    let bytes = fs::read(&path).map_err(|error| {
        invalid_component(format!(
            "Cannot read Office render runtime receipt: {error}"
        ))
    })?;
    if bytes.contains(&0) {
        return Err(invalid_component(
            "The Office render runtime receipt contains a NUL byte.",
        ));
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        invalid_component(format!(
            "The Office render runtime receipt is not valid strict JSON: {error}"
        ))
    })
}

fn validate_receipt(receipt: &ComponentReceipt) -> Result<(), OfficeEngineError> {
    if receipt.schema_version != 2
        || receipt.provider_id != OFFICE_RENDER_RUNTIME_PROVIDER_ID
        || receipt.bundle_version != OFFICE_RENDER_RUNTIME_BUNDLE_VERSION
    {
        return Err(invalid_component(
            "The Office render runtime receipt uses an unsupported schema, provider, or bundle version.",
        ));
    }
    if receipt.platform != current_platform() || receipt.arch != current_arch() {
        return Err(invalid_component(format!(
            "The Office render runtime targets {}-{}, not {}-{}.",
            receipt.platform,
            receipt.arch,
            current_platform(),
            current_arch()
        )));
    }
    if receipt.browser.family != EXPECTED_BROWSER_FAMILY
        || receipt.browser.version != EXPECTED_BROWSER_VERSION
        || receipt.browser.playwright_version != EXPECTED_PLAYWRIGHT_VERSION
        || receipt.browser.revision != EXPECTED_PLAYWRIGHT_REVISION
    {
        return Err(invalid_component(
            "The Office render runtime browser identity does not match the application-pinned Chromium release.",
        ));
    }
    if receipt.archive != expected_archive_receipt() {
        return Err(invalid_component(
            "The Office render runtime archive identity does not match the application-pinned Chromium release.",
        ));
    }
    validate_relative_path(&receipt.browser.executable)?;
    if receipt.files.is_empty() || receipt.files.len() > MAX_COMPONENT_FILES {
        return Err(invalid_component(format!(
            "The Office render runtime receipt must contain between 1 and {MAX_COMPONENT_FILES} files."
        )));
    }
    let mut previous: Option<&str> = None;
    let mut paths = BTreeSet::new();
    let mut total = 0_u64;
    for file in &receipt.files {
        validate_relative_path(&file.path)?;
        if file.path == COMPONENT_RECEIPT {
            return Err(invalid_component(
                "The Office render runtime receipt cannot include itself in the file set.",
            ));
        }
        if previous.is_some_and(|value| value >= file.path.as_str()) || !paths.insert(&file.path) {
            return Err(invalid_component(
                "The Office render runtime files must be unique and sorted by path.",
            ));
        }
        previous = Some(&file.path);
        if file.size > MAX_COMPONENT_FILE_BYTES || !valid_sha256(&file.sha256) {
            return Err(invalid_component(
                "The Office render runtime receipt contains an invalid file size or SHA-256 digest.",
            ));
        }
        total = total.checked_add(file.size).ok_or_else(|| {
            invalid_component("The Office render runtime component size overflowed.")
        })?;
        if total > MAX_COMPONENT_BYTES {
            return Err(invalid_component(
                "The Office render runtime component exceeds the 1 GiB safety limit.",
            ));
        }
    }
    if !paths.contains(&receipt.browser.executable) {
        return Err(invalid_component(
            "The pinned Chromium executable is absent from the receipt file set.",
        ));
    }
    let expected = compute_bundle_revision(receipt)?;
    if receipt.bundle_revision != expected {
        return Err(integrity_error(
            "The Office render runtime bundle revision does not match its frozen receipt.",
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
        browser: &receipt.browser,
        archive: &receipt.archive,
        files: &receipt.files,
    };
    let value = serde_json::to_value(&identity).map_err(|error| {
        invalid_component(format!(
            "Cannot canonicalize Office render runtime receipt: {error}"
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
                    "Cannot encode canonical Office render runtime string: {error}"
                ))
            })?;
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
                        "Cannot encode canonical Office render runtime key: {error}"
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
) -> Result<BTreeMap<String, VerifiedFile>, OfficeEngineError> {
    let actual = collect_component_files(root)?;
    let expected = receipt
        .files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    if actual.len() != expected.len()
        || actual
            .keys()
            .map(String::as_str)
            .ne(expected.keys().copied())
    {
        return Err(integrity_error(
            "The Office render runtime file set differs from its frozen receipt.",
        ));
    }
    let mut verified = BTreeMap::new();
    for (relative, path) in actual {
        let frozen = expected.get(relative.as_str()).ok_or_else(|| {
            integrity_error("The Office render runtime contains an unrecognized file.")
        })?;
        verify_file(&path, frozen.size, &frozen.sha256)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            integrity_error(format!(
                "Cannot freeze Office render runtime identity: {error}"
            ))
        })?;
        verified.insert(
            relative,
            VerifiedFile {
                #[cfg(not(unix))]
                size: frozen.size,
                #[cfg(not(unix))]
                sha256: frozen.sha256.clone(),
                identity: file_identity(&metadata),
            },
        );
    }
    Ok(verified)
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

fn collect_component_files(root: &Path) -> Result<BTreeMap<String, PathBuf>, OfficeEngineError> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory).map_err(|error| {
            invalid_component(format!("Cannot enumerate Office render runtime: {error}"))
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| {
                invalid_component(format!("Cannot enumerate Office render runtime: {error}"))
            })?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path).map_err(|error| {
                invalid_component(format!(
                    "Cannot inspect Office render runtime entry: {error}"
                ))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(invalid_component(
                    "The Office render runtime cannot contain symbolic links.",
                ));
            }
            if metadata.is_dir() {
                pending.push(path);
                continue;
            }
            if !metadata.is_file() {
                return Err(invalid_component(
                    "The Office render runtime can contain only regular files and directories.",
                ));
            }
            let relative = path.strip_prefix(root).map_err(|_| {
                invalid_component("Office render runtime entry escaped its component root.")
            })?;
            let relative = normalized_relative_path(relative)?;
            if relative == COMPONENT_RECEIPT {
                continue;
            }
            if files.insert(relative, path).is_some() || files.len() > MAX_COMPONENT_FILES {
                return Err(invalid_component(
                    "The Office render runtime contains too many or duplicate files.",
                ));
            }
        }
    }
    Ok(files)
}

fn resolve_component_file(root: &Path, relative: &str) -> Result<PathBuf, OfficeEngineError> {
    validate_relative_path(relative)?;
    let path = root.join(relative);
    let canonical = path.canonicalize().map_err(|error| {
        invalid_component(format!(
            "Cannot resolve Office render runtime file: {error}"
        ))
    })?;
    if !canonical.starts_with(root) {
        return Err(invalid_component(
            "Office render runtime file resolves outside its component root.",
        ));
    }
    Ok(canonical)
}

fn verify_file(
    path: &Path,
    expected_size: u64,
    expected_sha256: &str,
) -> Result<(), OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        integrity_error(format!(
            "Cannot inspect Office render runtime file: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() != expected_size
        || metadata.len() > MAX_COMPONENT_FILE_BYTES
    {
        return Err(integrity_error(
            "An Office render runtime file no longer matches its frozen size or type.",
        ));
    }
    let mut file = File::open(path).map_err(|error| {
        integrity_error(format!("Cannot open Office render runtime file: {error}"))
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 128 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            integrity_error(format!("Cannot hash Office render runtime file: {error}"))
        })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_COMPONENT_FILE_BYTES {
            return Err(integrity_error(
                "An Office render runtime file exceeds the per-file safety limit.",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if total != expected_size || hex_lower(&digest.finalize()) != expected_sha256 {
        return Err(integrity_error(
            "An Office render runtime file failed SHA-256 verification.",
        ));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), OfficeEngineError> {
    if value.is_empty() || value.contains('\\') || value.as_bytes().contains(&0) {
        return Err(invalid_component(
            "Office render runtime paths must be non-empty normalized POSIX-relative paths.",
        ));
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(invalid_component(
            "Office render runtime receipt contains an unsafe relative path.",
        ));
    }
    Ok(())
}

fn normalized_relative_path(path: &Path) -> Result<String, OfficeEngineError> {
    let mut parts = Vec::new();
    for component in path.components() {
        let Component::Normal(value) = component else {
            return Err(invalid_component(
                "Office render runtime contains a non-normalized path.",
            ));
        };
        let value = value
            .to_str()
            .ok_or_else(|| invalid_component("Office render runtime paths must be valid UTF-8."))?;
        parts.push(value);
    }
    if parts.is_empty() {
        return Err(invalid_component(
            "Office render runtime contains an empty path.",
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

fn current_arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x64"
    }
}

fn expected_archive_receipt() -> ArchiveReceipt {
    let (url, size, sha256) = match (current_platform(), current_arch()) {
        ("darwin", "arm64") => (
            "https://cdn.playwright.dev/builds/cft/149.0.7827.55/mac-arm64/chrome-headless-shell-mac-arm64.zip",
            98_043_456,
            "302f82603be06683947594ecd60f849e362a8fe3dd82a89bd4408477c97e75a6",
        ),
        ("darwin", "x64") => (
            "https://cdn.playwright.dev/builds/cft/149.0.7827.55/mac-x64/chrome-headless-shell-mac-x64.zip",
            103_452_247,
            "a32029e1861329a431b712d5b864e213d9cf8ef51a82ce4c24b27e25f6605434",
        ),
        ("linux", "arm64") => (
            "https://cdn.playwright.dev/dbazure/download/playwright/builds/chromium/1228/chromium-headless-shell-linux-arm64.zip",
            115_342_043,
            "1652929a70f4afb17aca36fce073fb7ed22262d16825be761b0801972f43ac4f",
        ),
        ("linux", "x64") => (
            "https://cdn.playwright.dev/builds/cft/149.0.7827.55/linux64/chrome-headless-shell-linux64.zip",
            119_778_157,
            "410c9407d5de3fea80d9398666be06f2aa09154a3fa7b327dc254e336bb4c4b7",
        ),
        ("win32", _) => (
            "https://cdn.playwright.dev/builds/cft/149.0.7827.55/win64/chrome-headless-shell-win64.zip",
            119_099_822,
            "5cfda0c763aa6a867ce2efad0c467e3220e9c5c01c4cba02fd57afe49ede5457",
        ),
        _ => unreachable!("Office renderer supports only fixed desktop targets"),
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
        "The application-managed Chromium render runtime is unavailable.",
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
    OfficeEngineError::new(
        OfficeEngineErrorCode::RenderBackendInvalid,
        OfficeEngineRecovery::InstallComponent,
        message,
    )
}

#[cfg(test)]
pub(super) fn write_test_render_runtime(root: &Path) {
    let executable = "browser/chrome-headless-shell";
    let path = root.join(executable);
    fs::create_dir_all(path.parent().expect("test browser parent")).unwrap();
    fs::write(&path, b"fixed-browser").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut receipt = ComponentReceipt {
        schema_version: 2,
        provider_id: OFFICE_RENDER_RUNTIME_PROVIDER_ID.to_string(),
        bundle_version: OFFICE_RENDER_RUNTIME_BUNDLE_VERSION.to_string(),
        platform: current_platform().to_string(),
        arch: current_arch().to_string(),
        browser: BrowserReceipt {
            family: EXPECTED_BROWSER_FAMILY.to_string(),
            version: EXPECTED_BROWSER_VERSION.to_string(),
            playwright_version: EXPECTED_PLAYWRIGHT_VERSION.to_string(),
            revision: EXPECTED_PLAYWRIGHT_REVISION.to_string(),
            executable: executable.to_string(),
        },
        archive: expected_archive_receipt(),
        files: vec![FileReceipt {
            path: executable.to_string(),
            size: 13,
            sha256: hex_lower(&Sha256::digest(b"fixed-browser")),
        }],
        bundle_revision: String::new(),
    };
    receipt.bundle_revision = compute_bundle_revision(&receipt).unwrap();
    fs::write(
        root.join(COMPONENT_RECEIPT),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_reverifies_a_frozen_component_tree() {
        let directory = tempfile::tempdir().unwrap();
        write_test_render_runtime(directory.path());
        let runtime = OfficeRenderRuntime::discover(
            &OfficeRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap();
        assert!(runtime
            .runtime_revision()
            .starts_with(RUNTIME_REVISION_PREFIX));
        runtime.verify_integrity().unwrap();
    }

    #[test]
    fn rejects_an_extra_or_modified_component_file() {
        let directory = tempfile::tempdir().unwrap();
        write_test_render_runtime(directory.path());
        fs::write(directory.path().join("unexpected"), b"x").unwrap();
        let error = OfficeRenderRuntime::discover(
            &OfficeRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::RenderBackendInvalid);
    }

    #[test]
    fn rejects_a_self_consistent_receipt_with_the_wrong_archive_identity() {
        let directory = tempfile::tempdir().unwrap();
        write_test_render_runtime(directory.path());
        let receipt_path = directory.path().join(COMPONENT_RECEIPT);
        let mut receipt: ComponentReceipt =
            serde_json::from_slice(&fs::read(&receipt_path).unwrap()).unwrap();
        receipt.archive.sha256 = "0".repeat(64);
        receipt.bundle_revision = compute_bundle_revision(&receipt).unwrap();
        fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

        let error = OfficeRenderRuntime::discover(
            &OfficeRenderRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(directory.path()),
        )
        .unwrap_err();
        assert_eq!(error.code(), OfficeEngineErrorCode::RenderBackendInvalid);
        assert!(error.message().contains("archive identity"));
    }

    #[cfg(unix)]
    #[test]
    fn browser_proxy_forces_a_private_profile_and_never_forwards_no_sandbox() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("fixed-browser");
        let captured = directory.path().join("argv.txt");
        let screenshot = directory.path().join("preview.png");
        let profile = directory.path().join("private-profile");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nfor value in \"$@\"; do case \"$value\" in --screenshot=*) output=${{value#--screenshot=}};; esac; done\nprintf '\\211PNG\\r\\n\\032\\nbody' > \"$output\"\n",
                captured.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let output_argument = format!("--screenshot={}", screenshot.display());
        let args = [
            OsString::from("--headless=new"),
            OsString::from("--disable-gpu"),
            OsString::from("--no-sandbox"),
            OsString::from(&output_argument),
            OsString::from("--window-size=800,600"),
            OsString::from("file:///private/input.html"),
        ];

        run_browser_proxy_with_paths(&args, &executable, &profile).unwrap();

        let argv = fs::read_to_string(captured).unwrap();
        assert!(argv.contains(&format!("--user-data-dir={}", profile.display())));
        assert!(argv.contains("--no-first-run\n"));
        assert!(argv.contains("--no-default-browser-check\n"));
        assert!(argv.contains("--proxy-server=http://127.0.0.1:9\n"));
        assert!(argv.contains("--proxy-bypass-list=<-loopback>\n"));
        assert!(argv.contains("--host-resolver-rules=MAP * ~NOTFOUND\n"));
        assert!(argv.contains("--disable-quic\n"));
        assert!(argv.contains("--force-webrtc-ip-handling-policy=disable_non_proxied_udp\n"));
        assert!(!argv.lines().any(|line| line == "--no-sandbox"));
        #[cfg(target_os = "macos")]
        assert!(argv.contains("--use-mock-keychain\n"));
        assert!(screenshot.is_file());
    }

    #[test]
    fn browser_proxy_rejects_untrusted_options_before_starting_chromium() {
        let args = [
            OsString::from("--dump-dom"),
            OsString::from("--user-data-dir=/tmp/model-controlled"),
            OsString::from("file:///private/input.html"),
        ];
        let error = run_browser_proxy_with_paths(
            &args,
            Path::new("/browser-must-not-run"),
            Path::new("/private/profile"),
        )
        .unwrap_err();
        assert_eq!(error.error_code, "office.render_backend_failed");
        assert!(error.message.contains("unsupported managed browser option"));
    }

    #[test]
    fn render_marker_requires_a_matching_terminal_proxy_proof() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("marker.json");
        let nonce = "0123456789abcdef0123456789abcdef";
        initialize_render_marker(&path, nonce, 2).unwrap();

        let pending = read_render_failure(&path, nonce, 2, true).unwrap().unwrap();
        assert_eq!(pending.error_code, "office.render_backend_failed");
        assert!(pending.message.contains("did not complete"));

        assert_eq!(begin_proxy_invocation(&path, nonce).unwrap(), 1);
        commit_proxy_success(&path, nonce, 1).unwrap();
        assert!(read_render_failure(&path, nonce, 2, true)
            .unwrap()
            .is_none());

        assert_eq!(begin_proxy_invocation(&path, nonce).unwrap(), 2);
        commit_proxy_success(&path, nonce, 2).unwrap();
        assert!(begin_proxy_invocation(&path, nonce).is_err());

        let timeout = render_failure(
            "office.render_backend_timeout",
            "The managed browser timed out.",
            true,
        );
        initialize_render_marker(&path, nonce, 2).unwrap();
        assert_eq!(begin_proxy_invocation(&path, nonce).unwrap(), 1);
        commit_proxy_failure(&path, nonce, 1, &timeout).unwrap();
        assert_eq!(
            read_render_failure(&path, nonce, 2, true).unwrap().unwrap(),
            timeout
        );

        let wrong_identity =
            read_render_failure(&path, "fedcba9876543210fedcba9876543210", 2, true)
                .unwrap()
                .unwrap();
        assert!(wrong_identity.message.contains("wrong identity"));
    }

    #[test]
    fn render_marker_admits_only_one_concurrent_browser_invocation() {
        use std::sync::{Arc, Barrier};

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("marker.json");
        let nonce = "0123456789abcdef0123456789abcdef";
        initialize_render_marker(&path, nonce, 1).unwrap();
        let barrier = Arc::new(Barrier::new(3));
        let workers = (0..2)
            .map(|_| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    begin_proxy_invocation(&path, nonce)
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let marker = read_render_marker(&path).unwrap().unwrap();
        assert_eq!(marker.status, OfficeRenderMarkerStatus::Failure);
        assert_eq!(marker.invocation_count, 1);
        assert!(read_render_failure(&path, nonce, 1, true)
            .unwrap()
            .is_some());
    }

    #[test]
    fn over_limit_browser_invocation_replaces_an_old_success_with_sticky_failure() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("marker.json");
        let nonce = "0123456789abcdef0123456789abcdef";
        initialize_render_marker(&path, nonce, 1).unwrap();
        assert_eq!(begin_proxy_invocation(&path, nonce).unwrap(), 1);
        commit_proxy_success(&path, nonce, 1).unwrap();

        assert!(begin_proxy_invocation(&path, nonce).is_err());
        let failure = read_render_failure(&path, nonce, 1, true)
            .unwrap()
            .expect("over-limit admission must remain terminal");
        assert_eq!(failure.error_code, "office.render_backend_failed");
        assert!(failure.message.contains("cannot admit another"));
        assert!(commit_proxy_success(&path, nonce, 1).is_err());
    }

    #[test]
    #[ignore = "requires MYCOPILOT_OFFICE_RENDERER_DIR to point to a prepared component"]
    fn prepared_component_matches_the_rust_receipt_contract() {
        let directory = std::env::var_os("MYCOPILOT_OFFICE_RENDERER_DIR")
            .expect("MYCOPILOT_OFFICE_RENDERER_DIR is required for the component smoke test");
        let runtime = OfficeRenderRuntime::discover(
            &OfficeRenderRuntimeDiscoveryOptions::new().with_configured_component_dir(directory),
        )
        .unwrap();
        assert_eq!(runtime.browser_version(), EXPECTED_BROWSER_VERSION);
        assert!(runtime
            .runtime_revision()
            .starts_with(RUNTIME_REVISION_PREFIX));
        runtime.verify_integrity().unwrap();
    }
}
