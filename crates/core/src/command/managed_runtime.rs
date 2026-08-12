use super::*;
use crate::artifact_runtime::{
    ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery,
};
use crate::AgentCommandRuntimeProfile;
use std::ffi::{OsStr, OsString};

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
    let mut arguments = prepared_runtime.invocation.arguments_prefix().to_vec();
    arguments.extend(parsed.process_arguments.iter().cloned());
    let mut environment =
        managed_environment(&prepared_runtime.invocation, prepared_inputs.as_ref());
    let hard_timeout = managed_command_hard_timeout(managed_pdf, request.timeout_ms);
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
const MANAGED_PDF_HARD_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManagedArtifactCommand {
    script: Option<String>,
    process_arguments: Vec<OsString>,
    pdf_shell: Option<super::managed_pdf_shell::ManagedPdfShellPlan>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManagedArtifactBuilderCommand {
    pub kind: AgentCommandRuntimeKind,
    pub script: String,
    pub output_paths: Vec<String>,
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
        process_arguments: tokens[1..].iter().map(OsString::from).collect(),
        pdf_shell: None,
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
    let script = &tokens[1];
    if script.starts_with('-') {
        return Err(
            "Managed Artifact Runtime 禁止 -e、-c、-m 及其他内联代码或启动选项。".to_string(),
        );
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
    use crate::artifact_runtime::ArtifactRuntimeKind;
    use std::collections::BTreeMap;
    use tempfile::TempDir;

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
