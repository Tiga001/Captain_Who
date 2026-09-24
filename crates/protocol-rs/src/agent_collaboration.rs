use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationSettings {
    pub enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentCollaborationSettingsUpdate {
    pub enabled: bool,
    pub expected_revision: u64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AgentCollaborationSettingsGetInput {}

pub const AGENT_COLLABORATION_SCHEMA_VERSION: u32 = 1;
pub const AGENT_COLLABORATION_EVENT_SCHEMA_VERSION: u32 = 3;
pub const AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION: u32 = 4;

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

/// Output-only display facts copied from the Host-validated history record. A matching JSON
/// string in ordinary message content is not sufficient authority to construct this field.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HumanInteractionResponseDisplayDto {
    #[serde(rename = "type")]
    pub result_type: String,
    pub schema_version: u32,
    pub request_id: String,
    pub response_id: String,
    pub answers: Vec<HumanInteractionAnswerDisplayDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum HumanInteractionAnswerDisplayDto {
    Option {
        question_id: String,
        question: String,
        option_id: String,
        answer: String,
    },
    Text {
        question_id: String,
        question: String,
        answer: String,
    },
    Skipped {
        question_id: String,
        question: String,
        answer: String,
    },
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub human_interaction_response: Option<HumanInteractionResponseDisplayDto>,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_stream: Option<AgentObserverLiveStreamSnapshotDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverStreamCursorDto {
    pub generation: String,
    pub sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverLiveStreamSnapshotDto {
    pub run_id: String,
    pub assistant_message_id: String,
    pub cursor: AgentObserverStreamCursorDto,
    pub stream: Option<AgentObserverLiveStreamDto>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentObserverLiveStreamDto {
    pub stream_id: String,
    pub attempt: usize,
    pub content: String,
    pub trace_boundary_sequence: u64,
    pub committed: bool,
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
    pub activity_id: String,
    pub semantic: CollaborationActivitySemanticDto,
    pub agent_id: String,
    pub task_name_snapshot: String,
    pub owner_agent_id: String,
    pub owner_conversation_id: String,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub task_message_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub anchor_message_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub trace_boundary_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CollaborationTransmissionKindDto {
    Message,
    Task,
    UserMessage,
    Completion,
}

/// Display-only routing facts. A null endpoint denotes the user; message bodies are omitted.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationTransmissionDto {
    pub id: String,
    pub kind: CollaborationTransmissionKindDto,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub source_agent_id: Option<String>,
    #[serde(deserialize_with = "deserialize_required_nullable")]
    pub target_agent_id: Option<String>,
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
    pub activities: Vec<CollaborationActivitySnapshotDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transmission: Option<CollaborationTransmissionDto>,
    pub occurred_at: i64,
}

/// A bare `Option<T>` silently maps a missing JSON field to `None`. Routing through an explicit
/// deserializer distinguishes an explicitly null routing or placement fact from a missing field.
fn deserialize_required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

fn deserialize_optional_transmission<'de, D>(
    deserializer: D,
) -> Result<Option<CollaborationTransmissionDto>, D::Error>
where
    D: Deserializer<'de>,
{
    CollaborationTransmissionDto::deserialize(deserializer).map(Some)
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
    activities: Vec<CollaborationActivitySnapshotDto>,
    #[serde(default, deserialize_with = "deserialize_optional_transmission")]
    transmission: Option<CollaborationTransmissionDto>,
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
        if let Some(transmission) = wire.transmission.as_ref() {
            let valid_id = |id: &str| {
                !id.is_empty() && id.trim() == id && id.len() <= 512 && !id.contains('\0')
            };
            let valid_ids = valid_id(&transmission.id)
                && transmission.source_agent_id.as_deref().is_none_or(valid_id)
                && transmission.target_agent_id.as_deref().is_none_or(valid_id);
            let root_event = wire.agent_id == wire.root_agent_id
                && wire.conversation_id == wire.root_conversation_id;
            let valid_route = match transmission.kind {
                CollaborationTransmissionKindDto::Message
                | CollaborationTransmissionKindDto::Task => {
                    wire.kind == CollaborationEventKindDto::MailboxEnqueued
                        && transmission.source_agent_id.is_some()
                        && transmission.source_agent_id != transmission.target_agent_id
                        && transmission.target_agent_id.as_ref() == Some(&wire.agent_id)
                }
                CollaborationTransmissionKindDto::UserMessage => {
                    root_event
                        && wire.kind == CollaborationEventKindDto::TurnStarted
                        && transmission.source_agent_id.is_none()
                        && transmission.target_agent_id.as_ref() == Some(&wire.root_agent_id)
                }
                CollaborationTransmissionKindDto::Completion => {
                    root_event
                        && wire.kind == CollaborationEventKindDto::TurnUpdated
                        && transmission.source_agent_id.as_ref() == Some(&wire.root_agent_id)
                        && transmission.target_agent_id.is_none()
                }
            };
            if !valid_ids || !valid_route {
                return Err(D::Error::custom(
                    "invalid collaboration transmission identity",
                ));
            }
        }
        let mut activity_ids = std::collections::HashSet::new();
        for activity in &wire.activities {
            if activity.schema_version != AGENT_COLLABORATION_ACTIVITY_SCHEMA_VERSION {
                return Err(D::Error::custom(
                    "unsupported collaboration activity schema",
                ));
            }
            let valid_text = |value: &str, maximum: usize| {
                !value.is_empty()
                    && value.trim() == value
                    && value.len() <= maximum
                    && !value.contains('\0')
            };
            if !valid_text(&activity.activity_id, 2_048)
                || !activity_ids.insert(&activity.activity_id)
                || !valid_text(&activity.agent_id, 256)
                || !valid_text(&activity.task_name_snapshot, 256)
                || !valid_text(&activity.owner_agent_id, 256)
                || !valid_text(&activity.owner_conversation_id, 256)
                || !activity
                    .task_message_id
                    .as_deref()
                    .is_none_or(|value| valid_text(value, 2_048))
                || !activity
                    .anchor_message_id
                    .as_deref()
                    .is_none_or(|value| valid_text(value, 2_048))
            {
                return Err(D::Error::custom(
                    "invalid or duplicate collaboration activity identity",
                ));
            }
            if activity.anchor_message_id.is_none() && activity.trace_boundary_sequence.is_some() {
                return Err(D::Error::custom(
                    "collaboration trace boundary requires an owner message anchor",
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
            let valid_owner = activity.owner_agent_id != activity.agent_id
                && ((activity.owner_agent_id == wire.root_agent_id)
                    == (activity.owner_conversation_id == wire.root_conversation_id));
            let valid_subject = match activity.semantic {
                CollaborationActivitySemanticDto::Updated => {
                    activity.task_message_id.is_none()
                        && activity.agent_id != wire.agent_id
                        && activity.owner_agent_id == wire.agent_id
                        && activity.owner_conversation_id == wire.conversation_id
                }
                _ => {
                    activity.task_message_id.is_some()
                        && activity.agent_id == wire.agent_id
                        && activity.owner_conversation_id != wire.conversation_id
                }
            };
            if !valid_kind || !valid_subject || !valid_owner {
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
            activities: wire.activities,
            transmission: wire.transmission,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_cursor: Option<AgentObserverStreamCursorDto>,
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
    #[serde(deserialize_with = "deserialize_canonical_project_ids")]
    pub project_ids: Vec<String>,
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
    pub template_id: String,
    pub expected_revision: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateDeleteRequest {
    pub template_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentTemplateProjectAssignmentRequest {
    pub project_id: String,
    pub template_id: String,
    pub assigned: bool,
}

fn deserialize_canonical_project_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let project_ids = Vec::<String>::deserialize(deserializer)?;
    if project_ids.len() > 256
        || project_ids.iter().any(|project_id| {
            project_id.is_empty()
                || project_id.trim() != project_id
                || project_id.len() > 512
                || project_id.contains('\0')
        })
        || project_ids
            .windows(2)
            .any(|pair| pair[0].as_bytes() >= pair[1].as_bytes())
    {
        return Err(D::Error::custom(
            "projectIds must be a bounded, canonical sorted unique list",
        ));
    }
    Ok(project_ids)
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
