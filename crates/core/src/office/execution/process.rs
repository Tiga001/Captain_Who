use super::*;

pub(super) struct ProcessOutput {
    pub(super) status: ExitStatus,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) timed_out: bool,
    pub(super) cancelled: bool,
    pub(super) stdout_truncated: bool,
    pub(super) stderr_truncated: bool,
    pub(super) output_capture: ProcessOutputCaptureMetadata,
    pub(super) stdout_spool: ProcessOutputSpool,
    pub(super) stderr_spool: ProcessOutputSpool,
    pub(super) render_failure: Option<OfficeRenderFailureMarker>,
}

#[derive(Clone, Copy)]
pub(super) struct BrowserProcessPolicy<'a> {
    proxy_executable: &'a Path,
    runtime: Option<&'a OfficeRenderRuntime>,
    required: bool,
    max_invocations: u32,
}

pub(super) struct ProcessRunOptions<'a> {
    pub(super) browser_policy: Option<BrowserProcessPolicy<'a>>,
    pub(super) environment: Option<&'a BTreeMap<std::ffi::OsString, std::ffi::OsString>>,
    pub(super) process_name: &'a str,
}

struct BrowserMarkerExpectation {
    path: PathBuf,
    nonce: String,
    required: bool,
    max_invocations: u32,
}

pub(super) fn browser_process_policy<'a>(
    engine: &'a OfficeCliEngine,
    request: &OfficeExecutionRequest,
) -> Result<Option<BrowserProcessPolicy<'a>>, OfficeEngineError> {
    let required = request_requires_browser_runtime(request);
    let Some(proxy_executable) = engine.browser_proxy_executable() else {
        if required {
            return Err(OfficeEngineError::new(
                OfficeEngineErrorCode::RenderBackendUnavailable,
                OfficeEngineRecovery::InstallComponent,
                "The trusted Office render browser launcher is unavailable.",
            ));
        }
        return Ok(None);
    };
    let runtime = if required {
        Some(engine.render_runtime()?)
    } else {
        engine.optional_render_runtime()
    };
    Ok(Some(BrowserProcessPolicy {
        proxy_executable,
        runtime,
        required,
        max_invocations: browser_invocation_limit(request),
    }))
}

fn browser_invocation_limit(request: &OfficeExecutionRequest) -> u32 {
    let document_contact_sheet = request.document_kind == OfficeDocumentKind::Document
        && matches!(
            request.typed_parameters(),
            OfficeOperationParameters::View {
                mode: OfficeViewMode::Screenshot,
                grid: Some(_),
                ..
            }
        );
    if document_contact_sheet {
        // OfficeCLI v1.0.139 uses one bounded page-count pass and one capture pass for a Word
        // contact sheet. This fixed limit is independent of page count and prevents per-page
        // browser spawning. Presentations and all other browser-backed requests get one pass.
        2
    } else {
        1
    }
}

pub(super) fn request_requires_browser_runtime(request: &OfficeExecutionRequest) -> bool {
    let OfficeOperationParameters::View {
        mode, page_count, ..
    } = request.typed_parameters()
    else {
        return false;
    };
    matches!(mode, OfficeViewMode::Screenshot | OfficeViewMode::Svg)
        || (*mode == OfficeViewMode::Stats
            && *page_count
            && request.document_kind == OfficeDocumentKind::Document)
}

pub(super) fn run_process(
    executable: &Path,
    cwd: Option<&Path>,
    argv: &[String],
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
    browser_policy: Option<BrowserProcessPolicy<'_>>,
) -> Result<ProcessOutput, OfficeEngineError> {
    run_process_with_options(
        executable,
        cwd,
        argv,
        timeout,
        cancellation,
        action_cancel_flag,
        ProcessRunOptions {
            browser_policy,
            environment: None,
            process_name: "OfficeCLI",
        },
    )
}

pub(super) fn run_process_with_options(
    executable: &Path,
    cwd: Option<&Path>,
    argv: &[String],
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
    options: ProcessRunOptions<'_>,
) -> Result<ProcessOutput, OfficeEngineError> {
    let private_home = tempfile::Builder::new()
        .prefix("mycopilot-office-runtime-")
        .tempdir()
        .map_err(|error| io_error("create the private Office runtime directory", error))?;
    let mut command = Command::new(executable);
    command.args(argv);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    configure_private_environment(&mut command, private_home.path());
    if let Some(environment) = options.environment {
        command.envs(environment);
    }
    let browser_failure_marker = options
        .browser_policy
        .map(|policy| configure_browser_environment(&mut command, private_home.path(), policy))
        .transpose()?;

    if cancellation_requested(cancellation, action_cancel_flag) {
        return synthetic_cancelled_status();
    }
    configure_command_process_group(&mut command);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| {
            OfficeEngineError::new(
                OfficeEngineErrorCode::ProcessFailure,
                OfficeEngineRecovery::Retry,
                format!("Cannot start {}: {error}", options.process_name),
            )
        })?;
    let capture_policy = ProcessOutputCapturePolicy::process_default();
    let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
    let stdout_reader = spawn_process_output_capture(
        child.stdout.take().ok_or_else(|| {
            process_error(format!("Cannot capture {} stdout.", options.process_name))
        })?,
        capture_budget.clone(),
        capture_policy,
    );
    let stderr_reader = spawn_process_output_capture(
        child.stderr.take().ok_or_else(|| {
            process_error(format!("Cannot capture {} stderr.", options.process_name))
        })?,
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
            break wait_for_child(&mut child, "cancelled", options.process_name)?;
        }
        match try_wait_command_process_group(&mut child).map_err(|error| {
            process_error(format!("Cannot wait for {}: {error}", options.process_name))
        })? {
            Some(status) => break status,
            None if started.elapsed() >= timeout => {
                timed_out = true;
                terminate_command_process_group(&mut child);
                break wait_for_child(&mut child, "timed out", options.process_name)?;
            }
            None => thread::sleep(Duration::from_millis(25)),
        }
    };
    let stdout_capture =
        join_process_output_capture(stdout_reader, &format!("{} stdout", options.process_name))
            .map_err(process_error)?;
    let stderr_capture =
        join_process_output_capture(stderr_reader, &format!("{} stderr", options.process_name))
            .map_err(process_error)?;
    let output_capture =
        ProcessOutputCaptureMetadata::from_streams(&stdout_capture, &stderr_capture);
    let render_failure = browser_failure_marker
        .as_ref()
        .map(|expectation| {
            read_render_failure(
                &expectation.path,
                &expectation.nonce,
                expectation.max_invocations,
                expectation.required,
            )
        })
        .transpose()?
        .flatten();
    Ok(ProcessOutput {
        status,
        stdout: stdout_capture.preview().to_string(),
        stderr: stderr_capture.preview().to_string(),
        timed_out,
        cancelled,
        stdout_truncated: stdout_capture.preview_truncated(),
        stderr_truncated: stderr_capture.preview_truncated(),
        output_capture,
        stdout_spool: stdout_capture.spool(),
        stderr_spool: stderr_capture.spool(),
        render_failure,
    })
}

fn configure_browser_environment(
    command: &mut Command,
    private_home: &Path,
    policy: BrowserProcessPolicy<'_>,
) -> Result<BrowserMarkerExpectation, OfficeEngineError> {
    if policy.required && policy.runtime.is_none() {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::RenderBackendUnavailable,
            OfficeEngineRecovery::InstallComponent,
            "The application-managed Chromium render runtime is unavailable.",
        ));
    }
    if let Some(runtime) = policy.runtime {
        runtime.verify_integrity()?;
    }
    let bin = private_home.join("browser-bin");
    let profile = private_home.join("browser-profile");
    fs::create_dir_all(&bin).map_err(|error| {
        io_error(
            "create the private Office browser launcher directory",
            error,
        )
    })?;
    fs::create_dir_all(&profile)
        .map_err(|error| io_error("create the private Office browser profile", error))?;
    secure_private_directory(&bin)?;
    secure_private_directory(&profile)?;
    let alias = bin.join(if cfg!(windows) {
        "google-chrome.exe"
    } else {
        "google-chrome"
    });
    create_browser_proxy_alias(policy.proxy_executable, &alias)?;
    if policy.required {
        verify_browser_proxy_alias(&alias, private_home)?;
    }
    let marker = private_home.join("browser-failure.json");
    let marker_nonce = uuid::Uuid::new_v4().simple().to_string();
    initialize_render_marker(&marker, &marker_nonce, policy.max_invocations)?;
    let unavailable = private_home.join("browser-runtime-unavailable");
    command.env("PATH", &bin);
    command.env(BROWSER_PROXY_MODE_ENV, "1");
    command.env(
        BROWSER_EXECUTABLE_ENV,
        policy
            .runtime
            .map(OfficeRenderRuntime::executable_path)
            .unwrap_or(unavailable.as_path()),
    );
    command.env(BROWSER_PROFILE_ENV, &profile);
    command.env(BROWSER_FAILURE_MARKER_ENV, &marker);
    command.env(BROWSER_MARKER_NONCE_ENV, &marker_nonce);
    command.env(
        BROWSER_MAX_INVOCATIONS_ENV,
        policy.max_invocations.to_string(),
    );
    Ok(BrowserMarkerExpectation {
        path: marker,
        nonce: marker_nonce,
        required: policy.required,
        max_invocations: policy.max_invocations,
    })
}

fn verify_browser_proxy_alias(alias: &Path, private_home: &Path) -> Result<(), OfficeEngineError> {
    let mut command = Command::new(alias);
    command.arg(BROWSER_PROXY_SELF_TEST_ARGUMENT);
    configure_private_environment(&mut command, private_home);
    command.env(BROWSER_PROXY_MODE_ENV, "1");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    configure_command_process_group(&mut command);
    let mut child = command.spawn().map_err(|error| {
        OfficeEngineError::new(
            OfficeEngineErrorCode::RenderBackendUnavailable,
            OfficeEngineRecovery::InstallComponent,
            format!("Cannot start the trusted Office browser proxy: {error}"),
        )
    })?;
    let started = Instant::now();
    loop {
        match try_wait_command_process_group(&mut child).map_err(|error| {
            OfficeEngineError::new(
                OfficeEngineErrorCode::RenderBackendUnavailable,
                OfficeEngineRecovery::InstallComponent,
                format!("Cannot verify the trusted Office browser proxy: {error}"),
            )
        })? {
            Some(status) if status.success() => {
                let mut output = Vec::new();
                if let Some(stdout) = child.stdout.take() {
                    stdout.take(128).read_to_end(&mut output).map_err(|error| {
                        OfficeEngineError::new(
                            OfficeEngineErrorCode::RenderBackendUnavailable,
                            OfficeEngineRecovery::InstallComponent,
                            format!(
                                "Cannot read the trusted Office browser proxy self-test: {error}"
                            ),
                        )
                    })?;
                }
                if output == BROWSER_PROXY_SELF_TEST_RESPONSE.as_bytes() {
                    return Ok(());
                }
                return Err(OfficeEngineError::new(
                    OfficeEngineErrorCode::RenderBackendUnavailable,
                    OfficeEngineRecovery::InstallComponent,
                    "The configured Office browser launcher did not identify itself as the trusted core-server proxy.",
                ));
            }
            Some(status) => {
                return Err(OfficeEngineError::new(
                    OfficeEngineErrorCode::RenderBackendUnavailable,
                    OfficeEngineRecovery::InstallComponent,
                    format!("The trusted Office browser proxy self-test exited with {status}."),
                ))
            }
            None if started.elapsed() >= BROWSER_PROXY_SELF_TEST_TIMEOUT => {
                terminate_command_process_group(&mut child);
                let _ = child.wait();
                return Err(OfficeEngineError::new(
                    OfficeEngineErrorCode::RenderBackendUnavailable,
                    OfficeEngineRecovery::InstallComponent,
                    "The trusted Office browser proxy self-test timed out.",
                ));
            }
            None => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(unix)]
fn create_browser_proxy_alias(source: &Path, destination: &Path) -> Result<(), OfficeEngineError> {
    std::os::unix::fs::symlink(source, destination)
        .map_err(|error| io_error("create the private Office browser launcher alias", error))
}

#[cfg(windows)]
fn create_browser_proxy_alias(source: &Path, destination: &Path) -> Result<(), OfficeEngineError> {
    fs::copy(source, destination)
        .map(|_| ())
        .map_err(|error| io_error("create the private Office browser launcher alias", error))
}

#[cfg(unix)]
fn secure_private_directory(path: &Path) -> Result<(), OfficeEngineError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("secure the private Office browser directory", error))
}

#[cfg(windows)]
fn secure_private_directory(_path: &Path) -> Result<(), OfficeEngineError> {
    Ok(())
}

fn configure_private_environment(command: &mut Command, private_home: &Path) {
    command.env_clear();
    command.env("HOME", private_home);
    command.env("XDG_CONFIG_HOME", private_home.join("config"));
    command.env("TMPDIR", private_home);
    command.env("TEMP", private_home);
    command.env("TMP", private_home);
    command.env("LANG", "C.UTF-8");
    command.env("LC_ALL", "C.UTF-8");
    command.env("TERM", "dumb");
    command.env("CI", "1");
    command.env("OFFICECLI_SKIP_UPDATE", "1");
    command.env("OFFICECLI_NO_AUTO_RESIDENT", "1");
    command.env("DOTNET_EnableDiagnostics", "0");
    #[cfg(windows)]
    {
        for name in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
    }
}

fn wait_for_child(
    child: &mut Child,
    state: &str,
    process_name: &str,
) -> Result<ExitStatus, OfficeEngineError> {
    child
        .wait()
        .map_err(|error| process_error(format!("Cannot wait for {state} {process_name}: {error}")))
}

pub(super) fn cancellation_requested(
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> bool {
    cancellation.is_cancelled()
        || action_cancel_flag.is_some_and(|flag| flag.load(Ordering::SeqCst))
}

#[cfg(unix)]
fn synthetic_cancelled_status() -> Result<ProcessOutput, OfficeEngineError> {
    use std::os::unix::process::ExitStatusExt;
    Ok(ProcessOutput {
        status: ExitStatus::from_raw(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        render_failure: None,
    })
}

#[cfg(windows)]
fn synthetic_cancelled_status() -> Result<ProcessOutput, OfficeEngineError> {
    use std::os::windows::process::ExitStatusExt;
    Ok(ProcessOutput {
        status: ExitStatus::from_raw(0),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        render_failure: None,
    })
}

pub(super) fn execution_error(output: &ProcessOutput) -> (Option<String>, Option<String>) {
    if output.cancelled {
        return (
            Some("office.cancelled".to_string()),
            Some("OfficeCLI execution was cancelled.".to_string()),
        );
    }
    if output.timed_out {
        return (
            Some("office.timeout".to_string()),
            Some("OfficeCLI execution exceeded its timeout.".to_string()),
        );
    }
    if let Some(failure) = &output.render_failure {
        return (
            Some(failure.error_code.clone()),
            Some(failure.message.clone()),
        );
    }
    match output.status.code() {
        Some(0) => (None, None),
        Some(exit_code) => (
            Some("office.nonzero_exit".to_string()),
            Some(format!(
                "OfficeCLI exited with non-zero status {exit_code}."
            )),
        ),
        None => (
            Some("office.terminated_without_exit_code".to_string()),
            Some("OfficeCLI terminated without an exit code.".to_string()),
        ),
    }
}

pub(super) fn cancelled_result(
    engine_revision: &str,
    request: &OfficeExecutionRequest,
    argv: Vec<String>,
) -> OfficeExecutionResult {
    OfficeExecutionResult {
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: engine_revision.to_string(),
        document_kind: request.document_kind,
        operation: request.operation,
        argv,
        cwd: ".".to_string(),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        duration_ms: 0,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        error_code: Some("office.cancelled".to_string()),
        error: Some("OfficeCLI execution was cancelled before launch.".to_string()),
        outputs: Vec::new(),
    }
}
