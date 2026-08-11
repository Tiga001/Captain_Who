use super::*;
#[cfg(test)]
use crate::artifact_runtime::{
    ArtifactRuntimeError, ArtifactRuntimeKind, ArtifactRuntimePreflight, ArtifactRuntimeRequirement,
};
use crate::artifact_runtime::{
    ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery, ARTIFACT_RUNTIME_PROVIDER_ID,
};
use crate::AgentCommandRuntimeProfile;
#[cfg(test)]
use crate::AgentCommandRuntimeResolvedPackage;
use std::ffi::{OsStr, OsString};

const MANAGED_PDF_CLI_SOURCE: &str = include_str!("pdf_runtime_cli.py");

#[cfg(target_os = "macos")]
const MANAGED_PDF_SANDBOX_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[cfg(target_os = "macos")]
const MANAGED_PDF_SANDBOX_PROFILE: &str = r#"(version 1)
(deny default)
(import "system.sb")
(deny network*)
(allow process-exec (literal (param "EXECUTABLE")))
(allow file-read-metadata file-test-existence
  (path-ancestors (param "RUNTIME_ROOT"))
  (path-ancestors (param "INPUT_ROOT"))
  (path-ancestors (param "EXECUTION_ROOT")))
(allow file-read* file-test-existence
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
    if let Some(builder) = infer_managed_artifact_builder_command(&request.command)
        .map_err(CommandExecutionError::from)?
    {
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

    let binding = match (&request.runtime_binding, &request.runtime) {
        (Some(binding), None) => binding,
        (None, Some(legacy)) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                runtime_resolution_error(
                    legacy,
                    super::COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE,
                    "reprepare",
                    "该命令使用旧版模型提供的精确依赖请求，缺少审批前冻结的运行时身份；请重新准备命令。",
                ),
                0,
            )))
        }
        (Some(binding), Some(_)) => {
            return Ok(immediate(runtime_failure_result(
                root.as_deref(),
                &cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_REQUEST,
                    "reprepare",
                    "命令同时包含旧版 runtime request 和新版 runtime binding，已拒绝执行。",
                ),
                0,
            )))
        }
        (None, None) => {
            return Err(CommandExecutionError::from(
                "ordinary command passed to managed runtime preparation".to_string(),
            ))
        }
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
    let mut parsed = match parse_frozen_managed_command(&request.command, binding) {
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
    if prepared_runtime.binding != **binding {
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
        .unwrap_or_default();
    if binding.profile == AgentCommandRuntimeProfile::Pdf {
        if let Err(message) =
            resolve_materialized_pdf_input_script(&mut parsed, prepared_inputs.as_ref())
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
    let mut arguments = prepared_runtime.invocation.arguments_prefix().to_vec();
    arguments.extend(parsed.process_arguments.iter().map(OsString::from));
    let mut environment =
        managed_environment(&prepared_runtime.invocation, prepared_inputs.as_ref());
    let hard_timeout = managed_command_hard_timeout(managed_pdf, request.timeout_ms);
    let launch = if let Some(workspace) = managed_workspace.as_ref() {
        match prepare_managed_pdf_sandbox_launch(
            &prepared_runtime.invocation,
            arguments,
            &mut environment,
            workspace,
            prepared_inputs.as_ref(),
            provider.component_root(),
        ) {
            Ok(launch) => launch,
            Err(message) => {
                return Ok(immediate(runtime_failure_result(
                    root.as_deref(),
                    &cwd,
                    request,
                    binding_resolution_error(
                        binding,
                        ERROR_UNAVAILABLE,
                        ArtifactRuntimeRecovery::InstallComponent.stable_name(),
                        &message,
                    ),
                    0,
                )))
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
    let completion_hook: super::session::CommandSessionCompletionHook = Box::new(move |result| {
        result.runtime = Some(resolution.clone());
        result.input_files = input_evidence;
        result.managed_outputs = managed_output_capture.clone();
        if let Err(error) = provider.verify_integrity() {
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
        if let Some((observer, before)) = observer.zip(before) {
            let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
            result.artifact_observation = Some(observer.finish(before, after));
        }
        drop(prepared_inputs);
    });
    Ok(ManagedCommandSessionPreparation::Ready {
        plan,
        completion_hook,
    })
}

fn managed_command_hard_timeout(
    managed_pdf: bool,
    requested_timeout_ms: Option<u64>,
) -> Option<Duration> {
    if managed_pdf {
        Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS))
    } else {
        requested_timeout_ms.map(|timeout| Duration::from_millis(timeout.clamp(1, MAX_TIMEOUT_MS)))
    }
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

fn prepare_managed_pdf_sandbox_launch(
    invocation: &ArtifactRuntimeInvocation,
    process_arguments: Vec<OsString>,
    environment: &mut Vec<(OsString, OsString)>,
    workspace: &ManagedCommandWorkspaceLease,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    runtime_root: &Path,
) -> Result<CommandDirectLaunchPlan, String> {
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (
            invocation,
            process_arguments,
            environment,
            workspace,
            prepared_inputs,
            runtime_root,
        );
        return Err(
            "Managed PDF Runtime 当前缺少受支持的 Host 文件系统沙箱，已拒绝裸进程执行。"
                .to_string(),
        );
    }

    #[cfg(target_os = "macos")]
    {
        let sandbox_metadata = std::fs::symlink_metadata(MANAGED_PDF_SANDBOX_EXECUTABLE)
            .map_err(|_| "Managed PDF Runtime 的 Host 沙箱不可用，已拒绝裸进程执行。")?;
        if sandbox_metadata.file_type().is_symlink() || !sandbox_metadata.is_file() {
            return Err("Managed PDF Runtime 的 Host 沙箱身份无效，已拒绝裸进程执行。".to_string());
        }

        let runtime_root = runtime_root
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证受管运行时边界。".to_string())?;
        let executable = invocation
            .executable()
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证受管 Python 身份。".to_string())?;
        if !executable.starts_with(&runtime_root) {
            return Err("Managed PDF Runtime 的 Python 不属于冻结的受管运行时。".to_string());
        }
        let execution_root = workspace
            .execution_root()
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证 Run 私有执行边界。".to_string())?;
        let input_root = prepared_inputs
            .map(PreparedAgentFileInputs::root)
            .unwrap_or(execution_root.as_path())
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证只读输入边界。".to_string())?;
        let home_root = workspace
            .home_root()
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证私有 HOME。".to_string())?;
        let temp_root = workspace
            .temp_root()
            .canonicalize()
            .map_err(|_| "Managed PDF Runtime 无法验证私有临时目录。".to_string())?;

        replace_managed_environment_path(environment, "HOME", &home_root);
        replace_managed_environment_path(environment, "TMPDIR", &temp_root);
        replace_managed_environment_path(environment, "TMP", &temp_root);
        replace_managed_environment_path(environment, "TEMP", &temp_root);
        replace_managed_environment_path(environment, "USERPROFILE", &home_root);

        let mut arguments = Vec::with_capacity(process_arguments.len() + 12);
        for (name, path) in [
            ("EXECUTABLE", executable.as_path()),
            ("RUNTIME_ROOT", runtime_root.as_path()),
            ("INPUT_ROOT", input_root.as_path()),
            ("EXECUTION_ROOT", execution_root.as_path()),
        ] {
            let value = path.to_str().ok_or_else(|| {
                "Managed PDF Runtime 的 Host 沙箱不支持非 UTF-8 私有路径。".to_string()
            })?;
            arguments.push(OsString::from("-D"));
            arguments.push(OsString::from(format!("{name}={value}")));
        }
        arguments.push(OsString::from("-p"));
        arguments.push(OsString::from(MANAGED_PDF_SANDBOX_PROFILE));
        arguments.push(executable.into_os_string());
        arguments.extend(process_arguments);

        Ok(CommandDirectLaunchPlan::isolated(
            PathBuf::from(MANAGED_PDF_SANDBOX_EXECUTABLE),
            arguments,
            environment.clone(),
        ))
    }
}

fn replace_managed_environment_path(
    environment: &mut Vec<(OsString, OsString)>,
    key: &str,
    path: &Path,
) {
    environment.retain(|(candidate, _)| candidate != OsStr::new(key));
    environment.push((OsString::from(key), path.as_os_str().to_os_string()));
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

const MAX_RUNTIME_PACKAGES: usize = 32;
const MAX_PACKAGE_NAME_BYTES: usize = 128;
const MAX_PACKAGE_VERSION_BYTES: usize = 64;

const ERROR_INVALID_REQUEST: &str = "artifactRuntime.invalidRequest";
const ERROR_INVALID_COMMAND: &str = "artifactRuntime.invalidCommandShape";
const ERROR_UNAVAILABLE: &str = "artifactRuntime.unavailable";
#[cfg(test)]
const ERROR_MISSING_DEPENDENCIES: &str = "artifactRuntime.missingDependencies";
const ERROR_LAUNCH_FAILED: &str = "artifactRuntime.launchFailed";
const ERROR_IO: &str = "artifactRuntime.io";
const MANAGED_PDF_HARD_TIMEOUT_MS: u64 = 300_000;

/// Executes a frozen command through the ordinary command authorization and
/// artifact-observation lifecycle, while replacing only the process launcher
/// with a revision-bound managed runtime.
///
/// Supplying no provider never falls back to PATH or the shell: runtime
/// requests receive a structured unavailable result. Non-runtime requests are
/// delegated to [`run_authorized_command`] unchanged.
pub fn run_authorized_command_with_artifact_runtime(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&ArtifactRuntimeProvider>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_authorized_command_with_artifact_runtime_and_inputs(
        workspace_root,
        request,
        permissions,
        authorization_source,
        cancellation_token,
        action_cancel_flag,
        artifact_runtime,
        None,
    )
}

/// Executes a command with the private, run-scoped authorities needed to revalidate declarative
/// file inputs. The context itself is never persisted or projected into a Tool Result.
#[allow(clippy::too_many_arguments)]
pub fn run_authorized_command_with_artifact_runtime_and_inputs(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&ArtifactRuntimeProvider>,
    file_inputs: Option<&AgentFileInputExecutionContext>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer(
        workspace_root,
        request,
        permissions,
        authorization_source,
        cancellation_token,
        action_cancel_flag,
        artifact_runtime,
        file_inputs,
        None,
    )
}

/// Executes the same authorized command path while forwarding a bounded, best-effort live preview
/// to the host. The observer is not part of authorization, audit persistence, or the final result.
#[allow(clippy::too_many_arguments)]
pub fn run_authorized_command_with_artifact_runtime_and_inputs_with_output_observer(
    workspace_root: Option<&Path>,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    authorization_source: CommandAuthorizationSource,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    artifact_runtime: Option<&ArtifactRuntimeProvider>,
    file_inputs: Option<&AgentFileInputExecutionContext>,
    output_observer: Option<ProcessOutputObserver>,
) -> Result<AgentCommandExecutionResult, CommandExecutionError> {
    if request.runtime.is_none() && request.runtime_binding.is_none() {
        if !request.inputs.is_empty() {
            return Err(CommandExecutionError::from(
                "run_command.inputs 只适用于后端已绑定运行配置的 Managed Builder。".to_string(),
            ));
        }
        return run_authorized_command_with_output_observer(
            workspace_root,
            request,
            permissions,
            authorization_source,
            cancellation_token,
            action_cancel_flag,
            output_observer,
        );
    }

    if request
        .runtime_binding
        .as_deref()
        .is_some_and(|binding| binding.profile == AgentCommandRuntimeProfile::Pdf)
    {
        return Err(CommandExecutionError::from(
            "可信 PDF 命令只能通过 Host Session 运行，以获得 Run 私有工作区。".to_string(),
        ));
    }

    let root = workspace_root
        .map(canonicalize_workspace_root)
        .transpose()?;
    let cwd = resolve_command_cwd(root.as_deref(), request.cwd.as_deref(), permissions.write)?;
    // The model-visible logical command remains the policy subject. Resolving
    // `node` or `python` to a private executable does not grant authorization.
    enforce_command_policy(
        &request.command,
        permissions,
        authorization_source,
        root.as_deref(),
        Some(&cwd),
    )?;
    if let Some(builder) = infer_managed_artifact_builder_command(&request.command)
        .map_err(CommandExecutionError::from)?
    {
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
    let observation_cancellation = cancellation_token.clone();
    let before = observer.as_ref().map(|observer| {
        observer.capture(
            AgentCommandArtifactObservationPhase::Before,
            Some(&observation_cancellation),
        )
    });

    let mut result = match (&request.runtime_binding, &request.runtime) {
        (Some(binding), None) => execute_with_frozen_profile(
            root.as_deref(),
            &cwd,
            request,
            permissions,
            cancellation_token,
            action_cancel_flag,
            artifact_runtime,
            binding,
            file_inputs,
            output_observer,
        ),
        (None, Some(legacy)) => runtime_failure_result(
            root.as_deref(),
            &cwd,
            request,
            runtime_resolution_error(
                legacy,
                super::COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE,
                "reprepare",
                "该命令使用旧版模型提供的精确依赖请求，缺少审批前冻结的运行时身份；请重新准备命令。",
            ),
            0,
        ),
        (Some(binding), Some(_)) => runtime_failure_result(
            root.as_deref(),
            &cwd,
            request,
            binding_resolution_error(
                binding,
                ERROR_INVALID_REQUEST,
                "reprepare",
                "命令同时包含旧版 runtime request 和新版 runtime binding，已拒绝执行。",
            ),
            0,
        ),
        (None, None) => unreachable!("ordinary commands returned before runtime setup"),
    };

    if let Some((observer, before)) = observer.as_ref().zip(before) {
        // A managed script can produce a valid Office artifact before it exits
        // non-zero, times out, or loses runtime integrity. Preserve that
        // evidence with the same independently bounded after snapshot.
        let after = observer.capture(AgentCommandArtifactObservationPhase::After, None);
        result.artifact_observation = Some(observer.finish(before, after));
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn execute_with_frozen_profile(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    provider: Option<&ArtifactRuntimeProvider>,
    binding: &AgentCommandRuntimeBinding,
    file_inputs: Option<&AgentFileInputExecutionContext>,
    output_observer: Option<ProcessOutputObserver>,
) -> AgentCommandExecutionResult {
    if let Err(error) = super::validate_command_runtime_binding(binding) {
        return runtime_failure_result(
            root,
            cwd,
            request,
            binding_resolution_error(binding, error.code(), error.recovery(), error.message()),
            0,
        );
    }
    if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return runtime_cancelled_result(
            root,
            cwd,
            request,
            unresolved_binding_resolution(binding),
        );
    }
    let mut parsed = match parse_frozen_managed_command(&request.command, binding) {
        Ok(parsed) => parsed,
        Err(message) => {
            return runtime_failure_result(
                root,
                cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            )
        }
    };
    if let Some(script) = parsed.script.as_deref() {
        if let Err(message) = validate_saved_script(cwd, root, permissions.read, script) {
            return runtime_failure_result(
                root,
                cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            );
        }
    }
    let Some(provider) = provider else {
        return runtime_failure_result(
            root,
            cwd,
            request,
            binding_resolution_error(
                binding,
                ERROR_UNAVAILABLE,
                ArtifactRuntimeRecovery::InstallComponent.stable_name(),
                "Managed Artifact Runtime 尚未安装或未由 host 配置。",
            ),
            0,
        );
    };

    // Resolve the trusted profile again immediately before spawn. Approval of a prepared binding
    // never becomes approval of a subsequently upgraded, replaced, or reconfigured component.
    let prepared =
        match super::prepare_command_runtime_profile(provider, binding.profile, binding.kind) {
            Ok(prepared) => prepared,
            Err(error) => {
                return runtime_failure_result(
                    root,
                    cwd,
                    request,
                    binding_resolution_error(
                        binding,
                        error.code(),
                        error.recovery(),
                        error.message(),
                    ),
                    0,
                )
            }
        };
    if prepared.binding != *binding {
        return runtime_failure_result(
            root,
            cwd,
            request,
            binding_resolution_error(
                binding,
                super::COMMAND_RUNTIME_PROFILE_ERROR_BINDING_MISMATCH,
                "reprepare",
                "Managed Artifact Runtime 在审批后发生变化；命令未启动，请重新准备并审批。",
            ),
            0,
        );
    }

    let resolution = ready_binding_resolution(binding, &prepared.invocation);
    let empty_input_context = AgentFileInputExecutionContext::default();
    let input_context = file_inputs.unwrap_or(&empty_input_context);
    let prepared_inputs = match materialize_agent_file_inputs(
        root,
        permissions,
        input_context,
        &request.inputs,
        Some(&cancellation_token),
    ) {
        Ok(prepared) => prepared,
        Err(error) => {
            return runtime_failure_result(
                root,
                cwd,
                request,
                binding_resolution_error(binding, error.code(), error.recovery(), error.message()),
                0,
            )
        }
    };
    if binding.profile == AgentCommandRuntimeProfile::Pdf {
        if let Err(message) =
            resolve_materialized_pdf_input_script(&mut parsed, prepared_inputs.as_ref())
        {
            return runtime_failure_result(
                root,
                cwd,
                request,
                binding_resolution_error(
                    binding,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            );
        }
    }
    let mut result = run_managed_process(
        root,
        cwd,
        request,
        &parsed,
        &prepared.invocation,
        resolution,
        cancellation_token,
        action_cancel_flag,
        prepared_inputs.as_ref(),
        output_observer,
    );
    if let Err(error) = provider.verify_integrity() {
        let code = format!("artifactRuntime.{}", error.code().stable_name());
        let resolution = with_resolution_error(
            result
                .runtime
                .take()
                .unwrap_or_else(|| unresolved_binding_resolution(binding)),
            &code,
            error.recovery().stable_name(),
            provider_error_message(error.code()),
        );
        result.error = resolution.message.clone();
        result.runtime = Some(resolution);
    }
    result
}

#[cfg(test)]
trait ManagedRuntimeProvider {
    fn preflight(
        &self,
        kind: ArtifactRuntimeKind,
        requirements: &[ArtifactRuntimeRequirement],
    ) -> Result<ArtifactRuntimePreflight, ArtifactRuntimeError>;

    fn verify_integrity(&self) -> Result<(), ArtifactRuntimeError>;

    fn bundle_version(&self) -> &str;

    fn bundle_revision(&self) -> &str;
}

#[cfg(test)]
impl ManagedRuntimeProvider for ArtifactRuntimeProvider {
    fn preflight(
        &self,
        kind: ArtifactRuntimeKind,
        requirements: &[ArtifactRuntimeRequirement],
    ) -> Result<ArtifactRuntimePreflight, ArtifactRuntimeError> {
        ArtifactRuntimeProvider::preflight(self, kind, requirements)
    }

    fn verify_integrity(&self) -> Result<(), ArtifactRuntimeError> {
        ArtifactRuntimeProvider::verify_integrity(self)
    }

    fn bundle_version(&self) -> &str {
        ArtifactRuntimeProvider::bundle_version(self)
    }

    fn bundle_revision(&self) -> &str {
        ArtifactRuntimeProvider::bundle_revision(self)
    }
}

#[cfg(test)]
fn execute_with_provider<P: ManagedRuntimeProvider>(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    permissions: AgentPermissions,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    provider: &P,
) -> AgentCommandExecutionResult {
    let runtime = request.runtime.as_ref().expect("runtime checked by caller");
    if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return runtime_cancelled_result(
            root,
            cwd,
            request,
            unresolved_runtime_resolution(runtime, Some(provider)),
        );
    }
    if let Err(message) = validate_command_runtime_request(runtime) {
        return runtime_failure_result(
            root,
            cwd,
            request,
            runtime_resolution_error(
                runtime,
                ERROR_INVALID_REQUEST,
                ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                &message,
            ),
            0,
        );
    }
    let parsed = match parse_managed_artifact_command(&request.command, runtime.kind) {
        Ok(parsed) => parsed,
        Err(message) => {
            return runtime_failure_result(
                root,
                cwd,
                request,
                runtime_resolution_error(
                    runtime,
                    ERROR_INVALID_COMMAND,
                    ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                    &message,
                ),
                0,
            );
        }
    };
    if let Err(message) = validate_saved_script(
        cwd,
        root,
        permissions.read,
        parsed
            .script
            .as_deref()
            .expect("legacy managed commands always carry a saved script"),
    ) {
        return runtime_failure_result(
            root,
            cwd,
            request,
            runtime_resolution_error(
                runtime,
                ERROR_INVALID_COMMAND,
                ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                &message,
            ),
            0,
        );
    }

    let kind = artifact_runtime_kind(runtime.kind);
    let requirements = runtime
        .required_packages
        .iter()
        .map(|package| ArtifactRuntimeRequirement::exact(&package.name, &package.version))
        .collect::<Result<Vec<_>, _>>();
    let requirements = match requirements {
        Ok(requirements) => requirements,
        Err(error) => {
            return runtime_failure_result(
                root,
                cwd,
                request,
                resolution_from_provider_error(runtime, &error),
                0,
            );
        }
    };

    let invocation = match provider.preflight(kind, &requirements) {
        Ok(ArtifactRuntimePreflight::Ready(invocation)) => invocation,
        Ok(ArtifactRuntimePreflight::MissingDependencies { status, missing }) => {
            let missing = missing
                .iter()
                .map(|package| match package.version() {
                    Some(version) => format!("{}@{version}", package.name()),
                    None => package.name().to_string(),
                })
                .collect::<Vec<_>>()
                .join(", ");
            let resolved = resolved_packages(runtime, &status.dependencies);
            let mut resolution = runtime_resolution_error(
                runtime,
                ERROR_MISSING_DEPENDENCIES,
                ArtifactRuntimeRecovery::ChangeRequest.stable_name(),
                &format!("Managed Artifact Runtime 缺少精确依赖：{missing}。"),
            );
            resolution.runtime_version = status.version;
            resolution.runtime_fingerprint = status.runtime_fingerprint;
            resolution.resolved_packages = resolved;
            return runtime_failure_result(root, cwd, request, resolution, 0);
        }
        Err(error) => {
            return runtime_failure_result(
                root,
                cwd,
                request,
                resolution_from_provider_error(runtime, &error),
                0,
            );
        }
    };

    let resolution = ready_resolution(runtime, &invocation);
    let mut result = run_managed_process(
        root,
        cwd,
        request,
        &parsed,
        &invocation,
        resolution,
        cancellation_token,
        action_cancel_flag,
        None,
        None,
    );

    // Treat postflight integrity as part of command success. The process's
    // exit code and output remain intact for diagnosis, but success cannot be
    // claimed after executing from a component whose receipt no longer holds.
    if let Err(error) = provider.verify_integrity() {
        let code = format!("artifactRuntime.{}", error.code().stable_name());
        let resolution = with_resolution_error(
            result
                .runtime
                .take()
                .unwrap_or_else(|| unresolved_runtime_resolution(runtime, Some(provider))),
            &code,
            error.recovery().stable_name(),
            provider_error_message(error.code()),
        );
        result.error = resolution.message.clone();
        result.runtime = Some(resolution);
    }
    result
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedArtifactCommand {
    script: Option<String>,
    input_script_mount: Option<String>,
    process_arguments: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedArtifactBuilderCommand {
    pub kind: AgentCommandRuntimeKind,
    pub script: String,
    pub output_paths: Vec<String>,
}

pub(crate) fn validate_managed_artifact_command_shape(
    command: &str,
    runtime: &AgentCommandRuntimeRequest,
) -> Result<(), String> {
    parse_managed_artifact_command(command, runtime.kind).map(|_| ())
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
                "runtimeProfile 只支持以 `node <script>.mjs`、`python <script>.py` 或 `python3 <script>.py` 开始的直接脚本调用。"
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
        Err(error) if looks_like_office_builder_intent(command) => return Err(error),
        Err(_) => return Ok(None),
    };
    let kind = match tokens[0].as_str() {
        "node" => AgentCommandRuntimeKind::Node,
        "python" | "python3" => AgentCommandRuntimeKind::Python,
        _ => return Ok(None),
    };
    let parsed = match parse_managed_artifact_command(command, kind) {
        Ok(parsed) => parsed,
        Err(error) if looks_like_office_builder_intent(command) => return Err(error),
        Err(_) => return Ok(None),
    };

    let mut output_paths = Vec::new();
    let mut saw_output = false;
    let mut index = 2;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "--" {
            break;
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
        output_paths,
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
    let script = &tokens[1];
    let expected_extension = match kind {
        AgentCommandRuntimeKind::Node => "mjs",
        AgentCommandRuntimeKind::Python => "py",
    };
    if Path::new(script).extension().and_then(OsStr::to_str) != Some(expected_extension) {
        return Err(format!(
            "Managed Artifact Runtime 的 {} 调用必须以已保存的 .{expected_extension} 脚本作为第二个 token。",
            runtime_kind_name(kind)
        ));
    }

    Ok(ManagedArtifactCommand {
        script: Some(script.clone()),
        input_script_mount: None,
        process_arguments: tokens[1..].iter().map(OsString::from).collect(),
    })
}

fn parse_frozen_managed_command(
    command: &str,
    binding: &AgentCommandRuntimeBinding,
) -> Result<ManagedArtifactCommand, String> {
    if binding.profile == AgentCommandRuntimeProfile::Pdf {
        return parse_managed_pdf_command(command)?.ok_or_else(|| {
            "Managed PDF Runtime 只接受一个直接调用：pdfinfo、pdftotext、pdftoppm、python -c，或来自 run_command.inputs 的 Python 脚本。临时多行 Python 请放在带引号的 -c 代码参数内。"
                .to_string()
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
    parse_managed_pdf_command(command)
        .map(|command| command.map(|_| AgentCommandRuntimeKind::Python))
}

/// Returns a workspace-relative PDF path that can be frozen implicitly for the deliberately
/// small, host-owned PDF CLI surface.
///
/// This is an ergonomic convenience for commands such as `pdfinfo "manual.pdf"` after the model
/// has already discovered that file in the selected workspace. It never resolves attachments,
/// Artifacts, external paths, environment variables, Python code, or output paths. The caller
/// must still prove that the returned file exists in the selected workspace and freeze its bytes
/// through the normal [`crate::file_input`] approval-boundary path.
pub(crate) fn infer_managed_pdf_workspace_input(command: &str) -> Result<Option<String>, String> {
    let lexed = lex_command(command).map_err(|error| {
        format!(
            "Managed PDF Runtime 命令语法无效（{}）：{}",
            error.code, error.reason
        )
    })?;
    if lexed.segments.len() != 1
        || !lexed.embedded_commands.is_empty()
        || lexed.segments[0].has_write_redirection
        || !lexed.segments[0].input_redirections.is_empty()
        || lexed.segments[0].has_heredoc
    {
        return Ok(None);
    }
    let tokens = &lexed.segments[0].tokens;
    let candidate = match tokens.first().map(String::as_str) {
        Some("pdfinfo") if tokens.len() == 2 => tokens.get(1).map(String::as_str),
        Some("pdftotext") => {
            managed_pdf_positional_arguments(&tokens[1..], &["-f", "-l"], &["-layout", "-nopgbrk"])
                .filter(|arguments| matches!(arguments.len(), 1 | 2))
                .and_then(|arguments| arguments.first().copied())
        }
        Some("pdftoppm") => managed_pdf_positional_arguments(
            &tokens[1..],
            &["-f", "-l", "-r"],
            &["-png", "-jpeg", "-jpg", "-singlefile"],
        )
        .filter(|arguments| arguments.len() == 2)
        .and_then(|arguments| arguments.first().copied()),
        _ => None,
    };
    candidate
        .map(normalize_managed_pdf_workspace_input)
        .transpose()
        .map(Option::flatten)
}

fn managed_pdf_positional_arguments<'a>(
    arguments: &'a [String],
    valued_options: &[&str],
    switches: &[&str],
) -> Option<Vec<&'a str>> {
    let mut positional = Vec::new();
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].as_str();
        if valued_options.contains(&argument) {
            arguments.get(index + 1)?;
            index += 2;
        } else if switches.contains(&argument) {
            index += 1;
        } else if argument.starts_with('-') && argument != "-" {
            return None;
        } else {
            positional.push(argument);
            index += 1;
        }
    }
    Some(positional)
}

fn normalize_managed_pdf_workspace_input(value: &str) -> Result<Option<String>, String> {
    if value.is_empty()
        || value.starts_with('@')
        || value.starts_with('~')
        || value.contains('$')
        || value.contains("://")
    {
        return Ok(None);
    }
    let path = Path::new(value);
    if path.is_absolute()
        || !path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| extension.eq_ignore_ascii_case("pdf"))
    {
        return Ok(None);
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part.to_str().ok_or_else(|| {
                    "Managed PDF Runtime 的 workspace PDF 路径必须是有效 UTF-8。".to_string()
                })?;
                parts.push(part);
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => {
                return Err(
                    "Managed PDF Runtime 的 workspace PDF 路径不能包含 `..`、根目录或平台前缀。"
                        .to_string(),
                );
            }
        }
    }
    if parts.is_empty() {
        return Ok(None);
    }
    Ok(Some(parts.join("/")))
}

fn parse_managed_pdf_command(command: &str) -> Result<Option<ManagedArtifactCommand>, String> {
    let lexed = lex_command(command).map_err(|error| {
        format!(
            "Managed PDF Runtime 命令语法无效（{}）：{}",
            error.code, error.reason
        )
    })?;
    let first_tokens = lexed.segments.first().map(|segment| &segment.tokens);
    let Some(program) = first_tokens
        .and_then(|tokens| tokens.first())
        .map(String::as_str)
    else {
        return Ok(None);
    };
    let python_mode_is_pdf = matches!(program, "python" | "python3")
        && first_tokens.is_some_and(|tokens| {
            tokens.get(1).map(String::as_str) == Some("-c")
                || tokens
                    .get(1)
                    .is_some_and(|script| pdf_input_script_mount(script).is_some())
        });
    if !matches!(program, "pdfinfo" | "pdftotext" | "pdftoppm") && !python_mode_is_pdf {
        return Ok(None);
    }
    if lexed.segments.len() != 1
        || !lexed.embedded_commands.is_empty()
        || lexed.segments[0].has_write_redirection
        || !lexed.segments[0].input_redirections.is_empty()
    {
        return Err(
            "Managed PDF Runtime 每次只接受一个无重定向、无管道、无复合运算符、无 heredoc、无命令替换的直接调用；临时多行 Python 请放在带引号的 `python -c` 代码参数内。".to_string(),
        );
    }
    let tokens = &lexed.segments[0].tokens;
    match program {
        "pdfinfo" | "pdftotext" | "pdftoppm" => {
            if tokens.len() < 2 {
                return Err(format!("Managed PDF Runtime 的 `{program}` 调用缺少参数。"));
            }
            let mut process_arguments = vec![
                OsString::from("-c"),
                OsString::from(MANAGED_PDF_CLI_SOURCE),
                OsString::from(program),
            ];
            process_arguments.extend(tokens[1..].iter().map(OsString::from));
            Ok(Some(ManagedArtifactCommand {
                script: None,
                input_script_mount: None,
                process_arguments,
            }))
        }
        "python" | "python3" if tokens.get(1).map(String::as_str) == Some("-c") => {
            if tokens.get(2).is_none_or(String::is_empty) {
                return Err("Managed PDF Runtime 的 python -c 调用缺少代码。".to_string());
            }
            Ok(Some(ManagedArtifactCommand {
                script: None,
                input_script_mount: None,
                process_arguments: tokens[1..].iter().map(OsString::from).collect(),
            }))
        }
        "python" | "python3" => {
            let script = tokens
                .get(1)
                .and_then(|script| pdf_input_script_mount(script))
                .transpose()?
                .ok_or_else(|| {
                    "Managed PDF Runtime 的保存脚本必须来自 `$MYCOPILOT_INPUT_ROOT/<mount>.py`。"
                        .to_string()
                })?;
            Ok(Some(ManagedArtifactCommand {
                script: None,
                input_script_mount: Some(script),
                process_arguments: tokens[1..].iter().map(OsString::from).collect(),
            }))
        }
        _ => Ok(None),
    }
}

fn pdf_input_script_mount(token: &str) -> Option<Result<String, String>> {
    let relative = pdf_input_mount(token)?;
    Some(relative.and_then(|relative| validate_pdf_input_script_mount(&relative)))
}

fn pdf_input_mount(token: &str) -> Option<Result<String, String>> {
    let relative = token
        .strip_prefix("$MYCOPILOT_INPUT_ROOT/")
        .or_else(|| token.strip_prefix("${MYCOPILOT_INPUT_ROOT}/"))?;
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Some(Err(
            "Managed PDF Runtime 的 input root 路径必须是不含 `..` 的相对路径。".to_string(),
        ));
    }
    Some(Ok(relative.to_string()))
}

fn validate_pdf_input_script_mount(relative: &str) -> Result<String, String> {
    let path = Path::new(relative);
    if relative.is_empty()
        || path.is_absolute()
        || path.extension().and_then(OsStr::to_str) != Some("py")
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return Err(
            "Managed PDF Runtime 的输入脚本必须是 input root 内无 `..` 的相对 .py 路径。"
                .to_string(),
        );
    }
    Ok(relative.to_string())
}

fn resolve_materialized_pdf_input_script(
    parsed: &mut ManagedArtifactCommand,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) -> Result<(), String> {
    let prepared = match (parsed.input_script_mount.as_deref(), prepared_inputs) {
        (Some(_), None) => {
            return Err(
                "Managed PDF Runtime 的保存脚本必须同时声明为 run_command.inputs。".to_string(),
            );
        }
        (_, Some(prepared)) => prepared,
        (None, None) => return Ok(()),
    };
    if let Some(mount_path) = parsed.input_script_mount.as_deref() {
        if !prepared
            .evidence()
            .iter()
            .any(|evidence| evidence.mount_path == mount_path)
        {
            return Err(format!(
                "Managed PDF Runtime 输入中不存在脚本 mountPath `{mount_path}`。"
            ));
        }
    }

    let input_root = prepared
        .root()
        .canonicalize()
        .map_err(|error| format!("无法规范化受管命令输入根：{error}"))?;
    for argument in &mut parsed.process_arguments {
        let Some(token) = argument.to_str() else {
            continue;
        };
        let explicit_input_root = pdf_input_mount(token);
        let requires_declared_mount = explicit_input_root.is_some();
        let relative = match explicit_input_root {
            Some(relative) => Some(relative?),
            None => normalize_managed_pdf_workspace_input(token)?,
        };
        let Some(relative) = relative else {
            continue;
        };
        let declared = prepared
            .evidence()
            .iter()
            .any(|evidence| evidence.mount_path == relative);
        if !declared && requires_declared_mount {
            return Err(format!(
                "Managed PDF Runtime 输入中不存在 mountPath `{relative}`。"
            ));
        }
        if !declared {
            continue;
        }
        let path = input_root.join(&relative);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("无法读取 Managed PDF Runtime 输入文件：{error}"))?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("Managed PDF Runtime 输入必须是非符号链接普通文件。".to_string());
        }
        let path = path
            .canonicalize()
            .map_err(|error| format!("无法规范化 Managed PDF Runtime 输入文件：{error}"))?;
        if !path.starts_with(&input_root) {
            return Err("Managed PDF Runtime 输入文件逃离了冻结输入根。".to_string());
        }
        *argument = path.into_os_string();
    }
    Ok(())
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
    let script = &tokens[1];
    if script.starts_with('-') {
        return Err(
            "Managed Artifact Runtime 禁止 -e、-c、-m 及其他内联代码或启动选项。".to_string(),
        );
    }
    Ok(tokens.clone())
}

pub(crate) fn validate_command_runtime_request(
    runtime: &AgentCommandRuntimeRequest,
) -> Result<(), String> {
    if runtime.required_packages.len() > MAX_RUNTIME_PACKAGES {
        return Err(format!(
            "run_command.runtime.requiredPackages 最多允许 {MAX_RUNTIME_PACKAGES} 项。"
        ));
    }
    let mut names = std::collections::BTreeSet::new();
    for package in &runtime.required_packages {
        validate_package_name(&package.name)?;
        validate_exact_package_version(&package.version)?;
        let normalized = normalize_package_name(runtime.kind, &package.name);
        if !names.insert(normalized) {
            return Err(format!(
                "run_command.runtime.requiredPackages 包含重复依赖 `{}`。",
                package.name
            ));
        }
    }
    Ok(())
}

fn validate_package_name(name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > MAX_PACKAGE_NAME_BYTES
        || name.trim() != name
        || name.chars().any(char::is_control)
    {
        return Err(format!(
            "runtime 依赖名必须是 1..={MAX_PACKAGE_NAME_BYTES} 字节的无控制字符精确名称。"
        ));
    }
    let valid = if let Some(scoped) = name.strip_prefix('@') {
        let mut parts = scoped.split('/');
        matches!((parts.next(), parts.next(), parts.next()), (Some(scope), Some(package), None)
            if valid_package_name_part(scope) && valid_package_name_part(package))
    } else {
        valid_package_name_part(name)
    };
    if !valid {
        return Err(format!(
            "runtime 依赖名 `{name}` 包含不支持的字符或路径结构。"
        ));
    }
    Ok(())
}

fn valid_package_name_part(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('.')
        && part
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn validate_exact_package_version(version: &str) -> Result<(), String> {
    if version.is_empty()
        || version.len() > MAX_PACKAGE_VERSION_BYTES
        || version.trim() != version
        || !version.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+' | b'_' | b'!')
        })
        || version
            .bytes()
            .any(|byte| matches!(byte, b'*' | b'^' | b'~' | b'<' | b'>' | b'=' | b','))
    {
        return Err(format!(
            "runtime 依赖版本 `{version}` 必须是 1..={MAX_PACKAGE_VERSION_BYTES} 字节的精确版本，不能使用范围、通配符、URL 或路径。"
        ));
    }
    Ok(())
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

#[allow(clippy::too_many_arguments)]
fn run_managed_process(
    root: Option<&Path>,
    cwd: &Path,
    request: &AgentCommandRequest,
    parsed: &ManagedArtifactCommand,
    invocation: &ArtifactRuntimeInvocation,
    resolution: AgentCommandRuntimeResolution,
    cancellation_token: AgentCancellationToken,
    action_cancel_flag: Option<Arc<AtomicBool>>,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
    output_observer: Option<ProcessOutputObserver>,
) -> AgentCommandExecutionResult {
    if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return AgentCommandExecutionResult {
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
            runtime: Some(resolution),
            managed_outputs: None,
            authoritative_archive_ref: None,
            history_open: None,
        };
    }

    let timeout_ms = request
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let started = Instant::now();
    let mut command = Command::new(invocation.executable());
    command.args(invocation.arguments_prefix());
    command.args(&parsed.process_arguments);
    configure_command_process_group(&mut command);
    configure_managed_environment(&mut command, invocation, prepared_inputs);
    let child = command
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(_) => {
            let runtime = with_resolution_error(
                resolution,
                ERROR_LAUNCH_FAILED,
                ArtifactRuntimeRecovery::Retry.stable_name(),
                "Managed Artifact Runtime 进程无法启动。",
            );
            return runtime_failure_result(
                root,
                cwd,
                request,
                runtime,
                started.elapsed().as_millis() as u64,
            );
        }
    };

    let Some(stdout) = child.stdout.take() else {
        terminate_command_process_group(&mut child);
        let _ = child.wait();
        let runtime = with_resolution_error(
            resolution,
            ERROR_IO,
            ArtifactRuntimeRecovery::Retry.stable_name(),
            "无法读取 Managed Artifact Runtime stdout。",
        );
        return runtime_failure_result(
            root,
            cwd,
            request,
            runtime,
            started.elapsed().as_millis() as u64,
        );
    };
    let Some(stderr) = child.stderr.take() else {
        terminate_command_process_group(&mut child);
        let _ = child.wait();
        let runtime = with_resolution_error(
            resolution,
            ERROR_IO,
            ArtifactRuntimeRecovery::Retry.stable_name(),
            "无法读取 Managed Artifact Runtime stderr。",
        );
        return runtime_failure_result(
            root,
            cwd,
            request,
            runtime,
            started.elapsed().as_millis() as u64,
        );
    };
    let capture_policy = ProcessOutputCapturePolicy::process_default();
    let capture_budget = ProcessOutputCaptureBudget::new(capture_policy.max_capture_bytes());
    let stdout_reader = spawn_process_output_capture_with_observer(
        stdout,
        capture_budget.clone(),
        capture_policy,
        Some(AgentCommandOutputStream::Stdout),
        output_observer.clone(),
    );
    let stderr_reader = spawn_process_output_capture_with_observer(
        stderr,
        capture_budget,
        capture_policy,
        Some(AgentCommandOutputStream::Stderr),
        output_observer,
    );

    let deadline = Duration::from_millis(timeout_ms);
    let mut timed_out = false;
    let mut cancelled = false;
    let exit_status = loop {
        if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
            cancelled = true;
            terminate_command_process_group(&mut child);
            break child.wait();
        }
        match try_wait_command_process_group(&mut child) {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if started.elapsed() >= deadline => {
                timed_out = true;
                terminate_command_process_group(&mut child);
                break child.wait();
            }
            Ok(None) => thread::sleep(Duration::from_millis(50)),
            Err(error) => break Err(error),
        }
    };

    let stdout = join_process_output_capture(stdout_reader, "stdout");
    let stderr = join_process_output_capture(stderr_reader, "stderr");
    match (exit_status, stdout, stderr) {
        (Ok(exit_status), Ok(stdout_capture), Ok(stderr_capture)) => {
            let output_capture =
                ProcessOutputCaptureMetadata::from_streams(&stdout_capture, &stderr_capture);
            AgentCommandExecutionResult {
                outputs: Vec::new(),
                command: request.command.clone(),
                cwd: relative_cwd(root, cwd),
                exit_code: exit_status.code(),
                stdout: stdout_capture.preview().to_string(),
                stderr: stderr_capture.preview().to_string(),
                timed_out,
                cancelled,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_truncated: stdout_capture.preview_truncated(),
                stderr_truncated: stderr_capture.preview_truncated(),
                output_capture,
                stdout_spool: stdout_capture.spool(),
                stderr_spool: stderr_capture.spool(),
                error: None,
                policy_evaluation: None,
                artifact_observation: None,
                input_files: prepared_inputs
                    .map(|prepared| prepared.evidence().to_vec())
                    .unwrap_or_default(),
                runtime: Some(resolution),
                managed_outputs: None,
                authoritative_archive_ref: None,
                history_open: None,
            }
        }
        _ => {
            let runtime = with_resolution_error(
                resolution,
                ERROR_IO,
                ArtifactRuntimeRecovery::Retry.stable_name(),
                "Managed Artifact Runtime 进程或输出收集失败。",
            );
            runtime_failure_result(
                root,
                cwd,
                request,
                runtime,
                started.elapsed().as_millis() as u64,
            )
        }
    }
}

fn configure_managed_environment(
    command: &mut Command,
    invocation: &ArtifactRuntimeInvocation,
    prepared_inputs: Option<&PreparedAgentFileInputs>,
) {
    command.env_clear();
    // Deliberately exclude PATH, NODE_OPTIONS, user Python injection variables,
    // and dynamic-loader overrides. Only inert locale/temp/system context is
    // inherited; provider-owned variables are added from the verified receipt.
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
            command.env(key, value);
        }
    }
    command.envs(invocation.environment());
    command.env("TERM", "dumb");
    command.env("CI", "1");
    if let Some(prepared_inputs) = prepared_inputs {
        command.env(AGENT_FILE_INPUT_ROOT_ENV, prepared_inputs.root());
    }
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

#[cfg(test)]
fn ready_resolution(
    runtime: &AgentCommandRuntimeRequest,
    invocation: &ArtifactRuntimeInvocation,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: invocation.provider_id().to_string(),
        profile: None,
        profile_revision: None,
        bundle_version: Some(invocation.bundle_version().to_string()),
        bundle_revision: Some(invocation.bundle_revision().to_string()),
        kind: runtime.kind,
        runtime_version: Some(invocation.version().to_string()),
        runtime_fingerprint: Some(invocation.runtime_fingerprint().to_string()),
        resolved_packages: runtime
            .required_packages
            .iter()
            .map(|package| AgentCommandRuntimeResolvedPackage {
                name: package.name.clone(),
                version: package.version.clone(),
            })
            .collect(),
        error_code: None,
        recovery: None,
        message: None,
    }
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

#[cfg(test)]
fn resolved_packages(
    runtime: &AgentCommandRuntimeRequest,
    available: &[crate::artifact_runtime::ArtifactRuntimeDependency],
) -> Vec<AgentCommandRuntimeResolvedPackage> {
    runtime
        .required_packages
        .iter()
        .filter_map(|required| {
            available
                .iter()
                .find(|available| {
                    normalize_package_name(runtime.kind, &available.name)
                        == normalize_package_name(runtime.kind, &required.name)
                        && available.version == required.version
                })
                .map(|available| AgentCommandRuntimeResolvedPackage {
                    name: available.name.clone(),
                    version: available.version.clone(),
                })
        })
        .collect()
}

fn runtime_resolution_error(
    runtime: &AgentCommandRuntimeRequest,
    code: &str,
    recovery: &str,
    message: &str,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        profile: None,
        profile_revision: None,
        bundle_version: None,
        bundle_revision: None,
        kind: runtime.kind,
        runtime_version: None,
        runtime_fingerprint: None,
        resolved_packages: Vec::new(),
        error_code: Some(code.to_string()),
        recovery: Some(recovery.to_string()),
        message: Some(message.to_string()),
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

#[cfg(test)]
fn resolution_from_provider_error(
    runtime: &AgentCommandRuntimeRequest,
    error: &ArtifactRuntimeError,
) -> AgentCommandRuntimeResolution {
    runtime_resolution_error(
        runtime,
        &format!("artifactRuntime.{}", error.code().stable_name()),
        error.recovery().stable_name(),
        provider_error_message(error.code()),
    )
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

#[cfg(test)]
fn unresolved_runtime_resolution(
    runtime: &AgentCommandRuntimeRequest,
    provider: Option<&dyn ManagedRuntimeProvider>,
) -> AgentCommandRuntimeResolution {
    AgentCommandRuntimeResolution {
        schema_version: AGENT_COMMAND_RUNTIME_RESOLUTION_SCHEMA_VERSION,
        provider_id: ARTIFACT_RUNTIME_PROVIDER_ID.to_string(),
        profile: None,
        profile_revision: None,
        bundle_version: provider.map(|provider| provider.bundle_version().to_string()),
        bundle_revision: provider.map(|provider| provider.bundle_revision().to_string()),
        kind: runtime.kind,
        runtime_version: None,
        runtime_fingerprint: None,
        resolved_packages: Vec::new(),
        error_code: None,
        recovery: None,
        message: None,
    }
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

#[cfg(test)]
fn artifact_runtime_kind(kind: AgentCommandRuntimeKind) -> ArtifactRuntimeKind {
    match kind {
        AgentCommandRuntimeKind::Node => ArtifactRuntimeKind::Node,
        AgentCommandRuntimeKind::Python => ArtifactRuntimeKind::Python,
    }
}

fn runtime_kind_name(kind: AgentCommandRuntimeKind) -> &'static str {
    match kind {
        AgentCommandRuntimeKind::Node => "node",
        AgentCommandRuntimeKind::Python => "python",
    }
}

fn normalize_package_name(kind: AgentCommandRuntimeKind, name: &str) -> String {
    match kind {
        AgentCommandRuntimeKind::Node => name.to_ascii_lowercase(),
        AgentCommandRuntimeKind::Python => name.to_ascii_lowercase().replace('_', "-"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact_runtime::{ArtifactRuntimeAvailability, ArtifactRuntimeStatus};
    use crate::{AgentCommandRuntimePackageRequirement, AgentCommandRuntimeProvider};
    use std::collections::BTreeMap;
    use tempfile::TempDir;

    fn runtime(kind: AgentCommandRuntimeKind) -> AgentCommandRuntimeRequest {
        AgentCommandRuntimeRequest {
            provider: AgentCommandRuntimeProvider::ManagedArtifact,
            kind,
            required_packages: vec![AgentCommandRuntimePackageRequirement {
                name: match kind {
                    AgentCommandRuntimeKind::Node => "exceljs".to_string(),
                    AgentCommandRuntimeKind::Python => "openpyxl".to_string(),
                },
                version: match kind {
                    AgentCommandRuntimeKind::Node => "4.4.0".to_string(),
                    AgentCommandRuntimeKind::Python => "3.1.5".to_string(),
                },
            }],
        }
    }

    #[test]
    fn validates_only_direct_saved_script_invocations() {
        assert!(validate_managed_artifact_command_shape(
            "node scripts/build.mjs --output out.xlsx",
            &runtime(AgentCommandRuntimeKind::Node)
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
                validate_managed_artifact_command_shape(
                    command,
                    &runtime(AgentCommandRuntimeKind::Node)
                )
                .is_err(),
                "unexpectedly accepted {command}"
            );
        }
        assert!(validate_managed_artifact_command_shape(
            "python3 scripts/build.py",
            &runtime(AgentCommandRuntimeKind::Python)
        )
        .is_ok());
        assert!(validate_managed_artifact_command_shape(
            "python -m build",
            &runtime(AgentCommandRuntimeKind::Python)
        )
        .is_err());
    }

    #[test]
    fn managed_pdf_parser_is_narrow_and_does_not_claim_unrelated_commands() {
        let pdfinfo = parse_managed_pdf_command("pdfinfo '$MYCOPILOT_INPUT_ROOT/manual.pdf'")
            .unwrap()
            .unwrap();
        assert!(pdfinfo.script.is_none());
        assert!(pdfinfo.input_script_mount.is_none());

        let script = parse_managed_pdf_command(
            "python '$MYCOPILOT_INPUT_ROOT/scripts/edit.py' --mode verify",
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            script.input_script_mount.as_deref(),
            Some("scripts/edit.py")
        );
        assert_eq!(
            script.process_arguments[0],
            OsString::from("$MYCOPILOT_INPUT_ROOT/scripts/edit.py")
        );

        assert!(
            parse_managed_pdf_command("python scripts/office.py --output report.docx")
                .unwrap()
                .is_none()
        );
        assert!(parse_managed_pdf_command("echo value | sed s/a/b/")
            .unwrap()
            .is_none());
        assert!(parse_managed_pdf_command("pdfinfo input.pdf | tee metadata.txt").is_err());
        assert!(parse_managed_pdf_command("python '$MYCOPILOT_INPUT_ROOT/../escape.py'").is_err());

        let multiline =
            parse_managed_pdf_command("python -c 'from pypdf import PdfReader\nprint(PdfReader)'")
                .unwrap()
                .unwrap();
        assert_eq!(
            multiline.process_arguments[1],
            OsString::from("from pypdf import PdfReader\nprint(PdfReader)")
        );
        let compound =
            parse_managed_pdf_command("python -c 'print(1)'\npython -c 'print(2)'").unwrap_err();
        assert!(compound.contains("一个无重定向"));
        assert!(compound.contains("python -c"));
    }

    #[test]
    fn managed_pdf_finds_only_the_native_workspace_pdf_input_argument() {
        let aspen = "AspenPolymer-Unit Operations and Reaction Models.pdf";
        assert_eq!(
            infer_managed_pdf_workspace_input(&format!("pdfinfo \"{aspen}\""))
                .unwrap()
                .as_deref(),
            Some(aspen)
        );
        assert_eq!(
            infer_managed_pdf_workspace_input(
                "pdftotext -f 307 -l 308 -layout manuals/aspen.pdf extracted.txt"
            )
            .unwrap()
            .as_deref(),
            Some("manuals/aspen.pdf")
        );
        assert_eq!(
            infer_managed_pdf_workspace_input(
                "pdftoppm -f 307 -l 308 -r 144 -png manuals/aspen.PDF outputs/aspen-page"
            )
            .unwrap()
            .as_deref(),
            Some("manuals/aspen.PDF")
        );
        for command in [
            "pdfinfo '$MYCOPILOT_INPUT_ROOT/manual.pdf'",
            "pdfinfo '@attachments/pdf-1/manual.pdf'",
            "pdfinfo 'artifact://sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
            "python -c 'from pypdf import PdfReader; PdfReader(\"manual.pdf\")'",
        ] {
            assert_eq!(infer_managed_pdf_workspace_input(command).unwrap(), None);
        }
        assert!(infer_managed_pdf_workspace_input("pdfinfo ../outside.pdf").is_err());
    }

    #[test]
    fn managed_pdf_uses_one_host_owned_hard_timeout() {
        let expected = Some(Duration::from_millis(MANAGED_PDF_HARD_TIMEOUT_MS));
        assert_eq!(managed_command_hard_timeout(true, None), expected);
        assert_eq!(managed_command_hard_timeout(true, Some(1)), expected);
        assert_eq!(managed_command_hard_timeout(false, None), None);
        assert_eq!(
            managed_command_hard_timeout(false, Some(MAX_TIMEOUT_MS + 1)),
            Some(Duration::from_millis(MAX_TIMEOUT_MS))
        );
    }

    #[test]
    fn managed_pdf_saved_script_is_resolved_only_from_the_frozen_input_root() {
        let workspace = TempDir::new().unwrap();
        fs::create_dir(workspace.path().join("scripts")).unwrap();
        let script_bytes = b"print('ok')\n";
        fs::write(workspace.path().join("scripts/edit.py"), script_bytes).unwrap();
        let binding = crate::AgentFileInputBinding {
            schema_version: crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: "scripts/edit.py".to_string(),
            source: crate::AgentFileInputRef::Workspace {
                path: "scripts/edit.py".to_string(),
            },
            size_bytes: script_bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(script_bytes)),
        };
        let prepared = materialize_agent_file_inputs(
            Some(workspace.path()),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            &AgentFileInputExecutionContext::default(),
            &[binding],
            None,
        )
        .unwrap()
        .unwrap();
        let mut parsed =
            parse_managed_pdf_command("python '$MYCOPILOT_INPUT_ROOT/scripts/edit.py'")
                .unwrap()
                .unwrap();
        resolve_materialized_pdf_input_script(&mut parsed, Some(&prepared)).unwrap();
        let resolved = PathBuf::from(&parsed.process_arguments[0]);
        assert!(resolved.is_absolute());
        assert!(resolved.starts_with(prepared.root().canonicalize().unwrap()));

        let mut missing =
            parse_managed_pdf_command("python '$MYCOPILOT_INPUT_ROOT/scripts/missing.py'")
                .unwrap()
                .unwrap();
        assert!(resolve_materialized_pdf_input_script(&mut missing, Some(&prepared)).is_err());
    }

    #[test]
    fn managed_pdf_relative_workspace_argument_resolves_to_the_frozen_single_file() {
        let workspace = TempDir::new().unwrap();
        let filename = "AspenPolymer-Unit Operations and Reaction Models.pdf";
        let bytes = b"%PDF-1.4\nfixture\n";
        fs::write(workspace.path().join(filename), bytes).unwrap();
        let binding = crate::AgentFileInputBinding {
            schema_version: crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: filename.to_string(),
            source: crate::AgentFileInputRef::Workspace {
                path: filename.to_string(),
            },
            size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        };
        let prepared = materialize_agent_file_inputs(
            Some(workspace.path()),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            &AgentFileInputExecutionContext::default(),
            &[binding],
            None,
        )
        .unwrap()
        .unwrap();
        let mut parsed = parse_managed_pdf_command(&format!("pdfinfo \"{filename}\""))
            .unwrap()
            .unwrap();

        resolve_materialized_pdf_input_script(&mut parsed, Some(&prepared)).unwrap();

        let resolved = PathBuf::from(parsed.process_arguments.last().unwrap());
        assert!(resolved.is_absolute());
        assert!(resolved.starts_with(prepared.root().canonicalize().unwrap()));
        assert_eq!(fs::read(resolved).unwrap(), bytes);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_pdf_sandbox_child_probe() {
        if std::env::var_os("MYCOPILOT_PDF_SANDBOX_CHILD").is_none() {
            return;
        }

        let input = PathBuf::from(std::env::var_os("SANDBOX_ALLOWED_INPUT").unwrap());
        assert_eq!(fs::read_to_string(input).unwrap(), "allowed");
        fs::write("normal-output.txt", b"written").unwrap();

        let external_read = PathBuf::from(std::env::var_os("SANDBOX_EXTERNAL_READ").unwrap());
        assert_eq!(
            fs::read(external_read).unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let external_write = PathBuf::from(std::env::var_os("SANDBOX_EXTERNAL_WRITE").unwrap());
        assert_eq!(
            fs::write(external_write, b"blocked").unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            std::process::Command::new("/usr/bin/true")
                .status()
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::PermissionDenied
        );
        let network_error = std::net::TcpStream::connect_timeout(
            &"127.0.0.1:9".parse().unwrap(),
            Duration::from_millis(100),
        )
        .unwrap_err();
        assert_eq!(network_error.kind(), std::io::ErrorKind::PermissionDenied);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn managed_pdf_sandbox_denies_external_authority_and_allows_private_io() {
        let fixture = TempDir::new().unwrap();
        let source_root = fixture.path().join("source");
        fs::create_dir(&source_root).unwrap();
        fs::write(source_root.join("allowed.txt"), b"allowed").unwrap();
        let binding = crate::AgentFileInputBinding {
            schema_version: crate::AGENT_FILE_INPUT_BINDING_SCHEMA_VERSION,
            mount_path: "allowed.txt".to_string(),
            source: crate::AgentFileInputRef::Workspace {
                path: "allowed.txt".to_string(),
            },
            size_bytes: 7,
            sha256: format!("{:x}", Sha256::digest(b"allowed")),
        };
        let prepared = materialize_agent_file_inputs(
            Some(&source_root),
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            &AgentFileInputExecutionContext::default(),
            &[binding],
            None,
        )
        .unwrap()
        .unwrap();
        let external_read = fixture.path().join("external-secret.txt");
        let external_write = fixture.path().join("external-write.txt");
        fs::write(&external_read, b"secret").unwrap();
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
        let mut environment = vec![
            (
                OsString::from("MYCOPILOT_PDF_SANDBOX_CHILD"),
                OsString::from("1"),
            ),
            (
                OsString::from("SANDBOX_ALLOWED_INPUT"),
                prepared.root().join("allowed.txt").into_os_string(),
            ),
            (
                OsString::from("SANDBOX_EXTERNAL_READ"),
                external_read.into_os_string(),
            ),
            (
                OsString::from("SANDBOX_EXTERNAL_WRITE"),
                external_write.clone().into_os_string(),
            ),
            (
                OsString::from(AGENT_FILE_INPUT_ROOT_ENV),
                prepared.root().as_os_str().to_os_string(),
            ),
        ];
        let plan = prepare_managed_pdf_sandbox_launch(
            &invocation,
            vec![
                OsString::from("command::managed_runtime::tests::managed_pdf_sandbox_child_probe"),
                OsString::from("--exact"),
                OsString::from("--nocapture"),
            ],
            &mut environment,
            &workspace,
            Some(&prepared),
            &runtime_root,
        )
        .unwrap();
        let spawn = CommandSpawnPlan::direct(
            "managed PDF sandbox probe".to_string(),
            workspace.execution_root().to_path_buf(),
            Some(workspace.execution_root()),
            None,
            plan,
        );
        let output = spawn.build().output().unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read(workspace.execution_root().join("normal-output.txt")).unwrap(),
            b"written"
        );
        assert!(!external_write.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "release smoke test; requires MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT"]
    fn managed_pdf_sandbox_runs_the_real_pdf_dependency_stack() {
        let component = std::env::var_os("MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT")
            .expect("set MYCOPILOT_ARTIFACT_RUNTIME_TEST_COMPONENT");
        let provider = ArtifactRuntimeProvider::discover(
            &crate::artifact_runtime::ArtifactRuntimeDiscoveryOptions::new()
                .with_configured_component_dir(component),
        )
        .unwrap();
        let ArtifactRuntimePreflight::Ready(invocation) = provider
            .preflight(ArtifactRuntimeKind::Python, &[])
            .unwrap()
        else {
            panic!("managed Python runtime must be ready")
        };
        assert!(
            invocation
                .arguments_prefix()
                .iter()
                .any(|argument| argument == OsStr::new("-B")),
            "managed Python must never write bytecode into the immutable component"
        );
        assert_eq!(
            invocation
                .environment()
                .get(OsStr::new("PYTHONDONTWRITEBYTECODE")),
            Some(&OsString::from("1"))
        );
        let fixture = TempDir::new().unwrap();
        let external = fixture.path().join("external-secret.txt");
        fs::write(&external, b"secret").unwrap();
        let workspace =
            ManagedCommandWorkspaceLease::open(fixture.path().join("managed-run")).unwrap();
        let script = format!(
            r#"from pathlib import Path
from reportlab.pdfgen import canvas
from pypdf import PdfReader, PdfWriter
import pdfplumber
import pypdfium2 as pdfium
import socket
import subprocess
p = Path('outputs/smoke.pdf')
c = canvas.Canvas(str(p))
c.drawString(72, 720, 'managed pdf sandbox smoke')
c.save()
assert len(PdfReader(str(p)).pages) == 1
with pdfplumber.open(p) as document:
    assert 'managed pdf sandbox smoke' in (document.pages[0].extract_text() or '')
page = pdfium.PdfDocument(p)[0]
page.render(scale=1).to_pil().save('outputs/smoke.png')

# Exercise editing and AcroForm handling against the exact bundled dependency stack. The source
# form remains private intermediate state; only reopened and rendered deliverables enter outputs/.
form_source = Path('form-source.pdf')
form_canvas = canvas.Canvas(str(form_source))
form_canvas.drawString(72, 720, 'Founder')
form_canvas.acroForm.textfield(
    name='founder',
    tooltip='Founder name',
    x=72,
    y=680,
    width=220,
    height=24,
)
form_canvas.save()
source_reader = PdfReader(str(form_source))
assert source_reader.get_fields()['founder'].get('/V', '') == ''

filled_path = Path('outputs/form-filled.pdf')
filled_writer = PdfWriter()
filled_writer.clone_document_from_reader(source_reader)
filled_writer.update_page_form_field_values(
    filled_writer.pages[0],
    {{'founder': 'Ada Lovelace'}},
    auto_regenerate=False,
)
with filled_path.open('wb') as stream:
    filled_writer.write(stream)
filled_reader = PdfReader(str(filled_path))
assert filled_reader.get_fields()['founder']['/V'] == 'Ada Lovelace'
filled_widgets = [
    annotation.get_object()
    for annotation in filled_reader.pages[0].get('/Annots', [])
    if annotation.get_object().get('/Subtype') == '/Widget'
]
assert filled_widgets and filled_widgets[0].get('/AP') is not None
pdfium.PdfDocument(filled_path)[0].render(scale=1).to_pil().save(
    'outputs/form-filled.png'
)

flattened_path = Path('outputs/form-flattened.pdf')
flattened_writer = PdfWriter()
flattened_writer.clone_document_from_reader(filled_reader)
flattened_writer.update_page_form_field_values(
    flattened_writer.pages[0],
    {{'founder': 'Ada Lovelace'}},
    auto_regenerate=False,
    flatten=True,
)
with flattened_path.open('wb') as stream:
    flattened_writer.write(stream)
flattened_reader = PdfReader(str(flattened_path))
assert flattened_reader.pages[0].extract_text()
pdfium.PdfDocument(flattened_path)[0].render(scale=1).to_pil().save(
    'outputs/form-flattened.png'
)

oversized_writer = PdfWriter()
oversized_writer.add_blank_page(width=100_000, height=100_000)
with Path('oversized.pdf').open('wb') as stream:
    oversized_writer.write(stream)
large_text_canvas = canvas.Canvas('large-text.pdf')
large_text_object = large_text_canvas.beginText(72, 720)
for line_number in range(200):
    large_text_object.textLine(f'bounded-text-{{line_number}}-' + ('x' * 80))
large_text_canvas.drawText(large_text_object)
large_text_canvas.save()
try:
    Path({external:?}).read_bytes()
    raise AssertionError('external read unexpectedly succeeded')
except PermissionError:
    pass
try:
    subprocess.run(['/usr/bin/true'], check=True)
    raise AssertionError('subprocess unexpectedly succeeded')
except PermissionError:
    pass
try:
    socket.create_connection(('127.0.0.1', 9), timeout=0.1)
    raise AssertionError('network unexpectedly succeeded')
except PermissionError:
    pass
print('pdf-sandbox-smoke-ok')"#,
            external = external.to_string_lossy()
        );
        let mut arguments = invocation.arguments_prefix().to_vec();
        arguments.extend([OsString::from("-c"), OsString::from(script)]);
        let mut environment = managed_environment(&invocation, None);
        let launch = prepare_managed_pdf_sandbox_launch(
            &invocation,
            arguments,
            &mut environment,
            &workspace,
            None,
            provider.component_root(),
        )
        .unwrap();
        let output = CommandSpawnPlan::direct(
            "managed PDF dependency smoke".to_string(),
            workspace.execution_root().to_path_buf(),
            Some(workspace.execution_root()),
            None,
            launch,
        )
        .build()
        .output()
        .unwrap();
        assert!(
            output.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(workspace.outputs_root().join("smoke.pdf").is_file());
        assert!(workspace.outputs_root().join("smoke.png").is_file());

        let run_managed_cli = |cli_source: &str, cli_arguments: &[&str]| {
            let mut arguments = invocation.arguments_prefix().to_vec();
            arguments.extend([OsString::from("-c"), OsString::from(cli_source)]);
            arguments.extend(
                cli_arguments
                    .iter()
                    .map(|argument| OsString::from(*argument)),
            );
            let mut environment = managed_environment(&invocation, None);
            let launch = prepare_managed_pdf_sandbox_launch(
                &invocation,
                arguments,
                &mut environment,
                &workspace,
                None,
                provider.component_root(),
            )
            .unwrap();
            CommandSpawnPlan::direct(
                "managed PDF CLI limit probe".to_string(),
                workspace.execution_root().to_path_buf(),
                Some(workspace.execution_root()),
                Some(Duration::from_secs(30)),
                launch,
            )
            .build()
            .output()
            .unwrap()
        };
        let oversized_render = run_managed_cli(
            MANAGED_PDF_CLI_SOURCE,
            &[
                "pdftoppm",
                "-f",
                "1",
                "-l",
                "1",
                "-r",
                "300",
                "oversized.pdf",
                "outputs/oversized",
            ],
        );
        assert!(!oversized_render.status.success());
        assert!(
            String::from_utf8_lossy(&oversized_render.stderr).contains("single-page safety limit"),
            "stderr={}",
            String::from_utf8_lossy(&oversized_render.stderr)
        );
        assert!(!workspace.outputs_root().join("oversized-1.png").exists());

        // Use the production implementation with a smaller test-only constant so the smoke test
        // proves fail-closed streaming behavior without manufacturing a 32 MiB fixture.
        let constrained_text_cli = MANAGED_PDF_CLI_SOURCE.replace(
            "MAX_EXTRACTED_TEXT_BYTES = 32 * 1024 * 1024",
            "MAX_EXTRACTED_TEXT_BYTES = 1024",
        );
        assert_ne!(constrained_text_cli, MANAGED_PDF_CLI_SOURCE);
        let oversized_text = run_managed_cli(
            &constrained_text_cli,
            &["pdftotext", "-f", "1", "-l", "1", "large-text.pdf", "-"],
        );
        assert!(!oversized_text.status.success());
        assert!(oversized_text.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&oversized_text.stderr)
                .contains("narrow the page range with -f and -l"),
            "stderr={}",
            String::from_utf8_lossy(&oversized_text.stderr)
        );

        // The CLI wrapper independently rejects absolute inputs outside the private roots. The
        // OS sandbox remains the enforcement boundary, but this deterministic validation keeps a
        // malformed command from relying on an opaque kernel denial for ordinary recovery.
        let mut cli_probe = std::process::Command::new(invocation.executable());
        cli_probe
            .env_clear()
            .envs(invocation.environment())
            .env("HOME", workspace.home_root())
            .env("TMPDIR", workspace.temp_root())
            .current_dir(workspace.execution_root())
            .args(invocation.arguments_prefix())
            .args([OsStr::new("-c"), OsStr::new(MANAGED_PDF_CLI_SOURCE)])
            .arg("pdfinfo")
            .arg(&external);
        let cli_output = cli_probe.output().unwrap();
        assert!(!cli_output.status.success());
        assert!(String::from_utf8_lossy(&cli_output.stderr)
            .contains("must come from the private input or execution root"));
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
        assert!(prepare_managed_pdf_sandbox_launch(
            &invocation,
            Vec::new(),
            &mut Vec::new(),
            &workspace,
            None,
            &runtime_root,
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
    fn rejects_ranges_duplicates_and_unknown_runtime_fields() {
        let mut request = runtime(AgentCommandRuntimeKind::Python);
        request.required_packages[0].version = "^3.1.5".to_string();
        assert!(validate_command_runtime_request(&request).is_err());

        let error = serde_json::from_value::<AgentCommandRuntimeRequest>(serde_json::json!({
            "provider": "managedArtifact",
            "kind": "node",
            "requiredPackages": [{"name": "exceljs", "version": "4.4.0"}],
            "executable": "/tmp/fake-node"
        }))
        .unwrap_err();
        assert!(error.to_string().contains("unknown field"));

        let mut request = runtime(AgentCommandRuntimeKind::Python);
        request
            .required_packages
            .push(AgentCommandRuntimePackageRequirement {
                name: "OpenPyXL".to_string(),
                version: "3.1.5".to_string(),
            });
        assert!(validate_command_runtime_request(&request).is_err());
    }

    #[derive(Debug)]
    enum FakePreflight {
        Ready(ArtifactRuntimeInvocation),
        Missing,
    }

    struct FakeProvider {
        preflight: std::sync::Mutex<Option<FakePreflight>>,
    }

    impl ManagedRuntimeProvider for FakeProvider {
        fn preflight(
            &self,
            kind: ArtifactRuntimeKind,
            requirements: &[ArtifactRuntimeRequirement],
        ) -> Result<ArtifactRuntimePreflight, ArtifactRuntimeError> {
            match self.preflight.lock().unwrap().take().unwrap() {
                FakePreflight::Ready(invocation) => Ok(ArtifactRuntimePreflight::Ready(invocation)),
                FakePreflight::Missing => Ok(ArtifactRuntimePreflight::MissingDependencies {
                    status: ArtifactRuntimeStatus {
                        kind,
                        availability: ArtifactRuntimeAvailability::Available,
                        version: Some("test".to_string()),
                        dependencies: Vec::new(),
                        runtime_fingerprint: Some("test-fingerprint".to_string()),
                        error_code: None,
                        recovery: None,
                        message: None,
                    },
                    missing: requirements.to_vec(),
                }),
            }
        }

        fn verify_integrity(&self) -> Result<(), ArtifactRuntimeError> {
            Ok(())
        }

        fn bundle_version(&self) -> &str {
            "test-bundle"
        }

        fn bundle_revision(&self) -> &str {
            "test-revision"
        }
    }

    #[test]
    fn missing_dependency_is_a_structured_non_execution_result() {
        let workspace = TempDir::new().unwrap();
        fs::write(workspace.path().join("build.py"), "print('never')\n").unwrap();
        let runtime = runtime(AgentCommandRuntimeKind::Python);
        let request = AgentCommandRequest {
            id: "runtime-missing".to_string(),
            command: "python build.py".to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: crate::AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime: Some(runtime),
            runtime_binding: None,
        };
        let provider = FakeProvider {
            preflight: std::sync::Mutex::new(Some(FakePreflight::Missing)),
        };
        let workspace_path = workspace.path().canonicalize().unwrap();
        let result = execute_with_provider(
            Some(&workspace_path),
            &workspace_path,
            &request,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            AgentCancellationToken::new(),
            None,
            &provider,
        );
        assert_eq!(
            result.runtime.unwrap().error_code.as_deref(),
            Some(ERROR_MISSING_DEPENDENCIES)
        );
        assert_eq!(result.exit_code, None);
    }

    #[test]
    fn public_execution_retires_legacy_model_supplied_runtime_requests() {
        let workspace = TempDir::new().unwrap();
        fs::write(
            workspace.path().join("build.py"),
            "from pathlib import Path\nPath('must-not-exist').write_text('ran')\n",
        )
        .unwrap();
        let request = AgentCommandRequest {
            id: "runtime-legacy".to_string(),
            command: "python build.py".to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: crate::AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime: Some(runtime(AgentCommandRuntimeKind::Python)),
            runtime_binding: None,
        };
        let result = run_authorized_command_with_artifact_runtime(
            Some(workspace.path()),
            &request,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: crate::AgentCommandPermission::RequireApproval,
                command_safety: crate::AgentCommandSafetyPolicy::Guarded,
                patch: crate::AgentPatchPermission::RequireApproval,
            },
            CommandAuthorizationSource::ExplicitUser,
            AgentCancellationToken::new(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.exit_code, None);
        assert_eq!(
            result.runtime.unwrap().error_code.as_deref(),
            Some(super::COMMAND_RUNTIME_PROFILE_ERROR_LEGACY_REPREPARE)
        );
        assert!(!workspace.path().join("must-not-exist").exists());
    }

    #[test]
    fn pre_cancelled_request_does_not_run_expensive_preflight() {
        let workspace = TempDir::new().unwrap();
        fs::write(workspace.path().join("build.py"), "print('never')\n").unwrap();
        let request = AgentCommandRequest {
            id: "runtime-cancelled".to_string(),
            command: "python build.py".to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: crate::AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime: Some(runtime(AgentCommandRuntimeKind::Python)),
            runtime_binding: None,
        };
        let provider = FakeProvider {
            preflight: std::sync::Mutex::new(Some(FakePreflight::Missing)),
        };
        let cancellation = AgentCancellationToken::new();
        cancellation.cancel();
        let workspace_path = workspace.path().canonicalize().unwrap();
        let result = execute_with_provider(
            Some(&workspace_path),
            &workspace_path,
            &request,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            cancellation,
            None,
            &provider,
        );

        assert!(result.cancelled);
        assert!(provider.preflight.lock().unwrap().is_some());
        assert!(result.runtime.unwrap().error_code.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn direct_launcher_ignores_path_and_inherited_node_options() {
        use std::os::unix::fs::PermissionsExt;

        let workspace = TempDir::new().unwrap();
        let fake_runtime = workspace.path().join("managed-node");
        fs::write(
            &fake_runtime,
            "#!/bin/sh\nprintf 'PATH=%s\\nNODE_OPTIONS=%s\\nARG=%s\\n' \"${PATH-unset}\" \"${NODE_OPTIONS-unset}\" \"$1\"\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&fake_runtime).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&fake_runtime, permissions).unwrap();
        fs::write(workspace.path().join("build.mjs"), "// saved script\n").unwrap();

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
        let mut environment_probe = Command::new(invocation.executable());
        environment_probe.env("PATH", "/tmp/hostile-bin");
        environment_probe.env("NODE_OPTIONS", "--require=/tmp/hostile.cjs");
        configure_managed_environment(&mut environment_probe, &invocation, None);
        let configured_environment = environment_probe
            .get_envs()
            .map(|(name, value)| (name.to_os_string(), value.map(OsStr::to_os_string)))
            .collect::<BTreeMap<_, _>>();
        assert!(!configured_environment.contains_key(OsStr::new("PATH")));
        assert!(!configured_environment.contains_key(OsStr::new("NODE_OPTIONS")));
        let provider = FakeProvider {
            preflight: std::sync::Mutex::new(Some(FakePreflight::Ready(invocation))),
        };
        let request = AgentCommandRequest {
            id: "runtime-direct".to_string(),
            command: "node build.mjs".to_string(),
            cwd: None,
            timeout_ms: Some(5_000),
            approval_status: crate::AgentApprovalStatus::Approved,
            risk_level: None,
            reason: None,
            observe: None,
            inputs: Vec::new(),
            runtime: Some(runtime(AgentCommandRuntimeKind::Node)),
            runtime_binding: None,
        };

        let workspace_path = workspace.path().canonicalize().unwrap();
        let result = execute_with_provider(
            Some(&workspace_path),
            &workspace_path,
            &request,
            AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                ..AgentPermissions::default()
            },
            AgentCancellationToken::new(),
            None,
            &provider,
        );

        assert_eq!(result.exit_code, Some(0));
        assert!(result.stdout.contains("NODE_OPTIONS=unset"));
        assert!(result.stdout.contains("ARG=build.mjs"));
        assert!(result.runtime.unwrap().error_code.is_none());
    }
}
