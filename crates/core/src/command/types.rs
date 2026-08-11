use super::*;

pub(super) const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub(super) const MAX_TIMEOUT_MS: u64 = 600_000;

pub(super) const MAX_OUTPUT_BYTES: usize = 128 * 1024;

/// One product-wide bound for the exact, possibly multi-line command accepted by both tool
/// preparation and execution policy. This is deliberately large enough for managed-runtime
/// `python -c` workflows while remaining bounded for approval, checkpoint, and audit persistence.
pub(crate) const MAX_COMMAND_CHARS: usize = 16_000;

/// Identifies who authorized a command execution.
///
/// This is deliberately separate from the approval status stored on a request: an automatically
/// approved action and a user-approved action may both be represented as `Approved` by the wire
/// protocol, but they must not receive the same command privileges.
///
/// The trusted host must choose this value from its own approval state. It must never be accepted
/// from model arguments, renderer payloads, or another untrusted request field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandAuthorizationSource {
    Automatic,
    ExplicitUser,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandPolicyDecision {
    Allow,
    RequireExplicitApproval,
    Deny,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandRiskClass {
    ReadOnly,
    SafeWorkspaceWrite,
    DirectWrite,
    Network,
    PackageManagement,
    HighImpact,
    Unknown,
    Catastrophic,
    Unsupported,
    ExternalRead,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyFinding {
    pub segment_index: usize,
    pub program: String,
    pub risk: CommandRiskClass,
    /// Stable, machine-readable identifier. Callers must branch on this rather than `reason`.
    pub code: String,
    /// Human-readable diagnostic. This is not a wire-level discriminator.
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyEvaluation {
    pub decision: CommandPolicyDecision,
    /// Stable, machine-readable summary code.
    pub code: String,
    pub reason: String,
    pub risk_level: AgentCommandRiskLevel,
    pub findings: Vec<CommandPolicyFinding>,
}

/// Failure returned before a command process could be started.
///
/// Operational failures retain a human-readable message. Policy failures additionally carry the
/// authoritative structured evaluation so host adapters can preserve stable codes and findings
/// all the way to the Tool Result instead of parsing localized error text.
#[derive(Debug, Clone)]
pub struct CommandExecutionError {
    pub(super) message: String,
    pub(super) policy_evaluation: Option<CommandPolicyEvaluation>,
    pub(super) artifact_observation: Option<Box<AgentCommandArtifactObservation>>,
}

impl CommandExecutionError {
    pub(super) fn from_policy(evaluation: CommandPolicyEvaluation) -> Self {
        let prefix = match evaluation.decision {
            CommandPolicyDecision::RequireExplicitApproval => "命令需要用户明确批准后才能执行",
            CommandPolicyDecision::Deny => "命令已被安全策略拒绝",
            CommandPolicyDecision::Allow => "命令策略校验失败",
        };
        Self {
            message: format!("{prefix}（{}）：{}", evaluation.code, evaluation.reason),
            policy_evaluation: Some(evaluation),
            artifact_observation: None,
        }
    }

    pub fn policy_evaluation(&self) -> Option<&CommandPolicyEvaluation> {
        self.policy_evaluation.as_ref()
    }

    pub fn artifact_observation(&self) -> Option<&AgentCommandArtifactObservation> {
        self.artifact_observation.as_deref()
    }

    pub(super) fn with_artifact_observation(
        mut self,
        artifact_observation: Option<AgentCommandArtifactObservation>,
    ) -> Self {
        self.artifact_observation = artifact_observation.map(Box::new);
        self
    }
}

impl From<String> for CommandExecutionError {
    fn from(message: String) -> Self {
        Self {
            message,
            policy_evaluation: None,
            artifact_observation: None,
        }
    }
}

impl std::fmt::Display for CommandExecutionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CommandExecutionError {}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCommandExecutionResult {
    /// Authoritative immutable files published after a trusted managed command exits.
    ///
    /// Receipts deliberately omit private execution and object-store paths. Consumer-specific
    /// projections may further reduce these entries, but the durable command result retains the
    /// verified content identity needed for audit and restart recovery.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<AgentCommandPublishedOutput>,
    pub command: String,
    pub cwd: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
    pub cancelled: bool,
    pub duration_ms: u64,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    #[serde(flatten, default)]
    pub output_capture: ProcessOutputCaptureMetadata,
    /// Backend-only complete stdout capture. Never serialized into events, audit receipts, or
    /// checkpoints; Exact History consumes it before those consumer projections are built.
    #[serde(skip, default)]
    pub stdout_spool: ProcessOutputSpool,
    /// Backend-only complete stderr capture paired with `stdout_spool`.
    #[serde(skip, default)]
    pub stderr_spool: ProcessOutputSpool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_evaluation: Option<CommandPolicyEvaluation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_observation: Option<AgentCommandArtifactObservation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input_files: Vec<crate::AgentFileInputEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime: Option<AgentCommandRuntimeResolution>,
    /// Backend-only view of outputs created inside a trusted run-scoped workspace.
    ///
    /// The physical path is never serialized or projected to the model. The capture keeps the
    /// workspace alive through terminal publication; the host separately retains one lease for
    /// the whole Agent Run so successive PDF commands can share safe intermediate files.
    #[serde(skip, default)]
    pub managed_outputs: Option<ManagedCommandOutputCapture>,
    /// Backend-only Exact History identity committed by the Host-owned command Session.
    /// `command_tool_result` converts it into an opaque `historyOpen`; consumers never receive the
    /// Archive row id or storage path directly.
    #[serde(skip, default)]
    pub authoritative_archive_ref: Option<String>,
    /// Opaque, conversation-checked recovery capability derived from the authoritative Archive.
    ///
    /// Unlike `authoritative_archive_ref`, this value is safe to persist in command audit evidence
    /// and to deliver to the model. Keeping the derived route in `command_result_json` lets manual
    /// settlement reconstruct the exact same ToolResult without exposing the raw Archive id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history_open: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentCommandPublishedOutputKind {
    Image,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCommandPublishedOutput {
    /// Safe relative name below the managed output root, never a private physical path.
    pub name: String,
    pub kind: AgentCommandPublishedOutputKind,
    pub read_path: String,
    pub mime_type: String,
    pub size_bytes: u64,
    pub sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl AgentCommandExecutionResult {
    pub fn output_spool_substitutions(&self) -> Vec<ProcessOutputSpoolSubstitution> {
        process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
}

#[derive(Clone)]
pub struct ManagedCommandOutputCapture {
    workspace: ManagedCommandWorkspaceLease,
    baseline: BTreeMap<String, String>,
}

#[derive(Clone)]
pub struct ManagedCommandWorkspaceLease {
    inner: Arc<ManagedCommandWorkspaceInner>,
}

struct ManagedCommandWorkspaceInner {
    execution_root: PathBuf,
    outputs_root: PathBuf,
    home_root: PathBuf,
    temp_root: PathBuf,
    cleanup_requested: AtomicBool,
}

#[derive(Clone)]
pub(crate) struct ManagedCommandWorkspaceRegistry {
    root: PathBuf,
    leases: Arc<Mutex<HashMap<String, std::sync::Weak<ManagedCommandWorkspaceInner>>>>,
}

impl ManagedCommandWorkspaceLease {
    /// Opens a host-derived run workspace. Callers must derive `execution_root` from trusted
    /// conversation/run identity, never from model input.
    pub(crate) fn open(execution_root: PathBuf) -> Result<Self, String> {
        ensure_private_directory(&execution_root)?;
        let outputs_root = execution_root.join("outputs");
        ensure_private_directory(&outputs_root)?;
        ensure_direct_child(&execution_root, &outputs_root)?;
        let home_root = execution_root.join(".home");
        ensure_private_directory(&home_root)?;
        ensure_direct_child(&execution_root, &home_root)?;
        let temp_root = execution_root.join(".tmp");
        ensure_private_directory(&temp_root)?;
        ensure_direct_child(&execution_root, &temp_root)?;
        Ok(Self {
            inner: Arc::new(ManagedCommandWorkspaceInner {
                execution_root,
                outputs_root,
                home_root,
                temp_root,
                cleanup_requested: AtomicBool::new(false),
            }),
        })
    }

    pub(crate) fn execution_root(&self) -> &Path {
        &self.inner.execution_root
    }

    pub(crate) fn output_capture(
        &self,
        baseline: BTreeMap<String, String>,
    ) -> ManagedCommandOutputCapture {
        ManagedCommandOutputCapture {
            workspace: self.clone(),
            baseline,
        }
    }

    pub fn outputs_root(&self) -> &Path {
        &self.inner.outputs_root
    }

    pub(crate) fn home_root(&self) -> &Path {
        &self.inner.home_root
    }

    pub(crate) fn temp_root(&self) -> &Path {
        &self.inner.temp_root
    }
}

impl ManagedCommandWorkspaceRegistry {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            leases: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn acquire(&self, scope_name: &str) -> Result<ManagedCommandWorkspaceLease, String> {
        validate_managed_workspace_scope_name(scope_name)?;
        let root_parent = self
            .root
            .parent()
            .ok_or_else(|| "受管命令工作区根缺少父目录。".to_string())?;
        ensure_private_directory(root_parent)?;
        ensure_private_directory(&self.root)?;
        ensure_direct_child(root_parent, &self.root)?;
        let scope_root = self.root.join(scope_name);
        if scope_root.parent() != Some(self.root.as_path()) {
            return Err("受管命令 Run 目录发生路径逃逸。".to_string());
        }

        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = leases.get(scope_name).and_then(std::sync::Weak::upgrade) {
            if existing.cleanup_requested.load(Ordering::SeqCst) {
                return Err("受管命令 Run 工作区已经进入清理阶段。".to_string());
            }
            return Ok(ManagedCommandWorkspaceLease { inner: existing });
        }
        leases.remove(scope_name);
        let lease = ManagedCommandWorkspaceLease::open(scope_root)?;
        leases.insert(scope_name.to_string(), Arc::downgrade(&lease.inner));
        Ok(lease)
    }

    pub(crate) fn request_cleanup(&self, scope_name: &str) -> Result<(), String> {
        validate_managed_workspace_scope_name(scope_name)?;
        let scope_root = self.root.join(scope_name);
        if scope_root.parent() != Some(self.root.as_path()) {
            return Err("受管命令 Run 目录发生路径逃逸。".to_string());
        }
        let mut leases = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(existing) = leases.get(scope_name).and_then(std::sync::Weak::upgrade) {
            existing.cleanup_requested.store(true, Ordering::SeqCst);
            return Ok(());
        }
        leases.remove(scope_name);
        remove_managed_workspace_directory(&scope_root)
    }

    pub(crate) fn prune_stale(
        &self,
        minimum_age: Duration,
        max_entries: usize,
        max_removals: usize,
    ) -> Result<usize, String> {
        let root_metadata = match std::fs::symlink_metadata(&self.root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(format!("无法检查受管命令工作区根：{error}")),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err("受管命令工作区根必须是非符号链接目录。".to_string());
        }
        let now = std::time::SystemTime::now();
        let active = self
            .leases
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut removed = 0usize;
        for entry in std::fs::read_dir(&self.root)
            .map_err(|error| format!("无法枚举受管命令工作区：{error}"))?
            .take(max_entries)
        {
            if removed >= max_removals {
                break;
            }
            let entry = entry.map_err(|error| format!("无法读取受管命令工作区条目：{error}"))?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if validate_managed_workspace_scope_name(&name).is_err()
                || active
                    .get(&name)
                    .and_then(std::sync::Weak::upgrade)
                    .is_some()
            {
                continue;
            }
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|error| format!("无法检查受管命令工作区条目：{error}"))?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                continue;
            }
            let old_enough = metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age >= minimum_age);
            if old_enough {
                remove_managed_workspace_directory(&entry.path())?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl ManagedCommandOutputCapture {
    #[cfg(test)]
    pub(crate) fn workspace(&self) -> &ManagedCommandWorkspaceLease {
        &self.workspace
    }

    pub fn root(&self) -> &Path {
        self.workspace.outputs_root()
    }

    pub fn baseline(&self) -> &BTreeMap<String, String> {
        &self.baseline
    }
}

impl Drop for ManagedCommandWorkspaceInner {
    fn drop(&mut self) {
        if self.cleanup_requested.load(Ordering::SeqCst) {
            // The root is host-derived and was verified when opened. Failure is intentionally
            // best-effort: an interrupted process or platform file lock is reclaimed by a later
            // acquisition/startup maintenance pass instead of making a completed tool fail.
            let _ = std::fs::remove_dir_all(&self.execution_root);
        }
    }
}

impl std::fmt::Debug for ManagedCommandOutputCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Do not accidentally expose the private physical path in logs or panic output.
        formatter
            .debug_struct("ManagedCommandOutputCapture")
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ManagedCommandWorkspaceLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ManagedCommandWorkspaceLease")
            .finish_non_exhaustive()
    }
}

fn validate_managed_workspace_scope_name(scope_name: &str) -> Result<(), String> {
    if scope_name.len() != 64
        || !scope_name
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err("受管命令 Run 工作区身份无效。".to_string());
    }
    Ok(())
}

fn remove_managed_workspace_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("拒绝清理符号链接形式的受管命令 Run 目录。".to_string())
        }
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path)
            .map_err(|error| format!("无法清理受管命令 Run 目录：{error}")),
        Ok(_) => Err("拒绝清理非目录形式的受管命令 Run 路径。".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("无法检查受管命令 Run 目录：{error}")),
    }
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err("受管命令私有路径必须是非符号链接目录。".to_string());
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(path)
                .map_err(|error| format!("无法创建受管命令私有目录：{error}"))?;
        }
        Err(error) => return Err(format!("无法检查受管命令私有目录：{error}")),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法限制受管命令私有目录权限：{error}"))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn ensure_direct_child(parent: &Path, child: &Path) -> Result<(), String> {
    let canonical_parent = parent
        .canonicalize()
        .map_err(|error| format!("无法规范化受管命令私有目录：{error}"))?;
    let canonical_child = child
        .canonicalize()
        .map_err(|error| format!("无法规范化受管命令输出目录：{error}"))?;
    if canonical_child.parent() != Some(canonical_parent.as_path()) {
        return Err("受管命令输出目录逃离了 Run 私有目录。".to_string());
    }
    Ok(())
}

#[derive(Debug, Clone, Default)]
pub struct CommandRunState {
    pub(super) inner: Arc<CommandRunStateInner>,
}

#[derive(Debug, Default)]
pub(super) struct CommandRunStateInner {
    pub(super) running: Mutex<HashMap<String, RunningCommand>>,
}

#[derive(Debug, Clone)]
pub(super) struct RunningCommand {
    pub(super) run_id: String,
    pub(super) cancel_flag: Arc<AtomicBool>,
}

pub struct CommandRunGuard {
    pub(super) action_id: String,
    pub(super) cancel_flag: Arc<AtomicBool>,
    pub(super) state: CommandRunState,
}

impl CommandRunState {
    pub fn register(&self, action_id: &str, run_id: &str) -> CommandRunGuard {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        let mut running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        running.insert(
            action_id.to_string(),
            RunningCommand {
                run_id: run_id.to_string(),
                cancel_flag: cancel_flag.clone(),
            },
        );

        CommandRunGuard {
            action_id: action_id.to_string(),
            cancel_flag,
            state: self.clone(),
        }
    }

    pub fn cancel(&self, action_id: &str) -> bool {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(command) = running.get(action_id) else {
            return false;
        };

        command.cancel_flag.store(true, Ordering::SeqCst);
        true
    }

    pub fn cancel_run(&self, run_id: &str) -> usize {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut cancelled = 0;
        for command in running.values().filter(|command| command.run_id == run_id) {
            command.cancel_flag.store(true, Ordering::SeqCst);
            cancelled += 1;
        }
        cancelled
    }

    /// Returns whether an approved process action still owns its pre-spawn or running guard.
    ///
    /// Destructive lifecycle operations use this together with the higher-level cancellation map:
    /// the guard is registered while the pending-action lock is held, before the async worker is
    /// queued, so it closes the otherwise unobservable approved-to-worker-start gap.
    pub fn has_active_run(&self, run_id: &str) -> bool {
        let running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        running.values().any(|command| command.run_id == run_id)
    }

    pub(super) fn unregister(&self, action_id: &str) {
        let mut running = self
            .inner
            .running
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        running.remove(action_id);
    }
}

impl CommandRunGuard {
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel_flag.clone()
    }
}

impl Drop for CommandRunGuard {
    fn drop(&mut self) {
        self.state.unregister(&self.action_id);
    }
}
