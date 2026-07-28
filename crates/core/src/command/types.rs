use super::*;

pub(super) const DEFAULT_TIMEOUT_MS: u64 = 120_000;

pub(super) const MAX_TIMEOUT_MS: u64 = 600_000;

pub(super) const MAX_OUTPUT_BYTES: usize = 128 * 1024;

pub(crate) const MAX_COMMAND_CHARS: usize = 2_000;

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
}

impl AgentCommandExecutionResult {
    pub fn output_spool_substitutions(&self) -> Vec<ProcessOutputSpoolSubstitution> {
        process_output_spool_substitutions(&self.stdout_spool, &self.stderr_spool)
    }
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
