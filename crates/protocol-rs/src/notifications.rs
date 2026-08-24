use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::collections::HashSet;

pub const NOTIFICATION_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKindDto {
    TaskCompleted,
    TaskFailed,
    TaskCancelled,
    ApprovalRequired,
    AutomationCompleted,
    AutomationFailed,
    AutomationCancelled,
    AutomationImportantUpdate,
    AutomationConfigurationBlocked,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSourceKindDto {
    HumanRoot,
    Automation,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSubjectKindDto {
    PromptExcerpt,
    AutomationTitle,
    AttachmentTask,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPriorityDto {
    Completed,
    Cancelled,
    ImportantUpdate,
    Failed,
    ConfigurationBlocked,
    ApprovalRequired,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationBatchStatusDto {
    Collecting,
    Pending,
    Claimed,
    Displayed,
    Sealed,
    Suppressed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationSoundLevelDto {
    None,
    Initial,
    Upgrade,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryDispositionDto {
    Delivered,
    SuppressedForeground,
    SuppressedStale,
    SuppressedDeleted,
    SuppressedResolved,
    SuppressedDisabled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationCountsDto {
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub approval_required: u64,
    pub important_update: u64,
    pub configuration_blocked: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationListItemDto {
    pub schema_version: u32,
    pub event_id: String,
    pub batch_id: Option<String>,
    pub kind: NotificationKindDto,
    pub source_kind: NotificationSourceKindDto,
    pub source_id: String,
    pub run_id: Option<String>,
    pub automation_id: Option<String>,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub approval_action_id: Option<String>,
    pub subject_kind: NotificationSubjectKindDto,
    pub subject_text: String,
    pub priority: NotificationPriorityDto,
    pub resource_revision: Option<u64>,
    pub seen_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub revision: u64,
    pub status: NotificationBatchStatusDto,
    pub highest_priority: NotificationPriorityDto,
    pub counts: NotificationCountsDto,
    pub item_count: u64,
    pub items: Vec<NotificationListItemDto>,
    pub collect_until: i64,
    pub replace_until: i64,
    pub notifications_enabled: bool,
    pub sound_enabled: bool,
    pub show_task_content: bool,
    pub sound_level_played: NotificationSoundLevelDto,
    pub delivered_revision: Option<u64>,
    pub delivered_priority: Option<NotificationPriorityDto>,
    pub is_update: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchesClaimInputDto {
    pub schema_version: u32,
    pub claim_token: String,
    pub lease_duration_ms: u64,
    pub limit: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchesClaimOutputDto {
    pub schema_version: u32,
    pub claim_token: String,
    pub batches: Vec<NotificationBatchDto>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchValidateInputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub claim_token: String,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchValidateOutputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub batch: Option<NotificationBatchDto>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchAcknowledgeInputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub claim_token: String,
    pub disposition: NotificationDeliveryDispositionDto,
    pub native_priority: NotificationPriorityDto,
    pub sound_level_played: NotificationSoundLevelDto,
    pub native_revision: u64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchAcknowledgeOutputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub status: NotificationBatchStatusDto,
    pub disposition: NotificationDeliveryDispositionDto,
    pub acknowledged_at: i64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchReleaseInputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub claim_token: String,
    pub retry_at: i64,
    pub error_code: NotificationDeliveryErrorCodeDto,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationDeliveryErrorCodeDto {
    NativeNotificationFailed,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchReleaseOutputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub status: NotificationBatchStatusDto,
    pub retry_at: i64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchSuppressInputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub reason: NotificationDeliveryDispositionDto,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationBatchSuppressOutputDto {
    pub schema_version: u32,
    pub batch_id: String,
    pub status: NotificationBatchStatusDto,
    pub reason: NotificationDeliveryDispositionDto,
    pub suppressed_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationListInputDto {
    pub schema_version: u32,
    pub cursor: Option<String>,
    pub limit: u32,
    #[serde(default)]
    pub unread_only: bool,
    pub batch_id: Option<String>,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationListOutputDto {
    pub schema_version: u32,
    pub items: Vec<NotificationListItemDto>,
    pub next_cursor: Option<String>,
    pub unread_count: u64,
    pub last_sequence: u64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSummaryInputDto {
    pub schema_version: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSummaryOutputDto {
    pub schema_version: u32,
    pub unread_count: u64,
    pub unresolved_count: u64,
    pub counts: NotificationCountsDto,
    pub latest_occurred_at: Option<i64>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NotificationMarkSeenTargetDto {
    Events {
        #[serde(deserialize_with = "deserialize_bounded_unique_event_ids")]
        event_ids: Vec<String>,
    },
    Batch {
        batch_id: String,
    },
    All,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationMarkSeenInputDto {
    pub schema_version: u32,
    pub target: NotificationMarkSeenTargetDto,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationMarkSeenOutputDto {
    pub schema_version: u32,
    pub updated_count: u64,
    pub seen_at: i64,
    pub summary: NotificationSummaryOutputDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSettingsDto {
    pub schema_version: u32,
    pub enabled: bool,
    pub sound_enabled: bool,
    pub show_task_content: bool,
    pub human_completed_enabled: bool,
    pub human_failed_enabled: bool,
    pub human_approval_enabled: bool,
    pub human_cancelled_enabled: bool,
    pub revision: u64,
    pub updated_at: i64,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSettingsValuesDto {
    pub enabled: bool,
    pub sound_enabled: bool,
    pub show_task_content: bool,
    pub human_completed_enabled: bool,
    pub human_failed_enabled: bool,
    pub human_approval_enabled: bool,
    pub human_cancelled_enabled: bool,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSettingsGetInputDto {
    pub schema_version: u32,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSettingsGetOutputDto {
    pub schema_version: u32,
    pub settings: NotificationSettingsDto,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationSettingsUpdateInputDto {
    pub schema_version: u32,
    pub expected_revision: u64,
    pub settings: NotificationSettingsValuesDto,
}
pub type NotificationSettingsUpdateOutputDto = NotificationSettingsGetOutputDto;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationChangeKindDto {
    Created,
    Updated,
    Seen,
    Resolved,
    SettingsUpdated,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationEventDto {
    pub schema_version: u32,
    pub sequence: u64,
    pub event_id: String,
    pub kind: NotificationChangeKindDto,
    pub notification_id: Option<String>,
    pub batch_id: Option<String>,
    pub resource_revision: Option<u64>,
    pub occurred_at: i64,
}
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotificationResyncReasonDto {
    CoreStarted,
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationResyncDto {
    pub schema_version: u32,
    pub reason: NotificationResyncReasonDto,
    pub last_sequence: u64,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum NotificationOpenDestinationDto {
    Application,
    Conversation {
        conversation_id: String,
        message_id: Option<String>,
        approval_action_id: Option<String>,
    },
    Automation {
        automation_id: String,
        run_id: Option<String>,
    },
}
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NotificationOpenRequestDto {
    pub schema_version: u32,
    pub batch_id: String,
    #[serde(deserialize_with = "deserialize_bounded_unique_event_ids")]
    pub event_ids: Vec<String>,
    pub destination: NotificationOpenDestinationDto,
}

fn deserialize_bounded_unique_event_ids<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let ids = Vec::<String>::deserialize(deserializer)?;
    if !(1..=100).contains(&ids.len()) {
        return Err(D::Error::custom("eventIds must contain 1 to 100 ids"));
    }
    if ids.iter().any(|id| id.is_empty() || id.len() > 512) {
        return Err(D::Error::custom(
            "eventIds entries must contain 1 to 512 bytes",
        ));
    }
    let unique = ids.iter().collect::<HashSet<_>>();
    if unique.len() != ids.len() {
        return Err(D::Error::custom("eventIds must contain unique ids"));
    }
    Ok(ids)
}
