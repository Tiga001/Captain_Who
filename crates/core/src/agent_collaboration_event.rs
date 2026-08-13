use serde::{Deserialize, Serialize};

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
