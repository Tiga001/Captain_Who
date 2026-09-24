use serde::{Deserialize, Serialize};

pub const AGENT_COLLABORATION_EVENT_SCHEMA_VERSION: u32 = 3;
pub const AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION: u32 = 4;

/// Durable root-tree invalidation and routing facts.
///
/// Conversation messages and traces remain the authoritative chat history. Consumers use this
/// append-only log to detect duplicates and gaps, then rehydrate state from the domain tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCollaborationEventKind {
    AgentCreated,
    AgentUpdated,
    MailboxEnqueued,
    MailboxUpdated,
    WakeCreated,
    WakeUpdated,
    TurnStarted,
    TurnUpdated,
    ApprovalProjected,
    ApprovalUpdated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCollaborationTransmissionKind {
    Message,
    Task,
    UserMessage,
    Completion,
}

/// Presentation-only routing read from committed message/Turn facts. No message text is exposed.
/// A missing endpoint represents the user, never an inferred Agent ancestor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationTransmission {
    pub id: String,
    pub kind: AgentCollaborationTransmissionKind,
    pub source_agent_id: Option<String>,
    pub target_agent_id: Option<String>,
}

/// Immutable renderer-safe meaning captured in the same transaction as its collaboration event.
///
/// The outer event's `agent_id` remains the invalidation subject. `agent_id` here is the activity
/// subject, which differs for an ordinary Mailbox message (recipient invalidation, sender
/// presentation). Display ownership follows the actual requester or message recipient, never an
/// inferred structural parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCollaborationActivitySemantic {
    Started,
    Updated,
    WaitingApproval,
    Completed,
    Failed,
    Interrupted,
}

impl AgentCollaborationActivitySemantic {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Updated => "updated",
            Self::WaitingApproval => "waiting_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentCollaborationEventError> {
        match value {
            "started" => Ok(Self::Started),
            "updated" => Ok(Self::Updated),
            "waiting_approval" => Ok(Self::WaitingApproval),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "interrupted" => Ok(Self::Interrupted),
            _ => Err(AgentCollaborationEventError::CorruptRecord),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationActivitySnapshot {
    pub schema_version: u32,
    pub activity_id: String,
    pub semantic: AgentCollaborationActivitySemantic,
    pub agent_id: String,
    pub task_name_snapshot: String,
    pub owner_agent_id: String,
    pub owner_conversation_id: String,
    pub task_message_id: Option<String>,
    /// Actual owner's active assistant message, or its latest committed message when idle.
    pub anchor_message_id: Option<String>,
    /// Inserts before this trace sequence in an active reply. With no trace boundary, placement
    /// is after the anchor message; with no anchor, placement is before the first message.
    pub trace_boundary_sequence: Option<u64>,
}

impl AgentCollaborationEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentCreated => "agent_created",
            Self::AgentUpdated => "agent_updated",
            Self::MailboxEnqueued => "mailbox_enqueued",
            Self::MailboxUpdated => "mailbox_updated",
            Self::WakeCreated => "wake_created",
            Self::WakeUpdated => "wake_updated",
            Self::TurnStarted => "turn_started",
            Self::TurnUpdated => "turn_updated",
            Self::ApprovalProjected => "approval_projected",
            Self::ApprovalUpdated => "approval_updated",
        }
    }

    pub fn parse(value: &str) -> Result<Self, AgentCollaborationEventError> {
        match value {
            "agent_created" => Ok(Self::AgentCreated),
            "agent_updated" => Ok(Self::AgentUpdated),
            "mailbox_enqueued" => Ok(Self::MailboxEnqueued),
            "mailbox_updated" => Ok(Self::MailboxUpdated),
            "wake_created" => Ok(Self::WakeCreated),
            "wake_updated" => Ok(Self::WakeUpdated),
            "turn_started" => Ok(Self::TurnStarted),
            "turn_updated" => Ok(Self::TurnUpdated),
            "approval_projected" => Ok(Self::ApprovalProjected),
            "approval_updated" => Ok(Self::ApprovalUpdated),
            _ => Err(AgentCollaborationEventError::CorruptRecord),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationEventRecord {
    /// Storage-only ordering across roots. It is used by the Core Server notifier and is omitted
    /// from renderer DTOs, whose merge contract is the per-root sequence.
    #[serde(skip_serializing)]
    pub global_sequence: u64,
    pub schema_version: u32,
    pub event_id: String,
    pub root_sequence: u64,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub turn_id: Option<String>,
    pub run_id: Option<String>,
    pub message_id: Option<String>,
    pub kind: AgentCollaborationEventKind,
    pub resource_revision: u64,
    pub activities: Vec<AgentCollaborationActivitySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transmission: Option<AgentCollaborationTransmission>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentCollaborationEventError {
    InvalidInput,
    CorruptRecord,
    StorageUnavailable,
}

impl std::fmt::Display for AgentCollaborationEventError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidInput => "invalid collaboration event query",
            Self::CorruptRecord => "corrupt collaboration event record",
            Self::StorageUnavailable => "collaboration event storage unavailable",
        })
    }
}

impl std::error::Error for AgentCollaborationEventError {}

pub const MAX_AGENT_COLLABORATION_EVENTS_PAGE: usize = 512;
