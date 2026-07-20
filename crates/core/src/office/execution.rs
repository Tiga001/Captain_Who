use super::discovery::OfficeCliEngine;
use super::types::{
    OfficeEngine, OfficeEngineAvailability, OfficeEngineError, OfficeEngineErrorCode,
    OfficeEngineRecovery, OfficeEngineStatus, OfficeExecutionContext, OfficeExecutionRequest,
    OfficeExecutionResult, OfficeFileState, OfficeFrozenPath, OfficeOperation,
    OfficeOperationAccess, OfficePathIdentity, OfficePathPurpose, OfficePathScope, OfficePathSlot,
    OfficePreparedExecution, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_ENGINE_STATUS_SCHEMA_VERSION, OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
use crate::command::{
    configure_command_process_group, join_output_reader, spawn_bounded_output_reader,
    terminate_command_process_group, try_wait_command_process_group,
};
use crate::{
    expand_system_path, AgentAttachmentLibraryContext, AgentCancellationToken, AgentPermissions,
    AgentReadPermission, AgentWritePermission,
};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_OFFICE_TIMEOUT_MS: u64 = 120_000;
pub const MAX_OFFICE_TIMEOUT_MS: u64 = 600_000;
pub const MAX_OFFICE_ARGUMENTS: usize = 256;
pub const MAX_OFFICE_ARGUMENT_BYTES: usize = 128 * 1024;
pub const MAX_OFFICE_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const FILE_REVISION_PREFIX: &str = "office-file-sha256-v1:";
static OFFICE_COMMIT_LOCKS: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
const WORKSPACE_REVISION_PREFIX: &str = "office-workspace-sha256-v1:";
const PATH_IDENTITY_PREFIX: &str = "office-path-identity-sha256-v1:";

pub(super) fn probe_engine(
    engine: &OfficeCliEngine,
    cancellation: AgentCancellationToken,
) -> OfficeEngineStatus {
    let capabilities = engine.capabilities();
    if let Err(error) = validate_platform() {
        return OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: OfficeEngineAvailability::Unavailable,
            source: Some(engine.source()),
            version: None,
            engine_revision: Some(engine.engine_revision().to_string()),
            capabilities: capabilities.clone(),
            error_code: Some(error.code().stable_name().to_string()),
            message: Some(error.message().to_string()),
        };
    }
    if let Err(error) = engine.verify_engine_revision() {
        return OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: OfficeEngineAvailability::Unavailable,
            source: Some(engine.source()),
            version: None,
            engine_revision: Some(engine.engine_revision().to_string()),
            capabilities,
            error_code: Some(error.code().stable_name().to_string()),
            message: Some(error.message().to_string()),
        };
    }
    let output = run_process(
        engine.executable_path(),
        None,
        &["--version".to_string()],
        PROBE_TIMEOUT,
        &cancellation,
        None,
    );
    match output {
        Ok(output) if !output.cancelled && !output.timed_out && output.status.code() == Some(0) => {
            let version = output
                .stdout
                .lines()
                .chain(output.stderr.lines())
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_string);
            OfficeEngineStatus {
                schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
                provider_id: OFFICECLI_PROVIDER_ID.to_string(),
                availability: OfficeEngineAvailability::Available,
                source: Some(engine.source()),
                version,
                engine_revision: Some(engine.engine_revision().to_string()),
                capabilities,
                error_code: None,
                message: None,
            }
        }
        Ok(output) => {
            let (code, message) = if output.cancelled {
                (
                    "office.probe_cancelled",
                    "OfficeCLI capability probe was cancelled.".to_string(),
                )
            } else if output.timed_out {
                (
                    "office.probe_timeout",
                    "OfficeCLI capability probe timed out.".to_string(),
                )
            } else {
                (
                    "office.probe_failed",
                    format!(
                        "OfficeCLI capability probe exited with status {}.",
                        output
                            .status
                            .code()
                            .map_or_else(|| "unknown".to_string(), |code| code.to_string())
                    ),
                )
            };
            OfficeEngineStatus {
                schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
                provider_id: OFFICECLI_PROVIDER_ID.to_string(),
                availability: OfficeEngineAvailability::Unavailable,
                source: Some(engine.source()),
                version: None,
                engine_revision: Some(engine.engine_revision().to_string()),
                capabilities,
                error_code: Some(code.to_string()),
                message: Some(message),
            }
        }
        Err(error) => OfficeEngineStatus {
            schema_version: OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
            provider_id: OFFICECLI_PROVIDER_ID.to_string(),
            availability: OfficeEngineAvailability::Unavailable,
            source: Some(engine.source()),
            version: None,
            engine_revision: Some(engine.engine_revision().to_string()),
            capabilities,
            error_code: Some(error.code().stable_name().to_string()),
            message: Some(error.message().to_string()),
        },
    }
}

pub(super) fn prepare_office_cli(
    engine: &OfficeCliEngine,
    context: &OfficeExecutionContext,
    request: &OfficeExecutionRequest,
) -> Result<OfficePreparedExecution, OfficeEngineError> {
    validate_platform()?;
    engine.verify_engine_revision()?;
    validate_arguments(&request.arguments)?;
    let context = ResolvedExecutionContext::resolve(context)?;
    let prepared = prepare_request(&context, request)?;
    Ok(OfficePreparedExecution {
        schema_version: OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: engine.engine_revision().to_string(),
        workspace_revision: context.workspace_revision.clone(),
        access: request.access(),
        request: request.clone(),
        argv: prepared.argv,
        paths: prepared.paths,
        document_precondition: None,
        output_precondition: None,
        destination_precondition: None,
        resource_preconditions: Vec::new(),
    })
}

pub(super) fn run_prepared_office_cli(
    engine: &OfficeCliEngine,
    context: &OfficeExecutionContext,
    prepared: &OfficePreparedExecution,
    cancellation: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<OfficeExecutionResult, OfficeEngineError> {
    validate_platform()?;
    validate_prepared_identity(engine, prepared)?;
    validate_arguments(&prepared.request.arguments)?;
    let context = ResolvedExecutionContext::resolve(context)?;
    if prepared.workspace_revision.is_some()
        && context.workspace_revision != prepared.workspace_revision
    {
        return Err(precondition_error(
            "The prepared Office operation belongs to a different workspace.",
        ));
    }
    let current = prepare_request(&context, &prepared.request)?;
    if current.argv != prepared.argv
        || prepared.access != prepared.request.access()
        || current.paths != prepared.paths
    {
        return Err(precondition_error(
            "The frozen Office request no longer matches its validated execution plan.",
        ));
    }
    if cancellation_requested(&cancellation, action_cancel_flag.as_ref()) {
        return Ok(cancelled_result(
            engine.engine_revision(),
            &prepared.request,
            prepared.argv.clone(),
        ));
    }

    let timeout = Duration::from_millis(
        prepared
            .request
            .timeout_ms
            .unwrap_or(DEFAULT_OFFICE_TIMEOUT_MS)
            .clamp(1, MAX_OFFICE_TIMEOUT_MS),
    );
    match prepared.access {
        OfficeOperationAccess::ReadOnly => execute_read_only(
            engine,
            &context,
            prepared,
            timeout,
            &cancellation,
            action_cancel_flag.as_ref(),
        ),
        OfficeOperationAccess::FileWrite if prepared.request.operation == OfficeOperation::View => {
            execute_render_transaction(
                engine,
                &context,
                prepared,
                timeout,
                &cancellation,
                action_cancel_flag.as_ref(),
            )
        }
        OfficeOperationAccess::FileWrite => execute_document_transaction(
            engine,
            &context,
            prepared,
            timeout,
            &cancellation,
            action_cancel_flag.as_ref(),
        ),
    }
}

fn validate_prepared_identity(
    engine: &OfficeCliEngine,
    prepared: &OfficePreparedExecution,
) -> Result<(), OfficeEngineError> {
    if prepared.schema_version != OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION
        || prepared.provider_id != OFFICECLI_PROVIDER_ID
    {
        return Err(precondition_error(
            "The prepared Office operation uses an unsupported schema or provider.",
        ));
    }
    engine.verify_engine_revision()?;
    if prepared.engine_revision != engine.engine_revision() {
        return Err(precondition_error(
            "OfficeCLI changed after the operation was prepared; prepare and approve it again.",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ResolvedExecutionContext {
    workspace: Option<PathBuf>,
    workspace_revision: Option<String>,
    permissions: AgentPermissions,
    attachment_library: Option<AgentAttachmentLibraryContext>,
}

impl ResolvedExecutionContext {
    fn resolve(context: &OfficeExecutionContext) -> Result<Self, OfficeEngineError> {
        let workspace = context
            .workspace_root()
            .map(canonical_workspace)
            .transpose()?;
        let workspace_revision = workspace.as_deref().map(workspace_revision).transpose()?;
        Ok(Self {
            workspace,
            workspace_revision,
            permissions: context.permissions(),
            attachment_library: context.attachment_library().cloned(),
        })
    }
}

fn verify_preconditions(
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
) -> Result<(), OfficeEngineError> {
    let current = prepare_request(context, &prepared.request).map_err(|error| {
        precondition_error(format!(
            "The Office execution plan no longer satisfies its prepared state: {}",
            error.message()
        ))
    })?;
    if current.argv != prepared.argv || current.paths != prepared.paths {
        return Err(precondition_error(
            "An Office input, target, resource, or parent directory changed after preparation.",
        ));
    }
    Ok(())
}

fn workspace_revision(workspace: &Path) -> Result<String, OfficeEngineError> {
    let metadata = fs::symlink_metadata(workspace).map_err(|error| {
        workspace_error(format!(
            "Cannot inspect workspace `{}`: {error}",
            workspace.display()
        ))
    })?;
    let identity = path_identity(workspace, &metadata)?;
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.office.workspace\0");
    digest.update(identity.revision.as_bytes());
    Ok(format!(
        "{WORKSPACE_REVISION_PREFIX}{}",
        hex_lower(&digest.finalize())
    ))
}

fn file_revision(path: &Path) -> Result<(String, u64), OfficeEngineError> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut file = options.open(path).map_err(|error| {
        workspace_error(format!(
            "Cannot open Office file `{}` for identity verification: {error}",
            path.display()
        ))
    })?;
    let before = file.metadata().map_err(|error| {
        workspace_error(format!(
            "Cannot inspect Office file `{}`: {error}",
            path.display()
        ))
    })?;
    if !before.is_file() || before.len() > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(workspace_error(format!(
            "Office file `{}` must be regular and no larger than {MAX_OFFICE_DOCUMENT_BYTES} bytes.",
            path.display()
        )));
    }
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            workspace_error(format!(
                "Cannot read Office file `{}` for identity verification: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > MAX_OFFICE_DOCUMENT_BYTES {
            return Err(workspace_error(format!(
                "Office file exceeds the {MAX_OFFICE_DOCUMENT_BYTES}-byte limit."
            )));
        }
        digest.update(&buffer[..read]);
    }
    let after = file.metadata().map_err(|error| {
        workspace_error(format!(
            "Cannot reinspect Office file `{}`: {error}",
            path.display()
        ))
    })?;
    if total != before.len() || after.len() != before.len() {
        return Err(precondition_error(
            "Office file changed during identity verification.",
        ));
    }
    Ok((
        format!("{FILE_REVISION_PREFIX}{}", hex_lower(&digest.finalize())),
        total,
    ))
}

fn execute_read_only(
    engine: &OfficeCliEngine,
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<OfficeExecutionResult, OfficeEngineError> {
    let private = tempfile::Builder::new()
        .prefix("mycopilot-office-read-")
        .tempdir()
        .map_err(|error| io_error("create private Office read snapshot", error))?;
    let mut actual_argv = prepared.argv.clone();
    if let Some(document) = frozen_path(prepared, &OfficePathSlot::Document) {
        let source = Path::new(&document.normalized_path);
        let name = source
            .file_name()
            .ok_or_else(|| invalid_request("Office document path has no file name."))?;
        let snapshot = private.path().join(name);
        copy_file_snapshot(source, &snapshot)?;
        let (revision, size) = file_revision(&snapshot)?;
        if document.content_revision.as_deref() != Some(&revision) || document.size != Some(size) {
            return Err(precondition_error(
                "Office input changed while the private read snapshot was created.",
            ));
        }
        if actual_argv.len() < 2 {
            return Err(precondition_error("Office document argv is incomplete."));
        }
        actual_argv[1] = snapshot.to_string_lossy().into_owned();
    }
    let (_resource_snapshots, resource_paths) = snapshot_prepared_resources(prepared)?;
    rewrite_path_bearing_properties(&resource_paths, &mut actual_argv)?;
    verify_preconditions(context, prepared)?;
    engine.verify_engine_revision()?;
    let started = Instant::now();
    let output = run_process(
        engine.executable_path(),
        Some(private.path()),
        &actual_argv,
        timeout,
        cancellation,
        action_cancel_flag,
    )?;
    Ok(process_result(engine, prepared, output, started))
}

fn execute_document_transaction(
    engine: &OfficeCliEngine,
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<OfficeExecutionResult, OfficeEngineError> {
    let document = frozen_path(prepared, &OfficePathSlot::Document)
        .ok_or_else(|| precondition_error("Office document precondition is missing."))?;
    let target_precondition =
        frozen_path(prepared, &OfficePathSlot::Destination).unwrap_or(document);
    let target = PathBuf::from(&target_precondition.normalized_path);
    let mut staging = StagingArea::new(&target)?;
    if prepared.request.operation != OfficeOperation::Create {
        let source = Path::new(&document.normalized_path);
        copy_file_snapshot(source, staging.path())?;
        let (revision, size) = file_revision(staging.path())?;
        if document.content_revision.as_deref() != Some(&revision) || document.size != Some(size) {
            return Err(precondition_error(
                "Office document changed while its mutation staging snapshot was created.",
            ));
        }
    }
    let mut actual_argv = prepared.argv.clone();
    actual_argv[1] = staging
        .path()
        .file_name()
        .ok_or_else(|| invalid_request("Staged Office document has no file name."))?
        .to_string_lossy()
        .into_owned();
    let (_resource_snapshots, resource_paths) = snapshot_prepared_resources(prepared)?;
    rewrite_path_bearing_properties(&resource_paths, &mut actual_argv)?;
    engine.verify_engine_revision()?;
    let started = Instant::now();
    let output = run_process(
        engine.executable_path(),
        Some(staging.directory()),
        &actual_argv,
        timeout,
        cancellation,
        action_cancel_flag,
    )?;
    let mut result = process_result(engine, prepared, output, started);
    if result.error_code.is_some() {
        return Ok(result);
    }
    if let Err(error) = validate_document_artifact(staging.path(), prepared.request.document_kind) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    let commit_lock = office_target_commit_lock(&target);
    let _commit_guard = commit_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Err(error) = verify_preconditions(context, prepared) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    run_commit_test_hook(&target, CommitTestPhase::BeforeCancellationCheck);
    if cancellation_requested(cancellation, action_cancel_flag) {
        mark_cancelled_before_commit(&mut result);
        return Ok(result);
    }
    // This is the commit-ready linearization point. A later cancellation cannot truthfully undo
    // the atomic rename, so from here on the result reports success or commit indeterminate.
    run_commit_test_hook(&target, CommitTestPhase::AfterCancellationCheck);
    if let Err(error) = staging.publish(&target, target_precondition.state) {
        attach_post_process_error(&mut result, error);
    }
    Ok(result)
}

fn execute_render_transaction(
    engine: &OfficeCliEngine,
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<OfficeExecutionResult, OfficeEngineError> {
    let document = frozen_path(prepared, &OfficePathSlot::Document)
        .ok_or_else(|| precondition_error("Office render input precondition is missing."))?;
    let output_precondition = frozen_path(prepared, &OfficePathSlot::Output)
        .ok_or_else(|| precondition_error("Office render output precondition is missing."))?;
    let source = Path::new(&document.normalized_path);
    let private = tempfile::Builder::new()
        .prefix("mycopilot-office-render-")
        .tempdir()
        .map_err(|error| io_error("create private Office render snapshot", error))?;
    let name = source
        .file_name()
        .ok_or_else(|| invalid_request("Office render input has no file name."))?;
    let snapshot = private.path().join(name);
    copy_file_snapshot(source, &snapshot)?;
    let (revision, size) = file_revision(&snapshot)?;
    if document.content_revision.as_deref() != Some(&revision) || document.size != Some(size) {
        return Err(precondition_error(
            "Office input changed while the private render snapshot was created.",
        ));
    }

    let target = PathBuf::from(&output_precondition.normalized_path);
    let mut staging = StagingArea::new(&target)?;
    let mut actual_argv = prepared.argv.clone();
    if actual_argv.len() < 2 {
        return Err(precondition_error("Office render argv is incomplete."));
    }
    actual_argv[1] = snapshot.to_string_lossy().into_owned();
    let output_index = actual_argv
        .len()
        .checked_sub(1)
        .ok_or_else(|| invalid_request("Office render argv is incomplete."))?;
    actual_argv[output_index] = staging.path().to_string_lossy().into_owned();
    engine.verify_engine_revision()?;
    let started = Instant::now();
    let output = run_process(
        engine.executable_path(),
        Some(private.path()),
        &actual_argv,
        timeout,
        cancellation,
        action_cancel_flag,
    )?;
    let mut result = process_result(engine, prepared, output, started);
    if result.error_code.is_some() {
        return Ok(result);
    }
    if let Err(error) = validate_render_artifact(
        staging.path(),
        prepared.request.arguments.first().map(String::as_str),
    ) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    let commit_lock = office_target_commit_lock(&target);
    let _commit_guard = commit_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Err(error) = verify_preconditions(context, prepared) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    run_commit_test_hook(&target, CommitTestPhase::BeforeCancellationCheck);
    if cancellation_requested(cancellation, action_cancel_flag) {
        mark_cancelled_before_commit(&mut result);
        return Ok(result);
    }
    // See the document transaction: after this point cancellation is intentionally ignored.
    run_commit_test_hook(&target, CommitTestPhase::AfterCancellationCheck);
    if let Err(error) = staging.publish(&target, output_precondition.state) {
        attach_post_process_error(&mut result, error);
    }
    Ok(result)
}

fn process_result(
    engine: &OfficeCliEngine,
    prepared: &OfficePreparedExecution,
    output: ProcessOutput,
    started: Instant,
) -> OfficeExecutionResult {
    let (error_code, error) = execution_error(&output);
    OfficeExecutionResult {
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: engine.engine_revision().to_string(),
        document_kind: prepared.request.document_kind,
        operation: prepared.request.operation,
        argv: prepared.argv.clone(),
        cwd: ".".to_string(),
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: output.timed_out,
        cancelled: output.cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        error_code,
        error,
    }
}

fn attach_post_process_error(result: &mut OfficeExecutionResult, error: OfficeEngineError) {
    result.error_code = Some(error.code().stable_name().to_string());
    result.error = Some(error.message().to_string());
}

fn mark_cancelled_before_commit(result: &mut OfficeExecutionResult) {
    result.cancelled = true;
    result.error_code = Some("office.cancelled".to_string());
    result.error = Some(
        "OfficeCLI execution was cancelled after validation and before the atomic commit."
            .to_string(),
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CommitTestPhase {
    BeforeCancellationCheck,
    AfterCancellationCheck,
}

#[cfg(test)]
type CommitTestHooks = std::sync::Mutex<HashMap<PathBuf, (CommitTestPhase, Arc<AtomicBool>)>>;

#[cfg(test)]
static COMMIT_TEST_HOOKS: std::sync::OnceLock<CommitTestHooks> = std::sync::OnceLock::new();

#[cfg(test)]
pub(super) fn install_commit_test_hook(
    target: PathBuf,
    phase: CommitTestPhase,
    cancellation: Arc<AtomicBool>,
) {
    COMMIT_TEST_HOOKS
        .get_or_init(Default::default)
        .lock()
        .expect("Office commit test hook mutex poisoned")
        .insert(target, (phase, cancellation));
}

#[cfg(test)]
fn run_commit_test_hook(target: &Path, phase: CommitTestPhase) {
    let Some(hooks) = COMMIT_TEST_HOOKS.get() else {
        return;
    };
    let cancellation = {
        let mut hooks = hooks
            .lock()
            .expect("Office commit test hook mutex poisoned");
        if hooks
            .get(target)
            .is_some_and(|(expected, _)| *expected == phase)
        {
            hooks.remove(target).map(|(_, cancellation)| cancellation)
        } else {
            None
        }
    };
    if let Some(cancellation) = cancellation {
        cancellation.store(true, Ordering::SeqCst);
    }
}

#[cfg(not(test))]
fn run_commit_test_hook(_target: &Path, _phase: CommitTestPhase) {}

struct PreparedRequest {
    argv: Vec<String>,
    paths: Vec<OfficeFrozenPath>,
}

fn prepare_request(
    context: &ResolvedExecutionContext,
    request: &OfficeExecutionRequest,
) -> Result<PreparedRequest, OfficeEngineError> {
    let resource_paths = validate_request_syntax(request)?;
    let mut argv = vec![request.operation.cli_name().to_string()];
    let mut paths = Vec::new();
    match request.operation {
        OfficeOperation::Help => {
            if request.document_path.is_some()
                || request.output_path.is_some()
                || request.destination_path.is_some()
            {
                return Err(invalid_request(
                    "The Office help operation cannot receive document, output, or destination paths.",
                ));
            }
        }
        OfficeOperation::Create => {
            let path = required_document_path(request)?;
            let frozen = freeze_path(
                context,
                OfficePathSlot::Document,
                path,
                OfficePathPurpose::WriteTarget,
            )?;
            validate_document_extension(request, Path::new(&frozen.normalized_path))?;
            paths.push(frozen);
            argv.push(path.to_string());
            if request.output_path.is_some() || request.destination_path.is_some() {
                return Err(invalid_request(
                    "The Office create operation cannot receive a separate output or destination path.",
                ));
            }
        }
        operation => {
            let path = required_document_path(request)?;
            let purpose = if request.access() == OfficeOperationAccess::FileWrite
                && request.destination_path.is_none()
                && operation != OfficeOperation::View
            {
                OfficePathPurpose::InPlaceTarget
            } else {
                OfficePathPurpose::ReadSource
            };
            let frozen = freeze_path(context, OfficePathSlot::Document, path, purpose)?;
            validate_document_extension(request, Path::new(&frozen.normalized_path))?;
            paths.push(frozen);
            argv.push(path.to_string());
            if operation != OfficeOperation::View && request.output_path.is_some() {
                return Err(invalid_request(
                    "Only the Office view operation accepts an output path.",
                ));
            }
            if request.destination_path.is_some()
                && !matches!(
                    operation,
                    OfficeOperation::Set
                        | OfficeOperation::Add
                        | OfficeOperation::Remove
                        | OfficeOperation::Move
                        | OfficeOperation::Swap
                )
            {
                return Err(invalid_request(
                    "Only Office mutation operations accept a destination path.",
                ));
            }
        }
    }
    argv.extend(request.arguments.iter().cloned());
    if let Some(output) = request.output_path.as_deref() {
        if request.operation != OfficeOperation::View {
            return Err(invalid_request(
                "Only the Office view operation accepts an output path.",
            ));
        }
        let mode = request.arguments.first().map(String::as_str);
        if !matches!(mode, Some("html" | "screenshot" | "svg")) {
            return Err(invalid_request(
                "Office view output paths are supported only for html, screenshot, or svg modes.",
            ));
        }
        paths.push(freeze_path(
            context,
            OfficePathSlot::Output,
            output,
            OfficePathPurpose::WriteTarget,
        )?);
        argv.push("-o".to_string());
        argv.push(output.to_string());
    } else if request.operation == OfficeOperation::View
        && matches!(
            request.arguments.first().map(String::as_str),
            Some("html" | "screenshot" | "svg")
        )
    {
        return Err(invalid_request(
            "Rendering requires an explicit output path.",
        ));
    }
    if let Some(destination) = request.destination_path.as_deref() {
        let destination = freeze_path(
            context,
            OfficePathSlot::Destination,
            destination,
            OfficePathPurpose::WriteTarget,
        )?;
        validate_document_extension(request, Path::new(&destination.normalized_path))?;
        let source = paths
            .iter()
            .find(|path| path.slot == OfficePathSlot::Document)
            .ok_or_else(|| precondition_error("Office document path is missing."))?;
        if source.normalized_path == destination.normalized_path {
            return Err(invalid_request(
                "A mutation destination path must differ from the source path; omit destinationPath for an in-place edit.",
            ));
        }
        paths.push(destination);
    }
    for (index, resource) in resource_paths.into_iter().enumerate() {
        paths.push(freeze_path(
            context,
            OfficePathSlot::Resource {
                index: u32::try_from(index)
                    .map_err(|_| invalid_request("Too many Office resource paths."))?,
            },
            &resource,
            OfficePathPurpose::ReadSource,
        )?);
    }
    Ok(PreparedRequest { argv, paths })
}

/// Validates the complete, side-effect-free portion of an Office request.
///
/// Dynamic approval routing calls this before it decides whether a request is
/// read-only. Keeping this validation free of filesystem access lets invalid or
/// ambiguous calls fail closed without weakening the prepare-time checks.
pub(crate) fn validate_office_request(
    request: &OfficeExecutionRequest,
) -> Result<(), OfficeEngineError> {
    validate_request_syntax(request).map(|_| ())
}

fn validate_request_syntax(
    request: &OfficeExecutionRequest,
) -> Result<Vec<String>, OfficeEngineError> {
    validate_arguments(&request.arguments)?;
    let resource_paths = validate_operation_arguments(request.operation, &request.arguments)?;

    if request
        .timeout_ms
        .is_some_and(|timeout| !(1..=MAX_OFFICE_TIMEOUT_MS).contains(&timeout))
    {
        return Err(invalid_request(format!(
            "Office timeout must be between 1 and {MAX_OFFICE_TIMEOUT_MS} milliseconds."
        )));
    }

    match request.operation {
        OfficeOperation::Help => {
            if request.document_path.is_some()
                || request.output_path.is_some()
                || request.destination_path.is_some()
            {
                return Err(invalid_request(
                    "The Office help operation cannot receive document, output, or destination paths.",
                ));
            }
        }
        OfficeOperation::Create => {
            required_document_path(request)?;
            if request.output_path.is_some() || request.destination_path.is_some() {
                return Err(invalid_request(
                    "The Office create operation cannot receive a separate output or destination path.",
                ));
            }
        }
        OfficeOperation::View => {
            required_document_path(request)?;
            if request.destination_path.is_some() {
                return Err(invalid_request(
                    "The Office view operation cannot receive a mutation destination path.",
                ));
            }
            let mode = request.arguments.first().map(String::as_str);
            let writes_render = matches!(mode, Some("html" | "screenshot" | "svg"));
            match (request.output_path.is_some(), writes_render) {
                (true, false) => {
                    return Err(invalid_request(
                        "Office view output paths are supported only for html, screenshot, or svg modes.",
                    ))
                }
                (false, true) => {
                    return Err(invalid_request(
                        "Rendering requires an explicit output path.",
                    ))
                }
                _ => {}
            }
        }
        OfficeOperation::Set
        | OfficeOperation::Add
        | OfficeOperation::Remove
        | OfficeOperation::Move
        | OfficeOperation::Swap => {
            required_document_path(request)?;
            if request.output_path.is_some() {
                return Err(invalid_request(
                    "Only the Office view operation accepts an output path.",
                ));
            }
        }
        OfficeOperation::Get | OfficeOperation::Query | OfficeOperation::Validate => {
            required_document_path(request)?;
            if request.output_path.is_some() || request.destination_path.is_some() {
                return Err(invalid_request(
                    "Read-only Office operations cannot receive output or mutation destination paths.",
                ));
            }
        }
    }

    if let Some(document_path) = request.document_path.as_deref() {
        validate_document_extension(request, Path::new(document_path.trim()))?;
    }
    if let Some(destination_path) = request.destination_path.as_deref() {
        validate_document_extension(request, Path::new(destination_path.trim()))?;
    }
    if let Some(output_path) = request.output_path.as_deref() {
        validate_render_output_extension(
            Path::new(output_path.trim()),
            request.arguments.first().map(String::as_str),
        )?;
    }

    Ok(resource_paths)
}

fn validate_render_output_extension(
    path: &Path,
    mode: Option<&str>,
) -> Result<(), OfficeEngineError> {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    let matches = match mode {
        Some("html") => matches!(extension.as_deref(), Some("html" | "htm")),
        Some("screenshot") => extension.as_deref() == Some("png"),
        Some("svg") => extension.as_deref() == Some("svg"),
        _ => false,
    };
    if matches {
        Ok(())
    } else {
        Err(invalid_request(
            "Office render output extension must match the requested html, screenshot, or svg mode.",
        ))
    }
}

fn required_document_path(request: &OfficeExecutionRequest) -> Result<&str, OfficeEngineError> {
    request
        .document_path
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| invalid_request("The Office operation requires a document path."))
}

fn validate_document_extension(
    request: &OfficeExecutionRequest,
    path: &Path,
) -> Result<(), OfficeEngineError> {
    if request.document_kind.accepts_path(path) {
        return Ok(());
    }
    Err(invalid_request(format!(
        "Document path `{}` does not match {:?}; expected one of: {}.",
        path.display(),
        request.document_kind,
        request.document_kind.accepted_extensions().join(", ")
    )))
}

fn validate_arguments(arguments: &[String]) -> Result<(), OfficeEngineError> {
    if arguments.len() > MAX_OFFICE_ARGUMENTS {
        return Err(invalid_request(format!(
            "Office argv exceeds the {MAX_OFFICE_ARGUMENTS}-argument limit."
        )));
    }
    let bytes = arguments
        .iter()
        .try_fold(0usize, |total, value| total.checked_add(value.len()))
        .ok_or_else(|| invalid_request("Office argv size overflowed."))?;
    if bytes > MAX_OFFICE_ARGUMENT_BYTES {
        return Err(invalid_request(format!(
            "Office argv exceeds the {MAX_OFFICE_ARGUMENT_BYTES}-byte limit."
        )));
    }
    if arguments.iter().any(|argument| argument.contains('\0')) {
        return Err(invalid_request("Office argv cannot contain NUL bytes."));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OfficeOptionArity {
    Flag,
    Value,
    OptionalValue,
}

fn validate_operation_arguments(
    operation: OfficeOperation,
    arguments: &[String],
) -> Result<Vec<String>, OfficeEngineError> {
    const RESERVED_OPTIONS: &[&str] = &[
        "-o",
        "--out",
        "--output",
        "--save",
        "--input",
        "--browser",
        "--resident",
        "--watch",
        "--serve",
        "--server",
    ];
    if arguments.iter().any(|argument| {
        let lower = argument.to_ascii_lowercase();
        lower.contains("file://") || lower.contains("http://") || lower.contains("https://")
    }) {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::UnsafeOperation,
            OfficeEngineRecovery::ChangeRequest,
            "Office arguments cannot request file URLs or network resources.",
        ));
    }

    let mut resource_paths = Vec::new();
    let mut positional_arguments = Vec::new();
    let mut add_type = None::<String>;
    let mut diagram_render = None::<String>;
    let mut index = 0usize;
    while index < arguments.len() {
        let argument = &arguments[index];
        if argument == "--" {
            return Err(unsafe_argument(
                argument,
                "pass-through argument separators are not allowed",
            ));
        }
        if !argument.starts_with('-') {
            positional_arguments.push(argument.as_str());
            index += 1;
            continue;
        }

        let (raw_name, attached_value) = match argument.split_once('=') {
            Some((name, value)) => (name, Some(value)),
            None => (argument.as_str(), None),
        };
        let normalized_name = raw_name.to_ascii_lowercase();
        if RESERVED_OPTIONS.contains(&normalized_name.as_str())
            || (normalized_name.starts_with("-o") && !normalized_name.starts_with("--"))
        {
            return Err(unsafe_argument(
                argument,
                "the option can write outside the managed Office transaction or changes process behavior",
            ));
        }
        if raw_name != normalized_name {
            return Err(invalid_request(
                "Office option names must use their canonical lowercase spelling.",
            ));
        }

        let Some(arity) = allowed_option(operation, &normalized_name) else {
            return Err(invalid_request(format!(
                "Office option `{raw_name}` is not allowed for `{}` by the managed v1.0.139 command profile.",
                operation.cli_name()
            )));
        };
        let value = match arity {
            OfficeOptionArity::Flag => {
                if attached_value.is_some() {
                    return Err(invalid_request(format!(
                        "Office flag `{raw_name}` does not accept a value."
                    )));
                }
                index += 1;
                None
            }
            OfficeOptionArity::Value => {
                if normalized_name == "--prop" && attached_value.is_some() {
                    return Err(invalid_request(
                        "Office properties must use separate `--prop` and `key=value` arguments; `--prop=...` is rejected.",
                    ));
                }
                let value = if let Some(value) = attached_value {
                    if value.is_empty() {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` requires a non-empty value."
                        )));
                    }
                    value
                } else {
                    let value = arguments.get(index + 1).ok_or_else(|| {
                        invalid_request(format!("Office option `{raw_name}` requires a value."))
                    })?;
                    if value.starts_with('-') {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` requires a value before `{value}`."
                        )));
                    }
                    index += 1;
                    value.as_str()
                };
                index += 1;
                Some(value)
            }
            OfficeOptionArity::OptionalValue => {
                let value = if let Some(value) = attached_value {
                    if value.is_empty() {
                        return Err(invalid_request(format!(
                            "Office option `{raw_name}` cannot receive an empty value."
                        )));
                    }
                    Some(value)
                } else if arguments
                    .get(index + 1)
                    .is_some_and(|value| !value.starts_with('-'))
                {
                    index += 1;
                    Some(arguments[index].as_str())
                } else {
                    None
                };
                index += 1;
                value
            }
        };

        if operation == OfficeOperation::Add && normalized_name == "--type" {
            add_type = value.map(|value| value.trim().to_ascii_lowercase());
        }
        if normalized_name != "--prop" {
            if operation == OfficeOperation::View
                && normalized_name == "--render"
                && value.is_some_and(|value| !matches!(value, "auto" | "html"))
            {
                return Err(unsafe_argument(
                    argument,
                    "native Office rendering may launch an unmanaged desktop application",
                ));
            }
            continue;
        }
        let property = value.expect("--prop has required value arity");
        let Some((name, value)) = property.split_once('=') else {
            return Err(invalid_request(
                "Office properties must use key=value syntax.",
            ));
        };
        if name.trim().is_empty() {
            return Err(invalid_request(
                "Office properties require a non-empty key.",
            ));
        }
        let normalized_property = name.trim().to_ascii_lowercase();
        if normalized_property == "render" {
            if !value.eq_ignore_ascii_case("native") {
                return Err(unsafe_argument(
                    property,
                    "managed diagram rendering must use the built-in `native` renderer and cannot launch a browser or fetch runtime assets",
                ));
            }
            diagram_render = Some("native".to_string());
        }
        if normalized_property == "data" {
            return Err(unsafe_argument(
                property,
                "the pinned provider treats `data` as either inline content or a FileSource; this ambiguous surface is disabled until it has an element-aware schema",
            ));
        }
        if let Some(resource) = property_resource_reference(name, value)? {
            let path = resource.path();
            if path.to_ascii_lowercase().starts_with("data:") {
                return Err(unsafe_argument(
                    property,
                    "inline resource payloads are not part of the frozen workspace resource set",
                ));
            }
            if !resource_paths.iter().any(|candidate| candidate == path) {
                resource_paths.push(path.to_string());
            }
        }
    }

    if operation == OfficeOperation::Add
        && add_type
            .as_deref()
            .is_some_and(|value| matches!(value, "diagram" | "flowchart" | "mermaid"))
        && diagram_render.as_deref() != Some("native")
    {
        return Err(unsafe_argument(
            "--type diagram",
            "diagram creation must explicitly include `--prop render=native` so the provider cannot start a browser or fetch mermaid.js",
        ));
    }

    let (minimum, maximum) = positional_argument_bounds(operation);
    let positional_count = positional_arguments.len();
    if !(minimum..=maximum).contains(&positional_count) {
        let expected = if minimum == maximum {
            minimum.to_string()
        } else {
            format!("{minimum} to {maximum}")
        };
        return Err(invalid_request(format!(
            "Office operation `{}` requires {expected} positional argument(s) after the document path; received {positional_count}.",
            operation.cli_name()
        )));
    }
    if operation == OfficeOperation::View {
        let mode = positional_arguments[0];
        if !matches!(
            mode,
            "text"
                | "annotated"
                | "outline"
                | "stats"
                | "issues"
                | "html"
                | "svg"
                | "screenshot"
                | "forms"
        ) {
            return Err(invalid_request(format!(
                "Office view mode `{mode}` is outside the managed v1.0.139 mode profile."
            )));
        }
    }
    Ok(resource_paths)
}

fn allowed_option(operation: OfficeOperation, name: &str) -> Option<OfficeOptionArity> {
    use OfficeOperation as Operation;
    use OfficeOptionArity::{Flag, OptionalValue, Value};

    if matches!(name, "-?" | "-h" | "--help") {
        return Some(Flag);
    }
    match (operation, name) {
        (Operation::Help, "--json" | "--jsonl") => Some(Flag),
        (Operation::Create, "--type" | "--locale") => Some(Value),
        (Operation::Create, "--force" | "--minimal" | "--json") => Some(Flag),
        (
            Operation::View,
            "--start"
            | "--end"
            | "--max-lines"
            | "--type"
            | "--limit"
            | "--cols"
            | "--page"
            | "--range"
            | "--screenshot-width"
            | "--screenshot-height"
            | "--render",
        ) => Some(Value),
        (Operation::View, "--grid") => Some(OptionalValue),
        (Operation::View, "--page-count" | "--json") => Some(Flag),
        (Operation::Get, "--depth") => Some(Value),
        (Operation::Get, "--json") => Some(Flag),
        (Operation::Query, "--find" | "--fields") => Some(Value),
        (Operation::Query, "--compact" | "--json") => Some(Flag),
        (Operation::Validate, "--json") => Some(Flag),
        (Operation::Set, "--prop" | "--find" | "--replace") => Some(Value),
        (Operation::Set, "--json" | "--force") => Some(Flag),
        (Operation::Add, "--type" | "--from" | "--index" | "--after" | "--before" | "--prop") => {
            Some(Value)
        }
        (Operation::Add, "--json" | "--force") => Some(Flag),
        (Operation::Remove, "--shift" | "--prop") => Some(Value),
        (Operation::Remove, "--json") => Some(Flag),
        (Operation::Move, "--to" | "--index" | "--after" | "--before" | "--prop") => Some(Value),
        (Operation::Move, "--json") => Some(Flag),
        (Operation::Swap, "--json") => Some(Flag),
        _ => None,
    }
}

fn positional_argument_bounds(operation: OfficeOperation) -> (usize, usize) {
    match operation {
        OfficeOperation::Help => (0, 3),
        OfficeOperation::Create | OfficeOperation::Validate => (0, 0),
        OfficeOperation::Get => (0, 1),
        OfficeOperation::View
        | OfficeOperation::Query
        | OfficeOperation::Set
        | OfficeOperation::Add
        | OfficeOperation::Remove
        | OfficeOperation::Move => (1, 1),
        OfficeOperation::Swap => (2, 2),
    }
}

fn unsafe_argument(argument: &str, reason: &str) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::UnsafeOperation,
        OfficeEngineRecovery::ChangeRequest,
        format!("Office argument `{argument}` is unsafe in the managed engine because {reason}."),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PropertyResourceReference<'a> {
    Direct(&'a str),
    ImagePrefixed(&'a str),
}

impl<'a> PropertyResourceReference<'a> {
    fn path(self) -> &'a str {
        match self {
            Self::Direct(path) | Self::ImagePrefixed(path) => path,
        }
    }

    fn rewrite(self, property_name: &str, snapshot: &Path) -> String {
        match self {
            Self::Direct(_) => format!("{property_name}={}", snapshot.to_string_lossy()),
            Self::ImagePrefixed(_) => {
                format!("{property_name}=image:{}", snapshot.to_string_lossy())
            }
        }
    }
}

fn property_resource_reference<'a>(
    name: &str,
    value: &'a str,
) -> Result<Option<PropertyResourceReference<'a>>, OfficeEngineError> {
    let name = name.trim().to_ascii_lowercase();
    let direct = matches!(
        name.as_str(),
        "csv"
            | "file"
            | "image"
            | "imagefill"
            | "imagepath"
            | "src"
            | "template"
            | "fallback"
            | "preview"
    );
    if direct {
        return Ok(Some(PropertyResourceReference::Direct(value)));
    }
    if name == "poster" {
        return if matches!(value.trim().to_ascii_lowercase().as_str(), "true" | "false") {
            Ok(None)
        } else {
            Ok(Some(PropertyResourceReference::Direct(value)))
        };
    }
    if name == "path" {
        return if matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "line" | "arc" | "circle" | "diamond" | "triangle" | "square" | "custom"
        ) {
            Ok(None)
        } else {
            Ok(Some(PropertyResourceReference::Direct(value)))
        };
    }
    if name == "background" {
        let lower = value.to_ascii_lowercase();
        if lower.starts_with("image:") {
            let path = &value["image:".len()..];
            if path.trim().is_empty() {
                return Err(invalid_request(
                    "Office image background requires a non-empty workspace resource path.",
                ));
            }
            return Ok(Some(PropertyResourceReference::ImagePrefixed(path)));
        }
    }
    Ok(None)
}

fn canonical_workspace(workspace: &Path) -> Result<PathBuf, OfficeEngineError> {
    let canonical = workspace.canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve workspace `{}`: {error}",
            workspace.display()
        ))
    })?;
    if !canonical.is_dir() {
        return Err(workspace_error(
            "The selected workspace is not a directory.",
        ));
    }
    Ok(canonical)
}

fn clean_workspace_relative_path(value: &str) -> Result<PathBuf, OfficeEngineError> {
    let value = value.trim();
    if value.is_empty() {
        return Err(invalid_request("Relative Office path cannot be empty."));
    }
    let mut output = PathBuf::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(value) => output.push(value),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(workspace_error(
                    "Relative Office paths cannot contain `..`, a root, or a platform prefix.",
                ));
            }
        }
    }
    if output.as_os_str().is_empty() {
        Err(invalid_request("Relative Office path cannot be empty."))
    } else {
        Ok(output)
    }
}

fn freeze_path(
    context: &ResolvedExecutionContext,
    slot: OfficePathSlot,
    logical_path: &str,
    purpose: OfficePathPurpose,
) -> Result<OfficeFrozenPath, OfficeEngineError> {
    let logical_path = logical_path.trim();
    if logical_path.is_empty() {
        return Err(invalid_request("Office path cannot be empty."));
    }
    if logical_path.contains('\0') {
        return Err(invalid_request("Office paths cannot contain NUL bytes."));
    }

    let (candidate, attachment) = if logical_path.starts_with("@attachments/") {
        if purpose != OfficePathPurpose::ReadSource {
            return Err(workspace_error(
                "Registered attachments are immutable read sources and cannot be Office write targets.",
            ));
        }
        (resolve_attachment(context, logical_path)?, true)
    } else if let Some(expanded) = expand_system_path(logical_path).map_err(workspace_error)? {
        (normalize_absolute_path(&expanded)?, false)
    } else {
        let path = Path::new(logical_path);
        if path.is_absolute() {
            (normalize_absolute_path(path)?, false)
        } else {
            let workspace = context.workspace.as_deref().ok_or_else(|| {
                workspace_error("Relative Office paths require a selected workspace.")
            })?;
            (
                workspace.join(clean_workspace_relative_path(logical_path)?),
                false,
            )
        }
    };

    let lexical_scope = if attachment {
        OfficePathScope::Attachment
    } else if context
        .workspace
        .as_deref()
        .is_some_and(|workspace| candidate.starts_with(workspace))
    {
        OfficePathScope::Workspace
    } else {
        OfficePathScope::External
    };
    authorize_path(context.permissions, purpose, lexical_scope)?;

    let requires_existing = purpose != OfficePathPurpose::WriteTarget;
    reject_symlink_components(&candidate, !requires_existing)?;
    let metadata = match fs::symlink_metadata(&candidate) {
        Ok(metadata) => Some(metadata),
        Err(error) if !requires_existing && error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(workspace_error(format!(
                "Cannot inspect Office path `{}`: {error}",
                candidate.display()
            )))
        }
    };

    let normalized = if metadata.is_some() {
        candidate.canonicalize().map_err(|error| {
            workspace_error(format!(
                "Cannot canonicalize Office path `{}`: {error}",
                candidate.display()
            ))
        })?
    } else {
        let parent = candidate
            .parent()
            .ok_or_else(|| workspace_error("Office output path has no parent directory."))?;
        let parent = parent.canonicalize().map_err(|error| {
            workspace_error(format!(
                "Office output parent `{}` must already exist and be accessible: {error}",
                parent.display()
            ))
        })?;
        parent.join(
            candidate
                .file_name()
                .ok_or_else(|| workspace_error("Office output path has no file name."))?,
        )
    };
    let scope = if attachment {
        OfficePathScope::Attachment
    } else if context
        .workspace
        .as_deref()
        .is_some_and(|workspace| normalized.starts_with(workspace))
    {
        OfficePathScope::Workspace
    } else {
        OfficePathScope::External
    };
    if scope != lexical_scope {
        return Err(workspace_error(
            "Office path scope changed during normalization; a symlink or mount escape may be present.",
        ));
    }
    authorize_path(context.permissions, purpose, scope)?;

    let parent = normalized
        .parent()
        .ok_or_else(|| workspace_error("Office path has no parent directory."))?;
    reject_symlink_components(parent, false)?;
    let parent_metadata = fs::symlink_metadata(parent).map_err(|error| {
        workspace_error(format!(
            "Cannot inspect Office parent `{}`: {error}",
            parent.display()
        ))
    })?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(workspace_error(
            "Office path parent must be a real directory, not a symlink or special file.",
        ));
    }
    let parent_identity = path_identity(parent, &parent_metadata)?;

    let (state, object_identity, content_revision, size) = match metadata {
        Some(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(workspace_error(format!(
                    "Office path `{}` must be a regular non-symlink file.",
                    normalized.display()
                )));
            }
            let identity = path_identity(&normalized, &metadata)?;
            let (revision, size) = file_revision(&normalized)?;
            (
                OfficeFileState::Present,
                Some(identity),
                Some(revision),
                Some(size),
            )
        }
        None => (OfficeFileState::Missing, None, None, None),
    };
    if requires_existing && state != OfficeFileState::Present {
        return Err(workspace_error(
            "Office source or in-place target must exist.",
        ));
    }
    let write_disposition = match purpose {
        OfficePathPurpose::ReadSource => None,
        OfficePathPurpose::WriteTarget | OfficePathPurpose::InPlaceTarget => Some(match state {
            OfficeFileState::Missing => OfficeWriteDisposition::CreateNew,
            OfficeFileState::Present => OfficeWriteDisposition::ReplaceExisting,
        }),
    };

    Ok(OfficeFrozenPath {
        slot,
        logical_path: logical_path.to_string(),
        purpose,
        scope,
        normalized_path: normalized.to_string_lossy().into_owned(),
        state,
        object_identity,
        parent_identity,
        content_revision,
        size,
        write_disposition,
    })
}

fn authorize_path(
    permissions: AgentPermissions,
    purpose: OfficePathPurpose,
    scope: OfficePathScope,
) -> Result<(), OfficeEngineError> {
    match purpose {
        OfficePathPurpose::ReadSource => {
            if scope == OfficePathScope::External && permissions.read != AgentReadPermission::All {
                return Err(workspace_error(
                    "Reading an Office source outside the workspace requires read=all.",
                ));
            }
        }
        OfficePathPurpose::WriteTarget | OfficePathPurpose::InPlaceTarget => {
            if permissions.write == AgentWritePermission::Denied {
                return Err(workspace_error(
                    "Office file changes are disabled by write=denied.",
                ));
            }
            if scope != OfficePathScope::Workspace && permissions.write != AgentWritePermission::All
            {
                return Err(workspace_error(
                    "Writing an Office target outside the workspace requires write=all.",
                ));
            }
        }
    }
    Ok(())
}

fn resolve_attachment(
    context: &ResolvedExecutionContext,
    logical_path: &str,
) -> Result<PathBuf, OfficeEngineError> {
    let library = context.attachment_library.as_ref().ok_or_else(|| {
        workspace_error("No registered attachment library is available for this Office run.")
    })?;
    let attachment_id = logical_path
        .strip_prefix("@attachments/")
        .and_then(|remainder| remainder.split('/').next())
        .filter(|id| !id.is_empty())
        .ok_or_else(|| invalid_request("Attachment paths must include an attachment id."))?;
    let reference = library
        .conversation_attachments
        .iter()
        .chain(library.project_attachments.iter())
        .find(|reference| reference.id == attachment_id)
        .ok_or_else(|| {
            workspace_error(format!(
                "Registered attachment `{attachment_id}` was not found."
            ))
        })?;
    if reference.read_path != logical_path {
        return Err(workspace_error(
            "Office attachment paths must exactly match the registered readPath.",
        ));
    }
    let root = library
        .root_path
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| workspace_error("The registered attachment library has no root path."))?;
    let root = Path::new(root).canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve the attachment library root: {error}"
        ))
    })?;
    if !root.is_dir() {
        return Err(workspace_error(
            "The registered attachment library root is not a directory.",
        ));
    }
    let relative = clean_workspace_relative_path(&reference.storage_rel_path)?;
    let candidate = root.join(relative);
    reject_symlink_components(&candidate, false)?;
    let canonical = candidate.canonicalize().map_err(|error| {
        workspace_error(format!(
            "Cannot resolve registered Office attachment: {error}"
        ))
    })?;
    if !canonical.starts_with(&root) {
        return Err(workspace_error(
            "Registered Office attachment escaped its attachment library.",
        ));
    }
    Ok(canonical)
}

fn normalize_absolute_path(path: &Path) -> Result<PathBuf, OfficeEngineError> {
    if !path.is_absolute() {
        return Err(invalid_request("Expected an absolute Office path."));
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(workspace_error(
                    "Absolute Office paths cannot contain `..`.",
                ))
            }
        }
    }
    Ok(normalized)
}

fn reject_symlink_components(
    path: &Path,
    allow_missing_final: bool,
) -> Result<(), OfficeEngineError> {
    let mut current = PathBuf::new();
    let component_count = path.components().count();
    for (index, component) in path.components().enumerate() {
        current.push(component.as_os_str());
        if matches!(component, Component::Prefix(_) | Component::RootDir) {
            continue;
        }
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(workspace_error(format!(
                        "Office path component `{}` is a symbolic link.",
                        current.display()
                    )));
                }
                if index + 1 < component_count && !metadata.is_dir() {
                    return Err(workspace_error(format!(
                        "Office path component `{}` is not a directory.",
                        current.display()
                    )));
                }
            }
            Err(error)
                if allow_missing_final
                    && index + 1 == component_count
                    && error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(())
            }
            Err(error) => {
                return Err(workspace_error(format!(
                    "Cannot inspect Office path component `{}`: {error}",
                    current.display()
                )))
            }
        }
    }
    Ok(())
}

fn path_identity(
    path: &Path,
    metadata: &fs::Metadata,
) -> Result<OfficePathIdentity, OfficeEngineError> {
    let mut digest = Sha256::new();
    digest.update(b"mycopilot.office.path-identity\0");
    digest.update(path.as_os_str().as_encoded_bytes());
    digest.update(if metadata.is_dir() {
        &b"directory"[..]
    } else {
        &b"file"[..]
    });
    let created = metadata
        .created()
        .ok()
        .and_then(|value| value.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    digest.update(created.to_le_bytes());
    #[cfg(unix)]
    let (device, inode) = {
        use std::os::unix::fs::MetadataExt;
        (Some(metadata.dev()), Some(metadata.ino()))
    };
    #[cfg(not(unix))]
    let (device, inode) = (None, None);
    if let Some(device) = device {
        digest.update(device.to_le_bytes());
    }
    if let Some(inode) = inode {
        digest.update(inode.to_le_bytes());
    }
    Ok(OfficePathIdentity {
        revision: format!("{PATH_IDENTITY_PREFIX}{}", hex_lower(&digest.finalize())),
        device,
        inode,
    })
}

fn copy_file_snapshot(source: &Path, target: &Path) -> Result<(), OfficeEngineError> {
    let mut source_options = fs::OpenOptions::new();
    source_options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        source_options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    let mut source_file = source_options
        .open(source)
        .map_err(|error| io_error("open the Office source snapshot", error))?;
    let before = source_file
        .metadata()
        .map_err(|error| io_error("inspect the Office source snapshot", error))?;
    if !before.is_file() || before.len() > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(workspace_error(format!(
            "Office file exceeds the {MAX_OFFICE_DOCUMENT_BYTES}-byte limit."
        )));
    }
    let mut target_options = fs::OpenOptions::new();
    target_options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        target_options.mode(0o600);
    }
    let mut target_file = target_options
        .open(target)
        .map_err(|error| io_error("create the Office file snapshot", error))?;
    let copied = {
        let mut limited = (&mut source_file).take(MAX_OFFICE_DOCUMENT_BYTES + 1);
        std::io::copy(&mut limited, &mut target_file)
            .map_err(|error| io_error("copy the Office file into staging", error))?
    };
    if copied != before.len() || copied > MAX_OFFICE_DOCUMENT_BYTES {
        return Err(precondition_error(
            "Office file changed or exceeded its size limit while being copied into staging.",
        ));
    }
    let after = source_file
        .metadata()
        .map_err(|error| io_error("reinspect the Office source snapshot", error))?;
    if after.len() != before.len() {
        return Err(precondition_error(
            "Office file changed while being copied into staging.",
        ));
    }
    target_file
        .flush()
        .and_then(|_| target_file.sync_all())
        .map_err(|error| io_error("sync the staged Office file", error))?;
    fs::set_permissions(target, before.permissions())
        .map_err(|error| io_error("preserve Office file permissions in staging", error))
}

fn snapshot_prepared_resources(
    prepared: &OfficePreparedExecution,
) -> Result<(tempfile::TempDir, HashMap<String, PathBuf>), OfficeEngineError> {
    let snapshots = tempfile::Builder::new()
        .prefix("mycopilot-office-resources-")
        .tempdir()
        .map_err(|error| io_error("create private Office resource snapshots", error))?;
    let resources = prepared
        .paths
        .iter()
        .filter(|path| matches!(path.slot, OfficePathSlot::Resource { .. }))
        .collect::<Vec<_>>();
    let mut resolved = HashMap::with_capacity(resources.len());
    for (index, resource) in resources.into_iter().enumerate() {
        if resource.state != OfficeFileState::Present {
            return Err(precondition_error(
                "Office resource preconditions must describe existing files.",
            ));
        }
        let source = Path::new(&resource.normalized_path);
        let name = source
            .file_name()
            .ok_or_else(|| invalid_request("Office resource path has no file name."))?
            .to_string_lossy()
            .into_owned();
        let snapshot = snapshots.path().join(format!("{index}-{name}"));
        copy_file_snapshot(source, &snapshot)?;
        let (revision, size) = file_revision(&snapshot)?;
        if resource.content_revision.as_deref() != Some(&revision) || resource.size != Some(size) {
            return Err(precondition_error(format!(
                "Office resource `{}` changed while its private snapshot was created.",
                resource.logical_path
            )));
        }
        resolved.insert(resource.logical_path.clone(), snapshot);
    }
    Ok((snapshots, resolved))
}

fn frozen_path<'a>(
    prepared: &'a OfficePreparedExecution,
    slot: &OfficePathSlot,
) -> Option<&'a OfficeFrozenPath> {
    prepared.paths.iter().find(|path| &path.slot == slot)
}

fn rewrite_path_bearing_properties(
    resource_paths: &HashMap<String, PathBuf>,
    argv: &mut [String],
) -> Result<(), OfficeEngineError> {
    let mut index = 0;
    while index < argv.len() {
        if argv[index] == "--prop" {
            let property_index = index + 1;
            let (name, value) = {
                let property = argv
                    .get(property_index)
                    .ok_or_else(|| invalid_request("Office `--prop` requires a value."))?;
                let (name, value) = property.split_once('=').ok_or_else(|| {
                    invalid_request("Office properties must use key=value syntax.")
                })?;
                (name.to_string(), value.to_string())
            };
            if let Some(resource) = property_resource_reference(&name, &value)? {
                let resolved = resource_paths.get(resource.path()).ok_or_else(|| {
                    precondition_error(format!(
                        "Office resource `{}` is missing from the frozen execution plan.",
                        resource.path()
                    ))
                })?;
                argv[property_index] = resource.rewrite(&name, resolved);
            }
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn validate_document_artifact(
    path: &Path,
    kind: super::types::OfficeDocumentKind,
) -> Result<(), OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI did not produce a readable document: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OFFICE_DOCUMENT_BYTES
    {
        return Err(invalid_output(format!(
            "OfficeCLI output must be a non-empty regular file no larger than {MAX_OFFICE_DOCUMENT_BYTES} bytes."
        )));
    }
    if path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("csv"))
    {
        return Ok(());
    }

    let file = fs::File::open(path)
        .map_err(|error| invalid_output(format!("Cannot open OfficeCLI output: {error}")))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI output is not a valid OOXML ZIP package: {error}"
        ))
    })?;
    if archive.len() > 65_535 || archive.by_name("[Content_Types].xml").is_err() {
        return Err(invalid_output(
            "OfficeCLI output is missing required OOXML package metadata.",
        ));
    }
    let main_part = match kind {
        super::types::OfficeDocumentKind::Document => "word/document.xml",
        super::types::OfficeDocumentKind::Spreadsheet => "xl/workbook.xml",
        super::types::OfficeDocumentKind::Presentation => "ppt/presentation.xml",
    };
    if archive.by_name(main_part).is_err() {
        return Err(invalid_output(format!(
            "OfficeCLI output is missing required OOXML part `{main_part}`."
        )));
    }
    Ok(())
}

fn validate_render_artifact(path: &Path, mode: Option<&str>) -> Result<(), OfficeEngineError> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        invalid_output(format!(
            "OfficeCLI did not produce the requested render output: {error}"
        ))
    })?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OFFICE_DOCUMENT_BYTES
    {
        return Err(invalid_output(
            "OfficeCLI render output must be a non-empty bounded regular file.",
        ));
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase);
    match mode {
        Some("html") if matches!(extension.as_deref(), Some("html" | "htm")) => Ok(()),
        Some("svg") if extension.as_deref() == Some("svg") => Ok(()),
        Some("screenshot") if extension.as_deref() == Some("png") => {
            let mut file = fs::File::open(path)
                .map_err(|error| invalid_output(format!("Cannot inspect PNG output: {error}")))?;
            let mut magic = [0_u8; 8];
            file.read_exact(&mut magic)
                .map_err(|error| invalid_output(format!("Cannot inspect PNG output: {error}")))?;
            if magic == [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a] {
                Ok(())
            } else {
                Err(invalid_output(
                    "OfficeCLI screenshot output is not a PNG file.",
                ))
            }
        }
        _ => Err(invalid_output(
            "OfficeCLI render output extension does not match the requested mode.",
        )),
    }
}

struct StagingArea {
    directory: PathBuf,
    path: PathBuf,
    published: bool,
}

fn office_target_commit_lock(target: &Path) -> Arc<Mutex<()>> {
    let registry = OFFICE_COMMIT_LOCKS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut registry = registry.lock().unwrap_or_else(|error| error.into_inner());
    registry.retain(|_, lock| lock.strong_count() > 0);
    if let Some(lock) = registry.get(target).and_then(Weak::upgrade) {
        return lock;
    }
    let lock = Arc::new(Mutex::new(()));
    registry.insert(target.to_path_buf(), Arc::downgrade(&lock));
    lock
}

impl StagingArea {
    fn new(target: &Path) -> Result<Self, OfficeEngineError> {
        let parent = target
            .parent()
            .ok_or_else(|| workspace_error("Office target has no parent directory."))?;
        let file_name = target
            .file_name()
            .ok_or_else(|| workspace_error("Office target has no file name."))?;
        for _ in 0..16 {
            let directory = parent.join(format!(
                ".mycopilot-office-{}",
                uuid::Uuid::new_v4().simple()
            ));
            match fs::create_dir(&directory) {
                Ok(()) => {
                    set_private_directory_permissions(&directory)?;
                    let path = directory.join(file_name);
                    return Ok(Self {
                        directory,
                        path,
                        published: false,
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => {
                    return Err(io_error("create same-directory Office staging", error));
                }
            }
        }
        Err(io_error(
            "create same-directory Office staging",
            std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "could not allocate a unique staging directory",
            ),
        ))
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn directory(&self) -> &Path {
        &self.directory
    }

    fn publish(
        &mut self,
        target: &Path,
        expected_state: OfficeFileState,
    ) -> Result<(), OfficeEngineError> {
        set_publish_permissions(&self.path, target, expected_state)?;
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.path)
            .map_err(|error| io_error("open staged Office output for sync", error))?;
        file.sync_all()
            .map_err(|error| io_error("sync staged Office output", error))?;
        let rename = match expected_state {
            OfficeFileState::Missing => atomic_rename_noreplace(&self.path, target),
            OfficeFileState::Present => atomic_replace(&self.path, target),
        };
        rename.map_err(|error| {
            precondition_error(format!(
                "Cannot atomically publish the Office output; target state may have changed: {error}"
            ))
        })?;
        self.published = true;
        let parent = target
            .parent()
            .ok_or_else(|| workspace_error("Published Office output has no parent directory."))?;
        sync_directory(parent).map_err(|error| {
            OfficeEngineError::new(
                OfficeEngineErrorCode::CommitIndeterminate,
                OfficeEngineRecovery::InspectState,
                format!(
                    "Office output was renamed into place but its directory could not be synced: {error}"
                ),
            )
        })?;
        Ok(())
    }
}

fn set_publish_permissions(
    staging: &Path,
    target: &Path,
    expected_state: OfficeFileState,
) -> Result<(), OfficeEngineError> {
    if expected_state == OfficeFileState::Present {
        let permissions = fs::metadata(target)
            .map_err(|error| {
                precondition_error(format!("Cannot inspect target permissions: {error}"))
            })?
            .permissions();
        return fs::set_permissions(staging, permissions)
            .map_err(|error| io_error("preserve target Office file permissions", error));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staging, fs::Permissions::from_mode(0o600))
            .map_err(|error| io_error("protect new Office output", error))?;
    }
    Ok(())
}

impl Drop for StagingArea {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
        let _ = fs::remove_dir(&self.directory);
    }
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), OfficeEngineError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|error| io_error("protect same-directory Office staging", error))
}

#[cfg(windows)]
fn set_private_directory_permissions(_path: &Path) -> Result<(), OfficeEngineError> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::rename(source, target)
}

#[cfg(target_vendor = "apple")]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target contains NUL")
    })?;
    // SAFETY: both C strings are NUL-terminated and valid for the duration of the call.
    let result = unsafe { libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_EXCL) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source = CString::new(source.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "source contains NUL")
    })?;
    let target = CString::new(target.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "target contains NUL")
    })?;
    // SAFETY: pointers address live NUL-terminated C strings; renameat2 does not retain them.
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            target.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(all(
    unix,
    not(target_vendor = "apple"),
    not(any(target_os = "linux", target_os = "android"))
))]
fn atomic_rename_noreplace(_source: &Path, _target: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

#[cfg(windows)]
fn atomic_replace(source: &Path, target: &Path) -> std::io::Result<()> {
    move_file(source, target, true)
}

#[cfg(windows)]
fn atomic_rename_noreplace(source: &Path, target: &Path) -> std::io::Result<()> {
    move_file(source, target, false)
}

#[cfg(windows)]
fn move_file(source: &Path, target: &Path, replace: bool) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let source = source
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let target = target
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut flags = MOVEFILE_WRITE_THROUGH;
    if replace {
        flags |= MOVEFILE_REPLACE_EXISTING;
    }
    // SAFETY: both UTF-16 buffers are NUL-terminated and live for the call.
    let result = unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), flags) };
    if result == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

struct ProcessOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
    timed_out: bool,
    cancelled: bool,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

fn run_process(
    executable: &Path,
    cwd: Option<&Path>,
    argv: &[String],
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
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
                format!("Cannot start OfficeCLI: {error}"),
            )
        })?;
    let stdout_reader = spawn_bounded_output_reader(
        child
            .stdout
            .take()
            .ok_or_else(|| process_error("Cannot capture OfficeCLI stdout."))?,
    );
    let stderr_reader = spawn_bounded_output_reader(
        child
            .stderr
            .take()
            .ok_or_else(|| process_error("Cannot capture OfficeCLI stderr."))?,
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
            .map_err(|error| process_error(format!("Cannot wait for OfficeCLI: {error}")))?
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
    let (stdout, stdout_truncated) =
        join_output_reader(stdout_reader, "OfficeCLI stdout").map_err(process_error)?;
    let (stderr, stderr_truncated) =
        join_output_reader(stderr_reader, "OfficeCLI stderr").map_err(process_error)?;
    Ok(ProcessOutput {
        status,
        stdout,
        stderr,
        timed_out,
        cancelled,
        stdout_truncated,
        stderr_truncated,
    })
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

fn wait_for_child(child: &mut Child, state: &str) -> Result<ExitStatus, OfficeEngineError> {
    child
        .wait()
        .map_err(|error| process_error(format!("Cannot wait for {state} OfficeCLI: {error}")))
}

fn cancellation_requested(
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
    })
}

fn execution_error(output: &ProcessOutput) -> (Option<String>, Option<String>) {
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

fn cancelled_result(
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
        error_code: Some("office.cancelled".to_string()),
        error: Some("OfficeCLI execution was cancelled before launch.".to_string()),
    }
}

fn validate_platform() -> Result<(), OfficeEngineError> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(OfficeEngineError::new(
            OfficeEngineErrorCode::UnsupportedOperation,
            OfficeEngineRecovery::ChangeRequest,
            "Managed OfficeCLI execution currently requires Unix process-group isolation; Windows remains disabled until Job Object containment is available.",
        ))
    }
}

fn invalid_request(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::InvalidRequest,
        OfficeEngineRecovery::ChangeRequest,
        message,
    )
}

fn workspace_error(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::WorkspaceViolation,
        OfficeEngineRecovery::ChangeRequest,
        message,
    )
}

fn precondition_error(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::PreconditionFailed,
        OfficeEngineRecovery::Retry,
        message,
    )
}

fn invalid_output(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::InvalidOutput,
        OfficeEngineRecovery::ChangeRequest,
        message,
    )
}

fn io_error(operation: &str, error: std::io::Error) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::Io,
        OfficeEngineRecovery::Retry,
        format!("Cannot {operation}: {error}"),
    )
}

fn process_error(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::ProcessFailure,
        OfficeEngineRecovery::Retry,
        message,
    )
}
