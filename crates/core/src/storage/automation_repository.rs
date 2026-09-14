//! Durable storage boundary for user-authored scheduled automations.
//!
//! The repository deliberately stores normalized destination, schedule, permission, and execution
//! snapshots as JSON documents. Application code owns their versioned domain interpretation;
//! SQLite owns idempotency, compare-and-set revisions, non-overlap, and durable event ordering.

use crate::storage::{
    notification_repository::{self, NewNotificationEventRecord, NotificationEventRecord},
    now_ms,
};
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction, TransactionBehavior};

pub const AUTOMATION_SCHEMA_VERSION: i64 = 1;
pub const AUTOMATION_RUN_SCHEMA_VERSION: i64 = 1;
pub const AUTOMATION_EVENT_SCHEMA_VERSION: i64 = 1;
/// Internal sentinel returned only after the atomic admission transaction has durably blocked a
/// task whose elevated permission mode was revoked. Callers must not retry the HumanRoot Turn.
pub const AUTOMATION_PERMISSION_DISABLED_AT_ADMISSION: &str =
    "automation_permission_disabled_at_admission";
pub const AUTOMATION_PERMISSION_DISABLED_MESSAGE: &str =
    "The selected permission mode is no longer enabled in user settings.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredAutomationStatus {
    Active,
    Paused,
}

impl StoredAutomationStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "active" => Ok(Self::Active),
            "paused" => Ok(Self::Paused),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoredAutomationRunStatus {
    Queued,
    Admitting,
    Running,
    WaitingForApproval,
    Completed,
    Failed,
    Cancelled,
}

impl StoredAutomationRunStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Admitting => "admitting",
            Self::Running => "running",
            Self::WaitingForApproval => "waiting_for_approval",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> rusqlite::Result<Self> {
        match value {
            "queued" => Ok(Self::Queued),
            "admitting" => Ok(Self::Admitting),
            "running" => Ok(Self::Running),
            "waiting_for_approval" => Ok(Self::WaitingForApproval),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(rusqlite::Error::InvalidQuery),
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationConfigRecord {
    pub title: String,
    pub prompt: String,
    pub health_state: String,
    pub blocked_code: Option<String>,
    pub blocked_message: Option<String>,
    pub destination_kind: String,
    pub target_conversation_id: Option<String>,
    pub project_binding_kind: String,
    pub project_id: Option<String>,
    pub model_id: Option<String>,
    pub permission_mode: String,
    pub permission_mode_version: i64,
    pub permissions_json: String,
    pub reasoning_json: Option<String>,
    pub schedule_kind: String,
    pub schedule_json: String,
    pub rrule: String,
    pub timezone: String,
    pub anchor_at: i64,
    pub next_run_at: Option<i64>,
    pub notification_policy: String,
    pub target_project_snapshot: Option<String>,
    pub target_conversation_snapshot: Option<String>,
    pub target_model_snapshot: Option<String>,
    pub target_project_id_snapshot: Option<String>,
    pub target_conversation_id_snapshot: Option<String>,
    pub target_model_id_snapshot: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAutomationRecord {
    pub id: String,
    pub create_request_id: String,
    pub status: StoredAutomationStatus,
    pub config: AutomationConfigRecord,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRecord {
    pub id: String,
    pub schema_version: i64,
    pub create_request_id: String,
    pub status: StoredAutomationStatus,
    pub config: AutomationConfigRecord,
    pub last_scheduled_at: Option<i64>,
    pub last_run_at: Option<i64>,
    pub attention_required_at: Option<i64>,
    pub attention_read_at: Option<i64>,
    pub revision: i64,
    pub created_at: i64,
    pub updated_at: i64,
    pub deleted_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationCreateOutcome {
    Created(AutomationRecord),
    Existing(AutomationRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationCompareAndSetOutcome {
    Updated(AutomationRecord),
    RevisionConflict(AutomationRecord),
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationListCursor {
    pub updated_at: i64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationListInput {
    pub status: Option<StoredAutomationStatus>,
    pub query: Option<String>,
    pub cursor: Option<AutomationListCursor>,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationListPage {
    pub items: Vec<AutomationRecord>,
    pub next_cursor: Option<AutomationListCursor>,
    pub total_count: u64,
    pub active_count: u64,
    pub paused_count: u64,
    pub attention_count: u64,
    pub last_event_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunRecord {
    pub id: String,
    pub schema_version: i64,
    pub automation_id: String,
    pub config_revision: i64,
    pub config_snapshot_json: String,
    pub trigger_kind: String,
    pub scheduled_for: i64,
    pub manual_request_id: Option<String>,
    pub status: StoredAutomationRunStatus,
    pub status_revision: i64,
    pub retry_at: Option<i64>,
    pub admission_attempt: i64,
    pub admission_token: Option<String>,
    pub admission_expires_at: Option<i64>,
    pub cancellation_requested_at: Option<i64>,
    pub agent_run_id: Option<String>,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub report_kind: Option<String>,
    pub result_preview: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub attention_required_at: Option<i64>,
    pub attention_read_at: Option<i64>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewScheduledAutomationRunRecord {
    pub id: String,
    pub automation_id: String,
    pub trigger_kind: String,
    pub scheduled_for: i64,
    pub config_revision: i64,
    pub config_snapshot_json: String,
    pub expected_revision: i64,
    /// The next strictly-future occurrence calculated by the schedule engine outside SQLite.
    pub next_run_at: i64,
    pub claimed_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduledAutomationRunEnqueueOutcome {
    Enqueued(AutomationRunRecord),
    Existing(AutomationRunRecord),
    ActiveConflict(AutomationRunRecord),
    NoLongerDue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunAdmissionInput {
    pub automation_run_id: String,
    pub admission_token: String,
    pub config_revision: i64,
    /// Host-only mode parsed from the immutable run config snapshot. Renderer input never reaches
    /// this boundary directly.
    pub permission_mode: String,
    pub agent_run_id: String,
    pub conversation_id: String,
    pub user_message_id: String,
    pub assistant_message_id: String,
    pub admitted_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationPermissionAdmissionOutcome {
    Enabled,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationRunAdmissionOutcome {
    Admitted(AutomationRunRecord),
    Replayed(AutomationRunRecord),
    Stale(Option<AutomationRunRecord>),
    Cancelled(AutomationRunRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationRunMutationOutcome {
    Updated(AutomationRunRecord),
    Stale(Option<AutomationRunRecord>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunRecoveryRecord {
    pub run: AutomationRunRecord,
    pub trace_terminal_status: Option<String>,
    pub trace_terminal_error: Option<String>,
    pub has_pending_action: bool,
    pub automation_deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunSettlementInput {
    pub automation_run_id: String,
    pub agent_run_id: String,
    pub terminal_status: StoredAutomationRunStatus,
    pub report_kind: Option<String>,
    pub result_preview: Option<String>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
    pub settled_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewAutomationNotificationRecord {
    pub automation_id: String,
    pub automation_run_id: Option<String>,
    pub resource_revision: i64,
    pub notification_kind: String,
    pub title: String,
    pub body: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewManualAutomationRunRecord {
    pub id: String,
    pub automation_id: String,
    pub manual_request_id: String,
    pub scheduled_for: i64,
    pub config_revision: i64,
    pub config_snapshot_json: String,
    pub expected_revision: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationRunEnqueueOutcome {
    Enqueued(AutomationRunRecord),
    Existing(AutomationRunRecord),
    ActiveConflict(AutomationRunRecord),
    RevisionConflict(Box<AutomationRecord>),
    AutomationNotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunListCursor {
    pub created_at: i64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationRunListPage {
    pub items: Vec<AutomationRunRecord>,
    pub next_cursor: Option<AutomationRunListCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationAttentionSummary {
    pub task_count: u64,
    pub run_count: u64,
    pub total_count: u64,
    pub last_event_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationAttentionRecord {
    pub attention_id: String,
    pub attention_kind: String,
    pub automation_id: String,
    pub automation_run_id: Option<String>,
    pub required_at: i64,
    pub read_at: Option<i64>,
    pub code: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationAttentionListCursor {
    pub required_at: i64,
    pub attention_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationAttentionListPage {
    pub items: Vec<AutomationAttentionRecord>,
    pub next_cursor: Option<AutomationAttentionListCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationEventRecord {
    pub sequence: i64,
    pub schema_version: i64,
    pub event_id: String,
    pub event_kind: String,
    pub automation_id: String,
    pub automation_run_id: Option<String>,
    pub resource_revision: Option<i64>,
    pub payload_json: String,
    pub occurred_at: i64,
}

// These repository shards intentionally share this lexical module. Keeping the SQL helpers and
// transaction coordinators in one namespace preserves the exact transaction/CAS ordering while
// making each lifecycle area independently reviewable.
include!("automation_repository/configuration.rs");
include!("automation_repository/parent_deletion.rs");
include!("automation_repository/run_admission.rs");
include!("automation_repository/run_settlement.rs");
include!("automation_repository/queries.rs");

#[cfg(test)]
mod tests;
