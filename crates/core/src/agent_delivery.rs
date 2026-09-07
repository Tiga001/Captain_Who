//! Durable admission receipts for Agent-to-Agent input.
//!
//! Mailbox rows remain the only transport facts. These records answer the narrower question
//! "which caller Turn/model batch consumed this fact?" and make the safe-boundary path and the
//! future `wait_agent` path compete for the same immutable message identity.

use serde::{Deserialize, Serialize};

use crate::{AgentDisplayStatus, AgentMailboxKind, AgentWakeStatus};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentDeliveryPath {
    /// The projection was already part of the prepared Turn input.
    TurnStart,
    /// The item was appended to the active assistant trace at the single pre-sampling boundary.
    SafeBoundary,
    /// The item was returned by the caller-owned wait kernel. A future Tool adapter owns the
    /// corresponding Tool-result trace projection.
    WaitAgent,
}

impl AgentDeliveryPath {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TurnStart => "turn_start",
            Self::SafeBoundary => "safe_boundary",
            Self::WaitAgent => "wait_agent",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "turn_start" => Ok(Self::TurnStart),
            "safe_boundary" => Ok(Self::SafeBoundary),
            "wait_agent" => Ok(Self::WaitAgent),
            _ => Err(format!("unknown Agent delivery path `{value}`")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelBatchReceiptRecord {
    pub receipt_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
    pub sampling_bound_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelBatchReceiptItemRecord {
    pub receipt_id: String,
    pub message_id: String,
    pub ordinal: u32,
    pub mailbox_sequence: u64,
    pub delivery_path: AgentDeliveryPath,
    pub trace_sequence: Option<u64>,
    pub bound_at: i64,
}

/// Immutable first-ready target/status fact captured by a caller-owned wait receipt. Message
/// items alone are insufficient because a wait may become ready solely due to a target status
/// version advancing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelBatchReceiptTargetRecord {
    pub receipt_id: String,
    pub target_agent_id: String,
    pub target_task_name: String,
    pub ordinal: u32,
    pub target_status_version: u64,
    pub latest_wake_sequence: Option<u64>,
    pub latest_wake_status_revision: Option<u64>,
    pub latest_wake_status: Option<AgentWakeStatus>,
    pub display_status: AgentDisplayStatus,
    pub frozen_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDeliveredMailboxMessage {
    pub message_id: String,
    pub sender_agent_id: String,
    pub sender_task_name: String,
    pub sender_task_path: String,
    pub kind: AgentMailboxKind,
    pub content: String,
    pub mailbox_sequence: u64,
    pub delivery_path: AgentDeliveryPath,
    pub trace_sequence: Option<u64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentModelBatchDeliveryRecord {
    pub receipt: AgentModelBatchReceiptRecord,
    pub messages: Vec<AgentDeliveredMailboxMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindAgentTurnStartInput {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindAgentSafeBoundaryInput {
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
    pub expected_next_trace_sequence: u64,
    pub maximum: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCollaborationCursorRecord {
    pub caller_agent_id: String,
    pub run_id: String,
    pub target_agent_id: String,
    pub last_target_status_version: u64,
    pub last_message_sequence: u64,
    pub last_wake_sequence: u64,
    pub last_wake_status_revision: u64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PollAgentWaitInput {
    pub caller_agent_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub model_batch_index: u64,
    pub target_agent_ids: Vec<String>,
    pub maximum_messages: usize,
}

/// Tells the future Harness adapter how the wait result entered the shared Turn history.
///
/// `wait_agent` is unusual: selecting the first-ready facts advances durable cursors. The
/// corresponding ToolResult is therefore committed in the same SQLite transaction. Runtime
/// reproduces that exact prefix and publishes the paired snapshot through the idempotent trace
/// observer; it never executes the wait or consumes its receipt twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWaitModelProjection {
    PrecommittedToolResult,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWaitTargetSnapshot {
    pub target_agent_id: String,
    /// Immutable semantic selector read from the trusted Agent graph, never from model output.
    pub target_task_name: String,
    pub messages: Vec<AgentDeliveredMailboxMessage>,
    /// Monotonic durable semantic version derived from lifecycle plus every Wake status revision.
    pub target_status_version: u64,
    /// Latest durable Wake event version used by the wait cursor. A satisfied coalescing event can
    /// advance this version without replacing the target's active display state.
    pub latest_wake_sequence: Option<u64>,
    pub latest_wake_status_revision: Option<u64>,
    pub latest_wake_status: Option<AgentWakeStatus>,
    pub display_status: AgentDisplayStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentWaitReadySnapshot {
    pub receipt: AgentModelBatchReceiptRecord,
    pub source_receipt_id: Option<String>,
    pub targets: Vec<AgentWaitTargetSnapshot>,
    pub model_projection: AgentWaitModelProjection,
}

/// The sole model projection for both the atomic wait receipt commit and Runtime's returned
/// ToolResult. Delivery/Run identities and cursor versions remain private storage facts.
pub fn agent_wait_model_value(
    targets: &[AgentWaitTargetSnapshot],
) -> crate::AgentResult<serde_json::Value> {
    let targets = targets
        .iter()
        .map(|target| {
            if target.target_task_name.trim().is_empty() {
                return Err(crate::AgentError::new(
                    "Wait target is missing its trusted task name.",
                ));
            }
            let messages = target
                .messages
                .iter()
                .map(|message| {
                    serde_json::json!({
                        "senderTaskName": message.sender_task_name,
                        "kind": message.kind,
                        "content": message.content,
                        "createdAt": message.created_at,
                    })
                })
                .collect::<Vec<_>>();
            Ok(serde_json::json!({
                "taskName": target.target_task_name,
                "status": target.display_status,
                "messages": messages,
            }))
        })
        .collect::<crate::AgentResult<Vec<_>>>()?;
    Ok(serde_json::json!({ "targets": targets }))
}

#[cfg(test)]
mod model_projection_tests {
    use super::*;

    #[test]
    fn wait_projection_exposes_semantic_names_without_private_delivery_metadata() {
        let targets = vec![AgentWaitTargetSnapshot {
            target_agent_id: "private-target-id".into(),
            target_task_name: "reviewer".into(),
            messages: vec![AgentDeliveredMailboxMessage {
                message_id: "private-message-id".into(),
                sender_agent_id: "private-sender-id".into(),
                sender_task_name: "reviewer".into(),
                sender_task_path: "/root/private-path".into(),
                kind: AgentMailboxKind::Message,
                content: "User-authored text may mention agent-example verbatim.".into(),
                mailbox_sequence: 31,
                delivery_path: AgentDeliveryPath::WaitAgent,
                trace_sequence: Some(47),
                created_at: 100,
            }],
            target_status_version: 53,
            latest_wake_sequence: Some(59),
            latest_wake_status_revision: Some(61),
            latest_wake_status: Some(AgentWakeStatus::Running),
            display_status: AgentDisplayStatus::WaitingApproval,
        }];
        let projected = agent_wait_model_value(&targets).unwrap();
        assert_eq!(
            projected,
            serde_json::json!({"targets":[{
                "taskName":"reviewer", "status":"waiting_approval", "messages":[{
                    "senderTaskName":"reviewer", "kind":"message", "createdAt":100,
                    "content":"User-authored text may mention agent-example verbatim.",
                }],
            }]})
        );
        assert!(!projected.to_string().contains("private-"));
    }
}
