use serde::{Deserialize, Serialize};

pub const AUTOMATION_SCHEMA_VERSION: u32 = 1;
pub const AUTOMATION_PERMISSION_MODE_VERSION: u32 = 1;
pub const AUTOMATION_ERROR_CODE: i32 = -32045;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationStatusDto {
    Active,
    Paused,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationPermissionModeDto {
    Default,
    Full,
    Custom,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationNotificationPolicyDto {
    AllRuns,
    UnsuccessfulOnly,
    ImportantUpdates,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AutomationWeekdayDto {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationIntervalUnitDto {
    Minutes,
    Hours,
    Days,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCustomFrequencyDto {
    Hourly,
    Daily,
    Weekly,
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationScheduleInputDto {
    Interval {
        amount: u32,
        unit: AutomationIntervalUnitDto,
        anchor_at: i64,
        timezone: String,
    },
    Daily {
        time_minutes: u16,
        anchor_at: i64,
        timezone: String,
    },
    Weekdays {
        time_minutes: u16,
        anchor_at: i64,
        timezone: String,
    },
    Weekly {
        weekdays: Vec<AutomationWeekdayDto>,
        time_minutes: u16,
        anchor_at: i64,
        timezone: String,
    },
    Custom {
        frequency: AutomationCustomFrequencyDto,
        interval: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        minute_of_hour: Option<u8>,
        #[serde(skip_serializing_if = "Option::is_none")]
        time_minutes: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        weekdays: Option<Vec<AutomationWeekdayDto>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        month_days: Option<Vec<u8>>,
        #[serde(skip_serializing_if = "Option::is_none")]
        months: Option<Vec<u8>>,
        anchor_at: i64,
        timezone: String,
    },
}

pub type AutomationScheduleDto = AutomationScheduleInputDto;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationDestinationInputDto {
    NewChat {
        project_binding: AutomationProjectBindingDto,
        project_id: Option<String>,
        model_id: String,
    },
    ExistingChat {
        conversation_id: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationProjectBindingDto {
    None,
    Project,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReasoningModeDto {
    ProviderDefault,
    Enabled,
    Disabled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReasoningEffortDto {
    ProviderDefault,
    High,
    Max,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationReasoningProjectionDto {
    pub source: AutomationReasoningSourceDto,
    pub mode: AutomationReasoningModeDto,
    pub effort: AutomationReasoningEffortDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReasoningSourceDto {
    ModelConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationDestinationDto {
    NewChat {
        project_binding: AutomationProjectBindingDto,
        project_id: Option<String>,
        model_id: String,
        reasoning: AutomationReasoningProjectionDto,
    },
    ExistingChat {
        conversation_id: String,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(
    tag = "state",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum AutomationHealthDto {
    Ok,
    Blocked {
        code: AutomationBlockedCodeDto,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationBlockedCodeDto {
    TargetMissing,
    TargetArchived,
    ProjectMissing,
    ProjectPathMissing,
    ModelMissing,
    ModelDisabled,
    PermissionDisabled,
    ScheduleInvalid,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReadPermissionDto {
    WorkspaceOnly,
    All,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationWritePermissionDto {
    Denied,
    WorkspaceOnly,
    All,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCommandPermissionDto {
    RequireApproval,
    AutoApprove,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationCommandSafetyPolicyDto {
    Guarded,
    FullAccess,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationPatchPermissionDto {
    RequireApproval,
    AutoApprove,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationResolvedPermissionsDto {
    pub read: AutomationReadPermissionDto,
    pub write: AutomationWritePermissionDto,
    pub command: AutomationCommandPermissionDto,
    pub command_safety: AutomationCommandSafetyPolicyDto,
    pub patch: AutomationPatchPermissionDto,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunStatusDto {
    Queued,
    Starting,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRunTriggerKindDto {
    Scheduled,
    Manual,
    Recovery,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationReportKindDto {
    NoChange,
    ImportantUpdate,
    Completed,
    Unknown,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationAttentionKindDto {
    WaitingForApproval,
    RunFailed,
    ImportantUpdate,
    ConfigurationBlocked,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttentionDto {
    pub schema_version: u32,
    pub attention_id: String,
    pub automation_id: String,
    pub run_id: Option<String>,
    pub kind: AutomationAttentionKindDto,
    pub message: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRunDto {
    pub schema_version: u32,
    pub run_id: String,
    pub automation_id: String,
    pub config_revision: u64,
    pub trigger_kind: AutomationRunTriggerKindDto,
    pub scheduled_for: Option<i64>,
    pub status: AutomationRunStatusDto,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub report_kind: AutomationReportKindDto,
    pub result_preview: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attention: Option<AutomationAttentionDto>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationTargetSnapshotDto {
    pub project_name: Option<String>,
    pub conversation_title: Option<String>,
    pub model_display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationTaskDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub title: String,
    pub prompt: String,
    pub status: AutomationStatusDto,
    pub health: AutomationHealthDto,
    pub destination: AutomationDestinationDto,
    pub permission_mode: AutomationPermissionModeDto,
    pub permission_mode_version: u32,
    pub resolved_permissions: AutomationResolvedPermissionsDto,
    pub schedule: AutomationScheduleDto,
    pub schedule_summary: String,
    pub rrule: String,
    pub timezone: String,
    pub notification_policy: AutomationNotificationPolicyDto,
    pub target_snapshot: AutomationTargetSnapshotDto,
    pub next_run_at: Option<i64>,
    pub last_scheduled_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub latest_run: Option<AutomationRunDto>,
    pub attention: Option<AutomationAttentionDto>,
    pub revision: u64,
    pub created_at: i64,
    pub updated_at: i64,
}

pub type AutomationTaskSummaryDto = AutomationTaskDto;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationListInputDto {
    pub schema_version: u32,
    pub status: Option<AutomationStatusDto>,
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationGetInputDto {
    pub schema_version: u32,
    pub automation_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationCreateInputDto {
    pub schema_version: u32,
    pub request_id: String,
    pub status: AutomationStatusDto,
    pub title: String,
    pub prompt: String,
    pub destination: AutomationDestinationInputDto,
    pub permission_mode: AutomationPermissionModeDto,
    pub permission_mode_version: u32,
    pub schedule: AutomationScheduleInputDto,
    pub notification_policy: AutomationNotificationPolicyDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationUpdateInputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub expected_revision: u64,
    pub title: String,
    pub prompt: String,
    pub destination: AutomationDestinationInputDto,
    pub permission_mode: AutomationPermissionModeDto,
    pub permission_mode_version: u32,
    pub schedule: AutomationScheduleInputDto,
    pub notification_policy: AutomationNotificationPolicyDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationSetEnabledInputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub expected_revision: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRunNowInputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub request_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDeleteInputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub expected_revision: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRunsListInputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttentionSummaryInputDto {
    pub schema_version: u32,
    pub cursor: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttentionAcknowledgeInputDto {
    pub schema_version: u32,
    pub attention_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationListCountsDto {
    pub all: u64,
    pub active: u64,
    pub paused: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationListOutputDto {
    pub schema_version: u32,
    pub tasks: Vec<AutomationTaskSummaryDto>,
    pub next_cursor: Option<String>,
    pub counts: AutomationListCountsDto,
    pub attention_count: u64,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationRunsListOutputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub runs: Vec<AutomationRunDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttentionSummaryOutputDto {
    pub schema_version: u32,
    pub unread_count: u64,
    pub items: Vec<AutomationAttentionDto>,
    pub next_cursor: Option<String>,
    pub last_sequence: u64,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationAttentionAcknowledgeOutputDto {
    pub schema_version: u32,
    pub attention: AutomationAttentionDto,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationDeleteOutputDto {
    pub schema_version: u32,
    pub automation_id: String,
    pub deleted_at: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationEventKindDto {
    Created,
    Updated,
    Deleted,
    RunUpdated,
    AttentionChanged,
    NotificationRequested,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationEventDto {
    pub schema_version: u32,
    pub sequence: u64,
    pub event_id: String,
    pub kind: AutomationEventKindDto,
    pub automation_id: String,
    pub run_id: Option<String>,
    pub resource_revision: Option<u64>,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationResyncReasonDto {
    CoreStarted,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationResyncDto {
    pub schema_version: u32,
    pub reason: AutomationResyncReasonDto,
    pub last_sequence: u64,
    pub occurred_at: i64,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationErrorKindDto {
    NotFound,
    RevisionConflict,
    Validation,
    RunAlreadyActive,
    TargetInvalid,
    PermissionDisabled,
    ScheduleInvalid,
    Internal,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AutomationErrorDataDto {
    pub schema_version: u32,
    #[serde(rename = "type")]
    pub kind: AutomationErrorTypeDto,
    pub code: AutomationErrorKindDto,
    pub message: String,
    pub automation_id: Option<String>,
    pub current_revision: Option<u64>,
    pub field: Option<String>,
    pub retryable: bool,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AutomationErrorTypeDto {
    Automation,
}
