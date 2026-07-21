use super::*;
#[cfg(test)]
use crate::artifact_runtime::{
    ArtifactRuntimeError, ArtifactRuntimeKind, ArtifactRuntimePreflight, ArtifactRuntimeRequirement,
};
use crate::artifact_runtime::{
    ArtifactRuntimeErrorCode, ArtifactRuntimeInvocation, ArtifactRuntimeProvider,
    ArtifactRuntimeRecovery, ARTIFACT_RUNTIME_PROVIDER_ID,
};
#[cfg(test)]
use crate::AgentCommandRuntimeResolvedPackage;
use std::ffi::{OsStr, OsString};

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
    if request.runtime.is_none() && request.runtime_binding.is_none() {
        return run_authorized_command(
            workspace_root,
            request,
            permissions,
            authorization_source,
            cancellation_token,
            action_cancel_flag,
        );
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
    let parsed = match parse_managed_artifact_command(&request.command, binding.kind) {
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
    if let Err(message) = validate_saved_script(cwd, root, permissions.read, &parsed.script) {
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
    let mut result = run_managed_process(
        root,
        cwd,
        request,
        &parsed,
        &prepared.invocation,
        resolution,
        cancellation_token,
        action_cancel_flag,
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
    if let Err(message) = validate_saved_script(cwd, root, permissions.read, &parsed.script) {
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
    script: String,
    process_arguments: Vec<OsString>,
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
        script: script.clone(),
        process_arguments: tokens[1..].iter().map(OsString::from).collect(),
    })
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
) -> AgentCommandExecutionResult {
    if command_cancel_requested(&cancellation_token, action_cancel_flag.as_ref()) {
        return AgentCommandExecutionResult {
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
            error: None,
            policy_evaluation: None,
            artifact_observation: None,
            runtime: Some(resolution),
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
    configure_managed_environment(&mut command, invocation);
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
    let stdout_reader = spawn_bounded_output_reader(stdout);
    let stderr_reader = spawn_bounded_output_reader(stderr);

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

    let stdout = join_output_reader(stdout_reader, "stdout");
    let stderr = join_output_reader(stderr_reader, "stderr");
    match (exit_status, stdout, stderr) {
        (Ok(exit_status), Ok((stdout, stdout_truncated)), Ok((stderr, stderr_truncated))) => {
            AgentCommandExecutionResult {
                command: request.command.clone(),
                cwd: relative_cwd(root, cwd),
                exit_code: exit_status.code(),
                stdout,
                stderr,
                timed_out,
                cancelled,
                duration_ms: started.elapsed().as_millis() as u64,
                stdout_truncated,
                stderr_truncated,
                error: None,
                policy_evaluation: None,
                artifact_observation: None,
                runtime: Some(resolution),
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

fn configure_managed_environment(command: &mut Command, invocation: &ArtifactRuntimeInvocation) {
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
        error: None,
        policy_evaluation: None,
        artifact_observation: None,
        runtime: Some(runtime),
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
        error: runtime.message.clone(),
        policy_evaluation: None,
        artifact_observation: None,
        runtime: Some(runtime),
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
        configure_managed_environment(&mut environment_probe, &invocation);
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
