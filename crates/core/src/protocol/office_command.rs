use super::*;

/// Version of the persisted, approval-gated Office action envelope.
///
/// Version 6 binds the original flat semantic model request, its compiled
/// provider-neutral operation request, and a normalized, non-empty user-facing
/// reason of at most [`AGENT_OFFICE_REASON_MAX_CHARS`] characters to the same
/// frozen Office action. The Host re-parses and recompiles `semantic_args`
/// before execution, so neither approval nor history recovery ever has to infer
/// model intent from provider parameters. Provider argv remains trusted
/// Host-owned state. Older actions must be prepared again.
pub const AGENT_OFFICE_OPERATION_SCHEMA_VERSION: u32 = 6;

/// Maximum number of Unicode scalar values accepted in an Office call reason.
pub const AGENT_OFFICE_REASON_MAX_CHARS: usize = 240;

fn is_agent_office_reason_bidi_control(character: char) -> bool {
    matches!(
        character,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

pub(crate) fn has_unsafe_agent_office_reason_character(reason: &str) -> bool {
    reason.chars().any(|character| {
        character.is_control()
            || matches!(character, '\u{0085}' | '\u{2028}' | '\u{2029}')
            || is_agent_office_reason_bidi_control(character)
    })
}

pub(crate) fn normalize_agent_office_reason(reason: &str) -> Option<String> {
    if has_unsafe_agent_office_reason_character(reason) {
        return None;
    }
    let reason = reason.trim();
    (!reason.is_empty() && reason.chars().count() <= AGENT_OFFICE_REASON_MAX_CHARS)
        .then(|| reason.to_string())
}

/// Returns whether an Office action reason is valid in its canonical persisted form.
///
/// Tool input is trimmed before an action is frozen. Persisted and host-submitted
/// actions must already contain that normalized value so whitespace cannot be
/// changed after approval without invalidating the snapshot.
pub fn is_valid_agent_office_reason(reason: &str) -> bool {
    normalize_agent_office_reason(reason).as_deref() == Some(reason)
}

/// Wire contract for an approval-gated Office mutation.
///
/// The prepared execution is produced by the trusted Office adapter before an
/// approval is requested. It contains only a normalized, shell-free operation
/// plan and immutable preconditions; executable paths and environment values
/// are deliberately excluded from the persisted action.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentOfficeOperationRequest {
    pub schema_version: u32,
    pub id: String,
    pub semantic_args: Value,
    pub prepared: crate::office::OfficePreparedExecution,
    pub approval_status: AgentApprovalStatus,
    pub reason: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentTodoStatus {
    Pending,
    InProgress,
    Completed,
    Blocked,
}

impl AgentTodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
            Self::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTodoItem {
    pub id: String,
    pub title: String,
    pub status: AgentTodoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentTodoState {
    pub revision: u64,
    pub items: Vec<AgentTodoItem>,
    pub updated_at: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandOutputStream {
    Stdout,
    Stderr,
}

/// Renderer-safe lifecycle state for a Host-owned command Session.
///
/// This is intentionally separate from [`AgentRunStatus`] and approval state. A command Session
/// may remain `running` after the Agent Run which created it has completed.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandSessionStatus {
    Starting,
    Running,
    Exited,
    Interrupted,
    TimedOut,
    Failed,
    OutcomeUnknown,
}

impl AgentCommandSessionStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Exited | Self::Interrupted | Self::TimedOut | Self::Failed | Self::OutcomeUnknown
        )
    }
}

/// Terminal outcomes carried by `command_exited`; an explicit user/Host interruption uses the
/// separate `command_interrupted` event and therefore cannot be mislabeled here.
#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandExitStatus {
    Exited,
    TimedOut,
    Failed,
    OutcomeUnknown,
}

/// Complete bounded Host projection of one managed command Session.
///
/// Approval payloads, raw permission records, process identifiers, environment variables and
/// unbounded output are deliberately excluded from this cross-process DTO.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionSnapshot {
    pub schema_version: u32,
    pub session_id: String,
    pub conversation_id: String,
    pub assistant_message_id: String,
    pub origin_run_id: String,
    pub call_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub command: String,
    pub cwd: String,
    pub command_digest: String,
    pub status: AgentCommandSessionStatus,
    pub started_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub latest_sequence: u64,
    pub output_truncated: bool,
    /// Presentation-safe immutable receipts for outputs published by this terminal Session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<crate::command::AgentCommandPublishedOutput>,
    /// Best-effort, bounded Office file-effect evidence captured for this terminal Session.
    ///
    /// Active Sessions never expose an observation. Absence on a terminal Session means the
    /// command did not request Office observation; it must not be interpreted as "no changes".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_observation: Option<AgentCommandArtifactObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archive_ref: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionOutputChunk {
    pub sequence: u64,
    pub stream: AgentCommandOutputStream,
    pub output: String,
}

/// Cross-process upper bound for one Host transcript projection.
///
/// Keep the TypeScript protocol constant with the same name and value in sync. Host hydration,
/// live transcript retention, and the operational model cursor all use this resource bound, while
/// immutable receipts remain a separate exact-replay projection.
pub const AGENT_COMMAND_SESSION_MAX_TRANSCRIPT_CHUNKS: usize = 2_048;

/// Non-destructive, cursor-addressed transcript projection for Host reload recovery.
#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionTranscript {
    pub requested_after_sequence: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_available_sequence: Option<u64>,
    pub latest_sequence: u64,
    pub truncated_before: bool,
    pub output_capture_truncated: bool,
    pub chunks: Vec<AgentCommandSessionOutputChunk>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionListInput {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionListOutput {
    pub sessions: Vec<AgentCommandSessionSnapshot>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionGetInput {
    pub conversation_id: String,
    pub session_id: String,
    #[serde(default)]
    pub after_sequence: Option<u64>,
    #[serde(default)]
    pub max_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct AgentCommandSessionGetOutput {
    pub session: AgentCommandSessionSnapshot,
    pub transcript: AgentCommandSessionTranscript,
}

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentCommandRiskLevel {
    ReadOnly,
    WritesWorkspace,
    Network,
    Destructive,
    Unknown,
}

#[derive(Deserialize, Serialize, Clone, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentToolCall {
    /// Application-owned opaque identity shared by the call's entire lifecycle.
    ///
    /// The runtime assigns it once when accepting a model response. Execution, approval, events,
    /// audit, checkpoints, traces, results, and subsequent model requests must reuse it verbatim.
    /// UI and host consumers must not parse or synthesize this value.
    pub id: String,
    pub tool: String,
    pub args: Value,
    pub approval_status: AgentApprovalStatus,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub reason: Option<String>,
}

impl std::fmt::Debug for AgentToolCall {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentToolCall([REDACTED])")
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub safety: AgentToolSafety,
    pub requires_workspace: bool,
    pub requires_approval: bool,
    pub approval_mode: AgentToolApprovalMode,
}

#[derive(Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct AgentToolResult {
    pub call_id: String,
    pub tool: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Backend-only exact projection materialized from streaming captures.
    ///
    /// This file never crosses Protocol, Event, Trace, Checkpoint, or audit serialization. The
    /// generic Exact History boundary consumes it in preference to serializing the bounded
    /// in-memory result.
    #[serde(skip, default)]
    pub exact_archive_file: Option<crate::exact_capture::ExactToolResultArchiveFile>,
}

impl std::fmt::Debug for AgentToolResult {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AgentToolResult([REDACTED])")
    }
}
