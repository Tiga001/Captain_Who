use super::discovery::OfficeCliEngine;
use super::render_runtime::{
    initialize_render_marker, read_render_failure, OfficeRenderFailureMarker, OfficeRenderRuntime,
    BROWSER_EXECUTABLE_ENV, BROWSER_FAILURE_MARKER_ENV, BROWSER_MARKER_NONCE_ENV,
    BROWSER_MAX_INVOCATIONS_ENV, BROWSER_PROFILE_ENV, BROWSER_PROXY_MODE_ENV,
    BROWSER_PROXY_SELF_TEST_ARGUMENT, BROWSER_PROXY_SELF_TEST_RESPONSE,
    BROWSER_PROXY_SELF_TEST_TIMEOUT,
};
use super::types::{
    office_agent_input_placeholder, OfficeDocumentKind, OfficeElementPosition, OfficeEngine,
    OfficeEngineAvailability, OfficeEngineError, OfficeEngineErrorCode, OfficeEngineRecovery,
    OfficeEngineStatus, OfficeExecutionContext, OfficeExecutionRequest, OfficeExecutionResult,
    OfficeFileState, OfficeFrozenPath, OfficeGridLayout, OfficeOperation, OfficeOperationAccess,
    OfficeOperationParameters, OfficePathIdentity, OfficePathPurpose, OfficePathScope,
    OfficePathSlot, OfficePreparedExecution, OfficePresentationRenderPlan, OfficePropertyMap,
    OfficePublishedOutput, OfficePublishedOutputKind, OfficePublishedOutputRole,
    OfficeRenderPageSelection, OfficeViewMode, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX, OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
    OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
use crate::command::{
    configure_command_process_group, join_process_output_capture, spawn_process_output_capture,
    terminate_command_process_group, try_wait_command_process_group, ProcessOutputCaptureBudget,
    ProcessOutputCaptureMetadata, ProcessOutputCapturePolicy, ProcessOutputSpool,
};
use crate::file_input::{
    materialize_agent_file_inputs, normalize_agent_file_input_specs,
    prepare_agent_file_input_bindings, AgentFileInputError, AgentFileInputExecutionContext,
    PreparedAgentFileInputs,
};
use crate::{
    expand_system_path, AgentAttachmentLibraryContext, AgentCancellationToken,
    AgentFileInputBinding, AgentFileInputRef, AgentPermissions, AgentReadPermission,
    AgentWritePermission,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant};

mod arguments;
mod coverage;
mod filesystem;
mod presentation_render;
mod process;
mod validation;

#[cfg(test)]
use arguments::canonical_page_ranges;
pub(crate) use arguments::compile_office_arguments;
pub(crate) use coverage::verify_render_layout_coverage;
use filesystem::*;
use presentation_render::{apply_presentation_render_plan, resolve_presentation_render_plan};
use process::*;
pub(crate) use validation::validate_office_request;
use validation::*;

pub const DEFAULT_OFFICE_TIMEOUT_MS: u64 = 120_000;
pub const MAX_OFFICE_TIMEOUT_MS: u64 = 600_000;
pub const MAX_OFFICE_ARGUMENTS: usize = 256;
pub const MAX_OFFICE_ARGUMENT_BYTES: usize = 128 * 1024;
pub const MAX_OFFICE_DOCUMENT_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_OFFICE_PROPERTIES: usize = 96;
pub const MAX_OFFICE_LIST_VALUES: usize = 64;
pub const MAX_OFFICE_SCREENSHOT_DIMENSION: u32 = 16_384;
pub const MAX_OFFICE_GRID_COLUMNS: u16 = 32;
pub const MAX_OFFICE_PAGE_NUMBER: u32 = 10_000;
pub const MAX_OFFICE_TOTAL_PAGES: u32 = 128;
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
    let _ = compile_office_arguments(request)?;
    let context = ResolvedExecutionContext::resolve(context)?;
    let prepared = prepare_request(&context, request)?;
    let input_bindings = prepare_agent_file_input_bindings(
        context.workspace.as_deref(),
        context.permissions,
        &context.file_inputs,
        &request.inputs,
        None,
    )
    .map_err(office_input_prepare_error)?;
    if request_requires_browser_runtime(request) {
        engine.render_runtime()?.verify_integrity()?;
    }
    Ok(OfficePreparedExecution {
        schema_version: OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
        provider_id: OFFICECLI_PROVIDER_ID.to_string(),
        engine_revision: engine.engine_revision().to_string(),
        workspace_revision: context.workspace_revision.clone(),
        access: request.access(),
        request: request.clone(),
        argv: prepared.argv,
        resolved_render_plan: prepared.resolved_render_plan,
        paths: prepared.paths,
        input_bindings,
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
    let _ = compile_office_arguments(&prepared.request)?;
    if request_requires_browser_runtime(&prepared.request) {
        engine.render_runtime()?.verify_integrity()?;
    }
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
        || current.resolved_render_plan != prepared.resolved_render_plan
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
    validate_frozen_agent_inputs(&prepared.request, &prepared.input_bindings)?;
    let prepared_inputs = materialize_agent_file_inputs(
        context.workspace.as_deref(),
        context.permissions,
        &context.file_inputs,
        &prepared.input_bindings,
        Some(&cancellation),
    )
    .map_err(office_input_execution_error)?;

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
            prepared_inputs.as_ref(),
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
            prepared_inputs.as_ref(),
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
    file_inputs: AgentFileInputExecutionContext,
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
            file_inputs: context.file_inputs().clone(),
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
    if current.argv != prepared.argv
        || current.resolved_render_plan != prepared.resolved_render_plan
        || current.paths != prepared.paths
    {
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
    prepared_inputs: Option<&PreparedAgentFileInputs>,
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
    let (_resource_snapshots, mut resource_paths) = snapshot_prepared_resources(prepared)?;
    extend_agent_input_resource_paths(prepared, prepared_inputs, &mut resource_paths)?;
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
        browser_process_policy(engine, &prepared.request)?,
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
    prepared_inputs: Option<&PreparedAgentFileInputs>,
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
    let (_resource_snapshots, mut resource_paths) = snapshot_prepared_resources(prepared)?;
    extend_agent_input_resource_paths(prepared, prepared_inputs, &mut resource_paths)?;
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
        browser_process_policy(engine, &prepared.request)?,
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
    let mut output = run_process(
        engine.executable_path(),
        Some(private.path()),
        &actual_argv,
        timeout,
        cancellation,
        action_cancel_flag,
        browser_process_policy(engine, &prepared.request)?,
    )?;
    redact_private_render_paths(
        &mut output,
        [
            (
                staging.path(),
                prepared
                    .request
                    .output_path
                    .as_deref()
                    .unwrap_or("<render-output>"),
            ),
            (
                snapshot.as_path(),
                prepared
                    .request
                    .document_path
                    .as_deref()
                    .unwrap_or("<office-document>"),
            ),
            (staging.directory(), "<office-staging>"),
            (private.path(), "<office-render-snapshot>"),
        ],
    );
    let mut result = process_result(engine, prepared, output, started);
    if result.error_code.is_some() {
        return Ok(result);
    }
    if let Err(error) = validate_render_artifact(
        staging.path(),
        prepared.request.view_mode().map(OfficeViewMode::cli_name),
    ) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    let staged_output = match prepare_published_render_output(context, prepared, staging.path()) {
        Ok(output) => output,
        Err(error) => {
            attach_post_process_error(&mut result, error);
            return Ok(result);
        }
    };
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
    match staging.publish(&target, output_precondition.state) {
        Ok(()) => match prepare_published_render_output(context, prepared, &target) {
            Ok(published_output) if published_output == staged_output => {
                result.outputs.push(published_output);
            }
            Ok(_) => attach_post_process_error(
                &mut result,
                published_output_verification_error(
                    "The final Office render bytes changed during atomic publication.",
                ),
            ),
            Err(error) => attach_post_process_error(
                &mut result,
                published_output_verification_error(format!(
                    "The final Office render content identity could not be verified: {}",
                    error.message()
                )),
            ),
        },
        Err(error) => attach_post_process_error(&mut result, error),
    }
    Ok(result)
}

fn published_output_verification_error(message: impl Into<String>) -> OfficeEngineError {
    OfficeEngineError::new(
        OfficeEngineErrorCode::CommitIndeterminate,
        OfficeEngineRecovery::InspectState,
        message,
    )
}

fn prepare_published_render_output(
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    staged_path: &Path,
) -> Result<OfficePublishedOutput, OfficeEngineError> {
    let output = frozen_path(prepared, &OfficePathSlot::Output)
        .ok_or_else(|| precondition_error("Office render output precondition is missing."))?;
    let final_path = Path::new(&output.normalized_path);
    let read_path = match output.scope {
        OfficePathScope::Workspace => {
            let workspace = context.workspace.as_deref().ok_or_else(|| {
                precondition_error("Workspace-scoped Office output has no workspace identity.")
            })?;
            let relative = final_path.strip_prefix(workspace).map_err(|_| {
                precondition_error(
                    "Workspace-scoped Office output escaped the current workspace identity.",
                )
            })?;
            portable_relative_path(relative)?
        }
        OfficePathScope::External => final_path.to_string_lossy().into_owned(),
        OfficePathScope::Attachment => {
            return Err(precondition_error(
                "An Office render output cannot target an immutable attachment.",
            ))
        }
    };
    let readable_by_agent = match output.scope {
        OfficePathScope::Workspace => true,
        OfficePathScope::External => context.permissions.read == AgentReadPermission::All,
        OfficePathScope::Attachment => false,
    };
    let source = match output.scope {
        OfficePathScope::Workspace => AgentFileInputRef::Workspace {
            path: read_path.clone(),
        },
        OfficePathScope::External => AgentFileInputRef::External {
            path: read_path.clone(),
        },
        OfficePathScope::Attachment => unreachable!("rejected above"),
    };
    let (content_revision, size_bytes) = file_revision(staged_path).map_err(|error| {
        let private_path = staged_path.to_string_lossy();
        OfficeEngineError::new(
            error.code(),
            error.recovery(),
            error
                .message()
                .replace(private_path.as_ref(), "<office-render-output>"),
        )
    })?;
    let sha256 = content_revision
        .strip_prefix(FILE_REVISION_PREFIX)
        .ok_or_else(|| precondition_error("Office render output has an invalid content identity."))?
        .to_string();
    let mode = prepared.request.view_mode().map(OfficeViewMode::cli_name);
    let (kind, mime_type, dimensions) = match mode {
        Some("screenshot") => (
            OfficePublishedOutputKind::Image,
            "image/png",
            Some(png_dimensions(staged_path)?),
        ),
        Some("svg") => (OfficePublishedOutputKind::Image, "image/svg+xml", None),
        Some("html") => (OfficePublishedOutputKind::Document, "text/html", None),
        _ => {
            return Err(precondition_error(
                "Office render output has an unsupported managed render mode.",
            ))
        }
    };
    let (width, height) =
        dimensions.map_or((None, None), |(width, height)| (Some(width), Some(height)));
    let layout_coverage = if prepared.request.document_kind == OfficeDocumentKind::Presentation
        && mode == Some("screenshot")
    {
        let plan = prepared.resolved_render_plan.as_ref().ok_or_else(|| {
            precondition_error(
                "Presentation screenshot output is missing its frozen Host render plan.",
            )
        })?;
        let (actual_width, actual_height) = dimensions.ok_or_else(|| {
            published_output_verification_error(
                "Presentation screenshot dimensions could not be verified.",
            )
        })?;
        Some(verify_render_layout_coverage(
            plan,
            actual_width,
            actual_height,
        )?)
    } else {
        None
    };
    Ok(OfficePublishedOutput {
        role: OfficePublishedOutputRole::Render,
        kind,
        mime_type: mime_type.to_string(),
        source,
        read_path,
        scope: output.scope,
        readable_by_agent,
        size_bytes,
        sha256,
        width,
        height,
        page_selection: requested_page_selection(&prepared.request),
        layout_coverage,
    })
}

fn portable_relative_path(path: &Path) -> Result<String, OfficeEngineError> {
    let components = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(Ok(value.to_string_lossy().into_owned())),
            Component::CurDir => None,
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => Some(Err(
                precondition_error("Office output has a non-portable workspace-relative path."),
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if components.is_empty() {
        return Err(precondition_error(
            "Office output path cannot resolve to the workspace root.",
        ));
    }
    Ok(components.join("/"))
}

fn requested_page_selection(request: &OfficeExecutionRequest) -> OfficeRenderPageSelection {
    let OfficeOperationParameters::View { pages, .. } = request.typed_parameters() else {
        return OfficeRenderPageSelection::All;
    };
    if pages.is_empty() {
        return OfficeRenderPageSelection::All;
    }
    let pages = pages
        .iter()
        .flat_map(|range| range.start..=range.end.unwrap_or(range.start))
        .take(MAX_OFFICE_TOTAL_PAGES as usize)
        .collect();
    OfficeRenderPageSelection::Explicit { pages }
}

fn png_dimensions(path: &Path) -> Result<(u32, u32), OfficeEngineError> {
    let file = fs::File::open(path)
        .map_err(|error| invalid_output(format!("Cannot inspect PNG output: {error}")))?;
    let mut reader =
        image::ImageReader::with_format(std::io::BufReader::new(file), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_OFFICE_SCREENSHOT_DIMENSION);
    limits.max_image_height = Some(MAX_OFFICE_SCREENSHOT_DIMENSION);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|_| {
        invalid_output("OfficeCLI screenshot output is not a completely decodable bounded PNG.")
    })?;
    let dimensions = (decoded.width(), decoded.height());
    if dimensions.0 == 0 || dimensions.1 == 0 {
        return Err(invalid_output(
            "OfficeCLI screenshot output has invalid zero dimensions.",
        ));
    }
    Ok(dimensions)
}

fn redact_private_render_paths<'a, const N: usize>(
    output: &mut ProcessOutput,
    replacements: [(&'a Path, &'a str); N],
) {
    let mut spool_redactions = Vec::new();
    for (private_path, replacement) in replacements {
        let mut spellings = vec![private_path.to_string_lossy().into_owned()];
        if let Ok(canonical) = fs::canonicalize(private_path) {
            let canonical = canonical.to_string_lossy().into_owned();
            if !spellings.contains(&canonical) {
                spellings.push(canonical);
            }
        }
        #[cfg(target_vendor = "apple")]
        {
            let aliases = spellings
                .iter()
                .filter_map(|path| {
                    if path.starts_with("/var/") || path.starts_with("/tmp/") {
                        Some(format!("/private{path}"))
                    } else {
                        path.strip_prefix("/private")
                            .filter(|path| path.starts_with("/var/") || path.starts_with("/tmp/"))
                            .map(str::to_string)
                    }
                })
                .collect::<Vec<_>>();
            for alias in aliases {
                if !spellings.contains(&alias) {
                    spellings.push(alias);
                }
            }
        }
        spellings.sort_by_key(|path| std::cmp::Reverse(path.len()));
        for private_path in spellings {
            if private_path.is_empty() {
                continue;
            }
            spool_redactions.push((private_path.clone(), replacement.to_string()));
            output.stdout = output.stdout.replace(&private_path, replacement);
            output.stderr = output.stderr.replace(&private_path, replacement);
            if let Some(failure) = output.render_failure.as_mut() {
                failure.message = failure.message.replace(&private_path, replacement);
            }
        }
    }
    output.stdout_spool = output.stdout_spool.with_redactions(&spool_redactions);
    output.stderr_spool = output.stderr_spool.with_redactions(&spool_redactions);
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
        timed_out: output.timed_out
            || output
                .render_failure
                .as_ref()
                .is_some_and(|failure| failure.timed_out),
        cancelled: output.cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        stdout_truncated: output.stdout_truncated,
        stderr_truncated: output.stderr_truncated,
        output_capture: output.output_capture,
        stdout_spool: output.stdout_spool,
        stderr_spool: output.stderr_spool,
        error_code,
        error,
        outputs: Vec::new(),
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
    resolved_render_plan: Option<OfficePresentationRenderPlan>,
}

fn prepare_request(
    context: &ResolvedExecutionContext,
    request: &OfficeExecutionRequest,
) -> Result<PreparedRequest, OfficeEngineError> {
    let (_, resource_paths) = validate_request_syntax(request)?;
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
    let document = paths
        .iter()
        .find(|path| path.slot == OfficePathSlot::Document)
        .filter(|_| request.operation != OfficeOperation::Create)
        .map(|path| Path::new(&path.normalized_path));
    let resolved_render_plan = document
        .map(|path| resolve_presentation_render_plan(request, path))
        .transpose()?
        .flatten();
    let provider_request = apply_presentation_render_plan(request, resolved_render_plan.as_ref());
    argv.extend(compile_office_arguments(&provider_request)?);
    if let Some(output) = request.output_path.as_deref() {
        if request.operation != OfficeOperation::View {
            return Err(invalid_request(
                "Only the Office view operation accepts an output path.",
            ));
        }
        let mode = request.view_mode().map(OfficeViewMode::cli_name);
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
            request.view_mode().map(OfficeViewMode::cli_name),
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
        if resource.starts_with(OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX) {
            continue;
        }
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
    Ok(PreparedRequest {
        argv,
        paths,
        resolved_render_plan,
    })
}

/// Validates the complete, side-effect-free portion of an Office request.
///
/// Dynamic approval routing calls this before it decides whether a request is
/// read-only. Keeping this validation free of filesystem access lets invalid or
/// ambiguous calls fail closed without weakening the prepare-time checks.
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

#[cfg(test)]
mod page_range_limit_tests {
    use super::*;
    use crate::office::OfficePageRange;

    #[test]
    fn rejects_a_single_range_over_the_total_page_limit() {
        let error = canonical_page_ranges(&[OfficePageRange {
            start: 1,
            end: Some(129),
        }])
        .unwrap_err();

        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
        assert!(error.message().contains("128 pages"));
    }

    #[test]
    fn rejects_a_page_number_over_the_hard_limit() {
        let error = canonical_page_ranges(&[OfficePageRange {
            start: 10_001,
            end: None,
        }])
        .unwrap_err();

        assert_eq!(error.code(), OfficeEngineErrorCode::InvalidRequest);
        assert!(error.message().contains("10000"));
    }

    #[test]
    fn accepts_the_total_page_and_page_number_boundaries() {
        let ranges = [
            OfficePageRange {
                start: 1,
                end: Some(127),
            },
            OfficePageRange {
                start: 10_000,
                end: None,
            },
        ];

        assert_eq!(canonical_page_ranges(&ranges).unwrap(), "1-127,10000");
    }
}
