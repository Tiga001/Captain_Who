use super::*;
use crate::artifact_runtime::{
    ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery,
};
use crate::office::{
    OfficeEngine, OfficeExecutionContext, OfficePresentationEditRequest,
    OfficePresentationEditResult,
};
use crate::AgentCommandRuntimeProfile;
use crate::{AgentFileInputRef, AgentFileInputSpec};
use std::ffi::{OsStr, OsString};

const PRESENTATION_EDITOR_PLAN_TIMEOUT_MS: u64 = 30_000;
const PRESENTATION_EDITOR_OFFICE_TIMEOUT_MS: u64 = 120_000;

#[cfg(target_os = "macos")]
const MANAGED_PDF_SANDBOX_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[cfg(target_os = "macos")]
const MANAGED_PDF_SHELL_EXECUTABLE: &str = "/bin/bash";

#[cfg(target_os = "macos")]
const MANAGED_PDF_SANDBOX_PROFILE: &str = r#"(version 1)
(deny default)
(import "dyld-support.sb")
(allow sysctl-read)
(allow system-info)
(deny syscall-unix (syscall-number SYS_setsid))
(deny syscall-unix (syscall-number SYS_setpgid))
(deny network*)
(deny process-fork)
(deny process-exec)
(deny file-link file-clone)
(deny file-write-create (vnode-type SYMLINK))
(allow file-read-metadata file-test-existence
  (literal "/dev")
  (literal "/dev/null")
  (literal "/dev/urandom"))
(allow file-read-data
  (literal "/dev/null")
  (literal "/dev/urandom"))
(allow file-write-data (literal "/dev/null"))
(with-filter (process-path (param "SANDBOX_EXEC"))
  (allow process-exec (literal (param "SHELL"))))
(with-filter (process-path (param "SHELL"))
  (allow process-fork)
  (allow process-exec
    (literal (param "PYTHON"))
    (literal (param "RIPGREP"))))
(allow file-read-metadata file-test-existence
  (literal (param "SHELL"))
  (literal (param "SANDBOX_EXEC"))
  (literal (param "RUNTIME_ROOT"))
  (literal (param "INPUT_ROOT"))
  (literal (param "EXECUTION_ROOT"))
  (path-ancestors (param "RUNTIME_ROOT"))
  (path-ancestors (param "INPUT_ROOT"))
  (path-ancestors (param "EXECUTION_ROOT")))
(allow file-read* file-test-existence
  (literal (param "SHELL"))
  (literal (param "SANDBOX_EXEC"))
  (literal (param "RUNTIME_ROOT"))
  (literal (param "INPUT_ROOT"))
  (literal (param "EXECUTION_ROOT"))
  (subpath (param "RUNTIME_ROOT"))
  (subpath (param "INPUT_ROOT"))
  (subpath (param "EXECUTION_ROOT")))
(allow file-map-executable (subpath (param "RUNTIME_ROOT")))
(allow file-write* (subpath (param "EXECUTION_ROOT")))"#;

pub(crate) enum ManagedCommandSessionPreparation {
    Immediate(Box<AgentCommandExecutionResult>),
    Ready {
        plan: CommandSpawnPlan,
        completion_hook: super::session::CommandSessionCompletionHook,
    },
}

pub(crate) struct ManagedCommandSessionServices<'a> {
    pub(crate) artifact_runtime: Option<Arc<ArtifactRuntimeProvider>>,
    pub(crate) office_engine: Option<Arc<dyn OfficeEngine>>,
    pub(crate) file_inputs: Option<&'a AgentFileInputExecutionContext>,
    pub(crate) managed_workspace: Option<ManagedCommandWorkspaceLease>,
}

/// Freezes a managed-runtime launch into the same process-session kernel used by ordinary shell
/// commands. Runtime paths, the private input directory, integrity verification and artifact
/// observation all remain host-owned and live until terminal settlement.
pub(crate) fn prepare_managed_command_session(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    services: ManagedCommandSessionServices<'_>,
) -> Result<ManagedCommandSessionPreparation, CommandExecutionError> {
    let ManagedCommandSessionServices {
        artifact_runtime,
        office_engine,
        file_inputs,
        managed_workspace,
    } = services;
    let managed_pdf = request
        .runtime_binding
        .as_deref()
        .is_some_and(|binding| binding.profile == AgentCommandRuntimeProfile::Pdf);
    let managed_workspace = match (managed_pdf, managed_workspace) {
        (true, Some(workspace)) => Some(workspace),
        (true, None) => {
            return Err(CommandExecutionError::from(
                "可信 PDF 命令缺少 Host 提供的 Run 私有工作区。".to_string(),
            ));
        }
        (false, Some(_)) => {
            return Err(CommandExecutionError::from(
                "非 PDF 命令不能接收 PDF Run 私有工作区。".to_string(),
            ));
        }
        (false, None) => None,
    };
    let input_workspace_root = workspace_root
        .map(canonicalize_workspace_root)
        .transpose()?;
    let root = if let Some(workspace) = managed_workspace.as_ref() {
        Some(canonicalize_workspace_root(workspace.execution_root())?)
    } else {
        input_workspace_root.clone()
    };
    let cwd = resolve_command_cwd(root.as_deref(), request.cwd.as_deref(), permissions.write)?;
    enforce_command_policy(
        &request.command,
        permissions,
        authorization_source,
        root.as_deref(),
        Some(&cwd),
    )?;
    let managed_builder = infer_managed_artifact_builder_command(&request.command)
        .map_err(CommandExecutionError::from)?;
    if let Some(builder) = managed_builder.as_ref() {
        validate_managed_artifact_builder_output_scope(
            root.as_deref(),
            &cwd,
            &builder.output_paths,
            permissions.write,
        )
        .map_err(CommandExecutionError::from)?;
    }

    let observer = CommandArtifactObserver::prepare(
        root.as_deref(),
        &cwd,
        request.observe.as_ref(),
        permissions,
    );
    let before = observer.as_ref().map(|observer| {
        observer.capture(
            AgentCommandArtifactObservationPhase::Before,
            Some(&cancellation_token),
        )
    });
    let immediate = |mut result: AgentCommandExecutionResult| {
        if let Some((observer, before)) = observer.as_ref().zip(before.as_ref()) {
            let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
            result.artifact_observation = Some(observer.finish(before.clone(), after));
        }
        ManagedCommandSessionPreparation::Immediate(Box::new(result))
    };

    let Some(binding) = request.runtime_binding.as_deref() else {
        return Err(CommandExecutionError::from(
            "ordinary command passed to managed runtime preparation".to_string(),
        ));
    };

    if let Err(error) = super::validate_command_runtime_binding(binding) {
        return Ok(immediate(runtime_failure_result(
            root.as_deref(),
            &cwd,
            request,
            binding_resolution_error(binding, error.code(), error.recovery(), error.message()),
            0,
        )));
    }
    if cancellation_token.is_cancelled() {
        return Ok(immediate(runtime_cancelled_result(
            root.as_deref(),
            &cwd,
            request,
            unresolved_binding_resolution(binding),
        )));
    }
    let parsed = match parse_frozen_managed_command(&request.command, binding) {
        Ok(parsed) => parsed,
        Err(message) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            )))
        }
    };
    if let Some(script) = parsed.script.as_deref() {
        if let Err(message) = validate_saved_script(&cwd, root.as_deref(), permissions.read, script)
        {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            )));
        }
    }
    let Some(provider) = artifact_runtime else {
        return Ok(immediate(runtime_failure_result(
            root.as_deref(),
            &cwd,
            request,
            binding_resolution_error(
                binding,
                ERROR_UNAVAILABLE,
                ArtifactRuntimeRecovery::InstallComponent.stable_name(),
                "Managed Artifact Runtime 尚未安装或未由 host 配置。",
            ),
            0,
        )));
    };
    let prepared_runtime = match super::prepare_command_runtime_profile(
        provider.as_ref(),
        binding.profile,
        binding.kind,
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(binding, error.code(), error.recovery(), error.message()),
                0,
            )))
        }
    };
    if prepared_runtime.binding != *binding {
        return Ok(immediate(runtime_failure_result(
            root.as_deref(),
            &cwd,
            request,
            binding_resolution_error(
                binding,
                super::COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
                "reprepare",
                "Managed Artifact Runtime 在审批后发生变化；命令未启动，请重新准备并审批。",
            ),
            0,
        )));
    }

    let resolution = ready_binding_resolution(binding, &prepared_runtime.invocation);
    let editor_contract = match presentation_editor_contract(
        input_workspace_root.as_deref(),
        &cwd,
        request,
        binding,
        &parsed,
        managed_builder.as_ref(),
    ) {
        Ok(contract) => contract,
        Err(message) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            )))
        }
    };
    let empty_input_context = AgentFileInputExecutionContext::default();
    let prepared_inputs = match materialize_agent_file_inputs(
        input_workspace_root.as_deref(),
        permissions,
        file_inputs.unwrap_or(&empty_input_context),
        &request.inputs,
        Some(&cancellation_token),
    ) {
        Ok(inputs) => inputs,
        Err(error) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(binding, error.code(), error.recovery(), error.message()),
                0,
            )))
        }
    };
    let input_evidence = prepared_inputs
        .as_ref()
        .map(|inputs| inputs.evidence().to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|input| input.mount_path != super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .collect::<Vec<_>>();
    let editor_execution = match editor_contract
        .map(|contract| {
            prepare_presentation_editor_execution(
                contract,
                prepared_inputs.as_ref(),
                provider.component_root(),
            )
        })
        .transpose()
    {
        Ok(editor) => editor,
        Err(message) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::Retry.stable_name(),
                    &message,
                ),
                0,
            )))
        }
    };
    let editor_office_context = editor_execution.as_ref().map(|_| {
        let file_inputs = file_inputs.cloned().unwrap_or_default();
        OfficeExecutionContext::new(
            input_workspace_root.clone(),
            permissions,
            file_inputs.attachment_library().cloned(),
        )
        .with_file_inputs(file_inputs)
    });
    let mut arguments = prepared_runtime.invocation.arguments_prefix().to_vec();
    let mut environment =
        managed_environment(&prepared_runtime.invocation, prepared_inputs.as_ref());
    if let Some(editor) = editor_execution.as_ref() {
        arguments = canonical_editor_arguments_prefix(&prepared_runtime.invocation)?;
        arguments.push(OsString::from("--experimental-permission"));
        arguments.push(OsString::from(format!(
            "--allow-fs-read={}",
            editor.runtime_root.display()
        )));
        arguments.push(OsString::from(format!(
            "--allow-fs-read={}",
            editor.frozen_script_path.display()
        )));
        arguments.push(OsString::from(format!(
            "--allow-fs-write={}",
            editor
                .plan_path
                .parent()
                .expect("private plan path has a parent")
                .display()
        )));
        arguments.push(OsString::from("--disallow-code-generation-from-strings"));
        arguments.push(OsString::from("--no-experimental-fetch"));
        arguments.push(OsString::from("--max-old-space-size=256"));
        arguments.push(editor.frozen_script_path.clone().into_os_string());
        arguments.extend(parsed.process_arguments.iter().skip(1).cloned());
        environment = presentation_editor_environment(&prepared_runtime.invocation, editor);
        environment.push((
            OsString::from("MYCOPILOT_PRESENTATION_EDIT_MODE"),
            OsString::from("v1"),
        ));
        environment.push((
            OsString::from("MYCOPILOT_PRESENTATION_EDITOR_ENTRY"),
            editor.frozen_script_path.clone().into_os_string(),
        ));
        environment.push((
            OsString::from(super::PRESENTATION_EDITOR_PLAN_ENV),
            editor.plan_path.clone().into_os_string(),
        ));
    } else {
        arguments.extend(parsed.process_arguments.iter().cloned());
    }
    let requires_automatic_node_syntax_check = should_automatically_check_node_builder(
        binding.profile,
        binding.kind,
        &parsed,
        managed_builder.as_ref(),
    );
    if requires_automatic_node_syntax_check {
        let frozen_editor_script = editor_execution
            .as_ref()
            .map(|editor| editor.frozen_script_path.to_string_lossy().into_owned());
        let script = frozen_editor_script.as_deref().unwrap_or_else(|| {
            parsed
                .script
                .as_deref()
                .expect("managed Node Builder carries a saved script")
        });
        let redactions = managed_node_syntax_check_redactions(
            provider.component_root(),
            &prepared_runtime.invocation,
            &cwd,
            script,
        );
        let syntax_check = run_managed_node_syntax_check(
            &prepared_runtime.invocation,
            script,
            &cwd,
            &environment,
            &cancellation_token,
            redactions,
        );
        if !syntax_check.succeeded() {
            let mut result = managed_node_syntax_check_failure_result(
                root.as_deref(),
                &cwd,
                request,
                resolution.clone(),
                syntax_check,
            );
            result.input_files = input_evidence;
            return Ok(immediate(result));
        }
        if let Err(error) = provider.verify_integrity() {
            let code = format!("artifactRuntime.{}", error.code().stable_name());
            let runtime = with_resolution_error(
                resolution.clone(),
                &code,
                error.recovery().stable_name(),
                provider_error_message(error.code()),
            );
            let mut result = runtime_failure_result(root.as_deref(), &cwd, request, runtime, 0);
            result.input_files = input_evidence;
            return Ok(immediate(result));
        }
    }
    let hard_timeout =
        managed_command_hard_timeout(managed_pdf, editor_execution.is_some(), request.timeout_ms);
    let launch = if let Some(workspace) = managed_workspace.as_ref() {
        let pdf_cli = match provider.pdf_cli_path() {
            Ok(path) => path,
            Err(error) => {
                return Ok(immediate(runtime_failure_result(
                    root.as_deref(),
                    &cwd,
                    request,
                    binding_resolution_error(
                        binding,
                        error.code().stable_name(),
                        error.recovery().stable_name(),
                        error.message(),
                    ),
                    0,
                )))
            }
        };
        let ripgrep = match provider.ripgrep_executable() {
            Ok(path) => path,
            Err(error) => {
                return Ok(immediate(runtime_failure_result(
                    root.as_deref(),
                    &cwd,
                    request,
                    binding_resolution_error(
                        binding,
                        error.code().stable_name(),
                        error.recovery().stable_name(),
                        error.message(),
                    ),
                    0,
                )))
            }
        };
        match prepare_managed_pdf_shell_launch(
            &prepared_runtime.invocation,
            parsed
                .pdf_shell
                .as_ref()
                .expect("managed PDF workspace must have an authoritative shell plan"),
            &mut environment,
            workspace,
            prepared_inputs.as_ref(),
            ManagedPdfToolPaths {
                runtime_root: provider.component_root(),
                pdf_cli: &pdf_cli,
                ripgrep: &ripgrep,
            },
        ) {
            Ok(launch) => launch,
            Err(error) => {
                let (code, recovery, message) = match error {
                    ManagedPdfShellLaunchError::InvalidCommand(message) => (
                        ERROR_INVALID_COMMAND,
                        ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                        message,
                    ),
                    ManagedPdfShellLaunchError::Unavailable(message) => (
                        ERROR_UNAVAILABLE,
                        ArtifactRuntimeRecovery::InstallComponent.stable_name(),
                        message,
                    ),
                };
                return Ok(immediate(runtime_failure_result(
                    root.as_deref(),
                    &cwd,
                    request,
                    binding_resolution_error(binding, code, recovery, &message),
                    0,
                )));
            }
        }
    } else {
        CommandDirectLaunchPlan::isolated(
            prepared_runtime.invocation.executable().to_path_buf(),
            arguments,
            environment,
        )
    };
    let managed_output_capture = if let Some(workspace) = managed_workspace.as_ref() {
        let baseline = super::snapshot_managed_command_output_baseline(workspace.outputs_root())
            .map_err(CommandExecutionError::from)?;
        Some(workspace.output_capture(baseline))
    } else {
        None
    };
    let output_redactions = if let Some(workspace) = managed_workspace.as_ref() {
        managed_pdf_output_redactions(
            workspace.execution_root(),
            workspace.outputs_root(),
            prepared_inputs.as_ref(),
            provider.component_root(),
        )
    } else if let Some(editor) = editor_execution.as_ref() {
        presentation_editor_output_redactions(
            editor,
            prepared_inputs.as_ref(),
            &prepared_runtime.invocation,
            &cwd,
        )
    } else if parsed.node_syntax_check {
        managed_node_syntax_check_redactions(
            provider.component_root(),
            &prepared_runtime.invocation,
            &cwd,
            parsed
                .script
                .as_deref()
                .expect("managed Node syntax check carries a saved script"),
        )
    } else {
        super::output_capture::ProcessOutputRedactionSet::default()
    };
    let plan = CommandSpawnPlan::direct(
        request.command.clone(),
        cwd,
        root.as_deref(),
        hard_timeout,
        launch,
    )
    .with_output_redactions(output_redactions);
    let editor_cancellation = cancellation_token.clone();
    let has_editor = editor_execution.is_some();
    let completion_hook: super::session::CommandSessionCompletionHook = Box::new(
        move |result, session_cancel_flag| {
            result.runtime = Some(resolution.clone());
            result.input_files = input_evidence;
            result.managed_outputs = managed_output_capture.clone();
            let mut runtime_integrity_valid = true;
            if let Err(error) = provider.verify_integrity() {
                runtime_integrity_valid = false;
                let code = format!("artifactRuntime.{}", error.code().stable_name());
                let runtime = with_resolution_error(
                    result
                        .runtime
                        .take()
                        .expect("managed session completion has runtime evidence"),
                    &code,
                    error.recovery().stable_name(),
                    provider_error_message(error.code()),
                );
                result.error = runtime.message.clone();
                result.runtime = Some(runtime);
            }
            let mut editor_applied = !has_editor;
            if let Some(editor) = editor_execution {
                if runtime_integrity_valid
                    && result.exit_code == Some(0)
                    && !result.timed_out
                    && !result.cancelled
                    && result.error.is_none()
                {
                    if editor_cancellation.is_cancelled()
                        || session_cancel_flag.load(std::sync::atomic::Ordering::Acquire)
                    {
                        result.cancelled = true;
                        result.error = Some(
                            "Presentation edit was cancelled before the Host transaction began."
                                .to_string(),
                        );
                    } else {
                        match super::read_presentation_editor_plan(&editor.plan_path) {
                        Ok(plan)
                            if plan.source.mount_path() == editor.contract.source_mount_path
                                && plan.destination.path()
                                    == editor.contract.destination_path =>
                        {
                            match (office_engine.as_ref(), editor_office_context.as_ref()) {
                                (Some(engine), Some(context)) => {
                                    let request = OfficePresentationEditRequest {
                                        source_path: editor.contract.source_path.clone(),
                                        source_binding: editor.contract.source_binding.clone(),
                                        destination_path: editor.contract.destination_path.clone(),
                                        inputs: editor.contract.asset_specs.clone(),
                                        input_bindings: editor.contract.asset_bindings.clone(),
                                        operations: plan.operations,
                                        timeout_ms: Some(PRESENTATION_EDITOR_OFFICE_TIMEOUT_MS),
                                    };
                                    match engine.execute_presentation_edit(
                                        context,
                                        &request,
                                        editor_cancellation.clone(),
                                        Some(Arc::clone(&session_cancel_flag)),
                                    ) {
                                        Ok(office_result) => {
                                            editor_applied = settle_presentation_editor_office_result(
                                                result,
                                                office_result,
                                            );
                                        }
                                        Err(error) => {
                                            fail_presentation_editor_result(
                                                result,
                                                &format!(
                                                    "Presentation Editor Host transaction failed ({}): {}",
                                                    error.code().stable_name(),
                                                    error.message()
                                                ),
                                            );
                                        }
                                    }
                                }
                                _ => fail_presentation_editor_result(
                                    result,
                                    "Presentation Editor requires the Host Office engine, but it is unavailable.",
                                ),
                            }
                        }
                        Ok(_) => fail_presentation_editor_result(
                            result,
                            "Presentation Editor plan source or destination differs from the approved command.",
                        ),
                        Err(error) => fail_presentation_editor_result(result, &error),
                    }
                    }
                }
            }
            if editor_applied {
                if let Some((observer, before)) = observer.zip(before) {
                    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
                    result.artifact_observation = Some(observer.finish(before, after));
                }
            }
            drop(prepared_inputs);
        },
    );
    Ok(ManagedCommandSessionPreparation::Ready {
        plan,
        completion_hook,
    })
}

fn managed_command_hard_timeout(
    managed_pdf: bool,
    presentation_editor: bool,
    requested_timeout_ms: Option<u64>,
) -> Option<Duration> {
    if managed_pdf {
        Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS))
    } else if presentation_editor {
        Some(Duration::from_millis(
            requested_timeout_ms
                .unwrap_or(PRESENTATION_EDITOR_PLAN_TIMEOUT_MS)
                .clamp(1, PRESENTATION_EDITOR_PLAN_TIMEOUT_MS),
        ))
    } else {
        requested_timeout_ms.map(|timeout| Duration::from_millis(timeout.clamp(1, MAX_TIMEOUT_MS)))
    }
}

fn settle_presentation_editor_office_result(
    command: &mut AgentCommandExecutionResult,
    office: OfficePresentationEditResult,
) -> bool {
    let succeeded = office.exit_code == Some(0)
        && !office.timed_out
        && !office.cancelled
        && office.error_code.is_none()
        && office.error.is_none();
    if succeeded {
        command.exit_code = Some(0);
        return true;
    }
    command.exit_code = office.exit_code.or(Some(1));
    command.timed_out |= office.timed_out;
    command.cancelled |= office.cancelled;
    let preserve_missing_exit =
        (office.timed_out || office.cancelled) && office.exit_code.is_none();
    let message = presentation_editor_office_failure_message(&office);
    fail_presentation_editor_result(command, &message);
    if preserve_missing_exit {
        command.exit_code = None;
    }
    false
}

fn presentation_editor_office_failure_message(office: &OfficePresentationEditResult) -> String {
    const FIELD_LIMIT: usize = 1_024;
    const MESSAGE_LIMIT: usize = 4_096;

    let provider = stable_office_json_diagnostic(&office.stdout, FIELD_LIMIT).or_else(|| {
        bounded_diagnostic_text(&office.stderr, FIELD_LIMIT)
            .map(|diagnostic| format!("OfficeCLI stderr: {diagnostic}"))
    });
    let host_code = office
        .error_code
        .as_deref()
        .and_then(|value| bounded_diagnostic_text(value, FIELD_LIMIT));
    let host_error = office
        .error
        .as_deref()
        .and_then(|value| bounded_diagnostic_text(value, FIELD_LIMIT));

    let mut parts = Vec::new();
    push_unique_diagnostic(&mut parts, provider);
    push_unique_diagnostic(
        &mut parts,
        host_code.map(|code| format!("Host code: {code}")),
    );
    push_unique_diagnostic(
        &mut parts,
        host_error.map(|error| format!("Host error: {error}")),
    );
    if parts.is_empty() {
        parts.push("Presentation Editor Host transaction failed.".to_string());
    }
    parts.join(" | ").chars().take(MESSAGE_LIMIT).collect()
}

fn stable_office_json_diagnostic(stdout: &str, field_limit: usize) -> Option<String> {
    let mut fields = Vec::new();
    for value in serde_json::Deserializer::from_str(stdout)
        .into_iter::<serde_json::Value>()
        .take(8)
    {
        let Ok(value) = value else {
            break;
        };
        if value.get("success").and_then(serde_json::Value::as_bool) == Some(true) {
            continue;
        }
        for key in ["code", "error", "message"] {
            let Some(value) = find_stable_json_string(&value, key)
                .and_then(|value| bounded_diagnostic_text(value, field_limit))
            else {
                continue;
            };
            let field = format!("{key}={value}");
            if !fields.contains(&field) {
                fields.push(field);
            }
        }
    }
    (!fields.is_empty()).then(|| format!("OfficeCLI: {}", fields.join("; ")))
}

fn find_stable_json_string<'a>(value: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    match value {
        serde_json::Value::Object(values) => values
            .get(key)
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                values
                    .values()
                    .find_map(|value| find_stable_json_string(value, key))
            }),
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_stable_json_string(value, key)),
        _ => None,
    }
}

fn bounded_diagnostic_text(value: &str, limit: usize) -> Option<String> {
    let normalized = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|character| !character.is_control())
        .take(limit)
        .collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

fn push_unique_diagnostic(parts: &mut Vec<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if !parts
        .iter()
        .any(|part| part == &candidate || part.ends_with(&candidate) || candidate.ends_with(part))
    {
        parts.push(candidate);
    }
}

fn fail_presentation_editor_result(result: &mut AgentCommandExecutionResult, message: &str) {
    result.exit_code = result.exit_code.filter(|code| *code != 0).or(Some(1));
    result.error = Some(message.chars().take(4_096).collect());
}

fn should_automatically_check_node_builder(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
    parsed: &ManagedArtifactCommand,
    builder: Option<&ManagedArtifactBuilderCommand>,
) -> bool {
    profile == AgentCommandRuntimeProfile::Presentations
        && kind == AgentCommandRuntimeKind::Node
        && !parsed.node_syntax_check
        && builder.is_some_and(|builder| !builder.output_paths.is_empty())
}

#[derive(Debug, Clone)]
struct PresentationEditorContract {
    source_mount_path: String,
    source_path: String,
    source_binding: crate::AgentFileInputBinding,
    destination_path: String,
    asset_specs: Vec<AgentFileInputSpec>,
    asset_bindings: Vec<crate::AgentFileInputBinding>,
}

struct PreparedPresentationEditor {
    contract: PresentationEditorContract,
    _plan_directory: tempfile::TempDir,
    plan_path: PathBuf,
    frozen_script_path: PathBuf,
    runtime_root: PathBuf,
    private_home: PathBuf,
    private_tmp: PathBuf,
}

fn presentation_editor_contract(
    workspace_root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    binding: &crate::AgentCommandRuntimeBinding,
    parsed: &ManagedArtifactCommand,
    builder: Option<&ManagedArtifactBuilderCommand>,
) -> Result<Option<PresentationEditorContract>, String> {
    let editor_bindings = request
        .inputs
        .iter()
        .filter(|input| input.mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH)
        .collect::<Vec<_>>();
    if editor_bindings.is_empty() {
        return Ok(None);
    }
    if editor_bindings.len() != 1
        || binding.profile != AgentCommandRuntimeProfile::Presentations
        || binding.kind != AgentCommandRuntimeKind::Node
        || parsed.node_syntax_check
    {
        return Err(
            "Presentation Editor reserved script binding has an invalid runtime identity."
                .to_string(),
        );
    }
    let builder = builder.ok_or_else(|| {
        "Presentation Editor requires one direct, statically inspectable Builder command."
            .to_string()
    })?;
    if !is_presentation_editor_direct_command(&request.command) {
        return Err(
            "Presentation Editor accepts only `node <editor.mjs> --source <mount.pptx> --output <destination.pptx>` (flags may be swapped)."
                .to_string(),
        );
    }
    let workspace_root = workspace_root.ok_or_else(|| {
        "Presentation Editor requires the workspace which owns its materialized script.".to_string()
    })?;
    let script_binding = editor_bindings[0];
    let AgentFileInputRef::Workspace {
        path: frozen_script_source,
    } = &script_binding.source
    else {
        return Err("Presentation Editor script binding must be workspace-owned.".to_string());
    };
    let requested_script = Path::new(
        parsed
            .script
            .as_deref()
            .ok_or_else(|| "Presentation Editor command is missing its script.".to_string())?,
    );
    let requested_script = if requested_script.is_absolute() {
        requested_script.to_path_buf()
    } else {
        cwd.join(requested_script)
    };
    let frozen_source = workspace_root.join(frozen_script_source);
    if requested_script.canonicalize().ok() != frozen_source.canonicalize().ok() {
        return Err(
            "Presentation Editor command script does not match its frozen materialization input."
                .to_string(),
        );
    }

    let source_mount_path = builder.source_paths[0].clone();
    if source_mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
        || Path::new(&source_mount_path)
            .extension()
            .and_then(OsStr::to_str)
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("pptx"))
    {
        return Err("Presentation Editor --source must name one mounted .pptx input.".to_string());
    }
    let source_matches = request
        .inputs
        .iter()
        .filter(|input| input.mount_path == source_mount_path)
        .collect::<Vec<_>>();
    if source_matches.len() != 1 {
        return Err(
            "Presentation Editor --source must match exactly one frozen input binding.".to_string(),
        );
    }
    let source_binding = source_matches[0].clone();
    let source_path = presentation_editor_source_path(&source_binding.source)?;
    let destination_path = builder.output_paths[0].clone();
    if source_mount_path == destination_path {
        return Err(
            "Presentation Editor save-as destination must differ from its source mount."
                .to_string(),
        );
    }

    let mut asset_specs = Vec::new();
    let mut asset_bindings = Vec::new();
    for input in &request.inputs {
        if input.mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
            || input.mount_path == source_mount_path
        {
            continue;
        }
        asset_specs.push(AgentFileInputSpec {
            mount_path: input.mount_path.clone(),
            source: input.source.clone(),
        });
        asset_bindings.push(input.clone());
    }
    Ok(Some(PresentationEditorContract {
        source_mount_path,
        source_path,
        source_binding,
        destination_path,
        asset_specs,
        asset_bindings,
    }))
}

pub(crate) fn is_presentation_editor_direct_command(command: &str) -> bool {
    let Ok(Some(builder)) = infer_managed_artifact_builder_command(command) else {
        return false;
    };
    if builder.kind != AgentCommandRuntimeKind::Node
        || builder.node_syntax_check
        || builder.source_paths.len() != 1
        || builder.output_paths.len() != 1
    {
        return false;
    }
    let Ok(tokens) = managed_artifact_command_tokens(command) else {
        return false;
    };
    tokens.len() == 6
        && tokens[0] == "node"
        && tokens[1].ends_with(".mjs")
        && matches!(tokens[2].as_str(), "--source" | "--output")
        && matches!(tokens[4].as_str(), "--source" | "--output")
        && tokens[2] != tokens[4]
}

fn presentation_editor_source_path(source: &AgentFileInputRef) -> Result<String, String> {
    match source {
        AgentFileInputRef::Workspace { path } | AgentFileInputRef::External { path } => {
            Ok(path.clone())
        }
        AgentFileInputRef::Attachment { read_path } => Ok(read_path.clone()),
        AgentFileInputRef::GeneratedArtifact { path, .. } => Ok(path.clone()),
        AgentFileInputRef::SkillResource { .. } => Err(
            "Presentation Editor source must be a workspace, attachment, Artifact, or authorized external .pptx."
                .to_string(),
        ),
    }
}

fn prepare_presentation_editor_execution(
    contract: PresentationEditorContract,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
) -> Result<PreparedPresentationEditor, String> {
    let inputs = prepared_inputs.ok_or_else(|| {
        "Presentation Editor is missing its frozen private input snapshot.".to_string()
    })?;
    let frozen_script_path = inputs
        .root()
        .join(super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH);
    let metadata = fs::symlink_metadata(&frozen_script_path)
        .map_err(|error| format!("Cannot inspect frozen Presentation Editor script: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Frozen Presentation Editor script must be a regular file.".to_string());
    }
    let frozen_script_path = frozen_script_path.canonicalize().map_err(|error| {
        format!("Cannot canonicalize frozen Presentation Editor script: {error}")
    })?;
    super::validate_presentation_editor_script(&frozen_script_path)?;
    let runtime_root = runtime_root
        .canonicalize()
        .map_err(|error| format!("Cannot canonicalize Managed Runtime root: {error}"))?;
    let plan_directory = tempfile::Builder::new()
        .prefix("mycopilot-presentation-editor-")
        .tempdir()
        .map_err(|error| {
            format!("Cannot create private Presentation Editor plan directory: {error}")
        })?;
    let plan_root = plan_directory.path().canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor plan directory: {error}")
    })?;
    let private_home = plan_root.join("home");
    let private_tmp = plan_root.join("tmp");
    fs::create_dir(&private_home)
        .and_then(|_| fs::create_dir(&private_tmp))
        .map_err(|error| {
            format!("Cannot prepare private Presentation Editor environment: {error}")
        })?;
    let private_home = private_home.canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor HOME: {error}")
    })?;
    let private_tmp = private_tmp.canonicalize().map_err(|error| {
        format!("Cannot canonicalize private Presentation Editor temporary directory: {error}")
    })?;
    let plan_path = plan_root.join("edit-plan.json");
    Ok(PreparedPresentationEditor {
        contract,
        _plan_directory: plan_directory,
        plan_path,
        frozen_script_path,
        runtime_root,
        private_home,
        private_tmp,
    })
}

fn canonical_editor_arguments_prefix(
    invocation: &ArtifactRuntimeInvocation,
) -> Result<Vec<OsString>, CommandExecutionError> {
    let mut arguments = invocation.arguments_prefix().to_vec();
    let mut index = 0usize;
    while index < arguments.len() {
        if arguments[index] == OsStr::new("--import") {
            let bootstrap = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Node runtime is missing its fixed bootstrap path.".to_string(),
                )
            })?;
            let bootstrap = Path::new(bootstrap).canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Node bootstrap: {error}"
                ))
            })?;
            arguments[index + 1] = bootstrap.into_os_string();
            index += 2;
        } else {
            index += 1;
        }
    }
    Ok(arguments)
}

fn presentation_editor_environment(
    invocation: &ArtifactRuntimeInvocation,
    editor: &PreparedPresentationEditor,
) -> Vec<(OsString, OsString)> {
    let mut environment = invocation
        .environment()
        .iter()
        .map(|(key, value)| {
            if key == OsStr::new("MYCOPILOT_ARTIFACT_NODE_MODULES") {
                let canonical = Path::new(value)
                    .canonicalize()
                    .unwrap_or_else(|_| PathBuf::from(value));
                (key.clone(), canonical.into_os_string())
            } else {
                (key.clone(), value.clone())
            }
        })
        .collect::<Vec<_>>();
    environment.extend([
        (OsString::from("LANG"), OsString::from("C.UTF-8")),
        (OsString::from("LC_ALL"), OsString::from("C")),
        (OsString::from("TZ"), OsString::from("UTC")),
        (OsString::from("TERM"), OsString::from("dumb")),
        (OsString::from("CI"), OsString::from("1")),
        (
            OsString::from("HOME"),
            editor.private_home.clone().into_os_string(),
        ),
        (
            OsString::from("USERPROFILE"),
            editor.private_home.clone().into_os_string(),
        ),
        (
            OsString::from("TMPDIR"),
            editor.private_tmp.clone().into_os_string(),
        ),
        (
            OsString::from("TMP"),
            editor.private_tmp.clone().into_os_string(),
        ),
        (
            OsString::from("TEMP"),
            editor.private_tmp.clone().into_os_string(),
        ),
    ]);
    environment
}

fn presentation_editor_output_redactions(
    editor: &PreparedPresentationEditor,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    append_private_path_spellings(
        &mut replacements,
        editor
            .plan_path
            .parent()
            .expect("private editor plan has a parent"),
        "<presentation-editor-private>",
    );
    append_private_path_spellings(
        &mut replacements,
        &editor.frozen_script_path,
        "<presentation-editor.mjs>",
    );
    if let Some(inputs) = prepared_inputs {
        append_private_path_spellings(
            &mut replacements,
            inputs.root(),
            "<presentation-editor-inputs>",
        );
    }
    append_private_path_spellings(&mut replacements, &editor.runtime_root, "<managed-runtime>");
    append_private_path_spellings(&mut replacements, invocation.executable(), "<managed-node>");
    append_private_path_spellings(&mut replacements, cwd, ".");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

#[derive(Debug)]
struct ManagedNodeSyntaxCheckExecution {
    exit_code: Option<i32>,
    stdout: Option<CapturedProcessOutput>,
    stderr: Option<CapturedProcessOutput>,
    timed_out: bool,
    cancelled: bool,
    duration_ms: u64,
    error: Option<String>,
}

impl ManagedNodeSyntaxCheckExecution {
    fn failed(started: Instant, error: impl Into<String>) -> Self {
        Self {
            exit_code: None,
            stdout: None,
            stderr: None,
            timed_out: false,
            cancelled: false,
            duration_ms: elapsed_millis_saturating(started),
            error: Some(error.into()),
        }
    }

    fn succeeded(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out && !self.cancelled && self.error.is_none()
    }
}

fn run_managed_node_syntax_check(
    invocation: &ArtifactRuntimeInvocation,
    script: &str,
    cwd: &Path,
    environment: &[(OsString, OsString)],
    cancellation: &AgentCancellationToken,
    redactions: super::output_capture::ProcessOutputRedactionSet,
) -> ManagedNodeSyntaxCheckExecution {
    let started = Instant::now();
    if cancellation.is_cancelled() {
        return ManagedNodeSyntaxCheckExecution {
            cancelled: true,
            ..ManagedNodeSyntaxCheckExecution::failed(
                started,
                "Managed Builder Node 语法预检在启动前被取消。",
            )
        };
    }

    let mut arguments = invocation.arguments_prefix().to_vec();
    arguments.push(OsString::from("--check"));
    arguments.push(OsString::from(script));
    let launch = CommandDirectLaunchPlan::isolated(
        invocation.executable().to_path_buf(),
        arguments,
        environment.to_vec(),
    );
    let plan = CommandSpawnPlan::direct(
        "managed-node-syntax-check".to_string(),
        cwd.to_path_buf(),
        None,
        Some(Duration::from_millis(MANAGED_NODE_SYNTAX_CHECK_TIMEOUT_MS)),
        launch,
    );
    let mut command = plan.build();
    let mut child = match command.spawn() {
        Ok(child) => ManagedCommandChild::new(child),
        Err(_) => {
            return ManagedNodeSyntaxCheckExecution::failed(
                started,
                "无法启动固定受管 Node 进行 Builder 语法预检。",
            )
        }
    };
    let Some(stdout) = child.child_mut().stdout.take() else {
        return ManagedNodeSyntaxCheckExecution::failed(
            started,
            "无法捕获受管 Node 语法预检的 stdout。",
        );
    };
    let Some(stderr) = child.child_mut().stderr.take() else {
        return ManagedNodeSyntaxCheckExecution::failed(
            started,
            "无法捕获受管 Node 语法预检的 stderr。",
        );
    };
    let capture_policy = ProcessOutputCapturePolicy::process_default();
    let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
    let stdout_reader = super::output_capture::spawn_process_output_capture_with_observers(
        stdout,
        capture_budget.clone(),
        capture_policy,
        Some(crate::AgentCommandOutputStream::Stdout),
        None,
        None,
        redactions.clone(),
    );
    let stderr_reader = super::output_capture::spawn_process_output_capture_with_observers(
        stderr,
        capture_budget,
        capture_policy,
        Some(crate::AgentCommandOutputStream::Stderr),
        None,
        None,
        redactions,
    );

    let mut timed_out = false;
    let mut cancelled = false;
    let mut error = None;
    let exit_status = loop {
        match try_wait_command_process_group(child.child_mut()) {
            Ok(Some(status)) => {
                child.mark_reaped();
                break Some(status);
            }
            Ok(None) => {}
            Err(_) => {
                error = Some("等待受管 Node 语法预检失败。".to_string());
                force_terminate_command_process_group(child.child_mut());
                let status = child.child_mut().wait().ok();
                if status.is_some() {
                    child.mark_reaped();
                }
                break status;
            }
        }
        if cancellation.is_cancelled() {
            cancelled = true;
            force_terminate_command_process_group(child.child_mut());
            let status = child.child_mut().wait().ok();
            if status.is_some() {
                child.mark_reaped();
            }
            break status;
        }
        if started.elapsed() >= Duration::from_millis(MANAGED_NODE_SYNTAX_CHECK_TIMEOUT_MS) {
            timed_out = true;
            force_terminate_command_process_group(child.child_mut());
            let status = child.child_mut().wait().ok();
            if status.is_some() {
                child.mark_reaped();
            }
            break status;
        }
        thread::sleep(Duration::from_millis(10));
    };
    drop(child);

    let stdout = match join_process_output_capture(stdout_reader, "受管 Node 语法预检 stdout")
    {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| "读取受管 Node 语法预检 stdout 失败。".to_string());
            None
        }
    };
    let stderr = match join_process_output_capture(stderr_reader, "受管 Node 语法预检 stderr")
    {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| "读取受管 Node 语法预检 stderr 失败。".to_string());
            None
        }
    };
    ManagedNodeSyntaxCheckExecution {
        exit_code: exit_status
            .as_ref()
            .and_then(std::process::ExitStatus::code),
        stdout,
        stderr,
        timed_out,
        cancelled,
        duration_ms: elapsed_millis_saturating(started),
        error,
    }
}

fn managed_node_syntax_check_failure_result(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    resolution: AgentCommandRuntimeResolution,
    execution: ManagedNodeSyntaxCheckExecution,
) -> AgentCommandExecutionResult {
    let (code, recovery, message) = if execution.cancelled {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            "Managed Builder Node 语法预检已取消；Builder 未启动。",
        )
    } else if execution.timed_out {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            "Managed Builder Node 语法预检超时；Builder 未启动。",
        )
    } else if execution.error.is_some() || execution.exit_code.is_none() {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            ArtifactRuntimeRecovery::RepairComponent.stable_name(),
            "Managed Builder Node 语法预检无法完成；Builder 未启动。",
        )
    } else {
        (
            ERROR_BUILDER_SYNTAX_INVALID,
            "changeBuilder",
            "Managed Builder 未通过 Node 语法校验；Builder 未启动。请修复脚本后重新检查。",
        )
    };
    let runtime = with_resolution_error(resolution, code, recovery, message);
    let mut result = if execution.cancelled {
        runtime_cancelled_result(root, cwd, request, runtime)
    } else {
        runtime_failure_result(root, cwd, request, runtime, execution.duration_ms)
    };
    result.exit_code = execution.exit_code;
    result.timed_out = execution.timed_out;
    result.cancelled = execution.cancelled;
    result.duration_ms = execution.duration_ms;
    result.output_capture = ProcessOutputCaptureMetadata::from_optional_streams(
        execution.stdout.as_ref(),
        execution.stderr.as_ref(),
        execution.stdout.is_none() || execution.stderr.is_none(),
    );
    if let Some(stdout) = execution.stdout.as_ref() {
        result.stdout = stdout.preview().to_string();
        result.stdout_truncated = stdout.preview_truncated();
        result.stdout_spool = stdout.spool();
    } else {
        result.stdout_truncated = true;
    }
    if let Some(stderr) = execution.stderr.as_ref() {
        result.stderr = stderr.preview().to_string();
        result.stderr_truncated = stderr.preview_truncated();
        result.stderr_spool = stderr.spool();
    } else {
        result.stderr_truncated = true;
    }
    result
}

fn managed_node_syntax_check_redactions(
    runtime_root: &Path,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
    script: &str,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    let requested_script = Path::new(script);
    let script_path = if requested_script.is_absolute() {
        requested_script.to_path_buf()
    } else {
        cwd.join(requested_script)
    };
    let script_projection = if requested_script.is_absolute() {
        "<managed-builder>"
    } else {
        script
    };
    append_private_path_spellings(&mut replacements, &script_path, script_projection);
    append_private_path_spellings(&mut replacements, cwd, ".");
    append_private_path_spellings(&mut replacements, runtime_root, "<managed-runtime>");
    append_private_path_spellings(&mut replacements, invocation.executable(), "<managed-node>");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

fn elapsed_millis_saturating(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn managed_pdf_output_redactions(
    execution_root: &Path,
    outputs_root: &Path,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    append_private_path_spellings(&mut replacements, outputs_root, "outputs");
    if let Some(prepared_inputs) = prepared_inputs {
        append_private_path_spellings(
            &mut replacements,
            prepared_inputs.root(),
            "$MYCOPILOT_INPUT_ROOT",
        );
    }
    append_private_path_spellings(&mut replacements, execution_root, ".");
    append_private_path_spellings(&mut replacements, runtime_root, "<managed-runtime>");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

struct ManagedPdfToolPaths<'a> {
    runtime_root: &'a Path,
    pdf_cli: &'a Path,
    ripgrep: &'a Path,
}

#[derive(Debug)]
enum ManagedPdfShellLaunchError {
    InvalidCommand(String),
    Unavailable(String),
}

fn prepare_managed_pdf_shell_launch(
    invocation: &ArtifactRuntimeInvocation,
    shell_plan: &super::managed_pdf_shell::ManagedPdfShellPlan,
    environment: &mut Vec<(OsString, OsString)>,
    workspace: &ManagedCommandWorkspaceLease,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    tools: ManagedPdfToolPaths<'_>,
) -> Result<CommandDirectLaunchPlan, ManagedPdfShellLaunchError> {
    let ManagedPdfToolPaths {
        runtime_root,
        pdf_cli,
        ripgrep,
    } = tools;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            invocation,
            shell_plan,
            environment,
            workspace,
            prepared_inputs,
            runtime_root,
            pdf_cli,
            ripgrep,
        );
        return Err(ManagedPdfShellLaunchError::Unavailable(
            "Managed PDF Runtime 当前缺少受支持的 Host 文件系统沙箱，已拒绝裸进程执行。"
                .to_string(),
        ));
    }

    #[cfg(target_os = "macos")]
    {
        for (path, label) in [
            (Path::new(MANAGED_PDF_SANDBOX_EXECUTABLE), "Host 沙箱"),
            (
                Path::new(MANAGED_PDF_SHELL_EXECUTABLE),
                "固定非交互式 Shell",
            ),
        ] {
            let metadata = std::fs::symlink_metadata(path).map_err(|_| {
                ManagedPdfShellLaunchError::Unavailable(format!(
                    "Managed PDF Runtime 的 {label} 不可用，已拒绝执行。"
                ))
            })?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(ManagedPdfShellLaunchError::Unavailable(format!(
                    "Managed PDF Runtime 的 {label} 身份无效，已拒绝执行。"
                )));
            }
        }

        let runtime_root = runtime_root.canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证受管运行时边界。".to_string(),
            )
        })?;
        let executable = invocation.executable().canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证受管 Python 身份。".to_string(),
            )
        })?;
        if !executable.starts_with(&runtime_root) {
            return Err(ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 的 Python 不属于冻结的受管运行时。".to_string(),
            ));
        }
        let pdf_cli = pdf_cli.canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证受管 PDF CLI。".to_string(),
            )
        })?;
        let ripgrep = ripgrep.canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证受管 ripgrep。".to_string(),
            )
        })?;
        if !pdf_cli.starts_with(&runtime_root)
            || !ripgrep.starts_with(&runtime_root)
            || !pdf_cli.is_file()
            || !ripgrep.is_file()
        {
            return Err(ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 的 receipt 工具不属于冻结的受管运行时。".to_string(),
            ));
        }
        let execution_root = workspace.execution_root().canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证 Run 私有执行边界。".to_string(),
            )
        })?;
        let input_root = prepared_inputs
            .map(PreparedAgentFileInputs::root)
            .unwrap_or(execution_root.as_path())
            .canonicalize()
            .map_err(|_| {
                ManagedPdfShellLaunchError::Unavailable(
                    "Managed PDF Runtime 无法验证只读输入边界。".to_string(),
                )
            })?;
        workspace.home_root().canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证私有 HOME。".to_string(),
            )
        })?;
        workspace.temp_root().canonicalize().map_err(|_| {
            ManagedPdfShellLaunchError::Unavailable(
                "Managed PDF Runtime 无法验证私有临时目录。".to_string(),
            )
        })?;

        shell_plan
            .validate_private_redirections(&execution_root)
            .map_err(ManagedPdfShellLaunchError::InvalidCommand)?;
        let compiled_command = shell_plan
            .compile(
                prepared_inputs,
                &super::managed_pdf_shell::ManagedPdfShellCompileTools {
                    python: &executable,
                    pdf_cli: &pdf_cli,
                    ripgrep: &ripgrep,
                    input_root: &input_root,
                },
            )
            .map_err(ManagedPdfShellLaunchError::InvalidCommand)?;

        // The compiled shell uses exact, receipt-verified executable paths. PATH stays empty and
        // no model-visible environment value contains a private or runtime path.
        environment.retain(|(key, _)| {
            let key = key.to_string_lossy();
            !matches!(key.as_ref(), "PATH" | "LANG" | "LC_ALL" | "LC_CTYPE" | "TZ")
                && !key.starts_with("MYCOPILOT_")
                && !key.starts_with("RIPGREP_")
        });
        for (key, value) in [
            ("PATH", OsStr::new("")),
            ("LANG", OsStr::new("C.UTF-8")),
            ("LC_ALL", OsStr::new("C.UTF-8")),
            ("TZ", OsStr::new("UTC")),
            ("HOME", OsStr::new(".home")),
            ("USERPROFILE", OsStr::new(".home")),
            ("TMPDIR", OsStr::new(".tmp")),
            ("TMP", OsStr::new(".tmp")),
            ("TEMP", OsStr::new(".tmp")),
        ] {
            replace_managed_environment_value(environment, key, value);
        }

        // A single outer Seatbelt covers Bash and every descendant. Only Bash may fork to build a
        // validated pipeline, and `process-path` restricts every exec edge: sandbox-exec may start
        // the fixed Bash, while Bash may start only exact managed Python and ripgrep. Python and rg
        // can neither fork nor reuse those exec grants. Raw setsid/setpgid syscalls are denied so a
        // child cannot detach from cancellation.
        let mut arguments = Vec::with_capacity(24);
        for (name, path) in [
            ("SHELL", Path::new(MANAGED_PDF_SHELL_EXECUTABLE)),
            ("SANDBOX_EXEC", Path::new(MANAGED_PDF_SANDBOX_EXECUTABLE)),
            ("PYTHON", executable.as_path()),
            ("RIPGREP", ripgrep.as_path()),
            ("RUNTIME_ROOT", runtime_root.as_path()),
            ("INPUT_ROOT", input_root.as_path()),
            ("EXECUTION_ROOT", execution_root.as_path()),
        ] {
            let value = path.to_str().ok_or_else(|| {
                ManagedPdfShellLaunchError::Unavailable(
                    "Managed PDF Runtime 的 Host 沙箱不支持非 UTF-8 私有路径。".to_string(),
                )
            })?;
            arguments.push(OsString::from("-D"));
            arguments.push(OsString::from(format!("{name}={value}")));
        }
        arguments.push(OsString::from("-p"));
        arguments.push(OsString::from(MANAGED_PDF_SANDBOX_PROFILE));
        arguments.push(OsString::from(MANAGED_PDF_SHELL_EXECUTABLE));
        arguments.push(OsString::from("--noprofile"));
        arguments.push(OsString::from("--norc"));
        arguments.push(OsString::from("-o"));
        arguments.push(OsString::from("pipefail"));
        arguments.push(OsString::from("-c"));
        arguments.push(OsString::from(format!(
            "readonly PATH HOME TMPDIR TMP TEMP USERPROFILE LANG LC_ALL TZ\n{compiled_command}"
        )));

        Ok(CommandDirectLaunchPlan::isolated(
            PathBuf::from(MANAGED_PDF_SANDBOX_EXECUTABLE),
            arguments,
            environment.clone(),
        ))
    }
}

fn replace_managed_environment_value(
    environment: &mut Vec<(OsString, OsString)>,
    key: &str,
    value: &OsStr,
) {
    environment.retain(|(candidate, _)| candidate != OsStr::new(key));
    environment.push((OsString::from(key), value.to_os_string()));
}

fn append_private_path_spellings(
    replacements: &mut Vec<(String, String)>,
    path: &Path,
    replacement: &str,
) {
    let mut spellings = Vec::new();
    spellings.push(path.to_string_lossy().into_owned());
    if let Ok(canonical) = path.canonicalize() {
        spellings.push(canonical.to_string_lossy().into_owned());
    }
    #[cfg(target_os = "macos")]
    {
        for spelling in spellings.clone() {
            if let Some(without_private) = spelling.strip_prefix("/private/") {
                spellings.push(format!("/{without_private}"));
            } else if spelling.starts_with("/var/") || spelling.starts_with("/tmp/") {
                spellings.push(format!("/private{spelling}"));
            }
        }
    }
    replacements.extend(
        spellings
            .into_iter()
            .filter(|spelling| !spelling.is_empty())
            .map(|spelling| (spelling, replacement.to_string())),
    );
}

const ERROR_INVALID_COMMAND: &str = "artifactRuntime.invalidCommandShape";
const ERROR_UNAVAILABLE: &str = "artifactRuntime.unavailable";
const ERROR_BUILDER_SYNTAX_INVALID: &str = "managedBuilder.syntaxInvalid";
const ERROR_BUILDER_SYNTAX_CHECK_FAILED: &str = "managedBuilder.syntaxCheckFailed";
const MANAGED_PDF_HARD_TIMEOUT_MS: u64 = 300_000;
const MANAGED_NODE_SYNTAX_CHECK_TIMEOUT_MS: u64 = 30_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedArtifactCommand {
    script: Option<String>,
    process_arguments: Vec<OsString>,
    pdf_shell: Option<super::managed_pdf_shell::ManagedPdfShellPlan>,
    node_syntax_check: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedArtifactBuilderCommand {
    pub kind: AgentCommandRuntimeKind,
    pub script: String,
    pub source_paths: Vec<String>,
    pub output_paths: Vec<String>,
    pub node_syntax_check: bool,
}

pub(crate) fn infer_managed_artifact_command_kind(
    command: &str,
) -> Result<AgentCommandRuntimeKind, String> {
    let tokens = managed_artifact_command_tokens(command)?;
    let kind = match tokens[0].as_str() {
        "node" => AgentCommandRuntimeKind::Node,
        "python" | "python3" => AgentCommandRuntimeKind::Python,
        _ => {
            return Err(
                "runtimeProfile 只支持 `node <script>.mjs`、`node --check <script>.mjs`、`python <script>.py` 或 `python3 <script>.py` 直接调用。"
                    .to_string(),
            )
        }
    };
    parse_managed_artifact_command(command, kind)?;
    Ok(kind)
}

/// Recognizes the deliberately narrow Managed Builder command shape and extracts its declared
/// output paths without executing the script. Ordinary shell commands return `None`; malformed
/// direct Builder output flags fail closed so approval cannot freeze an ambiguous observation.
pub(crate) fn infer_managed_artifact_builder_command(
    command: &str,
) -> Result<Option<ManagedArtifactBuilderCommand>, String> {
    let tokens = match managed_artifact_command_tokens(command) {
        Ok(tokens) => tokens,
        Err(error)
            if looks_like_office_builder_intent(command)
                || looks_like_node_syntax_check_intent(command) =>
        {
            return Err(error)
        }
        Err(_) => return Ok(None),
    };
    let kind = match tokens[0].as_str() {
        "node" => AgentCommandRuntimeKind::Node,
        "python" | "python3" => AgentCommandRuntimeKind::Python,
        _ => return Ok(None),
    };
    let parsed = match parse_managed_artifact_command(command, kind) {
        Ok(parsed) => parsed,
        Err(error)
            if looks_like_office_builder_intent(command)
                || looks_like_node_syntax_check_intent(command) =>
        {
            return Err(error)
        }
        Err(_) => return Ok(None),
    };

    let mut source_paths = Vec::new();
    let mut output_paths = Vec::new();
    let mut saw_source = false;
    let mut saw_output = false;
    let mut index = 2;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "--" {
            break;
        }
        let source = if token == "--source" {
            if saw_source {
                return Err(
                    "Managed Builder 只允许一个静态 `--source` 参数；请拆分为独立命令。"
                        .to_string(),
                );
            }
            index += 1;
            Some(
                tokens
                    .get(index)
                    .ok_or_else(|| {
                        "Managed Builder 的 `--source` 必须紧跟一个输入挂载路径。".to_string()
                    })?
                    .as_str(),
            )
        } else {
            let source = token.strip_prefix("--source=");
            if source.is_some() && saw_source {
                return Err(
                    "Managed Builder 只允许一个静态 `--source` 参数；请拆分为独立命令。"
                        .to_string(),
                );
            }
            source
        };
        if let Some(source) = source {
            let source = source.trim();
            if source.is_empty() || source.starts_with('-') {
                return Err("Managed Builder 的 `--source` 必须指定非空输入挂载路径。".to_string());
            }
            saw_source = true;
            source_paths.push(source.to_string());
            index += 1;
            continue;
        }
        let output = if token == "--output" {
            if saw_output {
                return Err(
                    "Managed Builder 只允许一个静态 `--output` 参数；请拆分为独立命令。"
                        .to_string(),
                );
            }
            index += 1;
            Some(
                tokens
                    .get(index)
                    .ok_or_else(|| {
                        "Managed Builder 的 `--output` 必须紧跟一个输出文件路径。".to_string()
                    })?
                    .as_str(),
            )
        } else {
            let output = token.strip_prefix("--output=");
            if output.is_some() && saw_output {
                return Err(
                    "Managed Builder 只允许一个静态 `--output` 参数；请拆分为独立命令。"
                        .to_string(),
                );
            }
            output
        };
        if let Some(output) = output {
            let output = output.trim();
            if output.is_empty() || output.starts_with('-') {
                return Err("Managed Builder 的 `--output` 必须指定非空输出文件路径。".to_string());
            }
            saw_output = true;
            output_paths.push(output.to_string());
        }
        index += 1;
    }

    Ok(Some(ManagedArtifactBuilderCommand {
        kind,
        script: parsed
            .script
            .expect("ordinary managed artifact commands always carry a saved script"),
        source_paths,
        output_paths,
        node_syntax_check: parsed.node_syntax_check,
    }))
}

fn looks_like_office_builder_intent(command: &str) -> bool {
    let command = command.trim_start();
    let has_direct_launcher = ["node", "python", "python3"].iter().any(|launcher| {
        command
            .strip_prefix(launcher)
            .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    });
    let lower = command.to_ascii_lowercase();
    has_direct_launcher
        && lower.contains("--output")
        && [".docx", ".xlsx", ".pptx"]
            .iter()
            .any(|extension| lower.contains(extension))
}

fn looks_like_node_syntax_check_intent(command: &str) -> bool {
    let command = command.trim_start();
    command.strip_prefix("node").is_some_and(|rest| {
        rest.starts_with(char::is_whitespace)
            && rest.contains("--check")
            && rest.to_ascii_lowercase().contains(".mjs")
    })
}

/// Revalidates every host-bound Builder output against the effective write scope.
///
/// This runs both while preparing an action and again immediately before process spawn. It is
/// deliberately independent of artifact observation: observation is telemetry and never grants
/// filesystem authority.
pub(crate) fn validate_managed_artifact_builder_output_scope(
    workspace_root: Option<&Path>,
    cwd: &Path,
    outputs: &[String],
    write_permission: AgentWritePermission,
) -> Result<(), String> {
    if outputs.is_empty() || write_permission == AgentWritePermission::All {
        return Ok(());
    }
    let workspace_root = workspace_root.ok_or_else(|| {
        "没有 workspace 时，Managed Builder 输出需要 write=all 权限。".to_string()
    })?;
    if !cwd.is_absolute() || !workspace_root.is_absolute() {
        return Err("Managed Builder 输出范围校验要求绝对 workspace 与 cwd。".to_string());
    }
    for output in outputs {
        let requested = Path::new(output);
        let candidate = if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            cwd.join(requested)
        };
        let normalized = normalize_absolute_builder_path(&candidate)?;
        let resolved = resolve_builder_scope_path(&normalized)?;
        if !resolved.starts_with(workspace_root) {
            return Err(format!(
                "Managed Builder 输出 `{output}` 位于 workspace 外；当前权限只允许写入 workspace。"
            ));
        }
    }
    Ok(())
}

fn normalize_absolute_builder_path(path: &Path) -> Result<PathBuf, String> {
    if !path.is_absolute() {
        return Err("Managed Builder 输出路径规范化前必须是绝对路径。".to_string());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err("Managed Builder 输出路径不能越过文件系统根目录。".to_string());
                }
            }
        }
    }
    Ok(normalized)
}

fn resolve_builder_scope_path(path: &Path) -> Result<PathBuf, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            return path
                .canonicalize()
                .map_err(|error| format!("Managed Builder 输出路径无法规范化：{error}"));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("Managed Builder 输出路径无法检查：{error}"));
        }
    }
    let mut ancestor = path
        .parent()
        .ok_or_else(|| "Managed Builder 输出路径缺少可解析的父目录。".to_string())?;
    let mut tail = Vec::new();
    loop {
        match fs::symlink_metadata(ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    "Managed Builder 输出路径没有可访问的已有祖先目录。".to_string()
                })?;
                tail.push(name.to_os_string());
                ancestor = ancestor.parent().ok_or_else(|| {
                    "Managed Builder 输出路径没有可访问的已有祖先目录。".to_string()
                })?;
            }
            Err(error) => {
                return Err(format!(
                    "Managed Builder 输出路径的已有祖先无法检查：{error}"
                ));
            }
        }
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|error| format!("Managed Builder 输出路径的已有祖先无法规范化：{error}"))?;
    for component in tail.iter().rev() {
        resolved.push(component);
    }
    if let Some(name) = path.file_name() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn parse_managed_artifact_command(
    command: &str,
    kind: AgentCommandRuntimeKind,
) -> Result<ManagedArtifactCommand, String> {
    let tokens = managed_artifact_command_tokens(command)?;
    let expected_programs: &[&str] = match kind {
        AgentCommandRuntimeKind::Node => &["node"],
        AgentCommandRuntimeKind::Python => &["python", "python3"],
    };
    if !expected_programs.contains(&tokens[0].as_str()) {
        return Err(format!(
            "runtime.kind={} 要求命令首 token 为 {}。",
            runtime_kind_name(kind),
            expected_programs.join(" 或 ")
        ));
    }
    let (script_index, node_syntax_check) = match (kind, tokens[1].as_str()) {
        (AgentCommandRuntimeKind::Node, "--check") => {
            if tokens.len() != 3 {
                return Err(
                    "Managed Artifact Runtime 的 Node 语法预检只接受精确的 `node --check <script>.mjs` 调用。"
                        .to_string(),
                );
            }
            (2, true)
        }
        (_, option) if option.starts_with('-') => {
            return Err(
                "Managed Artifact Runtime 禁止 -e、-c、-m 及其他内联代码或启动选项；Node 仅额外支持精确的 `--check <script>.mjs`。"
                    .to_string(),
            )
        }
        _ => (1, false),
    };
    let script = &tokens[script_index];
    if script.starts_with('-') {
        return Err("Managed Artifact Runtime 脚本路径不能以 `-` 开始。".to_string());
    }
    let expected_extension = match kind {
        AgentCommandRuntimeKind::Node => "mjs",
        AgentCommandRuntimeKind::Python => "py",
    };
    if Path::new(script).extension().and_then(OsStr::to_str) != Some(expected_extension) {
        return Err(format!(
            "Managed Artifact Runtime 的 {} 调用必须指定一个已保存的 .{expected_extension} 脚本参数。",
            runtime_kind_name(kind)
        ));
    }

    Ok(ManagedArtifactCommand {
        script: Some(script.clone()),
        process_arguments: tokens[1..].iter().map(OsString::from).collect(),
        pdf_shell: None,
        node_syntax_check,
    })
}

fn parse_frozen_managed_command(
    command: &str,
    binding: &AgentCommandRuntimeBinding,
) -> Result<ManagedArtifactCommand, String> {
    if binding.profile == AgentCommandRuntimeProfile::Pdf {
        let pdf_shell =
            super::managed_pdf_shell::parse_managed_pdf_shell(command)?.ok_or_else(|| {
                "Managed PDF Runtime 只接受受管 PDF/Python/rg Shell 工作流。".to_string()
            })?;
        return Ok(ManagedArtifactCommand {
            script: None,
            process_arguments: Vec::new(),
            pdf_shell: Some(pdf_shell),
            node_syntax_check: false,
        });
    }
    parse_managed_artifact_command(command, binding.kind)
}

/// Detects the deliberately small command surface that the trusted bundled PDF Skill may bind.
///
/// The return value is only a runtime-family hint; callers must separately prove the exact
/// `bundled:application:pdf` activation before creating a frozen binding.
pub(crate) fn infer_managed_pdf_command_kind(
    command: &str,
) -> Result<Option<AgentCommandRuntimeKind>, String> {
    super::managed_pdf_shell::parse_managed_pdf_shell(command)
        .map(|command| command.map(|_| AgentCommandRuntimeKind::Python))
}

/// Returns every workspace-relative PDF operand that must cross the ordinary approval-bound
/// FileInput path before a trusted managed shell can start.
pub(crate) fn infer_managed_pdf_workspace_inputs(command: &str) -> Result<Vec<String>, String> {
    Ok(super::managed_pdf_shell::parse_managed_pdf_shell(command)?
        .map(|plan| plan.implicit_workspace_inputs())
        .unwrap_or_default())
}

fn managed_artifact_command_tokens(command: &str) -> Result<Vec<String>, String> {
    let lexed = lex_command(command).map_err(|error| {
        format!(
            "Managed Artifact Runtime 命令语法无效（{}）：{}",
            error.code, error.reason
        )
    })?;
    if lexed.segments.len() != 1
        || !lexed.embedded_commands.is_empty()
        || lexed.segments[0].has_write_redirection
        || !lexed.segments[0].input_redirections.is_empty()
    {
        return Err(
            "Managed Artifact Runtime 只接受一个无重定向、无管道、无命令替换的直接脚本调用。"
                .to_string(),
        );
    }
    let tokens = &lexed.segments[0].tokens;
    if tokens.len() < 2 {
        return Err("Managed Artifact Runtime 命令必须包含一个已保存的脚本路径。".to_string());
    }
    Ok(tokens.clone())
}

fn validate_saved_script(
    cwd: &Path,
    workspace_root: Option<&Path>,
    read_permission: AgentReadPermission,
    script: &str,
) -> Result<(), String> {
    let requested = Path::new(script);
    let path = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        cwd.join(requested)
    };
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| "Managed Artifact Runtime 脚本不存在或不可读取。".to_string())?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Managed Artifact Runtime 脚本必须是非符号链接的普通文件。".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|_| "Managed Artifact Runtime 脚本路径无法规范化。".to_string())?;
    if workspace_root.is_none_or(|root| !canonical.starts_with(root))
        && read_permission != AgentReadPermission::All
    {
        return Err(
            "Managed Artifact Runtime 读取 workspace 外脚本需要 read=all 权限。".to_string(),
        );
    }
    Ok(())
}

fn managed_environment(
    invocation: &ArtifactRuntimeInvocation,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Vec<(OsString, OsString)> {
    let mut environment = Vec::new();
    for key in [
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "TMP",
        "TEMP",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            environment.push((OsString::from(key), value));
        }
    }
    environment.extend(
        invocation
            .environment()
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    environment.push((OsString::from("TERM"), OsString::from("dumb")));
    environment.push((OsString::from("CI"), OsString::from("1")));
    if let Some(prepared_inputs) = prepared_inputs {
        environment.push((
            OsString::from(AGENT_FILE_INPUT_ROOT_ENV),
            prepared_inputs.root().as_os_str().to_os_string(),
        ));
    }
    environment
}

fn ready_binding_resolution(
    binding: &AgentCommandRuntimeBinding,
    invocation: &ArtifactRuntimeInvocation,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: invocation.provider_id().to_string(),
        profile: Some(binding.profile),
        profile_revision: Some(binding.profile_revision.clone()),
        bundle_version: Some(invocation.bundle_version().to_string()),
        bundle_revision: Some(invocation.bundle_revision().to_string()),
        kind: binding.kind,
        runtime_version: Some(invocation.version().to_string()),
        runtime_fingerprint: Some(invocation.runtime_fingerprint().to_string()),
        resolved_packages: binding.resolved_packages.clone(),
        error_code: None,
        recovery: None,
        message: None,
    }
}

fn binding_resolution_error(
    binding: &AgentCommandRuntimeBinding,
    code: &str,
    recovery: &str,
    message: &str,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: binding.provider_id.clone(),
        profile: Some(binding.profile),
        profile_revision: Some(binding.profile_revision.clone()),
        bundle_version: Some(binding.bundle_version.clone()),
        bundle_revision: Some(binding.bundle_revision.clone()),
        kind: binding.kind,
        runtime_version: Some(binding.runtime_version.clone()),
        runtime_fingerprint: Some(binding.runtime_fingerprint.clone()),
        resolved_packages: binding.resolved_packages.clone(),
        error_code: Some(code.to_string()),
        recovery: Some(recovery.to_string()),
        message: Some(message.to_string()),
    }
}

fn provider_error_message(code: ArtifactRuntimeErrorCode) -> &'static str {
    match code {
        ArtifactRuntimeErrorCode::InvalidRequest => "Managed Artifact Runtime 请求无效。",
        ArtifactRuntimeErrorCode::Unavailable => "Managed Artifact Runtime 组件不可用。",
        ArtifactRuntimeErrorCode::InvalidComponent => {
            "Managed Artifact Runtime 组件格式无效；请修复或重新安装组件。"
        }
        ArtifactRuntimeErrorCode::IntegrityMismatch => {
            "Managed Artifact Runtime 未通过完整性校验；请修复或重新安装组件。"
        }
        ArtifactRuntimeErrorCode::UnsupportedTarget => {
            "Managed Artifact Runtime 不支持当前系统或架构。"
        }
        ArtifactRuntimeErrorCode::RuntimeConflict => {
            "Managed Artifact Runtime 组件与当前运行请求冲突。"
        }
        ArtifactRuntimeErrorCode::Io => "Managed Artifact Runtime 发生 I/O 错误。",
    }
}

fn with_resolution_error(
    mut resolution: AgentCommandRuntimeResolution,
    code: &str,
    recovery: &str,
    message: &str,
) -> AgentCommandRuntimeResolution {
    resolution.error_code = Some(code.to_string());
    resolution.recovery = Some(recovery.to_string());
    resolution.message = Some(message.to_string());
    resolution
}

fn unresolved_binding_resolution(
    binding: &AgentCommandRuntimeBinding,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: binding.provider_id.clone(),
        profile: Some(binding.profile),
        profile_revision: Some(binding.profile_revision.clone()),
        bundle_version: Some(binding.bundle_version.clone()),
        bundle_revision: Some(binding.bundle_revision.clone()),
        kind: binding.kind,
        runtime_version: Some(binding.runtime_version.clone()),
        runtime_fingerprint: Some(binding.runtime_fingerprint.clone()),
        resolved_packages: binding.resolved_packages.clone(),
        error_code: None,
        recovery: None,
        message: None,
    }
}

fn runtime_cancelled_result(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    runtime: AgentCommandRuntimeResolution,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: request.command.clone(),
        cwd: relative_cwd(root, cwd),
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
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        input_files: evidence_from_bindings(&request.inputs),
        runtime: Some(runtime),
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    }
}

fn runtime_failure_result(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    runtime: AgentCommandRuntimeResolution,
    duration_ms: u64,
) -> AgentCommandExecutionResult {
    AgentCommandExecutionResult {
        outputs: Vec::new(),
        command: request.command.clone(),
        cwd: relative_cwd(root, cwd),
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        timed_out: false,
        cancelled: false,
        duration_ms,
        stdout_truncated: false,
        stderr_truncated: false,
        output_capture: ProcessOutputCaptureMetadata::default(),
        stdout_spool: ProcessOutputSpool::default(),
        stderr_spool: ProcessOutputSpool::default(),
        error: runtime.message.clone(),
        policy_evaluation: None,
        artifact_observation: None,
        input_files: evidence_from_bindings(&request.inputs),
        runtime: Some(runtime),
        managed_outputs: None,
        authoritative_archive_ref: None,
        history_open: None,
    }
}

fn runtime_kind_name(kind: AgentCommandRuntimeKind) -> &'static str {
    match kind {
        AgentCommandRuntimeKind::Node => "node",
        AgentCommandRuntimeKind::Python => "python",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_runtime::{
        ArtifactRuntimeDiscoveryOptions, ArtifactRuntimeKind, ArtifactRuntimeProvider,
        ARTIFACT_RUNTIME_BUNDLE_VERSION, ARTIFACT_RUNTIME_NODE_VERSION,
        ARTIFACT_RUNTIME_PROVIDER_ID, ARTIFACT_RUNTIME_PYTHON_VERSION,
        ARTIFACT_RUNTIME_RIPGREP_VERSION,
    };
    use crate::file_input::prepare_agent_file_input_bindings;
    use crate::office::{
        OfficeEngineCapabilities, OfficeEngineError, OfficeExecutionRequest, OfficeExecutionResult,
        OfficePreparedExecution,
    };
    use crate::{
        AgentApprovalStatus, AgentCommandArtifactObservationKind,
        AgentCommandArtifactObservationRequest, AgentCommandPermission, AgentCommandSafetyPolicy,
        AgentFileInputRef, AgentFileInputSpec, AgentPatchPermission, AgentReadPermission,
        AgentWritePermission,
    };
    use serde::Serialize;
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::atomic::AtomicUsize;
    use tempfile::TempDir;

    #[cfg(unix)]
    const EDITOR_PLAN_JSON: &str = r#"{"schemaVersion":1,"source":{"type":"input","mountPath":"source.pptx"},"destination":{"type":"output","path":"edited.pptx"},"mode":"saveAs","operations":[{"type":"set","target":"/slide[1]/shape[@id=1]","replacement":{"find":"Old","replace":"New"}}]}"#;

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestComponentReceipt {
        schema_version: u32,
        provider_id: String,
        bundle_version: String,
        build_inputs_revision: String,
        platform: String,
        arch: String,
        runtimes: TestRuntimeSet,
        tools: TestToolSet,
        files: Vec<TestFileReceipt>,
        bundle_revision: String,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestRuntimeSet {
        node: TestRuntimeReceipt,
        python: TestRuntimeReceipt,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestRuntimeReceipt {
        version: String,
        executable: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        package_root: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        runtime_home: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        bootstrap: Option<String>,
        dependencies: Vec<TestDependencyReceipt>,
        identity_files: Vec<String>,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestDependencyReceipt {
        name: String,
        version: String,
        identity_file: String,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestToolSet {
        pdf_cli: TestPdfCliReceipt,
        ripgrep: TestExecutableToolReceipt,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestPdfCliReceipt {
        version: String,
        path: String,
        identity_files: Vec<String>,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestExecutableToolReceipt {
        version: String,
        executable: String,
        identity_files: Vec<String>,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestFileReceipt {
        path: String,
        size: u64,
        sha256: String,
    }

    #[cfg(unix)]
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct TestReceiptRevisionPayload<'a> {
        schema_version: u32,
        provider_id: &'a str,
        bundle_version: &'a str,
        build_inputs_revision: &'a str,
        platform: &'a str,
        arch: &'a str,
        runtimes: &'a TestRuntimeSet,
        tools: &'a TestToolSet,
        files: &'a [TestFileReceipt],
    }

    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum TestOfficeOutcome {
        Success,
        Failure,
        Cancelled,
    }

    #[cfg(unix)]
    struct RecordingPresentationOfficeEngine {
        calls: AtomicUsize,
        outcome: TestOfficeOutcome,
    }

    #[cfg(unix)]
    impl RecordingPresentationOfficeEngine {
        fn new(outcome: TestOfficeOutcome) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                outcome,
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[cfg(unix)]
    impl OfficeEngine for RecordingPresentationOfficeEngine {
        fn capabilities(&self) -> OfficeEngineCapabilities {
            OfficeEngineCapabilities::office_cli()
        }

        fn status(
            &self,
            _cancellation: AgentCancellationToken,
        ) -> crate::office::OfficeEngineStatus {
            panic!("status is not used by the managed-runtime completion hook fixture")
        }

        fn prepare(
            &self,
            _context: &OfficeExecutionContext,
            _request: &OfficeExecutionRequest,
        ) -> Result<OfficePreparedExecution, OfficeEngineError> {
            panic!("prepare is not used by the managed-runtime completion hook fixture")
        }

        fn execute_prepared(
            &self,
            _context: &OfficeExecutionContext,
            _prepared: &OfficePreparedExecution,
            _cancellation: AgentCancellationToken,
            _action_cancel_flag: Option<Arc<AtomicBool>>,
        ) -> Result<OfficeExecutionResult, OfficeEngineError> {
            panic!("execute_prepared is not used by the managed-runtime completion hook fixture")
        }

        fn execute_presentation_edit(
            &self,
            context: &OfficeExecutionContext,
            request: &OfficePresentationEditRequest,
            _cancellation: AgentCancellationToken,
            _action_cancel_flag: Option<Arc<AtomicBool>>,
        ) -> Result<OfficePresentationEditResult, OfficeEngineError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.source_path, "source.pptx");
            assert_eq!(request.source_binding.mount_path, "source.pptx");
            assert_eq!(request.destination_path, "edited.pptx");
            assert_eq!(request.operations.len(), 1);
            match self.outcome {
                TestOfficeOutcome::Success => {
                    let destination = context
                        .workspace_root()
                        .expect("editor fixture has a workspace")
                        .join(&request.destination_path);
                    fs::write(destination, b"fixture presentation output").unwrap();
                    Ok(OfficePresentationEditResult {
                        exit_code: Some(0),
                        stdout: r#"{"success":true,"message":"provider success"}"#.to_string(),
                        stderr: "provider success noise".to_string(),
                        timed_out: false,
                        cancelled: false,
                        duration_ms: 1,
                        error_code: None,
                        error: None,
                    })
                }
                TestOfficeOutcome::Failure => Ok(OfficePresentationEditResult {
                    exit_code: Some(7),
                    stdout: r#"{"success":false,"error":{"code":"invalid_target","error":"Could not find the inspected target.","message":"Re-inspect the deck.","details":"/private/var/folders/secret/presentation-edit-plan.json"}}"#.to_string(),
                    stderr:
                        "provider trace /private/var/folders/secret/presentation-edit-plan.json"
                            .to_string(),
                    timed_out: false,
                    cancelled: false,
                    duration_ms: 1,
                    error_code: Some("office.fixture_failure".to_string()),
                    error: Some("fixture Office failure".to_string()),
                }),
                TestOfficeOutcome::Cancelled => Ok(OfficePresentationEditResult {
                    exit_code: None,
                    stdout: String::new(),
                    stderr: String::new(),
                    timed_out: false,
                    cancelled: true,
                    duration_ms: 1,
                    error_code: Some("office.cancelled".to_string()),
                    error: Some("fixture Office cancellation".to_string()),
                }),
            }
        }
    }

    #[cfg(unix)]
    struct PreparedEditorCompletionFixture {
        _runtime: TempDir,
        workspace: TempDir,
        office: Arc<RecordingPresentationOfficeEngine>,
        plan: CommandSpawnPlan,
        completion_hook: super::super::session::CommandSessionCompletionHook,
    }

    #[cfg(unix)]
    fn test_sha256(bytes: &[u8]) -> String {
        sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    #[cfg(unix)]
    fn test_platform() -> &'static str {
        match std::env::consts::OS {
            "macos" => "darwin",
            "windows" => "win32",
            other => other,
        }
    }

    #[cfg(unix)]
    fn test_arch() -> &'static str {
        match std::env::consts::ARCH {
            "aarch64" => "arm64",
            "x86_64" => "x64",
            other => other,
        }
    }

    #[cfg(unix)]
    fn test_dependency(name: &str, version: &str, identity_file: &str) -> TestDependencyReceipt {
        TestDependencyReceipt {
            name: name.to_string(),
            version: version.to_string(),
            identity_file: identity_file.to_string(),
        }
    }

    #[cfg(unix)]
    fn create_test_artifact_runtime() -> (TempDir, Arc<ArtifactRuntimeProvider>) {
        use std::os::unix::fs::PermissionsExt;

        let directory = TempDir::new().unwrap();
        let node_fixture = format!(
            r#"#!/bin/sh
for argument in "$@"; do
  if [ "$argument" = "--check" ]; then
    exit 0
  fi
done
if [ -z "$MYCOPILOT_PRESENTATION_EDIT_PLAN" ]; then
  exit 9
fi
printf '%s' '{}' > "$MYCOPILOT_PRESENTATION_EDIT_PLAN"
"#,
            EDITOR_PLAN_JSON
        );
        let mut files = vec![
            ("dependencies/node/bin/node", node_fixture.into_bytes()),
            (
                "dependencies/node/node_modules/docx/package.json",
                br#"{"name":"docx","version":"9.6.1"}"#.to_vec(),
            ),
            (
                "dependencies/node/node_modules/exceljs/package.json",
                br#"{"name":"exceljs","version":"4.4.0"}"#.to_vec(),
            ),
            (
                "dependencies/node/node_modules/pptxgenjs/package.json",
                br#"{"name":"pptxgenjs","version":"4.0.1"}"#.to_vec(),
            ),
            (
                "dependencies/python/bin/python3",
                b"python fixture".to_vec(),
            ),
            ("dependencies/tools/rg", b"ripgrep fixture".to_vec()),
            ("legal/ripgrep/COPYING", b"fixture copyright\n".to_vec()),
            (
                "legal/ripgrep/LICENSE-MIT",
                b"fixture MIT license\n".to_vec(),
            ),
            ("legal/ripgrep/UNLICENSE", b"fixture unlicense\n".to_vec()),
            ("runtime/node-bootstrap.mjs", b"// fixture\n".to_vec()),
            (
                "runtime/pdf-runtime-cli.py",
                b"# managed PDF CLI fixture\n".to_vec(),
            ),
        ];
        for (name, version, path) in [
            (
                "openpyxl",
                "3.1.5",
                "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA",
            ),
            (
                "pdfplumber",
                "0.11.9",
                "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA",
            ),
            (
                "pypdf",
                "6.15.0",
                "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA",
            ),
            (
                "pypdfium2",
                "5.12.1",
                "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA",
            ),
            (
                "python-docx",
                "1.2.0",
                "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA",
            ),
            (
                "python-pptx",
                "1.0.2",
                "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA",
            ),
            (
                "reportlab",
                "4.4.9",
                "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA",
            ),
            (
                "XlsxWriter",
                "3.2.9",
                "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA",
            ),
        ] {
            files.push((
                path,
                format!("Name: {name}\nVersion: {version}\n").into_bytes(),
            ));
        }
        files.sort_by_key(|(path, _)| *path);
        for (relative, bytes) in &files {
            let path = directory.path().join(relative);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, bytes).unwrap();
            if relative.ends_with("/node")
                || relative.ends_with("/python3")
                || relative.ends_with("/rg")
            {
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
        }
        let file_receipts = files
            .iter()
            .map(|(path, bytes)| TestFileReceipt {
                path: (*path).to_string(),
                size: bytes.len() as u64,
                sha256: test_sha256(bytes),
            })
            .collect::<Vec<_>>();
        let node_dependencies = vec![
            test_dependency(
                "docx",
                "9.6.1",
                "dependencies/node/node_modules/docx/package.json",
            ),
            test_dependency(
                "exceljs",
                "4.4.0",
                "dependencies/node/node_modules/exceljs/package.json",
            ),
            test_dependency(
                "pptxgenjs",
                "4.0.1",
                "dependencies/node/node_modules/pptxgenjs/package.json",
            ),
        ];
        let python_dependencies = vec![
            test_dependency(
                "openpyxl",
                "3.1.5",
                "dependencies/python/lib/python3.12/site-packages/openpyxl-3.1.5.dist-info/METADATA",
            ),
            test_dependency(
                "pdfplumber",
                "0.11.9",
                "dependencies/python/lib/python3.12/site-packages/pdfplumber-0.11.9.dist-info/METADATA",
            ),
            test_dependency(
                "pypdf",
                "6.15.0",
                "dependencies/python/lib/python3.12/site-packages/pypdf-6.15.0.dist-info/METADATA",
            ),
            test_dependency(
                "pypdfium2",
                "5.12.1",
                "dependencies/python/lib/python3.12/site-packages/pypdfium2-5.12.1.dist-info/METADATA",
            ),
            test_dependency(
                "python-docx",
                "1.2.0",
                "dependencies/python/lib/python3.12/site-packages/python_docx-1.2.0.dist-info/METADATA",
            ),
            test_dependency(
                "python-pptx",
                "1.0.2",
                "dependencies/python/lib/python3.12/site-packages/python_pptx-1.0.2.dist-info/METADATA",
            ),
            test_dependency(
                "reportlab",
                "4.4.9",
                "dependencies/python/lib/python3.12/site-packages/reportlab-4.4.9.dist-info/METADATA",
            ),
            test_dependency(
                "xlsxwriter",
                "3.2.9",
                "dependencies/python/lib/python3.12/site-packages/xlsxwriter-3.2.9.dist-info/METADATA",
            ),
        ];
        let mut receipt = TestComponentReceipt {
            schema_version: 3,
            provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
            bundle_version: ARTIFACT_RUNTIME_BUNDLE_VERSION.to_string(),
            build_inputs_revision: format!(
                "artifact-runtime-build-inputs-sha256-v1:{}",
                "a".repeat(64)
            ),
            platform: test_platform().to_string(),
            arch: test_arch().to_string(),
            runtimes: TestRuntimeSet {
                node: TestRuntimeReceipt {
                    version: ARTIFACT_RUNTIME_NODE_VERSION.to_string(),
                    executable: "dependencies/node/bin/node".to_string(),
                    package_root: Some("dependencies/node/node_modules".to_string()),
                    runtime_home: None,
                    bootstrap: Some("runtime/node-bootstrap.mjs".to_string()),
                    identity_files: std::iter::once("dependencies/node/bin/node".to_string())
                        .chain(std::iter::once("runtime/node-bootstrap.mjs".to_string()))
                        .chain(
                            node_dependencies
                                .iter()
                                .map(|dependency| dependency.identity_file.clone()),
                        )
                        .collect(),
                    dependencies: node_dependencies,
                },
                python: TestRuntimeReceipt {
                    version: ARTIFACT_RUNTIME_PYTHON_VERSION.to_string(),
                    executable: "dependencies/python/bin/python3".to_string(),
                    package_root: None,
                    runtime_home: Some("dependencies/python".to_string()),
                    bootstrap: None,
                    identity_files: std::iter::once("dependencies/python/bin/python3".to_string())
                        .chain(
                            python_dependencies
                                .iter()
                                .map(|dependency| dependency.identity_file.clone()),
                        )
                        .collect(),
                    dependencies: python_dependencies,
                },
            },
            tools: TestToolSet {
                pdf_cli: TestPdfCliReceipt {
                    version: "1".to_string(),
                    path: "runtime/pdf-runtime-cli.py".to_string(),
                    identity_files: vec!["runtime/pdf-runtime-cli.py".to_string()],
                },
                ripgrep: TestExecutableToolReceipt {
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
            files: file_receipts,
            bundle_revision: String::new(),
        };
        let revision_payload = TestReceiptRevisionPayload {
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
        receipt.bundle_revision = format!(
            "artifact-runtime-bundle-sha256-v1:{}",
            test_sha256(&serde_json::to_vec(&revision_payload).unwrap())
        );
        fs::write(
            directory.path().join("component-receipt.json"),
            serde_json::to_vec_pretty(&receipt).unwrap(),
        )
        .unwrap();
        let provider = Arc::new(
            ArtifactRuntimeProvider::discover(
                &ArtifactRuntimeDiscoveryOptions::new()
                    .with_configured_component_dir(directory.path()),
            )
            .unwrap(),
        );
        (directory, provider)
    }

    #[cfg(unix)]
    fn editor_permissions() -> AgentPermissions {
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        }
    }

    #[cfg(unix)]
    fn prepare_editor_completion_fixture(
        outcome: TestOfficeOutcome,
    ) -> PreparedEditorCompletionFixture {
        let workspace = TempDir::new().unwrap();
        fs::create_dir_all(workspace.path().join("scripts")).unwrap();
        fs::write(
            workspace.path().join("scripts/editor.mjs"),
            include_str!("../skills/bundled/presentations/templates/editor.mjs"),
        )
        .unwrap();
        fs::write(workspace.path().join("source.pptx"), b"fixture source").unwrap();
        let input_context = AgentFileInputExecutionContext::default();
        let permissions = editor_permissions();
        let inputs = prepare_agent_file_input_bindings(
            Some(workspace.path()),
            permissions,
            &input_context,
            &[
                AgentFileInputSpec {
                    mount_path: super::super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH.to_string(),
                    source: AgentFileInputRef::Workspace {
                        path: "scripts/editor.mjs".to_string(),
                    },
                },
                AgentFileInputSpec {
                    mount_path: "source.pptx".to_string(),
                    source: AgentFileInputRef::Workspace {
                        path: "source.pptx".to_string(),
                    },
                },
            ],
            None,
        )
        .unwrap();
        let (runtime, provider) = create_test_artifact_runtime();
        let binding = super::super::prepare_command_runtime_profile(
            &provider,
            AgentCommandRuntimeProfile::Presentations,
            AgentCommandRuntimeKind::Node,
        )
        .unwrap()
        .binding;
        let request = AgentCommandRequest {
            id: "presentation-editor-completion".to_string(),
            command: "node scripts/editor.mjs --source source.pptx --output edited.pptx"
                .to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: Some(AgentCommandArtifactObservationRequest {
                kinds: vec![AgentCommandArtifactObservationKind::Office],
                expected_outputs: vec!["edited.pptx".to_string()],
                additional_roots: Vec::new(),
            }),
            inputs,
            runtime_binding: Some(Box::new(binding)),
        };
        let office = Arc::new(RecordingPresentationOfficeEngine::new(outcome));
        let preparation = prepare_managed_command_session(
            Some(workspace.path()),
            &request,
            permissions,
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            ManagedCommandSessionServices {
                artifact_runtime: Some(provider),
                office_engine: Some(office.clone()),
                file_inputs: Some(&input_context),
                managed_workspace: None,
            },
        )
        .unwrap();
        let ManagedCommandSessionPreparation::Ready {
            plan,
            completion_hook,
        } = preparation
        else {
            panic!("editor fixture unexpectedly failed during preparation")
        };
        PreparedEditorCompletionFixture {
            _runtime: runtime,
            workspace,
            office,
            plan,
            completion_hook,
        }
    }

    #[cfg(unix)]
    fn completion_result(
        plan: &CommandSpawnPlan,
        exit_code: Option<i32>,
    ) -> AgentCommandExecutionResult {
        AgentCommandExecutionResult {
            outputs: Vec::new(),
            command: plan.command().to_string(),
            cwd: plan.cwd_projection().to_string(),
            exit_code,
            stdout: String::new(),
            stderr: String::new(),
            timed_out: false,
            cancelled: false,
            duration_ms: 1,
            stdout_truncated: false,
            stderr_truncated: false,
            output_capture: ProcessOutputCaptureMetadata::default(),
            stdout_spool: ProcessOutputSpool::default(),
            stderr_spool: ProcessOutputSpool::default(),
            error: None,
            policy_evaluation: None,
            artifact_observation: None,
            input_files: Vec::new(),
            runtime: None,
            managed_outputs: None,
            authoritative_archive_ref: None,
            history_open: None,
        }
    }

    #[cfg(unix)]
    #[test]
    fn presentation_editor_completion_applies_office_once_before_after_observation() {
        let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Success);
        let output = fixture.plan.build().output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let mut result = completion_result(&fixture.plan, output.status.code());

        (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

        assert_eq!(fixture.office.calls(), 1);
        assert_eq!(result.exit_code, Some(0));
        assert!(result.error.is_none());
        assert!(result.stdout.is_empty());
        assert!(result.stderr.is_empty());
        assert!(fixture.workspace.path().join("edited.pptx").is_file());
        let observation = result
            .artifact_observation
            .as_ref()
            .expect("successful Host edit must perform the After capture");
        assert!(observation.coverage.after.roots_scanned > 0);
        assert!(observation
            .changes
            .iter()
            .any(|change| change.path == "edited.pptx"));
    }

    #[cfg(unix)]
    #[test]
    fn presentation_editor_completion_office_failure_fails_command_without_after_observation() {
        let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Failure);
        let output = fixture.plan.build().output().unwrap();
        assert!(output.status.success());
        let mut result = completion_result(&fixture.plan, output.status.code());

        (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

        assert_eq!(fixture.office.calls(), 1);
        assert_eq!(result.exit_code, Some(7));
        let error = result.error.as_deref().expect("bounded Office diagnostic");
        assert!(error.contains("invalid_target"));
        assert!(error.contains("Could not find the inspected target."));
        assert!(error.contains("Re-inspect the deck."));
        assert!(error.contains("office.fixture_failure"));
        assert!(error.contains("fixture Office failure"));
        assert!(!error.contains("/private/var/folders/secret"));
        assert!(!error.contains("presentation-edit-plan.json"));
        assert!(error.chars().count() <= 4_096);
        assert!(result.artifact_observation.is_none());
        assert!(!fixture.workspace.path().join("edited.pptx").exists());
    }

    #[cfg(unix)]
    #[test]
    fn presentation_editor_completion_plan_and_cancellation_failures_never_publish_after() {
        #[derive(Clone, Copy, Debug)]
        enum FailureCase {
            MissingPlan,
            MalformedPlan,
            OfficeCancelled,
        }

        for case in [
            FailureCase::MissingPlan,
            FailureCase::MalformedPlan,
            FailureCase::OfficeCancelled,
        ] {
            let outcome = match case {
                FailureCase::OfficeCancelled => TestOfficeOutcome::Cancelled,
                FailureCase::MissingPlan | FailureCase::MalformedPlan => TestOfficeOutcome::Success,
            };
            let fixture = prepare_editor_completion_fixture(outcome);
            let mut result = completion_result(&fixture.plan, Some(0));
            if !matches!(case, FailureCase::MissingPlan) {
                let mut command = fixture.plan.build();
                let plan_path = command
                    .get_envs()
                    .find_map(|(name, value)| {
                        (name == OsStr::new(super::super::PRESENTATION_EDITOR_PLAN_ENV))
                            .then_some(value)
                            .flatten()
                    })
                    .map(PathBuf::from)
                    .expect("Editor plan path is a Host-owned launch environment value");
                let output = command.output().unwrap();
                assert!(output.status.success(), "case={case:?}");
                if matches!(case, FailureCase::MalformedPlan) {
                    fs::write(plan_path, b"{malformed").unwrap();
                }
            }

            (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

            assert!(result.error.is_some(), "case={case:?}");
            assert!(result.artifact_observation.is_none(), "case={case:?}");
            assert!(
                !fixture.workspace.path().join("edited.pptx").exists(),
                "case={case:?}"
            );
            match case {
                FailureCase::OfficeCancelled => {
                    assert_eq!(fixture.office.calls(), 1);
                    assert!(result.cancelled);
                    assert_eq!(result.exit_code, None);
                }
                FailureCase::MissingPlan | FailureCase::MalformedPlan => {
                    assert_eq!(fixture.office.calls(), 0);
                    assert!(!result.cancelled);
                    assert_eq!(result.exit_code, Some(1));
                }
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn presentation_editor_completion_node_failure_skips_office_and_after_observation() {
        let fixture = prepare_editor_completion_fixture(TestOfficeOutcome::Success);
        let mut result = completion_result(&fixture.plan, Some(9));

        (fixture.completion_hook)(&mut result, Arc::new(AtomicBool::new(false)));

        assert_eq!(fixture.office.calls(), 0);
        assert_eq!(result.exit_code, Some(9));
        assert!(result.artifact_observation.is_none());
        assert!(!fixture.workspace.path().join("edited.pptx").exists());
    }

    #[cfg(unix)]
    fn fake_managed_node(runtime_root: &Path, marker: &Path) -> ArtifactRuntimeInvocation {
        use std::os::unix::fs::PermissionsExt;

        let executable = runtime_root.join("bin/node");
        fs::create_dir_all(executable.parent().unwrap()).unwrap();
        fs::write(
            &executable,
            br#"#!/bin/sh
if [ "$1" = "--check" ]; then
  script="$2"
  case "$script" in
    /*) absolute_script="$script" ;;
    *) absolute_script="$PWD/$script" ;;
  esac
  invalid=0
  while IFS= read -r line || [ -n "$line" ]; do
    case "$line" in
      *SYNTAX_ERROR*) invalid=1 ;;
    esac
  done < "$script"
  if [ "$invalid" -eq 1 ]; then
    printf '%s: SyntaxError: fixture\n' "$absolute_script" >&2
    printf 'runtime=%s\n' "$0" >&2
    exit 1
  fi
  exit 0
fi
shift
output=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--output" ]; then
    shift
    output="$1"
  fi
  shift
done
printf 'builder-ran' > "$TEST_BUILDER_MARKER"
if [ -n "$output" ]; then
  printf 'deck' > "$output"
fi
"#,
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        ArtifactRuntimeInvocation::new(
            "test-bundle".to_string(),
            "test-revision".to_string(),
            "test-fingerprint".to_string(),
            ArtifactRuntimeKind::Node,
            "test-node".to_string(),
            executable,
            Vec::new(),
            BTreeMap::from([(
                OsString::from("TEST_BUILDER_MARKER"),
                marker.as_os_str().to_os_string(),
            )]),
        )
    }

    fn syntax_check_request(command: &str) -> AgentCommandRequest {
        AgentCommandRequest {
            id: "syntax-check-test".to_string(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime_binding: None,
        }
    }

    fn syntax_check_resolution() -> AgentCommandRuntimeResolution {
        AgentCommandRuntimeResolution {
            schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
            provider_id: "test-provider".to_string(),
            profile: Some(AgentCommandRuntimeProfile::Presentations),
            profile_revision: Some("test-profile".to_string()),
            bundle_version: Some("test-bundle".to_string()),
            bundle_revision: Some("test-revision".to_string()),
            kind: AgentCommandRuntimeKind::Node,
            runtime_version: Some("test-node".to_string()),
            runtime_fingerprint: Some("test-fingerprint".to_string()),
            resolved_packages: Vec::new(),
            error_code: None,
            recovery: None,
            message: None,
        }
    }

    #[test]
    fn validates_only_direct_saved_script_invocations() {
        assert!(parse_managed_artifact_command(
            "node scripts/build.mjs --output out.xlsx",
            AgentCommandRuntimeKind::Node,
        )
        .is_ok());
        for command in [
            "node -e console.log(1)",
            "node scripts/build.mjs | tee output.txt",
            "node scripts/build.mjs && echo done",
            "node $(printf script.mjs)",
            "python scripts/build.mjs",
        ] {
            assert!(
                parse_managed_artifact_command(command, AgentCommandRuntimeKind::Node).is_err(),
                "unexpectedly accepted {command}"
            );
        }
        assert!(parse_managed_artifact_command(
            "python3 scripts/build.py",
            AgentCommandRuntimeKind::Python,
        )
        .is_ok());
        assert!(
            parse_managed_artifact_command("python -m build", AgentCommandRuntimeKind::Python,)
                .is_err()
        );
    }

    #[test]
    fn node_syntax_check_accepts_only_the_exact_saved_mjs_shape() {
        let parsed = parse_managed_artifact_command(
            "node --check 'scripts/build deck.mjs'",
            AgentCommandRuntimeKind::Node,
        )
        .unwrap();
        assert!(parsed.node_syntax_check);
        assert_eq!(parsed.script.as_deref(), Some("scripts/build deck.mjs"));
        assert_eq!(
            parsed.process_arguments,
            [
                OsString::from("--check"),
                OsString::from("scripts/build deck.mjs")
            ]
        );
        for command in [
            "node --check build.mjs --output deck.pptx",
            "node --check build.mjs | tee check.log",
            "node --check build.mjs && node build.mjs",
            "node --check -- build.mjs",
        ] {
            assert!(
                infer_managed_artifact_builder_command(command).is_err(),
                "malformed syntax check unexpectedly passed: {command}"
            );
        }
        for ordinary in ["node --check", "node --check build.js"] {
            assert!(
                infer_managed_artifact_builder_command(ordinary)
                    .unwrap()
                    .is_none(),
                "ordinary Node check was incorrectly claimed as a managed .mjs Builder: {ordinary}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn automatic_node_builder_syntax_gate_fails_closed_without_running_builder() {
        let fixture = TempDir::new().unwrap();
        let workspace = fixture.path().join("private-workspace");
        let runtime_root = fixture.path().join("private-runtime");
        fs::create_dir_all(&workspace).unwrap();
        let marker = workspace.join("builder.marker");
        let expected_output = workspace.join("deck.pptx");
        let script = "build_deck.mjs";
        fs::write(workspace.join(script), "const broken = SYNTAX_ERROR;\n").unwrap();
        let invocation = fake_managed_node(&runtime_root, &marker);
        let environment = managed_environment(&invocation, None);
        let execution = run_managed_node_syntax_check(
            &invocation,
            script,
            &workspace,
            &environment,
            &AgentCancellationToken::new(),
            managed_node_syntax_check_redactions(&runtime_root, &invocation, &workspace, script),
        );
        assert!(!execution.succeeded());
        assert_eq!(execution.exit_code, Some(1));
        assert!(
            !marker.exists(),
            "syntax failure must not execute Builder body"
        );
        assert!(
            !expected_output.exists(),
            "syntax failure must not create the declared output"
        );

        let result = managed_node_syntax_check_failure_result(
            Some(&workspace),
            &workspace,
            &syntax_check_request("node build_deck.mjs --output deck.pptx"),
            syntax_check_resolution(),
            execution,
        );
        assert_eq!(
            result.runtime.as_ref().unwrap().error_code.as_deref(),
            Some(ERROR_BUILDER_SYNTAX_INVALID)
        );
        assert!(result.error.as_deref().unwrap().contains("Builder 未启动"));
        for private in [
            workspace.to_string_lossy().as_ref(),
            runtime_root.to_string_lossy().as_ref(),
            invocation.executable().to_string_lossy().as_ref(),
        ] {
            assert!(!result.stderr.contains(private));
        }
        assert!(result.stderr.contains(script));
        let mut spool = String::new();
        result
            .stderr_spool
            .reopen()
            .unwrap()
            .read_to_string(&mut spool)
            .unwrap();
        assert!(!spool.contains(workspace.to_string_lossy().as_ref()));
        assert!(!spool.contains(runtime_root.to_string_lossy().as_ref()));
        assert!(spool.contains(script));
    }

    #[cfg(unix)]
    #[test]
    fn valid_node_builder_syntax_gate_allows_the_pinned_builder_launch() {
        let fixture = TempDir::new().unwrap();
        let workspace = fixture.path().join("workspace");
        let runtime_root = fixture.path().join("runtime");
        fs::create_dir_all(&workspace).unwrap();
        let marker = workspace.join("builder.marker");
        let output = workspace.join("deck.pptx");
        let script = "build_deck.mjs";
        fs::write(workspace.join(script), "export const deck = true;\n").unwrap();
        let invocation = fake_managed_node(&runtime_root, &marker);
        let environment = managed_environment(&invocation, None);
        let syntax_check = run_managed_node_syntax_check(
            &invocation,
            script,
            &workspace,
            &environment,
            &AgentCancellationToken::new(),
            managed_node_syntax_check_redactions(&runtime_root, &invocation, &workspace, script),
        );
        assert!(syntax_check.succeeded());
        assert!(
            !marker.exists(),
            "syntax check itself must not run Builder body"
        );

        let launch = CommandDirectLaunchPlan::isolated(
            invocation.executable().to_path_buf(),
            [
                OsString::from(script),
                OsString::from("--output"),
                output.as_os_str().to_os_string(),
            ]
            .into_iter()
            .collect(),
            environment,
        );
        let status = CommandSpawnPlan::direct(
            "node build_deck.mjs --output deck.pptx".to_string(),
            workspace.clone(),
            Some(&workspace),
            Some(Duration::from_secs(5)),
            launch,
        )
        .build()
        .status()
        .unwrap();
        assert!(status.success());
        assert_eq!(fs::read_to_string(marker).unwrap(), "builder-ran");
        assert_eq!(fs::read_to_string(output).unwrap(), "deck");
    }

    #[test]
    fn managed_pdf_uses_one_host_owned_hard_timeout() {
        let expected = Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS));
        assert_eq!(managed_command_hard_timeout(true, false, None), expected);
        assert_eq!(managed_command_hard_timeout(true, true, Some(1)), expected);
        assert_eq!(managed_command_hard_timeout(false, false, None), None);
        assert_eq!(
            managed_command_hard_timeout(false, false, Some(MAX_TIMEOUT_MS + 1)),
            Some(Duration::from_millis(MAX_TIMEOUT_MS))
        );
        assert_eq!(
            managed_command_hard_timeout(false, true, None),
            Some(Duration::from_millis(PRESENTATION_EDITOR_PLAN_TIMEOUT_MS))
        );
        assert_eq!(
            managed_command_hard_timeout(false, true, Some(250)),
            Some(Duration::from_millis(250))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "release smoke test; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT"]
    fn managed_pdf_shell_runs_receipt_tools_in_one_outer_sandbox() {
        let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
            .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT");
        let provider = ArtifactRuntimeProvider::discover(
            &crate::artifact_runtime::ArtifactRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(component),
        )
        .unwrap();
        let prepared = super::prepare_command_runtime_profile(
            &provider,
            AgentCommandRuntimeProfile::Pdf,
            AgentCommandRuntimeKind::Python,
        )
        .unwrap();
        let fixture = TempDir::new().unwrap();
        let workspace =
            ManagedCommandWorkspaceLease::open(fixture.path().join("managed-run")).unwrap();
        let external = fixture.path().join("external-secret.txt");
        fs::write(&external, b"secret").unwrap();
        let script = format!(
            r#"python - <<'PY'
import os, socket, subprocess
from pathlib import Path
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter
import pdfplumber
import pypdfium2 as pdfium

assert os.environ.get('PATH') == ''
assert os.environ.get('HOME') == '.home'
assert not any(key.startswith('MYCOPILOT_PDF_') for key in os.environ)
assert 'MYCOPILOT_INPUT_ROOT' not in os.environ
assert not any(key.startswith('MYCOPILOT_ARTIFACT_') for key in os.environ)
for label, operation in (
    ('fork', lambda: os.fork()),
    ('setsid', lambda: os.setsid()),
    ('setpgid', lambda: os.setpgid(0, 0)),
    ('exec-bash', lambda: os.execv('/bin/bash', ['bash', '-c', 'true'])),
    ('subprocess', lambda: subprocess.run(['/usr/bin/true'], check=True)),
    ('network', lambda: socket.create_connection(('127.0.0.1', 9), timeout=0.1)),
    ('system-passwd-read', lambda: Path('/etc/passwd').read_bytes()),
    ('system-version-read', lambda: Path('/System/Library/CoreServices/SystemVersion.plist').read_bytes()),
    ('external-read', lambda: Path({external:?}).read_bytes()),
    ('external-write', lambda: Path({external:?}).write_text('changed')),
    ('symlink', lambda: os.symlink({external:?}, 'outputs/escape-link')),
    ('hardlink', lambda: os.link({external:?}, 'outputs/escape-hardlink')),
):
    try:
        operation()
        raise AssertionError(f'{{label}} sandbox escape unexpectedly succeeded')
    except (PermissionError, OSError):
        pass

target = Path('outputs/smoke.pdf')
pdf = canvas.Canvas(str(target))
for page in range(40):
    pdf.drawString(72, 720, f'Smoke marker page {{page}}')
    pdf.showPage()
pdf.save()
assert len(PdfReader(str(target)).pages) == 40
with pdfplumber.open(target) as document:
    assert 'Smoke marker page 0' in (document.pages[0].extract_text() or '')
pdfium.PdfDocument(target)[0].render(scale=1).to_pil().save('outputs/smoke.png')

form_source = Path('form-source.pdf')
form = canvas.Canvas(str(form_source))
form.drawString(72, 720, 'Founder')
form.acroForm.textfield(
    name='founder', tooltip='Founder name', x=72, y=680, width=220, height=24
)
form.save()
source_reader = PdfReader(str(form_source))
assert source_reader.get_fields()['founder'].get('/V', '') == ''
filled = Path('outputs/form-filled.pdf')
writer = PdfWriter()
writer.clone_document_from_reader(source_reader)
writer.update_page_form_field_values(
    writer.pages[0], {{'founder': 'Ada Lovelace'}}, auto_regenerate=False
)
with filled.open('wb') as stream:
    writer.write(stream)
reopened = PdfReader(str(filled))
assert reopened.get_fields()['founder']['/V'] == 'Ada Lovelace'
widgets = [
    annotation.get_object()
    for annotation in reopened.pages[0].get('/Annots', [])
    if annotation.get_object().get('/Subtype') == '/Widget'
]
assert widgets and widgets[0].get('/AP') is not None
pdfium.PdfDocument(filled)[0].render(scale=1).to_pil().save('outputs/form-filled.png')
flattened = Path('outputs/form-flattened.pdf')
flattened_writer = PdfWriter()
flattened_writer.clone_document_from_reader(reopened)
flattened_writer.update_page_form_field_values(
    flattened_writer.pages[0], {{'founder': 'Ada Lovelace'}},
    auto_regenerate=False, flatten=True
)
with flattened.open('wb') as stream:
    flattened_writer.write(stream)
flattened_reader = PdfReader(str(flattened))
assert flattened_reader.pages[0].extract_text()
pdfium.PdfDocument(flattened)[0].render(scale=1).to_pil().save(
    'outputs/form-flattened.png'
)
PY
pdfinfo outputs/smoke.pdf | rg --max-count 1 '^Pages:'
pdftotext outputs/smoke.pdf - | rg --max-count 1 'Smoke marker page 0'
"#,
            external = external.to_string_lossy(),
        );
        let run = |command: &str| {
            let shell_plan = super::super::managed_pdf_shell::parse_managed_pdf_shell(command)
                .unwrap()
                .expect("smoke command uses the managed PDF shell surface");
            let mut environment = managed_environment(&prepared.invocation, None);
            let launch = prepare_managed_pdf_shell_launch(
                &prepared.invocation,
                &shell_plan,
                &mut environment,
                &workspace,
                None,
                ManagedPdfToolPaths {
                    runtime_root: provider.component_root(),
                    pdf_cli: &provider.pdf_cli_path().unwrap(),
                    ripgrep: &provider.ripgrep_executable().unwrap(),
                },
            )
            .unwrap();
            CommandSpawnPlan::direct(
                "managed PDF shell smoke".to_string(),
                workspace.execution_root().to_path_buf(),
                Some(workspace.execution_root()),
                Some(Duration::from_secs(60)),
                launch,
            )
            .build()
            .output()
            .unwrap()
        };
        let output = run(&script);
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(workspace.outputs_root().join("smoke.pdf").is_file());
        assert!(workspace.outputs_root().join("smoke.png").is_file());
        assert!(workspace.outputs_root().join("form-filled.pdf").is_file());
        assert!(workspace.outputs_root().join("form-filled.png").is_file());
        assert!(workspace
            .outputs_root()
            .join("form-flattened.pdf")
            .is_file());
        assert!(workspace
            .outputs_root()
            .join("form-flattened.png")
            .is_file());

        let no_match = run("pdftotext outputs/smoke.pdf - | rg 'definitely-not-present'");
        assert!(
            !no_match.status.success(),
            "pipefail must retain rg failure"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn managed_pdf_sandbox_fails_closed_on_unsupported_hosts() {
        let fixture = TempDir::new().unwrap();
        let workspace =
            ManagedCommandWorkspaceLease::open(fixture.path().join("managed-run")).unwrap();
        let executable = std::env::current_exe().unwrap().canonicalize().unwrap();
        let runtime_root = executable.parent().unwrap().to_path_buf();
        let invocation = ArtifactRuntimeInvocation::new(
            "test".to_string(),
            "test".to_string(),
            "test".to_string(),
            ArtifactRuntimeKind::Python,
            "test".to_string(),
            executable,
            Vec::new(),
            BTreeMap::new(),
        );
        let shell_plan =
            super::super::managed_pdf_shell::parse_managed_pdf_shell("pdfinfo outputs/test.pdf")
                .unwrap()
                .unwrap();
        assert!(prepare_managed_pdf_shell_launch(
            &invocation,
            &shell_plan,
            &mut Vec::new(),
            &workspace,
            None,
            ManagedPdfToolPaths {
                runtime_root: &runtime_root,
                pdf_cli: &runtime_root,
                ripgrep: &runtime_root,
            },
        )
        .unwrap_err()
        .contains("拒绝裸进程执行"));
    }

    #[test]
    fn managed_builder_output_contract_fails_closed_without_reaching_path() {
        let builder = infer_managed_artifact_builder_command(
            "python scripts/build.py --output 'outputs/report.docx'",
        )
        .unwrap()
        .unwrap();
        assert_eq!(builder.kind, AgentCommandRuntimeKind::Python);
        assert_eq!(builder.script, "scripts/build.py");
        assert_eq!(builder.output_paths, ["outputs/report.docx"]);

        for command in [
            "python scripts/build.py --output",
            "python scripts/build.py --output --title",
            "python scripts/build.py --output report.docx --output report.docx",
            "python scripts/build.py --output report.docx | tee build.log",
            "python scripts/build.py --output report.docx && echo done",
            "python -u scripts/build.py --output report.docx",
        ] {
            assert!(
                infer_managed_artifact_builder_command(command).is_err(),
                "malformed Builder intent unexpectedly fell through: {command}"
            );
        }
        assert!(
            infer_managed_artifact_builder_command("python ordinary.py | tee ordinary.log")
                .unwrap()
                .is_none()
        );
    }

    #[cfg(unix)]
    #[test]
    fn builder_scope_rejects_existing_and_broken_external_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().unwrap();
        let workspace_root = workspace.path().canonicalize().unwrap();
        let outside = TempDir::new().unwrap();
        symlink(outside.path(), workspace_root.join("external")).unwrap();
        assert!(validate_managed_artifact_builder_output_scope(
            Some(&workspace_root),
            &workspace_root,
            &["external/report.docx".to_string()],
            AgentWritePermission::WorkspaceOnly,
        )
        .is_err());

        symlink(
            outside.path().join("missing"),
            workspace_root.join("broken"),
        )
        .unwrap();
        assert!(validate_managed_artifact_builder_output_scope(
            Some(&workspace_root),
            &workspace_root,
            &["broken/report.docx".to_string()],
            AgentWritePermission::WorkspaceOnly,
        )
        .is_err());
        assert!(validate_managed_artifact_builder_output_scope(
            Some(&workspace_root),
            &workspace_root,
            &["external/report.docx".to_string()],
            AgentWritePermission::All,
        )
        .is_ok());
    }

    #[test]
    fn managed_environment_ignores_path_and_inherited_node_options() {
        let workspace = TempDir::new().unwrap();
        let fake_runtime = workspace.path().join("managed-node");
        let invocation = ArtifactRuntimeInvocation::new(
            "test-bundle".to_string(),
            "test-revision".to_string(),
            "test-fingerprint".to_string(),
            ArtifactRuntimeKind::Node,
            "test-node".to_string(),
            fake_runtime,
            Vec::new(),
            BTreeMap::new(),
        );
        let configured_environment = managed_environment(&invocation, None);
        assert!(!configured_environment
            .iter()
            .any(|(name, _)| name == OsStr::new("PATH")));
        assert!(!configured_environment
            .iter()
            .any(|(name, _)| name == OsStr::new("NODE_OPTIONS")));
    }
}
