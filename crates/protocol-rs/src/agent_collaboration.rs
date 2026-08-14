use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

pub const AGENT_COLLABORATION_SCHEMA_VERSION: u32 = 1;
pub const AGENT_COLLABORATION_EVENT_SCHEMA_VERSION: u32 = 2;
pub const AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentLifecycleDto {
    Active,
    Archived,
    Disabled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentDisplayStatusDto {
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

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentModelDisplayDto {
    pub model_config_id: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentSummaryDto {
    pub agent_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub parent_agent_id: Option<String>,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub task_name: String,
    pub task_path: String,
    pub lifecycle: AgentLifecycleDto,
    pub display_status: AgentDisplayStatusDto,
    pub latest_activity_at: i64,
    pub model: Option<AgentModelDisplayDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTreeRequest {
    pub root_conversation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTreeLookupDto {
    pub schema_version: u32,
    pub materialized: bool,
    pub tree: Option<AgentTreeSnapshotDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTreeSnapshotDto {
    pub schema_version: u32,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub agents: Vec<AgentSummaryDto>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDetailRequest {
    pub root_conversation_id: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateBindingDto {
    pub template_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub revision: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentDetailDto {
    pub schema_version: u32,
    pub summary: AgentSummaryDto,
    pub template: Option<AgentTemplateBindingDto>,
    pub reasoning_effort: Option<String>,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConversationLocatorRequest {
    pub root_conversation_id: String,
    pub agent_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentConversationLocatorDto {
    pub schema_version: u32,
    pub agent_id: String,
    pub conversation_id: String,
    pub mode: AgentConversationModeDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverConversationRequest {
    pub root_conversation_id: String,
    pub conversation_id: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentObserverInputOriginKindDto {
    Human,
    Agent,
    HistoricalSnapshot,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverInputOriginDto {
    pub kind: AgentObserverInputOriginKindDto,
    pub sender_agent_id: Option<String>,
    pub source_agent_message_id: Option<String>,
    pub snapshot_source_conversation_id: Option<String>,
    pub snapshot_source_message_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverAttachmentDto {
    pub attachment_id: String,
    pub kind: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub size_bytes: u64,
    pub preview_data: Option<String>,
    pub preview_mime_type: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverMessageDto {
    pub message_id: String,
    pub role: String,
    pub content: String,
    pub created_at: i64,
    pub status: Option<String>,
    pub input_origin: Option<AgentObserverInputOriginDto>,
    pub attachments: Vec<AgentObserverAttachmentDto>,
    pub agent_run_json: Option<String>,
    pub ui_state_json: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverConversationDto {
    pub schema_version: u32,
    pub agent_id: String,
    pub root_conversation_id: String,
    pub conversation_id: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub messages: Vec<AgentObserverMessageDto>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentConversationModeDto {
    Interactive,
    Observer,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationEventKindDto {
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

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationActivitySemanticDto {
    Started,
    Updated,
    WaitingApproval,
    Completed,
    Failed,
    Interrupted,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationActivitySnapshotDto {
    pub schema_version: u32,
    pub semantic: CollaborationActivitySemanticDto,
    pub agent_id: String,
    pub task_name_snapshot: String,
    pub root_anchor_message_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationEventEnvelopeDto {
    pub schema_version: u32,
    pub event_id: String,
    pub sequence: u64,
    pub workspace_id: Option<String>,
    pub project_id: Option<String>,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub turn_id: Option<String>,
    pub run_id: Option<String>,
    pub message_id: Option<String>,
    pub kind: CollaborationEventKindDto,
    pub resource_revision: u64,
    pub activity: Option<CollaborationActivitySnapshotDto>,
    pub occurred_at: i64,
}

/// A bare `Option<T>` silently maps a missing JSON field to `None`. Routing through an explicit
/// deserializer makes event schema v2 distinguish a present `null` activity (known non-semantic
/// event) from an older envelope which predates the projection.
fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationEventEnvelopeWireDto {
    schema_version: u32,
    event_id: String,
    sequence: u64,
    workspace_id: Option<String>,
    project_id: Option<String>,
    root_agent_id: String,
    root_conversation_id: String,
    agent_id: String,
    conversation_id: String,
    turn_id: Option<String>,
    run_id: Option<String>,
    message_id: Option<String>,
    kind: CollaborationEventKindDto,
    resource_revision: u64,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    activity: Option<CollaborationActivitySnapshotDto>,
    occurred_at: i64,
}

impl<'de> Deserialize<'de> for CollaborationEventEnvelopeDto {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = CollaborationEventEnvelopeWireDto::deserialize(deserializer)?;
        if wire.schema_version != AGENT_COLLABORATION_EVENT_SCHEMA_VERSION {
            return Err(D::Error::custom("unsupported collaboration event schema"));
        }
        if wire.workspace_id != wire.project_id {
            return Err(D::Error::custom(
                "collaboration event workspace/project identity mismatch",
            ));
        }
        if let Some(activity) = wire.activity.as_ref() {
            if activity.schema_version != AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
                return Err(D::Error::custom(
                    "unsupported collaboration activity schema",
                ));
            }
            let valid_kind = matches!(
                (activity.semantic, wire.kind),
                (
                    CollaborationActivitySemanticDto::Started,
                    CollaborationEventKindDto::WakeCreated
                ) | (
                    CollaborationActivitySemanticDto::Updated,
                    CollaborationEventKindDto::MailboxEnqueued
                ) | (
                    CollaborationActivitySemanticDto::WaitingApproval,
                    CollaborationEventKindDto::ApprovalProjected
                ) | (
                    CollaborationActivitySemanticDto::Completed
                        | CollaborationActivitySemanticDto::Failed
                        | CollaborationActivitySemanticDto::Interrupted,
                    CollaborationEventKindDto::WakeUpdated
                )
            );
            let valid_subject = match activity.semantic {
                CollaborationActivitySemanticDto::Updated => activity.agent_id != wire.agent_id,
                _ => activity.agent_id == wire.agent_id,
            };
            if !valid_kind || !valid_subject {
                return Err(D::Error::custom(
                    "collaboration activity kind/subject mismatch",
                ));
            }
        }

        Ok(Self {
            schema_version: wire.schema_version,
            event_id: wire.event_id,
            sequence: wire.sequence,
            workspace_id: wire.workspace_id,
            project_id: wire.project_id,
            root_agent_id: wire.root_agent_id,
            root_conversation_id: wire.root_conversation_id,
            agent_id: wire.agent_id,
            conversation_id: wire.conversation_id,
            turn_id: wire.turn_id,
            run_id: wire.run_id,
            message_id: wire.message_id,
            kind: wire.kind,
            resource_revision: wire.resource_revision,
            activity: wire.activity,
            occurred_at: wire.occurred_at,
        })
    }
}

/// Process-local child stream. Durable observer snapshots and the collaboration event log remain
/// authoritative across restart; these exact identities only authorize reuse of the existing
/// Renderer Agent-event reducer while the child is live.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverEventEnvelopeDto {
    pub schema_version: u32,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub run_id: String,
    pub assistant_message_id: String,
    pub event: serde_json::Value,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationEventsRequest {
    pub root_conversation_id: String,
    pub after_sequence: u64,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationEventsPageDto {
    pub schema_version: u32,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub events: Vec<CollaborationEventEnvelopeDto>,
    pub last_sequence: u64,
    pub has_more: bool,
}

/// Process-generation invalidation. Unlike a durable collaboration event this envelope is
/// intentionally global: every already-mounted root store must rehydrate after Core starts or
/// restarts, because the notifier's durable scan has no renderer-owned global cursor.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationResyncReasonDto {
    CoreStarted,
    ModelSettingsChanged,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationResyncEnvelopeDto {
    pub schema_version: u32,
    pub reason: CollaborationResyncReasonDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateDto {
    pub schema_version: u32,
    pub template_id: String,
    pub project_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
    pub model_display_name: Option<String>,
    pub enabled: bool,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateListRequest {
    pub project_id: String,
    pub include_disabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateListDto {
    pub schema_version: u32,
    pub templates: Vec<AgentTemplateDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateCreateRequest {
    pub template_id: String,
    pub project_id: String,
    pub machine_key: String,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateUpdateRequest {
    pub project_id: String,
    pub template_id: String,
    pub expected_revision: u64,
    pub name: String,
    pub description: String,
    pub instructions: String,
    pub model_config_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateSetEnabledRequest {
    pub project_id: String,
    pub template_id: String,
    pub expected_revision: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateDeleteRequest {
    pub project_id: String,
    pub template_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationApprovalStatusDto {
    Pending,
    Approved,
    Executing,
    Rejected,
    Cancelled,
    Completed,
    Failed,
    Expired,
    Interrupted,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalProjectionDto {
    pub schema_version: u32,
    pub approval_id: String,
    pub root_agent_id: String,
    pub root_conversation_id: String,
    pub source_agent_id: String,
    pub source_task_path: String,
    pub source_conversation_id: String,
    pub run_id: String,
    pub action_id: String,
    pub action_type: String,
    pub tool_name: String,
    pub action: serde_json::Value,
    pub status: CollaborationApprovalStatusDto,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalListRequest {
    pub root_conversation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalListDto {
    pub schema_version: u32,
    pub approvals: Vec<CollaborationApprovalProjectionDto>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationApprovalDecisionDto {
    Approve,
    Reject,
    Cancel,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalDecisionRequest {
    pub root_conversation_id: String,
    pub approval_id: String,
    pub decision: CollaborationApprovalDecisionDto,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationApprovalDecisionResultDto {
    pub schema_version: u32,
    pub approval_id: String,
    pub accepted: bool,
    pub already_settled: bool,
    pub status: CollaborationApprovalStatusDto,
}
