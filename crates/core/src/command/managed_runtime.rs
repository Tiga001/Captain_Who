use super::*;
use crate::artifact_runtime::{
    ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery,
};
use crate::office::{
    prepare_managed_script_staging, OfficeEngine, OfficeExecutionContext,
    OfficeManagedScriptOutputResult, OfficeManagedScriptPurpose, OfficeManagedScriptStaging,
    OfficePresentationEditRequest, OfficePresentationEditResult,
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
        plan: Box<CommandSpawnPlan>,
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
        .filter(|input| {
            request
                .managed_office_script
                .as_deref()
                .is_none_or(|binding| input.mount_path != binding.script_mount_path)
        })
        .collect::<Vec<_>>();
    let managed_office_context = request.managed_office_script.as_ref().map(|_| {
        let file_inputs = file_inputs.cloned().unwrap_or_default();
        OfficeExecutionContext::new(
            input_workspace_root.clone(),
            permissions,
            file_inputs.attachment_library().cloned(),
        )
        .with_file_inputs(file_inputs)
    });
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
    let generic_office_script = match request
        .managed_office_script
        .as_deref()
        .filter(|binding| binding.purpose != OfficeManagedScriptPurpose::EditPresentationPlan)
        .map(|binding| {
            prepare_managed_office_script_execution(
                managed_office_context
                    .as_ref()
                    .expect("managed Office binding has execution context"),
                binding,
                prepared_inputs.as_ref(),
            )
        })
        .transpose()
    {
        Ok(execution) => execution,
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
    let mut arguments = prepared_runtime.invocation.arguments_prefix().to_vec();
    let mut environment =
        managed_environment(&prepared_runtime.invocation, prepared_inputs.as_ref());
    if let Some(editor) = editor_execution.as_ref() {
        arguments = canonical_editor_arguments_prefix(&prepared_runtime.invocation)?;
        arguments.push(OsString::from("--permission"));
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
        // Network-capable builtins are denied by the fixed loader and fetch/WebSocket/EventSource
        // are removed by node-bootstrap before Editor code loads. Avoid version-sensitive
        // `--no-experimental-*` negation flags while keeping the boundary covered by sandbox tests.
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
    } else if let Some(script) = generic_office_script.as_ref() {
        arguments.push(script.frozen_script_path.clone().into_os_string());
        arguments.extend(rewrite_managed_office_script_arguments(
            &parsed.process_arguments[1..],
            script,
            prepared_inputs.as_ref(),
        )?);
    } else {
        arguments.extend(parsed.process_arguments.iter().cloned());
    }
    let automatic_syntax_check = automatic_managed_builder_syntax_check(
        binding.profile,
        binding.kind,
        &parsed,
        managed_builder.as_ref(),
    );
    if let Some(language) = automatic_syntax_check {
        let frozen_editor_script = editor_execution
            .as_ref()
            .map(|editor| editor.frozen_script_path.to_string_lossy().into_owned())
            .or_else(|| {
                generic_office_script
                    .as_ref()
                    .map(|script| script.frozen_script_path.to_string_lossy().into_owned())
            });
        let script = frozen_editor_script.as_deref().unwrap_or_else(|| {
            parsed
                .script
                .as_deref()
                .expect("managed Builder carries a saved script")
        });
        let redactions = managed_builder_syntax_check_redactions(
            provider.component_root(),
            &prepared_runtime.invocation,
            &cwd,
            script,
            language,
        );
        let syntax_check = run_managed_builder_syntax_check(
            &prepared_runtime.invocation,
            script,
            &cwd,
            &environment,
            &cancellation_token,
            redactions,
            language,
        );
        if !syntax_check.succeeded() {
            let mut result = managed_builder_syntax_check_failure_result(
                root.as_deref(),
                &cwd,
                request,
                resolution.clone(),
                syntax_check,
                language,
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
    } else if let Some(script) = generic_office_script.as_ref() {
        managed_office_script_output_redactions(
            script,
            prepared_inputs.as_ref(),
            provider.component_root(),
            &prepared_runtime.invocation,
            &cwd,
        )
    } else if parsed.node_syntax_check {
        managed_builder_syntax_check_redactions(
            provider.component_root(),
            &prepared_runtime.invocation,
            &cwd,
            parsed
                .script
                .as_deref()
                .expect("managed Node syntax check carries a saved script"),
            ManagedBuilderSyntaxLanguage::Node,
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
    let has_managed_office_transaction = request.managed_office_script.is_some();
    let managed_office_python = generic_office_script
        .as_ref()
        .filter(|script| {
            script.binding.document_kind == crate::office::OfficeDocumentKind::Spreadsheet
        })
        .map(|_| prepared_runtime.invocation.clone());
    let approved_input_bindings = request.inputs.clone();
    let approved_input_specs = approved_input_bindings
        .iter()
        .map(|binding| AgentFileInputSpec {
            mount_path: binding.mount_path.clone(),
            source: binding.source.clone(),
        })
        .collect::<Vec<_>>();
    let revalidation_context = file_inputs.cloned().unwrap_or_default();
    let revalidation_root = input_workspace_root.clone();
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
            let mut transaction_published = !has_managed_office_transaction;
            let command_succeeded = runtime_integrity_valid
                && result.exit_code == Some(0)
                && !result.timed_out
                && !result.cancelled
                && result.error.is_none();
            let mut inputs_still_valid = true;
            if has_managed_office_transaction && command_succeeded {
                if editor_cancellation.is_cancelled()
                    || session_cancel_flag.load(std::sync::atomic::Ordering::Acquire)
                {
                    result.cancelled = true;
                    result.error = Some(
                        "Managed Office script was cancelled before Host publication.".to_string(),
                    );
                    inputs_still_valid = false;
                } else {
                    match prepare_agent_file_input_bindings(
                        revalidation_root.as_deref(),
                        permissions,
                        &revalidation_context,
                        &approved_input_specs,
                        Some(&editor_cancellation),
                    ) {
                        Ok(current) if current == approved_input_bindings => {}
                        Ok(_) => {
                            inputs_still_valid = false;
                            fail_presentation_editor_result(
                                result,
                                "A Managed Office script, source, or asset changed before publication.",
                            );
                        }
                        Err(error) => {
                            inputs_still_valid = false;
                            fail_presentation_editor_result(
                                result,
                                &format!(
                                    "Managed Office input revalidation failed ({}): {}",
                                    error.code(),
                                    error.message()
                                ),
                            );
                        }
                    }
                }
            }
            if command_succeeded && inputs_still_valid {
                if let Some(editor) = editor_execution {
                    match super::read_presentation_editor_plan(&editor.plan_path) {
                        Ok(plan)
                            if plan.source.mount_path() == editor.contract.source_mount_path
                                && plan.destination.path()
                                    == editor.contract.plan_destination_path =>
                        {
                            match (office_engine.as_ref(), managed_office_context.as_ref()) {
                                (Some(engine), Some(context)) => {
                                    let request = OfficePresentationEditRequest {
                                        source_path: editor.contract.source_path.clone(),
                                        source_binding: editor.contract.source_binding.clone(),
                                        destination_path: editor.contract.destination_path.clone(),
                                        destination_binding: editor
                                            .contract
                                            .destination_binding
                                            .clone(),
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
                                            transaction_published = settle_presentation_editor_office_result(
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
                } else if let Some(mut script) = generic_office_script {
                    match (office_engine.as_ref(), managed_office_context.as_ref()) {
                        (Some(engine), Some(context)) => match engine.commit_managed_script_output(
                            context,
                            &mut script.staging,
                            managed_office_python.as_ref(),
                            editor_cancellation.clone(),
                            Some(Arc::clone(&session_cancel_flag)),
                        ) {
                            Ok(office_result) => {
                                transaction_published = settle_managed_office_script_result(
                                    result,
                                    office_result,
                                );
                            }
                            Err(error) => fail_presentation_editor_result(
                                result,
                                &format!(
                                    "Managed Office Host publication failed ({}): {}",
                                    error.code().stable_name(),
                                    error.message()
                                ),
                            ),
                        },
                        _ => fail_presentation_editor_result(
                            result,
                            "Managed Office Script requires the Host Office engine, but it is unavailable.",
                        ),
                    }
                }
            }
            if transaction_published {
                if let Some((observer, before)) = observer.zip(before) {
                    let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
                    result.artifact_observation = Some(observer.finish(before, after));
                }
            }
            drop(prepared_inputs);
        },
    );
    Ok(ManagedCommandSessionPreparation::Ready {
        plan: Box::new(plan),
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

fn settle_managed_office_script_result(
    command: &mut AgentCommandExecutionResult,
    office: OfficeManagedScriptOutputResult,
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
    let mut diagnostics = Vec::new();
    if let Some(error) = office.error {
        diagnostics.push(error);
    }
    if let Some(code) = office.error_code {
        diagnostics.push(format!("Host code: {code}"));
    }
    if let Some(provider) = stable_office_json_diagnostic(&office.stdout, 1_024)
        .or_else(|| bounded_diagnostic_text(&office.stderr, 1_024))
    {
        diagnostics.push(provider);
    }
    let message = if diagnostics.is_empty() {
        "Managed Office script output failed strict validation or atomic publication.".to_string()
    } else {
        diagnostics.join("; ")
    };
    fail_presentation_editor_result(command, &message);
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
        for key in [
            "type",
            "description",
            "path",
            "part",
            "code",
            "error",
            "message",
        ] {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ManagedBuilderSyntaxLanguage {
    Node,
    Python,
}

impl ManagedBuilderSyntaxLanguage {
    fn display_name(self) -> &'static str {
        match self {
            Self::Node => "Node",
            Self::Python => "Python",
        }
    }

    fn launch_name(self) -> &'static str {
        match self {
            Self::Node => "managed-node-syntax-check",
            Self::Python => "managed-python-syntax-check",
        }
    }

    fn executable_projection(self) -> &'static str {
        match self {
            Self::Node => "<managed-node>",
            Self::Python => "<managed-python>",
        }
    }
}

fn automatic_managed_builder_syntax_check(
    profile: AgentCommandRuntimeProfile,
    kind: AgentCommandRuntimeKind,
    parsed: &ManagedArtifactCommand,
    builder: Option<&ManagedArtifactBuilderCommand>,
) -> Option<ManagedBuilderSyntaxLanguage> {
    let has_declared_output = builder.is_some_and(|builder| !builder.output_paths.is_empty());
    if !has_declared_output {
        return None;
    }
    match (profile, kind) {
        (AgentCommandRuntimeProfile::Presentations, AgentCommandRuntimeKind::Node)
            if !parsed.node_syntax_check =>
        {
            Some(ManagedBuilderSyntaxLanguage::Node)
        }
        (
            AgentCommandRuntimeProfile::Documents | AgentCommandRuntimeProfile::Spreadsheets,
            AgentCommandRuntimeKind::Python,
        ) => Some(ManagedBuilderSyntaxLanguage::Python),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct PresentationEditorContract {
    source_mount_path: String,
    source_path: String,
    source_binding: crate::AgentFileInputBinding,
    destination_path: String,
    plan_destination_path: String,
    destination_binding: crate::office::OfficeManagedScriptBinding,
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

struct PreparedManagedOfficeScript {
    binding: crate::office::OfficeManagedScriptBinding,
    frozen_script_path: PathBuf,
    staging: OfficeManagedScriptStaging,
}

fn prepare_managed_office_script_execution(
    context: &OfficeExecutionContext,
    binding: &crate::office::OfficeManagedScriptBinding,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Result<PreparedManagedOfficeScript, String> {
    let inputs = prepared_inputs.ok_or_else(|| {
        "Managed Office Script is missing its frozen private input snapshot.".to_string()
    })?;
    let frozen_script_path = inputs.root().join(&binding.script_mount_path);
    let metadata = fs::symlink_metadata(&frozen_script_path)
        .map_err(|error| format!("Cannot inspect frozen Managed Office script: {error}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("Frozen Managed Office script must be a regular file.".to_string());
    }
    let frozen_script_path = frozen_script_path
        .canonicalize()
        .map_err(|error| format!("Cannot canonicalize frozen Managed Office script: {error}"))?;
    let staging = prepare_managed_script_staging(context, binding)
        .map_err(|error| format!("{}: {}", error.code().stable_name(), error.message()))?;
    Ok(PreparedManagedOfficeScript {
        binding: binding.clone(),
        frozen_script_path,
        staging,
    })
}

fn rewrite_managed_office_script_arguments(
    arguments: &[OsString],
    script: &PreparedManagedOfficeScript,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Result<Vec<OsString>, CommandExecutionError> {
    let approved_source_mount = script
        .binding
        .source_mount_path
        .as_deref()
        .map(|mount| {
            let inputs = prepared_inputs.ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Editor source snapshot is unavailable.".to_string(),
                )
            })?;
            let root = inputs.root().canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Office input root: {error}"
                ))
            })?;
            let path = root.join(mount);
            let canonical = path.canonicalize().map_err(|error| {
                CommandExecutionError::from(format!(
                    "Cannot canonicalize Managed Office Editor source snapshot: {error}"
                ))
            })?;
            if canonical == root || !canonical.starts_with(&root) {
                return Err(CommandExecutionError::from(
                    "Managed Office Editor source snapshot escaped the private input root."
                        .to_string(),
                ));
            }
            Ok(mount.to_string())
        })
        .transpose()?;
    let mut rewritten = Vec::with_capacity(arguments.len());
    let mut index = 0usize;
    let mut saw_output = false;
    let mut saw_source = false;
    while index < arguments.len() {
        let token = arguments[index].to_string_lossy();
        if token == "--output" {
            let _ = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script --output is missing its value.".to_string(),
                )
            })?;
            rewritten.push(OsString::from("--output"));
            rewritten.push(script.staging.candidate_path().as_os_str().to_os_string());
            saw_output = true;
            index += 2;
            continue;
        }
        if token.starts_with("--output=") {
            rewritten.push(OsString::from(format!(
                "--output={}",
                script.staging.candidate_path().display()
            )));
            saw_output = true;
            index += 1;
            continue;
        }
        if token == "--source" {
            let requested = arguments.get(index + 1).ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script --source is missing its value.".to_string(),
                )
            })?;
            let source = approved_source_mount.as_ref().ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script declared --source without a frozen input.".to_string(),
                )
            })?;
            if requested.to_string_lossy() != source.as_str() {
                return Err(CommandExecutionError::from(
                    "Managed Office Script --source differs from the approved logical mountPath."
                        .to_string(),
                ));
            }
            rewritten.push(OsString::from("--source"));
            // Keep the approved logical mountPath in argv. MYCOPILOT_INPUT_ROOT points at the
            // immutable private snapshot, and the fixed Word/Excel wrappers resolve beneath it.
            rewritten.push(OsString::from(source));
            saw_source = true;
            index += 2;
            continue;
        }
        if token.starts_with("--source=") {
            let source = approved_source_mount.as_ref().ok_or_else(|| {
                CommandExecutionError::from(
                    "Managed Office Script declared --source without a frozen input.".to_string(),
                )
            })?;
            if token.strip_prefix("--source=") != Some(source.as_str()) {
                return Err(CommandExecutionError::from(
                    "Managed Office Script --source differs from the approved logical mountPath."
                        .to_string(),
                ));
            }
            rewritten.push(OsString::from(format!("--source={source}")));
            saw_source = true;
            index += 1;
            continue;
        }
        rewritten.push(arguments[index].clone());
        index += 1;
    }
    if !saw_output {
        return Err(CommandExecutionError::from(
            "Managed Office Script is missing its approved --output.".to_string(),
        ));
    }
    if approved_source_mount.is_some() != saw_source {
        return Err(CommandExecutionError::from(
            "Managed Office Script source arguments differ from the approved binding.".to_string(),
        ));
    }
    Ok(rewritten)
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
        if request
            .managed_office_script
            .as_deref()
            .is_some_and(|binding| {
                binding.purpose == OfficeManagedScriptPurpose::EditPresentationPlan
            })
        {
            return Err(
                "Presentation Editor transaction is missing its frozen script input.".to_string(),
            );
        }
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
    let destination_binding = request
        .managed_office_script
        .as_deref()
        .filter(|binding| {
            binding.purpose == OfficeManagedScriptPurpose::EditPresentationPlan
                && binding.document_kind == crate::office::OfficeDocumentKind::Presentation
                && binding.script_mount_path == super::PRESENTATION_EDITOR_SCRIPT_MOUNT_PATH
                && binding.source_mount_path.as_deref() == Some(source_mount_path.as_str())
        })
        .ok_or_else(|| {
            "Presentation Editor is missing its approval-time destination binding.".to_string()
        })?
        .clone();

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
        destination_path: destination_binding.destination.logical_path.clone(),
        plan_destination_path: destination_path,
        destination_binding,
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
        AgentFileInputRef::BrowserDownload { reference, .. } => Ok(reference.clone()),
        AgentFileInputRef::SkillResource { .. } => Err(
            "Presentation Editor source must be a workspace, attachment, Artifact, browser download, or authorized external .pptx."
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

fn managed_office_script_output_redactions(
    script: &PreparedManagedOfficeScript,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
) -> super::output_capture::ProcessOutputRedactionSet {
    let mut replacements = Vec::new();
    append_private_path_spellings(
        &mut replacements,
        script.staging.private_directory(),
        "<office-candidate>",
    );
    append_private_path_spellings(
        &mut replacements,
        &script.frozen_script_path,
        "<managed-office-script>",
    );
    if let Some(inputs) = prepared_inputs {
        append_private_path_spellings(&mut replacements, inputs.root(), "$MYCOPILOT_INPUT_ROOT");
    }
    append_private_path_spellings(&mut replacements, runtime_root, "<managed-runtime>");
    append_private_path_spellings(
        &mut replacements,
        invocation.executable(),
        "<managed-runtime>",
    );
    append_private_path_spellings(&mut replacements, cwd, ".");
    super::output_capture::ProcessOutputRedactionSet::new(replacements)
}

#[derive(Debug)]
struct ManagedBuilderSyntaxCheckExecution {
    exit_code: Option<i32>,
    stdout: Option<CapturedProcessOutput>,
    stderr: Option<CapturedProcessOutput>,
    timed_out: bool,
    cancelled: bool,
    duration_ms: u64,
    error: Option<String>,
}

impl ManagedBuilderSyntaxCheckExecution {
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

fn run_managed_builder_syntax_check(
    invocation: &ArtifactRuntimeInvocation,
    script: &str,
    cwd: &Path,
    environment: &[(OsString, OsString)],
    cancellation: &AgentCancellationToken,
    redactions: super::output_capture::ProcessOutputRedactionSet,
    language: ManagedBuilderSyntaxLanguage,
) -> ManagedBuilderSyntaxCheckExecution {
    let started = Instant::now();
    if cancellation.is_cancelled() {
        return ManagedBuilderSyntaxCheckExecution {
            cancelled: true,
            ..ManagedBuilderSyntaxCheckExecution::failed(
                started,
                format!(
                    "Managed Builder {} 语法预检在启动前被取消。",
                    language.display_name()
                ),
            )
        };
    }

    let mut arguments = invocation.arguments_prefix().to_vec();
    match language {
        ManagedBuilderSyntaxLanguage::Node => {
            arguments.push(OsString::from("--check"));
            arguments.push(OsString::from(script));
        }
        ManagedBuilderSyntaxLanguage::Python => {
            arguments.push(OsString::from("-c"));
            arguments.push(OsString::from(
                "import sys; path = sys.argv[1]; compile(open(path, 'rb').read(), path, 'exec')",
            ));
            arguments.push(OsString::from(script));
        }
    }
    let launch = CommandDirectLaunchPlan::isolated(
        invocation.executable().to_path_buf(),
        arguments,
        environment.to_vec(),
    );
    let plan = CommandSpawnPlan::direct(
        language.launch_name().to_string(),
        cwd.to_path_buf(),
        None,
        Some(Duration::from_millis(
            MANAGED_BUILDER_SYNTAX_CHECK_TIMEOUT_MS,
        )),
        launch,
    );
    let mut command = plan.build();
    let mut child = match command.spawn() {
        Ok(child) => ManagedCommandChild::new(child),
        Err(_) => {
            return ManagedBuilderSyntaxCheckExecution::failed(
                started,
                format!(
                    "无法启动固定受管 {} 进行 Builder 语法预检。",
                    language.display_name()
                ),
            )
        }
    };
    let Some(stdout) = child.child_mut().stdout.take() else {
        return ManagedBuilderSyntaxCheckExecution::failed(
            started,
            format!(
                "无法捕获受管 {} 语法预检的 stdout。",
                language.display_name()
            ),
        );
    };
    let Some(stderr) = child.child_mut().stderr.take() else {
        return ManagedBuilderSyntaxCheckExecution::failed(
            started,
            format!(
                "无法捕获受管 {} 语法预检的 stderr。",
                language.display_name()
            ),
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
                error = Some(format!(
                    "等待受管 {} 语法预检失败。",
                    language.display_name()
                ));
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
        if started.elapsed() >= Duration::from_millis(MANAGED_BUILDER_SYNTAX_CHECK_TIMEOUT_MS) {
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

    let stdout_label = format!("受管 {} 语法预检 stdout", language.display_name());
    let stdout = match join_process_output_capture(stdout_reader, &stdout_label) {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| {
                format!(
                    "读取受管 {} 语法预检 stdout 失败。",
                    language.display_name()
                )
            });
            None
        }
    };
    let stderr_label = format!("受管 {} 语法预检 stderr", language.display_name());
    let stderr = match join_process_output_capture(stderr_reader, &stderr_label) {
        Ok(capture) => Some(capture),
        Err(_) => {
            error.get_or_insert_with(|| {
                format!(
                    "读取受管 {} 语法预检 stderr 失败。",
                    language.display_name()
                )
            });
            None
        }
    };
    ManagedBuilderSyntaxCheckExecution {
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

fn managed_builder_syntax_check_failure_result(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    resolution: AgentCommandRuntimeResolution,
    execution: ManagedBuilderSyntaxCheckExecution,
    language: ManagedBuilderSyntaxLanguage,
) -> AgentCommandExecutionResult {
    let language_name = language.display_name();
    let (code, recovery, message) = if execution.cancelled {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            format!("Managed Builder {language_name} 语法预检已取消；Builder 未启动。"),
        )
    } else if execution.timed_out {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            "retry",
            format!("Managed Builder {language_name} 语法预检超时；Builder 未启动。"),
        )
    } else if execution.error.is_some() || execution.exit_code.is_none() {
        (
            ERROR_BUILDER_SYNTAX_CHECK_FAILED,
            ArtifactRuntimeRecovery::RepairComponent.stable_name(),
            format!("Managed Builder {language_name} 语法预检无法完成；Builder 未启动。"),
        )
    } else {
        (
            ERROR_BUILDER_SYNTAX_INVALID,
            "changeBuilder",
            format!(
                "Managed Builder 未通过 {language_name} 语法校验；Builder 未启动。请修复脚本后重新执行。"
            ),
        )
    };
    let runtime = with_resolution_error(resolution, code, recovery, &message);
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

fn managed_builder_syntax_check_redactions(
    runtime_root: &Path,
    invocation: &ArtifactRuntimeInvocation,
    cwd: &Path,
    script: &str,
    language: ManagedBuilderSyntaxLanguage,
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
    append_private_path_spellings(
        &mut replacements,
        invocation.executable(),
        language.executable_projection(),
    );
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
const MANAGED_BUILDER_SYNTAX_CHECK_TIMEOUT_MS: u64 = 30_000;

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
mod tests;
