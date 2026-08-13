use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt;

use crate::provider_profile::ReasoningEffort;

pub const AGENT_GRAPH_SCHEMA_VERSION: u32 = 1;

/// Derives the one trusted root-Agent identity for a Conversation.
///
/// Keeping this in core prevents a lazily enabled Harness and a collaboration-owned
/// Conversation fork from inventing different root identities for the same durable Conversation.
pub fn root_agent_id_for_conversation(conversation_id: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(conversation_id.as_bytes()));
    format!("agent-root-{}", &digest[..32])
}

/// Derives the stable idempotency key used whenever core binds a Conversation to its root Agent.
pub fn root_agent_creation_request_id(conversation_id: &str) -> String {
    let digest = format!("{:x}", Sha256::digest(conversation_id.as_bytes()));
    format!("harness-root-{}", &digest[..32])
}

/// Applies the persisted root task-name constraints without changing the user-visible title.
pub fn bounded_root_agent_task_name(title: &str) -> String {
    let normalized = title
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let title = normalized.trim();
    let title = if title.is_empty() { "Root" } else { title };
    let mut boundary = title.len().min(256);
    while boundary > 0 && !title.is_char_boundary(boundary) {
        boundary -= 1;
    }
    title[..boundary].to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycle {
    Active,
    Archived,
    Disabled,
}

impl AgentLifecycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Archived => "archived",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "active" => Ok(Self::Active),
            "archived" => Ok(Self::Archived),
            "disabled" => Ok(Self::Disabled),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent lifecycle `{value}`"
            ))),
        }
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self == next || matches!(next, Self::Active | Self::Archived | Self::Disabled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMailboxKind {
    Task,
    Message,
    Followup,
    Result,
}

impl AgentMailboxKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Message => "message",
            Self::Followup => "followup",
            Self::Result => "result",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "task" => Ok(Self::Task),
            "message" => Ok(Self::Message),
            "followup" => Ok(Self::Followup),
            "result" => Ok(Self::Result),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent mailbox kind `{value}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMailboxDeliveryStatus {
    Queued,
    Claimed,
    Acknowledged,
}

impl AgentMailboxDeliveryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Acknowledged => "acknowledged",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "queued" => Ok(Self::Queued),
            "claimed" => Ok(Self::Claimed),
            "acknowledged" => Ok(Self::Acknowledged),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent mailbox delivery status `{value}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWakeStatus {
    Queued,
    Claimed,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Interrupted,
    Cancelled,
    OutcomeUnknown,
    /// A deferred Wake whose source message was absorbed by the Agent's already-running Turn.
    /// This is a successful coordination outcome, not a cancellation or a new Turn.
    Satisfied,
}

impl AgentWakeStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Claimed => "claimed",
            Self::Running => "running",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::Cancelled => "cancelled",
            Self::OutcomeUnknown => "outcome_unknown",
            Self::Satisfied => "satisfied",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "queued" => Ok(Self::Queued),
            "claimed" => Ok(Self::Claimed),
            "running" => Ok(Self::Running),
            "waiting_for_approval" => Ok(Self::WaitingForApproval),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            "cancelled" => Ok(Self::Cancelled),
            "outcome_unknown" => Ok(Self::OutcomeUnknown),
            "satisfied" => Ok(Self::Satisfied),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent wake status `{value}`"
            ))),
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed
                | Self::Failed
                | Self::Interrupted
                | Self::Cancelled
                | Self::OutcomeUnknown
                | Self::Satisfied
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Queued, Self::Claimed | Self::Cancelled)
                    | (Self::Queued, Self::Satisfied)
                    | (
                        Self::Claimed,
                        Self::Queued
                            | Self::Running
                            | Self::Failed
                            | Self::Interrupted
                            | Self::Cancelled
                            | Self::OutcomeUnknown
                    )
                    | (
                        Self::Running,
                        Self::WaitingForApproval
                            | Self::Completed
                            | Self::Failed
                            | Self::Interrupted
                            | Self::Cancelled
                            | Self::OutcomeUnknown
                    )
                    | (
                        Self::WaitingForApproval,
                        Self::Running
                            | Self::Completed
                            | Self::Failed
                            | Self::Interrupted
                            | Self::Cancelled
                            | Self::OutcomeUnknown
                    )
            )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationMessageOrigin {
    Human,
    Agent {
        sender_agent_id: String,
        source_agent_message_id: String,
    },
    /// Immutable actor provenance copied into a child's creation-time history snapshot.
    HistoricalSnapshot {
        source_conversation_id: String,
        source_message_id: String,
        original: Box<ConversationMessageOrigin>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplateSnapshot {
    pub template_id: String,
    pub project_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub template_revision: u64,
    pub model_config_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelSelectionSnapshot {
    pub model_config_id: String,
    pub display_name: String,
    pub supports_image: bool,
    pub effective_context_window_tokens: u32,
    pub model_settings_configuration_revision: String,
    pub provider_connection_revision: String,
    pub provider_protocol_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentNodeRecord {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub parent_agent_id: Option<String>,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub creation_request_id: String,
    pub task_name: String,
    pub task_path: String,
    pub template_snapshot: Option<AgentTemplateSnapshot>,
    pub model_snapshot: Option<AgentModelSelectionSnapshot>,
    /// Creation-time selector provenance. Roots have no frozen model selection; every child has
    /// exactly one source so idempotent retries cannot change selector semantics accidentally.
    pub model_selection_source: Option<AgentModelSelectionSource>,
    /// Optional spawn-time reasoning constraint. `None` means the child follows the selected
    /// model's ordinary persisted Provider Profile; `High`/`Max` mean the parent explicitly
    /// required that exact, already-configured capability. Roots never carry this snapshot.
    pub reasoning_effort_snapshot: Option<ReasoningEffort>,
    pub lifecycle: AgentLifecycle,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgentNodeInput {
    pub agent_id: String,
    pub root_agent_id: String,
    pub parent_agent_id: String,
    pub conversation_id: String,
    pub creation_request_id: String,
    pub task_name: String,
    pub task_path: String,
    pub template_snapshot: Option<AgentTemplateSnapshot>,
    pub model_snapshot: AgentModelSelectionSnapshot,
}

/// Creation-time history policy for a child Agent.
///
/// `Last` counts complete logical user/assistant turns as defined by the Conversation context
/// boundary. It never means a raw number of rows in `messages`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "count", rename_all = "snake_case")]
pub enum AgentForkTurns {
    None,
    All,
    Last(u32),
}

impl AgentForkTurns {
    pub fn validate(self) -> Result<Self, ChildAgentSpawnError> {
        if matches!(self, Self::Last(0)) {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "fork_turns",
                reason: "last-turn count must be greater than zero".to_string(),
            });
        }
        Ok(self)
    }
}

/// Host-internal request for atomically creating one direct child.
///
/// Stable object identifiers are deliberately absent. The persistence boundary owns them so an
/// idempotent retry can return the first committed bundle instead of trusting caller-selected
/// graph/message identities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateChildAgentInput {
    pub parent_agent_id: String,
    pub creation_request_id: String,
    pub task_name: String,
    pub task: String,
    pub template_machine_key: Option<String>,
    pub explicit_model_id: Option<String>,
    /// Optional exact constraint over the selected model's persisted Provider Profile. High/Max
    /// are accepted only when the current registered Runtime already exposes that exact enabled
    /// policy; this never mutates or overlays Provider configuration for one run.
    pub reasoning_effort: Option<ReasoningEffort>,
    pub fork_turns: AgentForkTurns,
}

/// Host-selected hard limits for one atomic child spawn.
///
/// The values are carried to SQLite rather than checked only in a model Tool adapter, so two
/// concurrent spawns cannot both observe the last available tree slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTreeResourceLimits {
    pub max_depth: u32,
    pub max_nodes: u32,
    pub max_task_bytes: usize,
}

impl Default for AgentTreeResourceLimits {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_nodes: 64,
            max_task_bytes: 64 * 1024,
        }
    }
}

impl AgentTreeResourceLimits {
    pub fn validate(self) -> Result<Self, ChildAgentSpawnError> {
        if self.max_depth == 0 || self.max_depth > 32 {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "max_depth",
                reason: "must be between 1 and 32".to_string(),
            });
        }
        if self.max_nodes < 2 || self.max_nodes > 1_024 {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "max_nodes",
                reason: "must be between 2 and 1024".to_string(),
            });
        }
        if self.max_task_bytes == 0 || self.max_task_bytes > 1_048_576 {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "max_task_bytes",
                reason: "must be between 1 and 1048576".to_string(),
            });
        }
        Ok(self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentModelSelectionSource {
    Explicit,
    Template,
    Parent,
    Default,
}

impl AgentModelSelectionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Template => "template",
            Self::Parent => "parent",
            Self::Default => "default",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "explicit" => Ok(Self::Explicit),
            "template" => Ok(Self::Template),
            "parent" => Ok(Self::Parent),
            "default" => Ok(Self::Default),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent model selection source `{value}`"
            ))),
        }
    }
}

/// Trusted collaboration identity supplied to the shared Turn executor by the Host.
///
/// This type has no permissive defaults and is never a model-facing tool argument. The entrusted
/// task remains the durable Mailbox payload identified by `source_agent_message_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationIdentity {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub parent_agent_id: String,
    pub parent_task_name: String,
    pub parent_task_path: String,
    pub conversation_id: String,
    pub task_name: String,
    pub task_path: String,
    /// Authenticated sender of the collaboration input which admitted this Wake. It may be the
    /// direct parent, another ancestor (follow-up), or a direct child (result).
    pub source_agent_id: String,
    pub source_kind: AgentMailboxKind,
    pub source_task_name: String,
    pub source_task_path: String,
    pub source_agent_message_id: String,
    pub entrusted_task: String,
    pub template_instructions: Option<String>,
}

impl AgentCollaborationIdentity {
    /// Validates the Host-owned identity before it is admitted to a model run or durable resume.
    pub fn validate(&self) -> Result<(), ChildAgentSpawnError> {
        const MAX_ID_BYTES: usize = 256;
        const MAX_NAME_BYTES: usize = 256;
        const MAX_PATH_BYTES: usize = 4_096;
        const MAX_TASK_BYTES: usize = 1_048_576;
        const MAX_INSTRUCTIONS_BYTES: usize = 65_536;

        fn bounded(
            field: &'static str,
            value: &str,
            maximum: usize,
        ) -> Result<(), ChildAgentSpawnError> {
            if value.trim().is_empty()
                || value.trim() != value
                || value.len() > maximum
                || value.chars().any(char::is_control)
            {
                return Err(ChildAgentSpawnError::InvalidInput {
                    field,
                    reason: format!(
                        "must be non-empty, trimmed, and no longer than {maximum} bytes"
                    ),
                });
            }
            Ok(())
        }

        fn bounded_multiline(
            field: &'static str,
            value: &str,
            maximum: usize,
        ) -> Result<(), ChildAgentSpawnError> {
            if value.trim().is_empty()
                || value.trim() != value
                || value.len() > maximum
                || value.contains('\0')
            {
                return Err(ChildAgentSpawnError::InvalidInput {
                    field,
                    reason: format!(
                        "must be non-empty, trimmed, NUL-free, and no longer than {maximum} bytes"
                    ),
                });
            }
            Ok(())
        }

        for (field, value) in [
            ("agent_id", self.agent_id.as_str()),
            ("root_agent_id", self.root_agent_id.as_str()),
            ("root_conversation_id", self.root_conversation_id.as_str()),
            ("parent_agent_id", self.parent_agent_id.as_str()),
            ("source_agent_id", self.source_agent_id.as_str()),
            ("conversation_id", self.conversation_id.as_str()),
            (
                "source_agent_message_id",
                self.source_agent_message_id.as_str(),
            ),
        ] {
            bounded(field, value, MAX_ID_BYTES)?;
        }
        bounded("parent_task_name", &self.parent_task_name, MAX_NAME_BYTES)?;
        bounded("source_task_name", &self.source_task_name, MAX_NAME_BYTES)?;
        bounded("task_name", &self.task_name, MAX_NAME_BYTES)?;
        bounded("parent_task_path", &self.parent_task_path, MAX_PATH_BYTES)?;
        bounded("source_task_path", &self.source_task_path, MAX_PATH_BYTES)?;
        bounded("task_path", &self.task_path, MAX_PATH_BYTES)?;
        bounded_multiline("entrusted_task", &self.entrusted_task, MAX_TASK_BYTES)?;
        if let Some(instructions) = self.template_instructions.as_deref() {
            bounded_multiline(
                "template_instructions",
                instructions,
                MAX_INSTRUCTIONS_BYTES,
            )?;
        }
        if self.agent_id == self.root_agent_id || self.agent_id == self.parent_agent_id {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "agent_id",
                reason: "a child Agent must differ from its root and parent".to_string(),
            });
        }
        let expected_path = format!(
            "{}/{}",
            self.parent_task_path.trim_end_matches('/'),
            self.task_name
        );
        if self.task_path != expected_path {
            return Err(ChildAgentSpawnError::InvalidInput {
                field: "task_path",
                reason: "must be the direct child path of parent_task_path".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildAgentSpawnRecord {
    pub agent: AgentNodeRecord,
    pub model_selection_source: AgentModelSelectionSource,
    pub task_message: AgentMailboxMessageRecord,
    pub initial_wake: AgentWakeRequestRecord,
    pub collaboration_identity: AgentCollaborationIdentity,
}

/// Durable authorization recovered for a running or approval-paused child Turn.
///
/// `claim_token` is never model input. The Host uses it only to bind lifecycle transitions back
/// to the exact active Wake after a checkpoint/resume boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedActiveChildWakeBundle {
    pub spawn: ChildAgentSpawnRecord,
    pub claim_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentModelUnavailableReason {
    SettingsMissing,
    NotFound,
    Disabled,
    InvalidConnection,
    InvalidProfile,
    MissingConnectionIdentity,
    MissingProtocolIdentity,
    UnsupportedRuntime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildAgentSpawnError {
    InvalidInput {
        field: &'static str,
        reason: String,
    },
    ParentNotFound(String),
    ParentUnavailable(String),
    ProjectRequiredForTemplate,
    TemplateNotFound(String),
    TemplateDisabled(String),
    ModelUnavailable {
        model_config_id: Option<String>,
        reason: AgentModelUnavailableReason,
    },
    UnsupportedReasoningEffort(ReasoningEffort),
    ResourceLimit {
        resource: &'static str,
        limit: u64,
    },
    IdempotencyConflict(String),
    Conflict(String),
    SnapshotUnavailable(String),
    CorruptRecord(String),
    StorageUnavailable(String),
}

impl fmt::Display for ChildAgentSpawnError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::ParentNotFound(id) => write!(formatter, "parent Agent `{id}` was not found"),
            Self::ParentUnavailable(id) => write!(formatter, "parent Agent `{id}` is unavailable"),
            Self::ProjectRequiredForTemplate => {
                formatter.write_str("a project-bound Agent template requires a project")
            }
            Self::TemplateNotFound(key) => {
                write!(formatter, "Agent template `{key}` was not found")
            }
            Self::TemplateDisabled(key) => {
                write!(formatter, "Agent template `{key}` is disabled")
            }
            Self::ModelUnavailable {
                model_config_id,
                reason,
            } => write!(
                formatter,
                "Agent model `{}` is unavailable: {reason:?}",
                model_config_id.as_deref().unwrap_or("<default>")
            ),
            Self::UnsupportedReasoningEffort(effort) => write!(
                formatter,
                "reasoning effort constraint `{effort:?}` is unsupported by the selected model"
            ),
            Self::ResourceLimit { resource, limit } => {
                write!(formatter, "child Agent {resource} limit exceeded ({limit})")
            }
            Self::IdempotencyConflict(reason) => {
                write!(formatter, "child Agent request was reused: {reason}")
            }
            Self::Conflict(reason) => write!(formatter, "child Agent conflict: {reason}"),
            Self::SnapshotUnavailable(reason) => {
                write!(
                    formatter,
                    "child Agent context snapshot is unavailable: {reason}"
                )
            }
            Self::CorruptRecord(reason) => {
                write!(formatter, "corrupt child Agent record: {reason}")
            }
            Self::StorageUnavailable(reason) => {
                write!(formatter, "child Agent storage is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for ChildAgentSpawnError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnsureRootAgentInput {
    pub agent_id: String,
    pub conversation_id: String,
    pub creation_request_id: String,
    pub task_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentMailboxMessageRecord {
    pub sequence: u64,
    pub message_id: String,
    pub root_agent_id: String,
    pub sender_agent_id: String,
    pub recipient_agent_id: String,
    pub request_id: String,
    pub kind: AgentMailboxKind,
    pub content: String,
    pub projection_message_id: String,
    pub delivery_status: AgentMailboxDeliveryStatus,
    pub claim_token: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub created_at: i64,
    pub claimed_at: Option<i64>,
    pub acknowledged_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueAgentMessageInput {
    pub message_id: String,
    pub root_agent_id: String,
    pub sender_agent_id: String,
    pub recipient_agent_id: String,
    pub request_id: String,
    pub kind: AgentMailboxKind,
    pub content: String,
    pub projection_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWakeRequestRecord {
    pub sequence: u64,
    pub wake_id: String,
    pub root_agent_id: String,
    pub agent_id: String,
    pub requester_agent_id: String,
    pub request_id: String,
    pub source_agent_message_id: Option<String>,
    pub status: AgentWakeStatus,
    /// Monotonic durable version used by observers. It advances on every real Wake state change,
    /// but not on a lease-only renewal.
    pub status_revision: u64,
    pub claim_token: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub result_message_id: Option<String>,
    pub terminal_error: Option<String>,
    /// Exact shared-Turn identities once execution has been admitted. Dispatch failures that
    /// happen before Turn preparation intentionally keep both fields empty.
    pub run_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub created_at: i64,
    pub claimed_at: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueAgentWakeInput {
    pub wake_id: String,
    pub root_agent_id: String,
    pub agent_id: String,
    pub requester_agent_id: String,
    pub request_id: String,
    pub source_agent_message_id: Option<String>,
}

/// Host-only capability used while atomically admitting a claimed Wake into the shared Turn
/// executor. The storage transaction binds these immutable facts to the newly created durable
/// Conversation trace before the Runtime is allowed to sample a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedAgentWakeTurnAdmission {
    pub agent_id: String,
    pub wake_id: String,
    pub claim_token: String,
    pub source_agent_message_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeAgentTaskAndWakeInput {
    pub message_id: String,
    pub message_claim_token: String,
    pub wake: EnqueueAgentWakeInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// Round-1 compatibility input for the low-level repository settlement primitive.
///
/// Production Dispatcher/Host code must use [`FinishAgentTurnResultInput`] so the result payload,
/// Artifact snapshot, direct-parent routing, and optional pre-admission failure identity are
/// created by the trusted typed service rather than supplied by a caller.
pub struct FinishAgentWakeWithResultInput {
    pub wake_id: String,
    pub expected_status: AgentWakeStatus,
    pub claim_token: String,
    pub terminal_status: AgentWakeStatus,
    pub terminal_error: Option<String>,
    pub result_message: EnqueueAgentMessageInput,
}

pub const AGENT_RESULT_ENVELOPE_SCHEMA_VERSION: u32 = 1;
pub const AGENT_RESULT_SUMMARY_MAX_BYTES: usize = 16_384;
pub const AGENT_RESULT_TERMINAL_ERROR_MAX_BYTES: usize = 4_096;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResultArtifactKind {
    Image,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentResultArtifactReference {
    pub artifact_id: String,
    pub kind: AgentResultArtifactKind,
    pub media_type: String,
}

/// The bounded, immutable payload placed in the direct parent's Mailbox when a delegated Turn
/// settles. Usage is deliberately absent: accounting remains owned by the child's Conversation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTurnResultEnvelope {
    pub schema_version: u32,
    pub child_agent_id: String,
    pub task_name: String,
    pub task_path: String,
    pub wake_id: String,
    pub turn_id: Option<String>,
    pub run_id: Option<String>,
    pub status: AgentWakeStatus,
    pub summary: String,
    pub artifact_refs: Vec<AgentResultArtifactReference>,
    pub terminal_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishAgentTurnResultInput {
    pub wake_id: String,
    pub expected_status: AgentWakeStatus,
    pub claim_token: String,
    pub terminal_status: AgentWakeStatus,
    /// `None`/`None` is valid for a dispatch failure before a Turn was prepared.
    pub run_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub summary: String,
    pub terminal_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTurnResultSettlement {
    pub wake: AgentWakeRequestRecord,
    pub result_message: AgentMailboxMessageRecord,
    /// Non-root direct parents receive a deferred Wake. Root results remain pending for the next
    /// user-driven Turn and therefore never create this value.
    pub parent_wake: Option<AgentWakeRequestRecord>,
    pub envelope: AgentTurnResultEnvelope,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendAgentMessageRequest {
    pub sender_agent_id: String,
    pub recipient_agent_id: String,
    pub request_id: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessageDispatch {
    pub message: AgentMailboxMessageRecord,
    /// Present only for follow-up. A plain send is deliberately queue-only.
    pub deferred_wake: Option<AgentWakeRequestRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDisplayStatus {
    Idle,
    Queued,
    Running,
    WaitingApproval,
    LatestCompleted,
    LatestFailed,
    LatestInterrupted,
    LatestOutcomeUnknown,
    Archived,
    Disabled,
}

impl AgentDisplayStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Running => "running",
            Self::WaitingApproval => "waiting_approval",
            Self::LatestCompleted => "latest_completed",
            Self::LatestFailed => "latest_failed",
            Self::LatestInterrupted => "latest_interrupted",
            Self::LatestOutcomeUnknown => "latest_outcome_unknown",
            Self::Archived => "archived",
            Self::Disabled => "disabled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentGraphError> {
        match value {
            "idle" => Ok(Self::Idle),
            "queued" => Ok(Self::Queued),
            "running" => Ok(Self::Running),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "latest_completed" => Ok(Self::LatestCompleted),
            "latest_failed" => Ok(Self::LatestFailed),
            "latest_interrupted" => Ok(Self::LatestInterrupted),
            "latest_outcome_unknown" => Ok(Self::LatestOutcomeUnknown),
            "archived" => Ok(Self::Archived),
            "disabled" => Ok(Self::Disabled),
            _ => Err(AgentGraphError::CorruptRecord(format!(
                "unknown Agent display status `{value}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDisplayStatusSnapshot {
    pub agent_id: String,
    pub status: AgentDisplayStatus,
    pub agent_revision: u64,
    pub latest_wake_id: Option<String>,
    pub latest_wake_sequence: Option<u64>,
    pub latest_wake_status_revision: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentWakeRecoveryAction {
    /// The exact Turn is terminal or has a resumable approval checkpoint. The Host may observe it
    /// using the rebound claim token; it must never start the model from the beginning.
    Observe(AgentWakeRequestRecord),
    /// Atomic Turn admission committed, but the pre-Runtime preparation was rolled back before a
    /// trace survived. This is definitely-not-dispatched and may be failed without replay.
    FailBeforeRuntime(AgentWakeRequestRecord),
    /// Execution crossed a durable dispatch boundary but its effects cannot be proven after the
    /// process disappeared. The only safe recovery is an explicit outcome-unknown result.
    OutcomeUnknown(AgentWakeRequestRecord),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AgentWakeRecoveryBatch {
    pub requeued_before_dispatch: usize,
    pub actions: Vec<AgentWakeRecoveryAction>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterruptAgentExecutionOutcome {
    NoPendingExecution,
    QueuedWakeCancelled { wake_id: String },
    ActiveTurn { wake_id: String, run_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentTemplateRecord {
    pub template_id: String,
    pub project_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgentTemplateInput {
    pub template_id: String,
    pub project_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAgentTemplateInput {
    pub project_id: String,
    pub template_id: String,
    pub expected_revision: u64,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedAgentTemplateForSpawn {
    pub template: AgentTemplateSnapshot,
    pub model: AgentModelSelectionSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdempotentCreate<T> {
    Created(T),
    Existing(T),
}

impl<T> IdempotentCreate<T> {
    pub fn record(&self) -> &T {
        match self {
            Self::Created(record) | Self::Existing(record) => record,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentGraphError {
    InvalidInput {
        field: &'static str,
        reason: String,
    },
    ConversationNotFound(String),
    AgentNotFound(String),
    MessageNotFound(String),
    WakeNotFound(String),
    RevisionConflict {
        expected: u64,
        current: u64,
    },
    ResourceLimit {
        resource: &'static str,
        limit: u64,
    },
    Conflict(String),
    IllegalLifecycleTransition {
        current: AgentLifecycle,
        requested: AgentLifecycle,
    },
    IllegalTransition {
        current: AgentWakeStatus,
        requested: AgentWakeStatus,
    },
    BoundConversation(String),
    BoundProject(String),
    CorruptRecord(String),
    StorageUnavailable(String),
}

impl fmt::Display for AgentGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::ConversationNotFound(id) => {
                write!(formatter, "conversation `{id}` was not found")
            }
            Self::AgentNotFound(id) => write!(formatter, "Agent `{id}` was not found"),
            Self::MessageNotFound(id) => write!(formatter, "Agent message `{id}` was not found"),
            Self::WakeNotFound(id) => write!(formatter, "Agent wake `{id}` was not found"),
            Self::RevisionConflict { expected, current } => write!(
                formatter,
                "Agent revision conflict: expected {expected}, current {current}"
            ),
            Self::ResourceLimit { resource, limit } => {
                write!(
                    formatter,
                    "Agent {resource} resource limit exceeded ({limit})"
                )
            }
            Self::Conflict(reason) => write!(formatter, "Agent graph conflict: {reason}"),
            Self::IllegalLifecycleTransition { current, requested } => write!(
                formatter,
                "illegal Agent lifecycle transition from {} to {}",
                current.as_str(),
                requested.as_str()
            ),
            Self::IllegalTransition { current, requested } => write!(
                formatter,
                "illegal Agent wake transition from {} to {}",
                current.as_str(),
                requested.as_str()
            ),
            Self::BoundConversation(id) => write!(
                formatter,
                "conversation `{id}` belongs to a persistent Agent tree"
            ),
            Self::BoundProject(id) => {
                write!(formatter, "project `{id}` contains a persistent Agent tree")
            }
            Self::CorruptRecord(reason) => {
                write!(formatter, "corrupt Agent graph record: {reason}")
            }
            Self::StorageUnavailable(reason) => {
                write!(formatter, "Agent graph storage is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for AgentGraphError {}

pub type AgentTemplateModelUnavailableReason = AgentModelUnavailableReason;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTemplateError {
    InvalidInput {
        field: &'static str,
        reason: String,
    },
    ProjectNotFound(String),
    TemplateNotFound(String),
    MachineKeyConflict(String),
    NameConflict(String),
    RevisionConflict {
        expected: u64,
        current: u64,
    },
    RevisionExhausted,
    TemplateInUse {
        template_id: String,
        agent_count: u64,
    },
    TemplateDisabled(String),
    ModelUnavailable {
        model_config_id: String,
        reason: AgentTemplateModelUnavailableReason,
    },
    CorruptRecord(String),
    StorageUnavailable(String),
}

impl fmt::Display for AgentTemplateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput { field, reason } => write!(formatter, "invalid {field}: {reason}"),
            Self::ProjectNotFound(id) => write!(formatter, "project `{id}` was not found"),
            Self::TemplateNotFound(id) => write!(formatter, "Agent template `{id}` was not found"),
            Self::MachineKeyConflict(key) => {
                write!(
                    formatter,
                    "Agent template machine key `{key}` already exists"
                )
            }
            Self::NameConflict(name) => {
                write!(formatter, "Agent template name `{name}` already exists")
            }
            Self::RevisionConflict { expected, current } => write!(
                formatter,
                "Agent template revision conflict: expected {expected}, current {current}"
            ),
            Self::RevisionExhausted => formatter.write_str("Agent template revision is exhausted"),
            Self::TemplateInUse {
                template_id,
                agent_count,
            } => write!(
                formatter,
                "Agent template `{template_id}` is snapshotted by {agent_count} Agent(s)"
            ),
            Self::TemplateDisabled(key) => write!(formatter, "Agent template `{key}` is disabled"),
            Self::ModelUnavailable {
                model_config_id,
                reason,
            } => write!(
                formatter,
                "Agent template model `{model_config_id}` is unavailable: {reason:?}"
            ),
            Self::CorruptRecord(reason) => write!(formatter, "corrupt Agent template: {reason}"),
            Self::StorageUnavailable(reason) => {
                write!(formatter, "Agent template storage is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for AgentTemplateError {}
