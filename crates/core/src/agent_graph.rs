use serde::{Deserialize, Serialize};
use std::fmt;

pub const AGENT_GRAPH_SCHEMA_VERSION: u32 = 1;

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
        )
    }

    pub fn can_transition_to(self, next: Self) -> bool {
        self == next
            || matches!(
                (self, next),
                (Self::Queued, Self::Claimed | Self::Cancelled)
                    | (
                        Self::Claimed,
                        Self::Queued
                            | Self::Running
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
    pub claim_token: Option<String>,
    pub lease_expires_at: Option<i64>,
    pub result_message_id: Option<String>,
    pub terminal_error: Option<String>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcknowledgeAgentTaskAndWakeInput {
    pub message_id: String,
    pub message_claim_token: String,
    pub wake: EnqueueAgentWakeInput,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishAgentWakeWithResultInput {
    pub wake_id: String,
    pub expected_status: AgentWakeStatus,
    pub claim_token: String,
    pub terminal_status: AgentWakeStatus,
    pub terminal_error: Option<String>,
    pub result_message: EnqueueAgentMessageInput,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentTemplateModelUnavailableReason {
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
