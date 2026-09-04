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

mod pdf_launch;
mod presentation;
mod runtime_support;

use pdf_launch::*;
pub(crate) use presentation::is_presentation_editor_direct_command;
use presentation::*;
use runtime_support::*;
pub(crate) use runtime_support::{
    infer_managed_artifact_builder_command, infer_managed_artifact_command_kind,
    infer_managed_pdf_command_kind, infer_managed_pdf_workspace_inputs,
    validate_managed_artifact_builder_output_scope, ManagedArtifactBuilderCommand,
};

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

#[cfg(test)]
mod tests;
