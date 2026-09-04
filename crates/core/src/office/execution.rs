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
    OfficeFileState, OfficeFrozenPath, OfficeGridLayout, OfficeManagedScriptBinding,
    OfficeManagedScriptOutputResult, OfficeOperation, OfficeOperationAccess,
    OfficeOperationParameters, OfficePathIdentity, OfficePathPurpose, OfficePathScope,
    OfficePathSlot, OfficePreparedExecution, OfficePresentationEditRequest,
    OfficePresentationEditResult, OfficePresentationRenderPlan, OfficePropertyMap,
    OfficePublishedOutput, OfficePublishedOutputKind, OfficePublishedOutputRole,
    OfficeRenderPageSelection, OfficeViewMode, OfficeWriteDisposition, OFFICECLI_PROVIDER_ID,
    OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX, OFFICE_ENGINE_STATUS_SCHEMA_VERSION,
    OFFICE_PREPARED_EXECUTION_SCHEMA_VERSION,
};
use crate::artifact_runtime::{ArtifactRuntimeInvocation, ArtifactRuntimeKind};
use crate::command::{
    configure_command_process_group, force_terminate_command_process_group,
    join_process_output_capture, spawn_process_output_capture, try_wait_command_process_group,
    ProcessOutputCaptureBudget, ProcessOutputCaptureMetadata, ProcessOutputCapturePolicy,
    ProcessOutputSpool,
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

/// Same-directory private candidate owned by the Host for one approved Office Skill script.
/// Dropping this value before publication removes the candidate and its private directory.
pub struct OfficeManagedScriptStaging {
    binding: OfficeManagedScriptBinding,
    staging: StagingArea,
}

impl OfficeManagedScriptStaging {
    pub(crate) fn candidate_path(&self) -> &Path {
        self.staging.path()
    }

    pub(crate) fn private_directory(&self) -> &Path {
        self.staging.directory()
    }

    pub(crate) fn document_kind(&self) -> OfficeDocumentKind {
        self.binding.document_kind
    }
}

/// Freezes the destination identity before command approval. This grants no script authority and
/// does not create a file; command execution must re-resolve the exact same binding.
pub(crate) fn prepare_managed_script_binding(
    context: &OfficeExecutionContext,
    document_kind: OfficeDocumentKind,
    purpose: super::types::OfficeManagedScriptPurpose,
    script_mount_path: String,
    source_mount_path: Option<String>,
    destination_path: &str,
) -> Result<OfficeManagedScriptBinding, OfficeEngineError> {
    let resolved = ResolvedExecutionContext::resolve(context)?;
    let destination = freeze_path(
        &resolved,
        OfficePathSlot::Destination,
        destination_path,
        OfficePathPurpose::WriteTarget,
    )?;
    if !document_kind.accepts_path(Path::new(&destination.normalized_path)) {
        return Err(invalid_request(format!(
            "Managed Office script output must use one of: {}.",
            document_kind.accepted_extensions().join(", ")
        )));
    }
    Ok(OfficeManagedScriptBinding {
        schema_version: super::types::OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION,
        document_kind,
        purpose,
        script_mount_path,
        source_mount_path,
        destination,
    })
}

/// Revalidates the approved target and allocates its same-directory private candidate.
pub(crate) fn prepare_managed_script_staging(
    context: &OfficeExecutionContext,
    binding: &OfficeManagedScriptBinding,
) -> Result<OfficeManagedScriptStaging, OfficeEngineError> {
    if binding.schema_version != super::types::OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION {
        return Err(invalid_request(
            "Managed Office script binding uses an unsupported schema version.",
        ));
    }
    let resolved = ResolvedExecutionContext::resolve(context)?;
    let current = freeze_path(
        &resolved,
        OfficePathSlot::Destination,
        &binding.destination.logical_path,
        OfficePathPurpose::WriteTarget,
    )?;
    if current != binding.destination {
        return Err(precondition_error(
            "Managed Office script destination changed after approval.",
        ));
    }
    if !binding
        .document_kind
        .accepts_path(Path::new(&current.normalized_path))
    {
        return Err(invalid_request(
            "Managed Office script destination extension changed after approval.",
        ));
    }
    let target = PathBuf::from(&current.normalized_path);
    Ok(OfficeManagedScriptStaging {
        binding: binding.clone(),
        staging: StagingArea::new(&target)?,
    })
}

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
    if request_requires_word_pdf_runtime(request) {
        engine.word_pdf_render_runtime()?.verify_integrity()?;
    } else if request_requires_browser_runtime(request) {
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
    if request_requires_word_pdf_runtime(&prepared.request) {
        engine.word_pdf_render_runtime()?.verify_integrity()?;
    } else if request_requires_browser_runtime(&prepared.request) {
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

/// Executes one fixed-facade existing-presentation edit with one provider open/save cycle and one
/// atomic publication. Each operation first passes the ordinary typed Office compiler; the
/// provider's `batch` surface is constructed only inside this adapter and is never model input.
pub(super) fn run_office_presentation_edit(
    engine: &OfficeCliEngine,
    context: &OfficeExecutionContext,
    request: &OfficePresentationEditRequest,
    cancellation: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<OfficePresentationEditResult, OfficeEngineError> {
    validate_platform()?;
    engine.verify_engine_revision()?;
    if request.operations.is_empty() || request.operations.len() > 256 {
        return Err(invalid_request(
            "Presentation edit requires between 1 and 256 typed operations.",
        ));
    }
    if request.source_path == request.destination_path {
        return Err(invalid_request(
            "Presentation edit source and save-as destination must differ.",
        ));
    }
    if request.destination_binding.schema_version
        != super::types::OFFICE_MANAGED_SCRIPT_BINDING_SCHEMA_VERSION
        || request.destination_binding.document_kind != OfficeDocumentKind::Presentation
        || request.destination_binding.purpose
            != super::types::OfficeManagedScriptPurpose::EditPresentationPlan
        || request.destination_binding.destination.logical_path != request.destination_path
    {
        return Err(precondition_error(
            "Presentation edit destination does not match its approval-time binding.",
        ));
    }
    let resolved_context = ResolvedExecutionContext::resolve(context)?;
    let source_spec = crate::AgentFileInputSpec {
        mount_path: request.source_binding.mount_path.clone(),
        source: request.source_binding.source.clone(),
    };
    let source_inputs = materialize_agent_file_inputs(
        resolved_context.workspace.as_deref(),
        resolved_context.permissions,
        &resolved_context.file_inputs,
        std::slice::from_ref(&request.source_binding),
        Some(&cancellation),
    )
    .map_err(office_input_execution_error)?
    .ok_or_else(|| precondition_error("Presentation edit source snapshot is missing."))?;
    let source_snapshot = source_inputs
        .root()
        .join(&request.source_binding.mount_path);
    let source_metadata = fs::symlink_metadata(&source_snapshot)
        .map_err(|error| io_error("inspect private presentation source snapshot", error))?;
    if source_metadata.file_type().is_symlink()
        || !source_metadata.is_file()
        || source_metadata.len() != request.source_binding.size_bytes
    {
        return Err(precondition_error(
            "Presentation edit source snapshot does not match its frozen approval identity.",
        ));
    }

    let destination = freeze_path(
        &resolved_context,
        OfficePathSlot::Destination,
        &request.destination_path,
        OfficePathPurpose::WriteTarget,
    )?;
    if destination != request.destination_binding.destination {
        return Err(precondition_error(
            "Presentation edit destination changed after approval.",
        ));
    }
    if !OfficeDocumentKind::Presentation.accepts_path(Path::new(&destination.normalized_path)) {
        return Err(invalid_request(
            "Presentation edit destination must use the .pptx extension.",
        ));
    }
    if destination.state != OfficeFileState::Missing {
        return Err(precondition_error(
            "Presentation Editor save-as destination already exists; choose a new output path.",
        ));
    }

    let expected_asset_bindings = request
        .input_bindings
        .iter()
        .map(|binding| (binding.mount_path.as_str(), binding))
        .collect::<std::collections::BTreeMap<_, _>>();
    if expected_asset_bindings.len() != request.input_bindings.len()
        || request.inputs.len() != request.input_bindings.len()
        || request.inputs.iter().any(|input| {
            expected_asset_bindings
                .get(input.mount_path.as_str())
                .is_none_or(|binding| binding.source != input.source)
        })
    {
        return Err(precondition_error(
            "Presentation edit asset specs do not match their frozen approval identities.",
        ));
    }

    struct PreparedPresentationMutation {
        request: OfficeExecutionRequest,
        argv: Vec<String>,
        input_bindings: Vec<AgentFileInputBinding>,
    }
    let mut prepared = Vec::with_capacity(request.operations.len());
    let mut referenced_input_mounts = std::collections::BTreeSet::new();
    for parameters in &request.operations {
        let operation = parameters.operation();
        if !matches!(
            operation,
            OfficeOperation::Set
                | OfficeOperation::Add
                | OfficeOperation::Remove
                | OfficeOperation::Move
                | OfficeOperation::Swap
        ) {
            return Err(invalid_request(
                "Presentation edit plans may contain only typed mutation operations.",
            ));
        }
        let operation_inputs = presentation_edit_inputs_for_operation(
            parameters,
            &request.inputs,
            &mut referenced_input_mounts,
        )?;
        let operation_request = OfficeExecutionRequest {
            document_kind: OfficeDocumentKind::Presentation,
            operation,
            document_path: Some(request.source_path.clone()),
            parameters: parameters.clone(),
            output_path: None,
            destination_path: Some(request.destination_path.clone()),
            inputs: operation_inputs,
            timeout_ms: request.timeout_ms,
        };
        let (arguments, resources) = validate_request_syntax(&operation_request)?;
        if resources
            .iter()
            .any(|resource| !resource.starts_with(OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX))
        {
            return Err(invalid_request(
                "Presentation Editor file resources must be declared through run_command.inputs.",
            ));
        }
        let input_bindings = operation_request
            .inputs
            .iter()
            .map(|input| {
                expected_asset_bindings
                    .get(input.mount_path.as_str())
                    .copied()
                    .cloned()
                    .ok_or_else(|| {
                        precondition_error(
                            "Presentation edit operation references an unfrozen asset.",
                        )
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        validate_frozen_agent_inputs(&operation_request, &input_bindings)?;
        let mut argv = vec![
            operation.cli_name().to_string(),
            request.source_binding.mount_path.clone(),
        ];
        argv.extend(arguments);
        prepared.push(PreparedPresentationMutation {
            request: operation_request,
            argv,
            input_bindings,
        });
    }
    let declared_input_mounts = request
        .inputs
        .iter()
        .map(|input| input.mount_path.clone())
        .collect::<std::collections::BTreeSet<_>>();
    if referenced_input_mounts != declared_input_mounts {
        return Err(invalid_request(
            "Every declared presentation edit asset must be referenced by at least one typed operation.",
        ));
    }
    if cancellation_requested(&cancellation, action_cancel_flag.as_ref()) {
        return Ok(cancelled_presentation_edit_result());
    }
    let target = PathBuf::from(&destination.normalized_path);
    let mut staging = StagingArea::new(&target)?;
    copy_file_snapshot(&source_snapshot, staging.path())?;
    make_private_staging_owner_writable(staging.path())?;
    let (revision, size) = file_revision(staging.path())?;
    if revision.strip_prefix(FILE_REVISION_PREFIX) != Some(request.source_binding.sha256.as_str())
        || size != request.source_binding.size_bytes
    {
        return Err(precondition_error(
            "Presentation source changed while its private edit snapshot was created.",
        ));
    }

    let materialized_inputs = materialize_agent_file_inputs(
        resolved_context.workspace.as_deref(),
        resolved_context.permissions,
        &resolved_context.file_inputs,
        &request.input_bindings,
        Some(&cancellation),
    )
    .map_err(office_input_execution_error)?;
    let mut resolved_assets = HashMap::new();
    if let Some(inputs) = materialized_inputs.as_ref() {
        for binding in &request.input_bindings {
            resolved_assets.insert(
                office_agent_input_placeholder(&binding.mount_path),
                inputs.root().join(&binding.mount_path),
            );
        }
    }
    let mut batch = Vec::with_capacity(prepared.len());
    for operation in &prepared {
        validate_frozen_agent_inputs(&operation.request, &operation.input_bindings)?;
        let mut argv = operation.argv.clone();
        rewrite_path_bearing_properties(&resolved_assets, &mut argv)?;
        batch.push(batch_entry_from_canonical_argv(&argv)?);
    }
    let batch_file = write_private_batch_file(staging.directory(), &batch)?;
    engine.verify_engine_revision()?;
    let timeout = Duration::from_millis(
        request
            .timeout_ms
            .unwrap_or(DEFAULT_OFFICE_TIMEOUT_MS)
            .clamp(1, MAX_OFFICE_TIMEOUT_MS),
    );
    let staged_name = staging
        .path()
        .file_name()
        .ok_or_else(|| invalid_request("Staged presentation has no file name."))?
        .to_string_lossy()
        .into_owned();
    let batch_name = batch_file
        .path()
        .file_name()
        .ok_or_else(|| invalid_request("Private presentation edit plan has no file name."))?
        .to_string_lossy()
        .into_owned();
    let argv = vec![
        "batch".to_string(),
        staged_name.clone(),
        "--input".to_string(),
        batch_name,
        "--stop-on-error".to_string(),
        "--json".to_string(),
    ];
    let started = Instant::now();
    let output = run_process(
        engine.executable_path(),
        Some(staging.directory()),
        &argv,
        timeout,
        &cancellation,
        action_cancel_flag.as_ref(),
        None,
    )?;
    let (error_code, error) = execution_error(&output);
    let mut result = OfficePresentationEditResult {
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: output.timed_out,
        cancelled: output.cancelled,
        duration_ms: started.elapsed().as_millis() as u64,
        error_code,
        error,
    };
    if result.error_code.is_none()
        && serde_json::from_str::<Value>(result.stdout.trim())
            .ok()
            .and_then(|value| value.get("success").and_then(Value::as_bool))
            != Some(true)
    {
        result.error_code = Some(
            OfficeEngineErrorCode::InvalidOutput
                .stable_name()
                .to_string(),
        );
        result.error = Some(
            "The pinned OfficeCLI batch did not return an explicit successful result.".to_string(),
        );
    }
    if result.error_code.is_some() {
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    if let Err(error) = validate_document_artifact(staging.path(), OfficeDocumentKind::Presentation)
    {
        attach_presentation_edit_error(&mut result, error);
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    let validation_started = Instant::now();
    let validation_output = run_process(
        engine.executable_path(),
        Some(staging.directory()),
        &["validate".to_string(), staged_name, "--json".to_string()],
        timeout,
        &cancellation,
        action_cancel_flag.as_ref(),
        None,
    )?;
    result.duration_ms = result
        .duration_ms
        .saturating_add(validation_started.elapsed().as_millis() as u64);
    if !validation_output.stdout.trim().is_empty() {
        if !result.stdout.is_empty() {
            result.stdout.push('\n');
        }
        result.stdout.push_str(&validation_output.stdout);
    }
    if !validation_output.stderr.trim().is_empty() {
        if !result.stderr.is_empty() {
            result.stderr.push('\n');
        }
        result.stderr.push_str(&validation_output.stderr);
    }
    let (validation_error_code, validation_error) = execution_error(&validation_output);
    let validation_semantically_succeeded =
        serde_json::from_str::<Value>(validation_output.stdout.trim())
            .ok()
            .and_then(|value| value.get("success").and_then(Value::as_bool))
            == Some(true);
    if validation_error_code.is_some() || !validation_semantically_succeeded {
        result.exit_code = validation_output.status.code();
        result.timed_out |= validation_output.timed_out;
        result.cancelled |= validation_output.cancelled;
        result.error_code = validation_error_code.or_else(|| {
            Some(
                OfficeEngineErrorCode::InvalidOutput
                    .stable_name()
                    .to_string(),
            )
        });
        result.error = validation_error.or_else(|| {
            Some("The edited presentation failed the pinned OfficeCLI validation gate.".to_string())
        });
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    let commit_lock = office_target_commit_lock(&target);
    let _commit_guard = commit_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let current_source = prepare_agent_file_input_bindings(
        resolved_context.workspace.as_deref(),
        resolved_context.permissions,
        &resolved_context.file_inputs,
        std::slice::from_ref(&source_spec),
        Some(&cancellation),
    )
    .map_err(office_input_execution_error)?;
    if current_source.as_slice() != std::slice::from_ref(&request.source_binding) {
        attach_presentation_edit_error(
            &mut result,
            precondition_error("Presentation edit source changed before publication."),
        );
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    let current_assets = prepare_agent_file_input_bindings(
        resolved_context.workspace.as_deref(),
        resolved_context.permissions,
        &resolved_context.file_inputs,
        &request.inputs,
        Some(&cancellation),
    )
    .map_err(office_input_execution_error)?;
    if current_assets != request.input_bindings {
        attach_presentation_edit_error(
            &mut result,
            precondition_error("A presentation edit asset changed before publication."),
        );
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    let current_destination = freeze_path(
        &resolved_context,
        OfficePathSlot::Destination,
        &request.destination_path,
        OfficePathPurpose::WriteTarget,
    )?;
    if current_destination != destination {
        attach_presentation_edit_error(
            &mut result,
            precondition_error("Presentation edit destination changed before publication."),
        );
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    if cancellation_requested(&cancellation, action_cancel_flag.as_ref()) {
        result.cancelled = true;
        result.error_code = Some("office.cancelled".to_string());
        result.error = Some(
            "Presentation edit was cancelled after validation and before atomic publication."
                .to_string(),
        );
        redact_presentation_edit_private_paths(
            &mut result,
            staging.directory(),
            &source_inputs,
            materialized_inputs.as_ref(),
        );
        return Ok(result);
    }
    if let Err(error) = staging.publish(&target, destination.state) {
        attach_presentation_edit_error(&mut result, error);
    }
    redact_presentation_edit_private_paths(
        &mut result,
        staging.directory(),
        &source_inputs,
        materialized_inputs.as_ref(),
    );
    Ok(result)
}

/// Strictly validates and atomically publishes the candidate produced by one real
/// provenance-bound Python/Node Office Skill script. The script process has already terminated;
/// this function is the only path from its private candidate to the approved destination.
pub(super) fn run_managed_script_output_commit(
    engine: Option<&OfficeCliEngine>,
    context: &OfficeExecutionContext,
    staging: &mut OfficeManagedScriptStaging,
    managed_python: Option<&ArtifactRuntimeInvocation>,
    cancellation: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
) -> Result<OfficeManagedScriptOutputResult, OfficeEngineError> {
    validate_platform()?;
    let started = Instant::now();
    if cancellation_requested(&cancellation, action_cancel_flag.as_ref()) {
        return Ok(OfficeManagedScriptOutputResult {
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled: true,
            duration_ms: 0,
            error_code: Some("office.cancelled".to_string()),
            error: Some(
                "Managed Office script output was cancelled before validation.".to_string(),
            ),
        });
    }
    if let Err(error) =
        validate_document_artifact(staging.staging.path(), staging.binding.document_kind)
    {
        return Ok(managed_script_output_error(started, error));
    }

    let timeout = Duration::from_millis(DEFAULT_OFFICE_TIMEOUT_MS);
    let (output, process_error_code, process_error, semantic_success) = match staging
        .binding
        .document_kind
    {
        OfficeDocumentKind::Spreadsheet => {
            let output = run_managed_openpyxl_reopen(
                managed_python.ok_or_else(|| {
                    OfficeEngineError::new(
                        OfficeEngineErrorCode::PreconditionFailed,
                        OfficeEngineRecovery::Retry,
                        "Managed spreadsheet publication requires the frozen Python runtime.",
                    )
                })?,
                staging.staging.path(),
                staging.staging.directory(),
                timeout,
                &cancellation,
                action_cancel_flag.as_ref(),
            )?;
            let (error_code, error) = managed_openpyxl_execution_error(&output);
            let success = error_code.is_none();
            (output, error_code, error, success)
        }
        OfficeDocumentKind::Document | OfficeDocumentKind::Presentation => {
            let engine = engine.ok_or_else(|| {
                OfficeEngineError::new(
                    OfficeEngineErrorCode::Unavailable,
                    OfficeEngineRecovery::InstallComponent,
                    "OfficeCLI is required to validate Word and PowerPoint output.",
                )
            })?;
            engine.verify_engine_revision()?;
            let candidate_name = staging
                .staging
                .path()
                .file_name()
                .ok_or_else(|| invalid_output("Managed Office script candidate has no file name."))?
                .to_string_lossy()
                .into_owned();
            let output = run_process(
                engine.executable_path(),
                Some(staging.staging.directory()),
                &["validate".to_string(), candidate_name, "--json".to_string()],
                timeout,
                &cancellation,
                action_cancel_flag.as_ref(),
                None,
            )?;
            let (error_code, error) = execution_error(&output);
            let success = strict_validation_succeeded(&output.stdout);
            (output, error_code, error, success)
        }
    };
    let mut result = OfficeManagedScriptOutputResult {
        exit_code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
        timed_out: output.timed_out,
        cancelled: output.cancelled,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        error_code: process_error_code,
        error: process_error,
    };
    if result.error_code.is_none() && !semantic_success {
        result.error_code = Some(
            OfficeEngineErrorCode::InvalidOutput
                .stable_name()
                .to_string(),
        );
        result.error = Some(
            "Managed Office script output failed the pinned OfficeCLI validation gate.".to_string(),
        );
    }
    if result.error_code.is_some() {
        redact_managed_script_paths(&mut result, staging);
        return Ok(result);
    }

    let resolved = ResolvedExecutionContext::resolve(context)?;
    let target = PathBuf::from(&staging.binding.destination.normalized_path);
    let commit_lock = office_target_commit_lock(&target);
    let _commit_guard = commit_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let current = freeze_path(
        &resolved,
        OfficePathSlot::Destination,
        &staging.binding.destination.logical_path,
        OfficePathPurpose::WriteTarget,
    )?;
    if current != staging.binding.destination {
        let error =
            precondition_error("Managed Office script destination changed before publication.");
        result.error_code = Some(error.code().stable_name().to_string());
        result.error = Some(error.message().to_string());
        redact_managed_script_paths(&mut result, staging);
        return Ok(result);
    }
    run_commit_test_hook(&target, CommitTestPhase::BeforeCancellationCheck);
    if cancellation_requested(&cancellation, action_cancel_flag.as_ref()) {
        result.cancelled = true;
        result.error_code = Some("office.cancelled".to_string());
        result.error = Some(
            "Managed Office script output was cancelled after validation and before atomic publication."
                .to_string(),
        );
        redact_managed_script_paths(&mut result, staging);
        return Ok(result);
    }
    run_commit_test_hook(&target, CommitTestPhase::AfterCancellationCheck);
    if let Err(error) = staging
        .staging
        .publish(&target, staging.binding.destination.state)
    {
        result.exit_code = result.exit_code.filter(|code| *code != 0).or(Some(1));
        result.error_code = Some(error.code().stable_name().to_string());
        result.error = Some(error.message().to_string());
    }
    redact_managed_script_paths(&mut result, staging);
    Ok(result)
}

const OPENPYXL_REOPEN_CHECK: &str = r#"import sys
from openpyxl import load_workbook
workbook = load_workbook(sys.argv[1], data_only=False, read_only=False)
if not workbook.sheetnames:
    raise ValueError("workbook has no worksheets")
workbook.close()
"#;

fn run_managed_openpyxl_reopen(
    invocation: &ArtifactRuntimeInvocation,
    candidate: &Path,
    cwd: &Path,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<ProcessOutput, OfficeEngineError> {
    if invocation.kind() != ArtifactRuntimeKind::Python {
        return Err(OfficeEngineError::new(
            OfficeEngineErrorCode::PreconditionFailed,
            OfficeEngineRecovery::Retry,
            "Managed spreadsheet publication received a non-Python runtime.",
        ));
    }
    let mut arguments = invocation
        .arguments_prefix()
        .iter()
        .map(|argument| {
            argument.to_str().map(str::to_string).ok_or_else(|| {
                OfficeEngineError::new(
                    OfficeEngineErrorCode::PreconditionFailed,
                    OfficeEngineRecovery::Retry,
                    "Managed Python runtime arguments must be UTF-8.",
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    arguments.extend([
        "-c".to_string(),
        OPENPYXL_REOPEN_CHECK.to_string(),
        candidate.to_string_lossy().into_owned(),
    ]);
    run_process_with_options(
        invocation.executable(),
        Some(cwd),
        &arguments,
        timeout,
        cancellation,
        action_cancel_flag,
        ProcessRunOptions {
            browser_policy: None,
            environment: Some(invocation.environment()),
            process_name: "managed openpyxl validation",
        },
    )
}

fn managed_openpyxl_execution_error(output: &ProcessOutput) -> (Option<String>, Option<String>) {
    if output.cancelled {
        return (
            Some("office.cancelled".to_string()),
            Some("Managed openpyxl validation was cancelled.".to_string()),
        );
    }
    if output.timed_out {
        return (
            Some("office.timeout".to_string()),
            Some("Managed openpyxl validation exceeded its timeout.".to_string()),
        );
    }
    match output.status.code() {
        Some(0) => (None, None),
        Some(_) => (
            Some(
                OfficeEngineErrorCode::InvalidOutput
                    .stable_name()
                    .to_string(),
            ),
            Some(
                "Spreadsheet output could not be reopened by the managed openpyxl runtime."
                    .to_string(),
            ),
        ),
        None => (
            Some("office.terminated_without_exit_code".to_string()),
            Some("Managed openpyxl validation terminated without an exit code.".to_string()),
        ),
    }
}

fn strict_validation_succeeded(stdout: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(stdout.trim()) else {
        return false;
    };
    if let Some(success) = value.get("success").and_then(Value::as_bool) {
        return success;
    }
    value.get("count").and_then(Value::as_u64) == Some(0)
}

fn managed_script_output_error(
    started: Instant,
    error: OfficeEngineError,
) -> OfficeManagedScriptOutputResult {
    OfficeManagedScriptOutputResult {
        exit_code: Some(1),
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        error_code: Some(error.code().stable_name().to_string()),
        error: Some(error.message().to_string()),
    }
}

fn redact_managed_script_paths(
    result: &mut OfficeManagedScriptOutputResult,
    staging: &OfficeManagedScriptStaging,
) {
    for private in [staging.staging.path(), staging.staging.directory()] {
        let spelling = private.to_string_lossy();
        result.stdout = result
            .stdout
            .replace(spelling.as_ref(), "<office-candidate>");
        result.stderr = result
            .stderr
            .replace(spelling.as_ref(), "<office-candidate>");
        if let Some(error) = result.error.as_mut() {
            *error = error.replace(spelling.as_ref(), "<office-candidate>");
        }
    }
}

fn presentation_edit_inputs_for_operation(
    parameters: &OfficeOperationParameters,
    declared: &[crate::AgentFileInputSpec],
    transaction_references: &mut std::collections::BTreeSet<String>,
) -> Result<Vec<crate::AgentFileInputSpec>, OfficeEngineError> {
    let encoded = serde_json::to_value(parameters).map_err(|error| {
        invalid_request(format!(
            "Cannot inspect presentation edit input references: {error}"
        ))
    })?;
    let mut placeholders = Vec::new();
    collect_presentation_edit_input_placeholders(&encoded, &mut placeholders);
    let mut inputs = Vec::new();
    for placeholder in placeholders {
        let mount_path = placeholder
            .strip_prefix(OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX)
            .expect("collector returns only Agent input placeholders");
        let input = declared
            .iter()
            .find(|input| input.mount_path == mount_path)
            .ok_or_else(|| {
                invalid_request(format!(
                    "Presentation edit resource `{placeholder}` has no declared input."
                ))
            })?;
        if inputs
            .iter()
            .any(|existing: &crate::AgentFileInputSpec| existing.mount_path == input.mount_path)
        {
            return Err(invalid_request(format!(
                "Presentation edit input `{mount_path}` is referenced more than once by one operation."
            )));
        }
        inputs.push(input.clone());
        transaction_references.insert(mount_path.to_string());
    }
    Ok(inputs)
}

fn collect_presentation_edit_input_placeholders(value: &Value, output: &mut Vec<String>) {
    match value {
        Value::String(value) if value.starts_with(OFFICE_AGENT_INPUT_PLACEHOLDER_PREFIX) => {
            output.push(value.clone());
        }
        Value::Array(values) => {
            for value in values {
                collect_presentation_edit_input_placeholders(value, output);
            }
        }
        Value::Object(values) => {
            for value in values.values() {
                collect_presentation_edit_input_placeholders(value, output);
            }
        }
        _ => {}
    }
}

fn cancelled_presentation_edit_result() -> OfficePresentationEditResult {
    OfficePresentationEditResult {
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: true,
        duration_ms: 0,
        error_code: Some("office.cancelled".to_string()),
        error: Some("Presentation edit was cancelled before execution.".to_string()),
    }
}

fn attach_presentation_edit_error(
    result: &mut OfficePresentationEditResult,
    error: OfficeEngineError,
) {
    result.error_code = Some(error.code().stable_name().to_string());
    result.error = Some(error.message().to_string());
}

fn write_private_batch_file(
    directory: &Path,
    batch: &[Value],
) -> Result<tempfile::NamedTempFile, OfficeEngineError> {
    let bytes = serde_json::to_vec(batch).map_err(|error| {
        invalid_request(format!("Cannot encode presentation edit plan: {error}"))
    })?;
    if bytes.len() > 512 * 1024 {
        return Err(invalid_request(
            "Compiled presentation edit plan exceeds the 512 KiB Host limit.",
        ));
    }
    let mut file = tempfile::Builder::new()
        .prefix("presentation-edit-plan-")
        .suffix(".json")
        .tempfile_in(directory)
        .map_err(|error| io_error("create private presentation edit plan", error))?;
    file.write_all(&bytes)
        .map_err(|error| io_error("write private presentation edit plan", error))?;
    file.as_file()
        .sync_all()
        .map_err(|error| io_error("sync private presentation edit plan", error))?;
    Ok(file)
}

fn redact_presentation_edit_private_paths(
    result: &mut OfficePresentationEditResult,
    staging_root: &Path,
    source_inputs: &PreparedAgentFileInputs,
    asset_inputs: Option<&PreparedAgentFileInputs>,
) {
    let mut paths = vec![staging_root, source_inputs.root()];
    if let Some(inputs) = asset_inputs {
        paths.push(inputs.root());
    }
    let mut spellings = Vec::new();
    for path in paths {
        spellings.push(path.to_string_lossy().into_owned());
        if let Ok(canonical) = path.canonicalize() {
            spellings.push(canonical.to_string_lossy().into_owned());
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
        spellings.extend(aliases);
    }
    spellings.sort_by_key(|path| std::cmp::Reverse(path.len()));
    spellings.dedup();
    for path in spellings {
        if path.is_empty() {
            continue;
        }
        result.stdout = result.stdout.replace(&path, "<presentation-edit-private>");
        result.stderr = result.stderr.replace(&path, "<presentation-edit-private>");
        if let Some(error) = result.error.as_mut() {
            *error = error.replace(&path, "<presentation-edit-private>");
        }
    }
}

fn batch_entry_from_canonical_argv(argv: &[String]) -> Result<Value, OfficeEngineError> {
    let command = argv
        .first()
        .map(String::as_str)
        .ok_or_else(|| precondition_error("Prepared Office mutation argv is empty."))?;
    if argv.len() < 3 || argv.last().map(String::as_str) != Some("--json") {
        return Err(precondition_error(
            "Prepared Office mutation argv is not canonical.",
        ));
    }
    let mut entry = serde_json::Map::new();
    entry.insert("command".to_string(), Value::String(command.to_string()));
    let positional = &argv[2..argv.len() - 1];
    let required_positional = match command {
        "set" | "add" | "remove" | "move" => 1,
        "swap" => 2,
        _ => {
            return Err(precondition_error(
                "Prepared presentation edit contains an unsupported operation.",
            ))
        }
    };
    let mut index = 0usize;
    let mut positionals = Vec::new();
    while index < positional.len() && !positional[index].starts_with('-') {
        positionals.push(positional[index].clone());
        index += 1;
    }
    if positionals.len() != required_positional {
        return Err(precondition_error(
            "Prepared presentation edit has an invalid positional contract.",
        ));
    }
    match command {
        "add" => {
            entry.insert("parent".to_string(), Value::String(positionals[0].clone()));
        }
        "swap" => {
            entry.insert("path".to_string(), Value::String(positionals[0].clone()));
            entry.insert("path2".to_string(), Value::String(positionals[1].clone()));
        }
        _ => {
            entry.insert("path".to_string(), Value::String(positionals[0].clone()));
        }
    }
    let mut properties = serde_json::Map::new();
    while index < positional.len() {
        let option = positional[index].as_str();
        if option == "--force" {
            entry.insert("force".to_string(), Value::Bool(true));
            index += 1;
            continue;
        }
        let value = positional.get(index + 1).ok_or_else(|| {
            precondition_error("Prepared presentation edit option is missing its value.")
        })?;
        if option == "--prop" {
            let (name, property_value) = value.split_once('=').ok_or_else(|| {
                precondition_error("Prepared presentation property is not canonical.")
            })?;
            properties.insert(name.to_string(), Value::String(property_value.to_string()));
        } else if matches!(option, "--find" | "--replace") {
            properties.insert(
                option.trim_start_matches('-').to_string(),
                Value::String(value.clone()),
            );
        } else if option == "--index" {
            let index = value.parse::<u32>().map_err(|_| {
                precondition_error(
                    "Prepared presentation edit index is not a canonical non-negative integer.",
                )
            })?;
            entry.insert("index".to_string(), Value::Number(index.into()));
        } else {
            let key = match option {
                "--type" => "type",
                "--from" => "from",
                "--after" => "after",
                "--before" => "before",
                "--to" => "to",
                "--shift" => "shift",
                _ => {
                    return Err(precondition_error(
                        "Prepared presentation edit contains an unsupported option.",
                    ))
                }
            };
            entry.insert(key.to_string(), Value::String(value.clone()));
        }
        index += 2;
    }
    if !properties.is_empty() {
        entry.insert("props".to_string(), Value::Object(properties));
    }
    Ok(Value::Object(entry))
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
    if request_requires_word_pdf_runtime(&prepared.request) {
        return execute_word_pdf_render_transaction(
            engine,
            context,
            prepared,
            timeout,
            cancellation,
            action_cancel_flag,
        );
    }
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
    let staged_output =
        match prepare_published_render_output(context, prepared, staging.path(), None) {
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
        Ok(()) => match prepare_published_render_output(context, prepared, &target, None) {
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

/// Converts one frozen DOCX snapshot to PDF with the application-managed LibreOffice runtime.
///
/// The model cannot supply converter argv or runtime paths. The source, profile, HOME/TMP,
/// conversion output, and publication candidate are all Host-private. Only a parsed,
/// non-encrypted, non-empty PDF is atomically published to the separately frozen output target.
fn execute_word_pdf_render_transaction(
    engine: &OfficeCliEngine,
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    timeout: Duration,
    cancellation: &AgentCancellationToken,
    action_cancel_flag: Option<&Arc<AtomicBool>>,
) -> Result<OfficeExecutionResult, OfficeEngineError> {
    let document = frozen_path(prepared, &OfficePathSlot::Document)
        .ok_or_else(|| precondition_error("Word PDF render input precondition is missing."))?;
    let output_precondition = frozen_path(prepared, &OfficePathSlot::Output)
        .ok_or_else(|| precondition_error("Word PDF render output precondition is missing."))?;
    let runtime = engine.word_pdf_render_runtime()?;

    let private = tempfile::Builder::new()
        .prefix("mycopilot-word-pdf-render-")
        .tempdir()
        .map_err(|error| io_error("create private Word PDF render directory", error))?;
    let source_snapshot = private.path().join("document.docx");
    copy_file_snapshot(Path::new(&document.normalized_path), &source_snapshot)?;
    let (revision, size) = file_revision(&source_snapshot)?;
    if document.content_revision.as_deref() != Some(&revision) || document.size != Some(size) {
        return Err(precondition_error(
            "Word input changed while the private PDF render snapshot was created.",
        ));
    }

    let profile = private.path().join("profile");
    let converted = private.path().join("converted");
    fs::create_dir(&profile)
        .map_err(|error| io_error("create private Word PDF renderer profile", error))?;
    fs::create_dir(&converted)
        .map_err(|error| io_error("create private Word PDF conversion directory", error))?;
    let profile_uri = private_directory_file_uri(&profile)?;
    let generated_pdf = converted.join("document.pdf");
    let actual_argv = vec![
        format!("-env:UserInstallation={profile_uri}"),
        "--headless".to_string(),
        "--nologo".to_string(),
        "--nodefault".to_string(),
        "--nolockcheck".to_string(),
        "--norestore".to_string(),
        "--convert-to".to_string(),
        "pdf:writer_pdf_Export".to_string(),
        "--outdir".to_string(),
        converted.to_string_lossy().into_owned(),
        source_snapshot.to_string_lossy().into_owned(),
    ];
    let target = PathBuf::from(&output_precondition.normalized_path);
    let mut staging = StagingArea::new(&target)?;
    let renderer_revision = runtime.runtime_revision().to_string();
    let started = Instant::now();
    let mut output = run_process_with_options(
        runtime.executable_path(),
        Some(private.path()),
        &actual_argv,
        timeout,
        cancellation,
        action_cancel_flag,
        ProcessRunOptions {
            browser_policy: None,
            environment: None,
            process_name: "managed Word PDF renderer",
        },
    )?;
    redact_private_render_paths(
        &mut output,
        [
            (runtime.component_root(), "<word-pdf-runtime>"),
            (source_snapshot.as_path(), "<frozen-word-document>"),
            (generated_pdf.as_path(), "<word-pdf-candidate>"),
            (profile.as_path(), "<word-pdf-profile>"),
            (converted.as_path(), "<word-pdf-output-directory>"),
            (private.path(), "<word-pdf-private>"),
        ],
    );
    let mut result = process_result_for_process(
        engine,
        prepared,
        output,
        started,
        "Managed Word PDF renderer",
    );
    if result.error_code.is_some() {
        return Ok(result);
    }
    if let Err(error) = copy_file_snapshot(&generated_pdf, staging.path()) {
        attach_post_process_error(
            &mut result,
            redact_private_office_error(error, private.path(), "<word-pdf-private>"),
        );
        return Ok(result);
    }
    if let Err(error) = validate_pdf_render_artifact(staging.path()) {
        attach_post_process_error(&mut result, error);
        return Ok(result);
    }
    let staged_output = match prepare_published_render_output(
        context,
        prepared,
        staging.path(),
        Some(&renderer_revision),
    ) {
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
    runtime.verify_integrity()?;
    run_commit_test_hook(&target, CommitTestPhase::BeforeCancellationCheck);
    if cancellation_requested(cancellation, action_cancel_flag) {
        mark_cancelled_before_commit(&mut result);
        return Ok(result);
    }
    run_commit_test_hook(&target, CommitTestPhase::AfterCancellationCheck);
    match staging.publish(&target, output_precondition.state) {
        Ok(()) => match prepare_published_render_output(
            context,
            prepared,
            &target,
            Some(&renderer_revision),
        ) {
            Ok(published_output) if published_output == staged_output => {
                result.outputs.push(published_output);
            }
            Ok(_) => attach_post_process_error(
                &mut result,
                published_output_verification_error(
                    "The final Word PDF bytes or verification receipt changed during atomic publication.",
                ),
            ),
            Err(error) => attach_post_process_error(
                &mut result,
                published_output_verification_error(format!(
                    "The final Word PDF identity could not be verified: {}",
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

fn redact_private_office_error(
    error: OfficeEngineError,
    private_root: &Path,
    replacement: &str,
) -> OfficeEngineError {
    let mut message = error
        .message()
        .replace(private_root.to_string_lossy().as_ref(), replacement);
    #[cfg(target_vendor = "apple")]
    {
        let spelling = private_root.to_string_lossy();
        let alias = if spelling.starts_with("/var/") || spelling.starts_with("/tmp/") {
            Some(format!("/private{spelling}"))
        } else {
            spelling
                .strip_prefix("/private")
                .filter(|path| path.starts_with("/var/") || path.starts_with("/tmp/"))
                .map(str::to_string)
        };
        if let Some(alias) = alias {
            message = message.replace(&alias, replacement);
        }
    }
    OfficeEngineError::new(error.code(), error.recovery(), message)
}

fn prepare_published_render_output(
    context: &ResolvedExecutionContext,
    prepared: &OfficePreparedExecution,
    staged_path: &Path,
    renderer_revision: Option<&str>,
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
    let (kind, mime_type, dimensions, page_count) = match mode {
        Some("screenshot") => (
            OfficePublishedOutputKind::Image,
            "image/png",
            Some(png_dimensions(staged_path)?),
            None,
        ),
        Some("svg") => (
            OfficePublishedOutputKind::Image,
            "image/svg+xml",
            None,
            None,
        ),
        Some("html") => (OfficePublishedOutputKind::Document, "text/html", None, None),
        Some("pdf") => (
            OfficePublishedOutputKind::Document,
            "application/pdf",
            None,
            Some(validate_pdf_render_artifact(staged_path)?),
        ),
        _ => {
            return Err(precondition_error(
                "Office render output has an unsupported managed render mode.",
            ))
        }
    };
    let (width, height) =
        dimensions.map_or((None, None), |(width, height)| (Some(width), Some(height)));
    let source_sha256 = if mode == Some("pdf") {
        let document = frozen_path(prepared, &OfficePathSlot::Document)
            .ok_or_else(|| precondition_error("Word PDF source identity is missing."))?;
        Some(
            document
                .content_revision
                .as_deref()
                .and_then(|revision| revision.strip_prefix(FILE_REVISION_PREFIX))
                .ok_or_else(|| precondition_error("Word PDF source identity is invalid."))?
                .to_string(),
        )
    } else {
        None
    };
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
        page_count,
        source_sha256,
        renderer_revision: renderer_revision.map(str::to_string),
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

fn validate_pdf_render_artifact(path: &Path) -> Result<u32, OfficeEngineError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| invalid_output("Managed Word PDF renderer did not produce a PDF file."))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OFFICE_DOCUMENT_BYTES
    {
        return Err(invalid_output(format!(
            "Managed Word PDF output must be a non-empty regular file no larger than {MAX_OFFICE_DOCUMENT_BYTES} bytes."
        )));
    }
    let mut file = fs::File::open(path).map_err(|_| {
        invalid_output("Managed Word PDF output cannot be opened for verification.")
    })?;
    let mut magic = [0_u8; 5];
    file.read_exact(&mut magic)
        .map_err(|_| invalid_output("Managed Word PDF output has a truncated header."))?;
    if magic != *b"%PDF-" {
        return Err(invalid_output(
            "Managed Word PDF output does not have a PDF header.",
        ));
    }
    let pdf = lopdf::Document::load(path)
        .map_err(|_| invalid_output("Managed Word PDF output is not a parsable PDF document."))?;
    if pdf.is_encrypted() || pdf.was_encrypted() || pdf.trailer.has(b"Encrypt") {
        return Err(invalid_output(
            "Managed Word PDF output must not be encrypted.",
        ));
    }
    let page_count = u32::try_from(pdf.get_pages().len())
        .map_err(|_| invalid_output("Managed Word PDF page count exceeds the Host limit."))?;
    if page_count == 0 || page_count > MAX_OFFICE_PAGE_NUMBER {
        return Err(invalid_output(format!(
            "Managed Word PDF output must contain between 1 and {MAX_OFFICE_PAGE_NUMBER} physical pages."
        )));
    }
    Ok(page_count)
}

fn private_directory_file_uri(path: &Path) -> Result<String, OfficeEngineError> {
    let absolute = path.canonicalize().map_err(|error| {
        io_error(
            "resolve the private Word PDF renderer profile directory",
            error,
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let encoded = percent_encode_file_uri_path(absolute.as_os_str().as_bytes(), false);
        if !encoded.starts_with('/') {
            return Err(precondition_error(
                "Private Word PDF renderer profile path is not absolute.",
            ));
        }
        return Ok(format!("file://{encoded}"));
    }
    #[cfg(windows)]
    {
        return windows_absolute_path_file_uri(&absolute.to_string_lossy());
    }
    #[allow(unreachable_code)]
    Err(precondition_error(
        "Managed Word PDF renderer profile URI is unsupported on this platform.",
    ))
}

fn percent_encode_file_uri_path(bytes: &[u8], allow_colon: bool) -> String {
    let mut encoded = String::with_capacity(bytes.len() + 8);
    for &byte in bytes {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else if allow_colon && byte == b':' {
            encoded.push(':');
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(any(windows, test))]
fn windows_absolute_path_file_uri(path: &str) -> Result<String, OfficeEngineError> {
    let normalized = path.replace('\\', "/");
    let bytes = normalized.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'/' {
        return Err(precondition_error(
            "Private Word PDF renderer profile must be an absolute drive path.",
        ));
    }
    Ok(format!(
        "file:///{}",
        percent_encode_file_uri_path(bytes, true)
    ))
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
    process_result_for_process(engine, prepared, output, started, "OfficeCLI")
}

fn process_result_for_process(
    engine: &OfficeCliEngine,
    prepared: &OfficePreparedExecution,
    output: ProcessOutput,
    started: Instant,
    process_name: &str,
) -> OfficeExecutionResult {
    let (error_code, error) = execution_error_for_process(&output, process_name);
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
        "Office operation was cancelled after validation and before the atomic commit.".to_string(),
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
        if !matches!(mode, Some("html" | "screenshot" | "svg" | "pdf")) {
            return Err(invalid_request(
                "Office view output paths are supported only for html, screenshot, svg, or managed Word PDF modes.",
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
            Some("html" | "screenshot" | "svg" | "pdf")
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
            "Managed Office process execution currently requires Unix process-group isolation; Windows remains disabled until Job Object containment is available.",
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

#[cfg(test)]
mod word_pdf_uri_tests {
    use super::*;
    use lopdf::{dictionary, Document, Object};

    fn write_pdf(path: &Path, page_count: u32) {
        let mut document = Document::with_version("1.7");
        let pages_id = document.new_object_id();
        let page_ids = (0..page_count)
            .map(|_| {
                document.add_object(dictionary! {
                    "Type" => "Page",
                    "Parent" => pages_id,
                    "MediaBox" => vec![0.into(), 0.into(), 612.into(), 792.into()],
                })
            })
            .collect::<Vec<_>>();
        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => page_ids.iter().copied().map(Object::Reference).collect::<Vec<_>>(),
                "Count" => i64::from(page_count),
            }),
        );
        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);
        document.save(path).unwrap();
    }

    #[test]
    fn windows_private_profile_uses_a_canonical_file_uri() {
        assert_eq!(
            windows_absolute_path_file_uri(r"C:\Program Data\Word QA\profile").unwrap(),
            "file:///C:/Program%20Data/Word%20QA/profile"
        );
        assert!(windows_absolute_path_file_uri(r"relative\profile").is_err());
    }

    #[test]
    fn pdf_validation_returns_the_physical_page_count_and_rejects_invalid_outputs() {
        let directory = tempfile::tempdir().unwrap();
        let valid = directory.path().join("valid.pdf");
        write_pdf(&valid, 3);
        assert_eq!(validate_pdf_render_artifact(&valid).unwrap(), 3);

        let empty = directory.path().join("empty.pdf");
        write_pdf(&empty, 0);
        assert_eq!(
            validate_pdf_render_artifact(&empty).unwrap_err().code(),
            OfficeEngineErrorCode::InvalidOutput
        );

        let invalid = directory.path().join("invalid.pdf");
        fs::write(&invalid, b"not a pdf").unwrap();
        assert_eq!(
            validate_pdf_render_artifact(&invalid).unwrap_err().code(),
            OfficeEngineErrorCode::InvalidOutput
        );
    }
}
