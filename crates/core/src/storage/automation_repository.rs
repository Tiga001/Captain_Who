//! Durable storage boundary for user-authored scheduled automations.
//!
//! The repository deliberately stores normalized destination, schedule, permission, and execution
//! snapshots as JSON documents. Application code owns their versioned domain interpretation;
//! SQLite owns idempotency, compare-and-set revisions, non-overlap, and durable event ordering.

use crate::storage::now_ms;
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
pub struct AutomationNotificationRecord {
    pub id: String,
    pub automation_id: String,
    pub automation_run_id: Option<String>,
    pub resource_revision: i64,
    pub notification_kind: String,
    pub title: String,
    pub body: String,
    pub status: String,
    pub retry_at: i64,
    pub claim_token: Option<String>,
    pub claim_expires_at: Option<i64>,
    pub attempt_count: i64,
    pub last_error_code: Option<String>,
    pub created_at: i64,
    pub delivered_at: Option<i64>,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
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

pub fn create_automation(
    connection: &mut Connection,
    input: &NewAutomationRecord,
) -> rusqlite::Result<AutomationCreateOutcome> {
    let transaction = connection.transaction()?;
    if let Some(existing) = query_automation_by_request(&transaction, &input.create_request_id)? {
        transaction.rollback()?;
        return Ok(AutomationCreateOutcome::Existing(existing));
    }

    let timestamp = now_ms();
    let next_run_at = effective_next_run_at(input.status, &input.config);
    transaction.execute(
        "INSERT INTO automations (
            id, schema_version, create_request_id, title, prompt, status,
            health_state, blocked_code, blocked_message, destination_kind,
            target_conversation_id, project_binding_kind, project_id, model_id,
            permission_mode, permission_mode_version, permissions_json, reasoning_json,
            schedule_kind, schedule_json, rrule, timezone, anchor_at, next_run_at,
            last_scheduled_at, last_run_at, notification_policy,
            target_project_snapshot, target_conversation_snapshot, target_model_snapshot,
            target_project_id_snapshot, target_conversation_id_snapshot, target_model_id_snapshot,
            attention_required_at, attention_read_at, revision, created_at, updated_at, deleted_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
            ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
            ?21, ?22, ?23, ?24, NULL, NULL, ?25, ?26, ?27, ?28,
            COALESCE(?29, ?13), COALESCE(?30, ?11), COALESCE(?31, ?14),
            NULL, NULL, 1, ?32, ?32, NULL
         )",
        params![
            &input.id,
            AUTOMATION_SCHEMA_VERSION,
            &input.create_request_id,
            &input.config.title,
            &input.config.prompt,
            input.status.as_str(),
            &input.config.health_state,
            input.config.blocked_code.as_deref(),
            input.config.blocked_message.as_deref(),
            &input.config.destination_kind,
            input.config.target_conversation_id.as_deref(),
            &input.config.project_binding_kind,
            input.config.project_id.as_deref(),
            input.config.model_id.as_deref(),
            &input.config.permission_mode,
            input.config.permission_mode_version,
            &input.config.permissions_json,
            input.config.reasoning_json.as_deref(),
            &input.config.schedule_kind,
            &input.config.schedule_json,
            &input.config.rrule,
            &input.config.timezone,
            input.config.anchor_at,
            next_run_at,
            &input.config.notification_policy,
            input.config.target_project_snapshot.as_deref(),
            input.config.target_conversation_snapshot.as_deref(),
            input.config.target_model_snapshot.as_deref(),
            input.config.target_project_id_snapshot.as_deref(),
            input.config.target_conversation_id_snapshot.as_deref(),
            input.config.target_model_id_snapshot.as_deref(),
            timestamp,
        ],
    )?;
    insert_event(
        &transaction,
        "created",
        &input.id,
        None,
        Some(1),
        "{}",
        timestamp,
    )?;
    let record = query_automation(&transaction, &input.id, false)?.expect("inserted automation");
    transaction.commit()?;
    Ok(AutomationCreateOutcome::Created(record))
}

pub fn get_automation(
    connection: &Connection,
    automation_id: &str,
) -> rusqlite::Result<Option<AutomationRecord>> {
    query_automation(connection, automation_id, false)
}

pub fn get_automation_by_create_request_id(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<AutomationRecord>> {
    connection
        .query_row(
            automation_select_sql("WHERE create_request_id = ?1 AND deleted_at IS NULL LIMIT 1")
                .as_str(),
            [request_id],
            row_to_automation,
        )
        .optional()
}

pub fn list_automations(
    connection: &Connection,
    input: &AutomationListInput,
) -> rusqlite::Result<AutomationListPage> {
    let limit = input.limit.clamp(1, 100);
    let status = input.status.map(StoredAutomationStatus::as_str);
    let query = input
        .query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let cursor_updated_at = input.cursor.as_ref().map(|cursor| cursor.updated_at);
    let cursor_id = input.cursor.as_ref().map(|cursor| cursor.id.as_str());
    let mut statement = connection.prepare(
        "SELECT
            id, schema_version, create_request_id, title, prompt, status,
            health_state, blocked_code, blocked_message, destination_kind,
            target_conversation_id, project_binding_kind, project_id, model_id,
            permission_mode, permission_mode_version, permissions_json, reasoning_json,
            schedule_kind, schedule_json, rrule, timezone, anchor_at, next_run_at,
            last_scheduled_at, last_run_at, notification_policy,
            target_project_snapshot, target_conversation_snapshot, target_model_snapshot,
            target_project_id_snapshot, target_conversation_id_snapshot, target_model_id_snapshot,
            attention_required_at, attention_read_at, revision, created_at, updated_at, deleted_at
         FROM automations
         WHERE deleted_at IS NULL
           AND (?1 IS NULL OR status = ?1)
           AND (
               ?2 IS NULL
               OR instr(lower(title), lower(?2)) > 0
               OR instr(lower(prompt), lower(?2)) > 0
           )
           AND (
               ?3 IS NULL
               OR updated_at < ?3
               OR (updated_at = ?3 AND id < ?4)
           )
         ORDER BY updated_at DESC, id DESC
         LIMIT ?5",
    )?;
    let mut items = statement
        .query_map(
            params![
                status,
                query,
                cursor_updated_at,
                cursor_id,
                limit as i64 + 1
            ],
            row_to_automation,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|record| AutomationListCursor {
            updated_at: record.updated_at,
            id: record.id.clone(),
        })
    } else {
        None
    };
    let (total_count, active_count, paused_count): (i64, i64, i64) = connection.query_row(
        "SELECT
            COUNT(*),
            COALESCE(SUM(status = 'active'), 0),
            COALESCE(SUM(status = 'paused'), 0)
         FROM automations
         WHERE deleted_at IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let attention_count = unread_attention_count(connection)?;
    let last_event_sequence = last_event_sequence(connection)?;
    Ok(AutomationListPage {
        items,
        next_cursor,
        total_count: nonnegative_u64(total_count),
        active_count: nonnegative_u64(active_count),
        paused_count: nonnegative_u64(paused_count),
        attention_count,
        last_event_sequence,
    })
}

pub fn replace_automation_config(
    connection: &mut Connection,
    automation_id: &str,
    expected_revision: i64,
    config: &AutomationConfigRecord,
) -> rusqlite::Result<AutomationCompareAndSetOutcome> {
    let transaction = connection.transaction()?;
    let Some(existing) = query_automation(&transaction, automation_id, false)? else {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::NotFound);
    };
    if existing.revision != expected_revision {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::RevisionConflict(existing));
    }
    let timestamp = now_ms().max(existing.updated_at.saturating_add(1));
    let next_run_at = effective_next_run_at(existing.status, config);
    transaction.execute(
        "UPDATE automations SET
            title = ?1, prompt = ?2,
            health_state = ?3, blocked_code = ?4, blocked_message = ?5,
            destination_kind = ?6, target_conversation_id = ?7,
            project_binding_kind = ?8, project_id = ?9, model_id = ?10,
            permission_mode = ?11, permission_mode_version = ?12, permissions_json = ?13,
            reasoning_json = ?14,
            schedule_kind = ?15, schedule_json = ?16, rrule = ?17,
            timezone = ?18, anchor_at = ?19, next_run_at = ?20,
            notification_policy = ?21,
            target_project_snapshot = ?22, target_conversation_snapshot = ?23,
            target_model_snapshot = ?24,
            target_project_id_snapshot = COALESCE(?25, ?9),
            target_conversation_id_snapshot = COALESCE(?26, ?7),
            target_model_id_snapshot = COALESCE(?27, ?10),
            attention_required_at = CASE WHEN ?3 = 'ok' THEN NULL ELSE attention_required_at END,
            attention_read_at = CASE WHEN ?3 = 'ok' THEN NULL ELSE attention_read_at END,
            revision = revision + 1, updated_at = ?28
         WHERE id = ?29 AND deleted_at IS NULL AND revision = ?30",
        params![
            &config.title,
            &config.prompt,
            &config.health_state,
            config.blocked_code.as_deref(),
            config.blocked_message.as_deref(),
            &config.destination_kind,
            config.target_conversation_id.as_deref(),
            &config.project_binding_kind,
            config.project_id.as_deref(),
            config.model_id.as_deref(),
            &config.permission_mode,
            config.permission_mode_version,
            &config.permissions_json,
            config.reasoning_json.as_deref(),
            &config.schedule_kind,
            &config.schedule_json,
            &config.rrule,
            &config.timezone,
            config.anchor_at,
            next_run_at,
            &config.notification_policy,
            config.target_project_snapshot.as_deref(),
            config.target_conversation_snapshot.as_deref(),
            config.target_model_snapshot.as_deref(),
            config.target_project_id_snapshot.as_deref(),
            config.target_conversation_id_snapshot.as_deref(),
            config.target_model_id_snapshot.as_deref(),
            timestamp,
            automation_id,
            expected_revision,
        ],
    )?;
    let updated =
        query_automation(&transaction, automation_id, false)?.expect("updated automation");
    insert_event(
        &transaction,
        "updated",
        automation_id,
        None,
        Some(updated.revision),
        "{}",
        timestamp,
    )?;
    transaction.commit()?;
    Ok(AutomationCompareAndSetOutcome::Updated(updated))
}

pub fn block_automation(
    connection: &mut Connection,
    automation_id: &str,
    expected_revision: i64,
    blocked_code: &str,
    blocked_message: &str,
) -> rusqlite::Result<AutomationCompareAndSetOutcome> {
    let transaction = connection.transaction()?;
    let Some(existing) = query_automation(&transaction, automation_id, false)? else {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::NotFound);
    };
    if existing.revision != expected_revision {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::RevisionConflict(existing));
    }
    let timestamp = now_ms().max(existing.updated_at.saturating_add(1));
    transaction.execute(
        "UPDATE automations SET
            health_state = 'blocked',
            blocked_code = ?1,
            blocked_message = ?2,
            next_run_at = NULL,
            attention_required_at = MAX(COALESCE(attention_required_at, 0), ?3),
            revision = revision + 1,
            updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL AND revision = ?5",
        params![
            blocked_code,
            blocked_message,
            timestamp,
            automation_id,
            expected_revision,
        ],
    )?;
    let unadmitted_run_id = transaction
        .query_row(
            "SELECT id FROM automation_runs
             WHERE automation_id = ?1 AND status IN ('queued', 'admitting') LIMIT 1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(run_id) = unadmitted_run_id {
        transaction.execute(
            "UPDATE automation_runs
             SET status = 'failed', status_revision = status_revision + 1,
                 retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
                 error_code = ?1, error_message = ?2,
                 attention_required_at = MAX(COALESCE(attention_required_at, 0), ?3),
                 completed_at = MAX(created_at, ?3), updated_at = MAX(updated_at, ?3)
             WHERE id = ?4 AND status IN ('queued', 'admitting')",
            params![blocked_code, blocked_message, timestamp, run_id],
        )?;
        let run = query_run(&transaction, &run_id)?.expect("blocked unadmitted automation run");
        insert_event(
            &transaction,
            "run_updated",
            automation_id,
            Some(&run_id),
            Some(run.status_revision),
            r#"{"status":"failed"}"#,
            timestamp,
        )?;
        insert_event(
            &transaction,
            "attention_changed",
            automation_id,
            Some(&run_id),
            Some(run.status_revision),
            "{}",
            timestamp,
        )?;
    }
    let updated = query_automation(&transaction, automation_id, false)?
        .expect("blocked automation must remain readable");
    transaction.commit()?;
    Ok(AutomationCompareAndSetOutcome::Updated(updated))
}

/// Projects Automation invalidation inside the legacy Agent-tree deletion transaction.
///
/// That transaction intentionally disables SQLite triggers while it deletes an Agent graph. The
/// normal parent-resource triggers therefore cannot run there, so this helper reproduces their
/// Automation-owned effects explicitly in the same transaction. Calling it while triggers are
/// enabled would duplicate events/outbox rows and is not supported.
pub fn invalidate_automations_before_trigger_disabled_conversation_delete(
    transaction: &Transaction<'_>,
    conversation_ids: &[String],
    invalidated_at: i64,
) -> rusqlite::Result<()> {
    let mut automation_ids = Vec::new();
    for conversation_id in conversation_ids {
        let conversation_title = transaction
            .query_row(
                "SELECT title FROM conversations WHERE id = ?1",
                [conversation_id],
                |row| row.get::<_, String>(0),
            )
            .optional()?;
        let mut statement = transaction.prepare(
            "SELECT id FROM automations
             WHERE deleted_at IS NULL AND destination_kind = 'existing_chat'
               AND target_conversation_id = ?1
             ORDER BY id",
        )?;
        let ids = statement
            .query_map([conversation_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for automation_id in ids {
            if !automation_ids.contains(&automation_id) {
                if let Some(title) = conversation_title.as_deref() {
                    transaction.execute(
                        "UPDATE automations
                         SET target_conversation_snapshot = COALESCE(target_conversation_snapshot, ?1)
                         WHERE id = ?2",
                        params![title, automation_id],
                    )?;
                }
                automation_ids.push(automation_id);
            }
        }
    }
    for automation_id in automation_ids {
        block_automation_without_triggers_in_transaction(
            transaction,
            &automation_id,
            "target_missing",
            "The target conversation no longer exists.",
            invalidated_at,
        )?;
    }
    Ok(())
}

/// Project counterpart to
/// `invalidate_automations_before_trigger_disabled_conversation_delete`.
pub fn invalidate_automations_before_trigger_disabled_project_delete(
    transaction: &Transaction<'_>,
    project_id: &str,
    conversation_ids: &[String],
    invalidated_at: i64,
) -> rusqlite::Result<()> {
    let project_name = transaction
        .query_row(
            "SELECT name FROM projects WHERE id = ?1",
            [project_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let mut automation_ids = {
        let mut statement = transaction.prepare(
            "SELECT id FROM automations
             WHERE deleted_at IS NULL
               AND project_binding_kind = 'project' AND project_id = ?1
             ORDER BY id",
        )?;
        let ids = statement
            .query_map([project_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    for conversation_id in conversation_ids {
        let mut statement = transaction.prepare(
            "SELECT id FROM automations
             WHERE deleted_at IS NULL AND destination_kind = 'existing_chat'
               AND target_conversation_id = ?1
             ORDER BY id",
        )?;
        let ids = statement
            .query_map([conversation_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        for automation_id in ids {
            if !automation_ids.contains(&automation_id) {
                automation_ids.push(automation_id);
            }
        }
    }
    for automation_id in automation_ids {
        if let Some(name) = project_name.as_deref() {
            transaction.execute(
                "UPDATE automations
                 SET target_project_snapshot = COALESCE(target_project_snapshot, ?1)
                 WHERE id = ?2",
                params![name, automation_id],
            )?;
        }
        block_automation_without_triggers_in_transaction(
            transaction,
            &automation_id,
            "project_missing",
            "The target project no longer exists.",
            invalidated_at,
        )?;
    }
    Ok(())
}

/// Makes resource deletion an independent durable cancellation authority for admitted Automation
/// runs. Agent deletion deliberately discards its terminal trace once the owning conversation is
/// fenced for deletion, so waiting for the process-local Agent observer is not sufficient here.
/// This projection must run in the same transaction, before conversations/messages/traces vanish.
pub fn terminalize_automation_runs_before_conversation_delete(
    transaction: &Transaction<'_>,
    conversation_ids: &[String],
    terminated_at: i64,
) -> rusqlite::Result<()> {
    let run_ids = deletion_run_ids_for_conversations(transaction, conversation_ids)?;
    terminalize_automation_runs_for_resource_deletion(
        transaction,
        run_ids,
        "conversation_deleted",
        "The scheduled task run was cancelled because its conversation was deleted.",
        terminated_at,
    )
}

/// Project deletion additionally covers a task bound to the project even if legacy/corrupt data
/// left its admitted conversation outside the project. Normal admitted runs are also found through
/// their conversation ownership.
pub fn terminalize_automation_runs_before_project_delete(
    transaction: &Transaction<'_>,
    project_id: &str,
    conversation_ids: &[String],
    terminated_at: i64,
) -> rusqlite::Result<()> {
    let mut run_ids = deletion_run_ids_for_conversations(transaction, conversation_ids)?;
    let mut statement = transaction.prepare(
        "SELECT run.id
         FROM automation_runs AS run
         INNER JOIN automations AS task ON task.id = run.automation_id
         WHERE run.status IN ('running', 'waiting_for_approval')
           AND task.project_binding_kind = 'project' AND task.project_id = ?1
         ORDER BY run.created_at, run.id",
    )?;
    run_ids.extend(
        statement
            .query_map([project_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?,
    );
    drop(statement);
    terminalize_automation_runs_for_resource_deletion(
        transaction,
        run_ids,
        "project_deleted",
        "The scheduled task run was cancelled because its project was deleted.",
        terminated_at,
    )
}

pub fn terminalize_automation_runs_before_message_delete(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    message_ids: &[String],
    terminated_at: i64,
) -> rusqlite::Result<()> {
    if message_ids.is_empty() {
        return Ok(());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id FROM automation_runs
         WHERE conversation_id = ?
           AND status IN ('running', 'waiting_for_approval')
           AND (user_message_id IN ({placeholders})
                OR assistant_message_id IN ({placeholders}))
         ORDER BY created_at, id"
    );
    let mut values = Vec::with_capacity(1 + message_ids.len() * 2);
    values.push(conversation_id.to_string());
    values.extend(message_ids.iter().cloned());
    values.extend(message_ids.iter().cloned());
    let mut statement = transaction.prepare(&sql)?;
    let run_ids = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    // Deleting only the bound user message still cancels the Turn while retaining its assistant
    // projection. Persist the trace terminal here because the fenced Agent executor intentionally
    // discards its own terminal event during destructive mutation.
    for run_id in &run_ids {
        let Some(run) = query_run(transaction, run_id)? else {
            continue;
        };
        let Some(assistant_message_id) = run.assistant_message_id.as_deref() else {
            continue;
        };
        if message_ids.iter().any(|id| id == assistant_message_id) {
            continue;
        }
        let timestamp = terminated_at
            .max(run.created_at)
            .max(run.updated_at.saturating_add(1));
        if let Some(agent_run_id) = run.agent_run_id.as_deref() {
            transaction.execute(
                "UPDATE conversation_turn_traces
                 SET terminal_status = 'cancelled',
                     terminal_error = 'The scheduled task turn was cancelled because its user message was deleted.',
                     completed_at = COALESCE(completed_at, ?1), updated_at = MAX(updated_at, ?1)
                 WHERE run_id = ?2 AND terminal_status = 'in_progress'",
                params![timestamp, agent_run_id],
            )?;
        }
        transaction.execute(
            "UPDATE messages SET status = 'cancelled'
             WHERE id = ?1 AND conversation_id = ?2 AND role = 'assistant'",
            params![assistant_message_id, conversation_id],
        )?;
    }
    terminalize_automation_runs_for_resource_deletion(
        transaction,
        run_ids,
        "messages_deleted",
        "The scheduled task run was cancelled because its turn messages were deleted.",
        terminated_at,
    )
}

fn deletion_run_ids_for_conversations(
    transaction: &Transaction<'_>,
    conversation_ids: &[String],
) -> rusqlite::Result<Vec<String>> {
    if conversation_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", conversation_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = transaction.prepare(&format!(
        "SELECT id FROM automation_runs
         WHERE conversation_id IN ({placeholders})
           AND status IN ('running', 'waiting_for_approval')
         ORDER BY created_at, id"
    ))?;
    let run_ids = statement
        .query_map(rusqlite::params_from_iter(conversation_ids.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(run_ids)
}

fn terminalize_automation_runs_for_resource_deletion(
    transaction: &Transaction<'_>,
    mut run_ids: Vec<String>,
    error_code: &str,
    error_message: &str,
    terminated_at: i64,
) -> rusqlite::Result<()> {
    run_ids.sort();
    run_ids.dedup();
    for run_id in run_ids {
        let Some(current) = query_run(transaction, &run_id)? else {
            continue;
        };
        if current.status.is_terminal() {
            continue;
        }
        let timestamp = terminated_at
            .max(current.created_at)
            .max(current.updated_at.saturating_add(1));
        let had_unread_attention = current
            .attention_required_at
            .is_some_and(|required_at| current.attention_read_at.unwrap_or(-1) < required_at);
        let changed = transaction.execute(
            "UPDATE automation_runs
             SET status = 'cancelled', status_revision = status_revision + 1,
                 retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
                 cancellation_requested_at = COALESCE(cancellation_requested_at, ?1),
                 report_kind = COALESCE(report_kind, 'unknown'),
                 error_code = ?2, error_message = ?3,
                 attention_read_at = CASE
                     WHEN attention_required_at IS NULL THEN attention_read_at
                     ELSE MAX(COALESCE(attention_read_at, 0), attention_required_at, ?1)
                 END,
                 completed_at = ?1, updated_at = ?1
             WHERE id = ?4 AND status IN ('running', 'waiting_for_approval')",
            params![timestamp, error_code, error_message, run_id],
        )?;
        if changed != 1 {
            continue;
        }
        let run = query_run(transaction, &run_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        transaction.execute(
            "UPDATE automations SET last_run_at = MAX(COALESCE(last_run_at, 0), ?1)
             WHERE id = ?2",
            params![timestamp, &run.automation_id],
        )?;
        insert_event(
            transaction,
            "run_updated",
            &run.automation_id,
            Some(&run.id),
            Some(run.status_revision),
            &serde_json::json!({
                "status": "cancelled",
                "reportKind": run.report_kind.as_deref().unwrap_or("unknown"),
                "errorCode": error_code,
            })
            .to_string(),
            timestamp,
        )?;
        if had_unread_attention {
            insert_event(
                transaction,
                "attention_changed",
                &run.automation_id,
                Some(&run.id),
                Some(run.status_revision),
                "{}",
                timestamp,
            )?;
        }
        transaction.execute(
            "UPDATE automation_notification_outbox
             SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL
             WHERE automation_run_id = ?1 AND notification_kind = 'approval_required'
               AND status = 'pending'",
            [&run.id],
        )?;
        let task_is_live = transaction
            .query_row(
                "SELECT deleted_at IS NULL FROM automations WHERE id = ?1",
                [&run.automation_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if task_is_live {
            let (title, policy) = run_notification_config(&run);
            let report_kind = run.report_kind.as_deref().unwrap_or("unknown");
            if terminal_notification_requested(&policy, run.status, report_kind) {
                enqueue_automation_notification_in_transaction(
                    transaction,
                    &NewAutomationNotificationRecord {
                        automation_id: run.automation_id.clone(),
                        automation_run_id: Some(run.id.clone()),
                        resource_revision: run.status_revision,
                        notification_kind: "run_result".to_string(),
                        title,
                        body: error_message.to_string(),
                        created_at: timestamp,
                    },
                )?;
            }
        }
    }
    Ok(())
}

fn block_automation_without_triggers_in_transaction(
    transaction: &Transaction<'_>,
    automation_id: &str,
    blocked_code: &str,
    blocked_message: &str,
    invalidated_at: i64,
) -> rusqlite::Result<()> {
    let Some(existing) = query_automation(transaction, automation_id, false)? else {
        return Ok(());
    };
    let timestamp = invalidated_at.max(existing.updated_at.saturating_add(1));
    let changed = transaction.execute(
        "UPDATE automations SET
            health_state = 'blocked', blocked_code = ?1, blocked_message = ?2,
            next_run_at = NULL,
            attention_required_at = MAX(COALESCE(attention_required_at, 0), ?3),
            revision = revision + 1, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL AND revision = ?5",
        params![
            blocked_code,
            blocked_message,
            timestamp,
            automation_id,
            existing.revision,
        ],
    )?;
    if changed != 1 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let updated = query_automation(transaction, automation_id, false)?
        .ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    insert_event(
        transaction,
        "attention_changed",
        automation_id,
        None,
        Some(updated.revision),
        &serde_json::json!({ "blockedCode": blocked_code }).to_string(),
        timestamp,
    )?;
    if matches!(
        updated.config.notification_policy.as_str(),
        "all_runs" | "unsuccessful_only" | "important_updates"
    ) {
        enqueue_automation_notification_in_transaction(
            transaction,
            &NewAutomationNotificationRecord {
                automation_id: automation_id.to_string(),
                automation_run_id: None,
                resource_revision: updated.revision,
                notification_kind: "configuration_blocked".to_string(),
                title: updated.config.title.clone(),
                body: truncate_utf8(blocked_message, 4_096),
                created_at: timestamp,
            },
        )?;
    }

    let unadmitted_run_id = transaction
        .query_row(
            "SELECT id FROM automation_runs
             WHERE automation_id = ?1 AND status IN ('queued', 'admitting') LIMIT 1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(run_id) = unadmitted_run_id {
        transaction.execute(
            "UPDATE automation_runs
             SET status = 'failed', status_revision = status_revision + 1,
                 retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
                 report_kind = COALESCE(report_kind, 'unknown'),
                 error_code = ?1, error_message = ?2,
                 attention_required_at = MAX(COALESCE(attention_required_at, 0), ?3),
                 completed_at = MAX(created_at, ?3), updated_at = MAX(updated_at, ?3)
             WHERE id = ?4 AND status IN ('queued', 'admitting')",
            params![blocked_code, blocked_message, timestamp, run_id],
        )?;
        let run = query_run(transaction, &run_id)?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
        insert_event(
            transaction,
            "run_updated",
            automation_id,
            Some(&run_id),
            Some(run.status_revision),
            r#"{"status":"failed"}"#,
            timestamp,
        )?;
        insert_event(
            transaction,
            "attention_changed",
            automation_id,
            Some(&run_id),
            Some(run.status_revision),
            &serde_json::json!({ "errorCode": blocked_code }).to_string(),
            timestamp,
        )?;
        transaction.execute(
            "UPDATE automations SET last_run_at = MAX(COALESCE(last_run_at, 0), ?1)
             WHERE id = ?2",
            params![timestamp, automation_id],
        )?;
    }
    Ok(())
}

pub fn set_automation_status(
    connection: &mut Connection,
    automation_id: &str,
    expected_revision: i64,
    status: StoredAutomationStatus,
    resumed_next_run_at: Option<i64>,
) -> rusqlite::Result<AutomationCompareAndSetOutcome> {
    let transaction = connection.transaction()?;
    let Some(existing) = query_automation(&transaction, automation_id, false)? else {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::NotFound);
    };
    if existing.revision != expected_revision {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::RevisionConflict(existing));
    }
    let timestamp = now_ms().max(existing.updated_at.saturating_add(1));
    let next_run_at = match (status, existing.config.health_state.as_str()) {
        (StoredAutomationStatus::Active, "ok") => resumed_next_run_at,
        _ => None,
    };
    transaction.execute(
        "UPDATE automations
         SET status = ?1, next_run_at = ?2, revision = revision + 1, updated_at = ?3
         WHERE id = ?4 AND deleted_at IS NULL AND revision = ?5",
        params![
            status.as_str(),
            next_run_at,
            timestamp,
            automation_id,
            expected_revision
        ],
    )?;
    let updated =
        query_automation(&transaction, automation_id, false)?.expect("updated automation");
    insert_event(
        &transaction,
        "updated",
        automation_id,
        None,
        Some(updated.revision),
        "{}",
        timestamp,
    )?;
    transaction.commit()?;
    Ok(AutomationCompareAndSetOutcome::Updated(updated))
}

pub fn tombstone_automation(
    connection: &mut Connection,
    automation_id: &str,
    expected_revision: i64,
) -> rusqlite::Result<AutomationCompareAndSetOutcome> {
    let transaction = connection.transaction()?;
    let Some(existing) = query_automation(&transaction, automation_id, false)? else {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::NotFound);
    };
    if existing.revision != expected_revision {
        transaction.rollback()?;
        return Ok(AutomationCompareAndSetOutcome::RevisionConflict(existing));
    }
    let timestamp = now_ms().max(existing.updated_at.saturating_add(1));
    transaction.execute(
        "UPDATE automations
         SET next_run_at = NULL, deleted_at = ?1, revision = revision + 1, updated_at = ?1
         WHERE id = ?2 AND deleted_at IS NULL AND revision = ?3",
        params![timestamp, automation_id, expected_revision],
    )?;
    let affected_run_id = transaction
        .query_row(
            "SELECT id FROM automation_runs
             WHERE automation_id = ?1
               AND status IN ('queued', 'admitting', 'running', 'waiting_for_approval')
             LIMIT 1",
            [automation_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if let Some(run_id) = affected_run_id {
        transaction.execute(
            "UPDATE automation_runs
             SET status = CASE
                     WHEN status IN ('queued', 'admitting') THEN 'cancelled'
                     ELSE status
                 END,
                 status_revision = status_revision + 1,
                 retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
                 cancellation_requested_at = ?1,
                 error_code = CASE
                     WHEN status IN ('queued', 'admitting') THEN 'automation_deleted'
                     ELSE error_code
                 END,
                 error_message = CASE
                     WHEN status IN ('queued', 'admitting')
                     THEN 'The scheduled task was deleted before it started.'
                     ELSE error_message
                 END,
                 completed_at = CASE
                     WHEN status IN ('queued', 'admitting') THEN MAX(created_at, ?1)
                     ELSE completed_at
                 END,
                 updated_at = MAX(updated_at, ?1)
             WHERE id = ?2
               AND status IN ('queued', 'admitting', 'running', 'waiting_for_approval')",
            params![timestamp, run_id],
        )?;
        let run = query_run(&transaction, &run_id)?.expect("tombstoned automation run");
        let payload = if run.status == StoredAutomationRunStatus::Cancelled {
            r#"{"status":"cancelled"}"#
        } else {
            r#"{"cancellationRequested":true}"#
        };
        insert_event(
            &transaction,
            "run_updated",
            automation_id,
            Some(&run_id),
            Some(run.status_revision),
            payload,
            timestamp,
        )?;
    }
    transaction.execute(
        "UPDATE automation_notification_outbox
         SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL
         WHERE automation_id = ?1 AND status = 'pending'",
        [automation_id],
    )?;
    let deleted = query_automation(&transaction, automation_id, true)?.expect("deleted automation");
    insert_event(
        &transaction,
        "deleted",
        automation_id,
        None,
        Some(deleted.revision),
        "{}",
        timestamp,
    )?;
    transaction.commit()?;
    Ok(AutomationCompareAndSetOutcome::Updated(deleted))
}

pub fn enqueue_manual_automation_run(
    connection: &mut Connection,
    input: &NewManualAutomationRunRecord,
) -> rusqlite::Result<AutomationRunEnqueueOutcome> {
    let transaction = connection.transaction()?;
    if let Some(existing) = query_run_by_manual_request(&transaction, &input.manual_request_id)? {
        transaction.rollback()?;
        return Ok(AutomationRunEnqueueOutcome::Existing(existing));
    }
    let Some(task) = query_automation(&transaction, &input.automation_id, false)? else {
        transaction.rollback()?;
        return Ok(AutomationRunEnqueueOutcome::AutomationNotFound);
    };
    if task.revision != input.expected_revision || input.config_revision != task.revision {
        transaction.rollback()?;
        return Ok(AutomationRunEnqueueOutcome::RevisionConflict(Box::new(
            task,
        )));
    }
    if let Some(active) = query_nonterminal_run(&transaction, &input.automation_id)? {
        transaction.rollback()?;
        return Ok(AutomationRunEnqueueOutcome::ActiveConflict(active));
    }

    let timestamp = now_ms();
    transaction.execute(
        "INSERT INTO automation_runs (
            id, schema_version, automation_id, config_revision, config_snapshot_json,
            trigger_kind, scheduled_for, manual_request_id, status, retry_at,
            admission_attempt, agent_run_id, conversation_id, user_message_id,
            assistant_message_id, report_kind, result_preview, error_code, error_message,
            attention_required_at, attention_read_at, created_at, started_at,
            completed_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            'manual', ?6, ?7, 'queued', NULL,
            0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
            NULL, NULL, ?8, NULL, NULL, ?8
         )",
        params![
            &input.id,
            AUTOMATION_RUN_SCHEMA_VERSION,
            &input.automation_id,
            input.config_revision,
            &input.config_snapshot_json,
            input.scheduled_for,
            &input.manual_request_id,
            timestamp,
        ],
    )?;
    let run = query_run(&transaction, &input.id)?.expect("inserted automation run");
    insert_event(
        &transaction,
        "run_updated",
        &input.automation_id,
        Some(&input.id),
        Some(run.status_revision),
        r#"{"status":"queued"}"#,
        timestamp,
    )?;
    transaction.commit()?;
    Ok(AutomationRunEnqueueOutcome::Enqueued(run))
}

/// Returns the fair, bounded due-task frontier without acquiring a write lock. The caller computes
/// each next occurrence outside SQLite, then submits the exact observed revision/occurrence to
/// `enqueue_scheduled_automation_run`, which performs the authoritative compare-and-set.
pub fn list_due_automations(
    connection: &Connection,
    due_at_or_before: i64,
    limit: usize,
) -> rusqlite::Result<Vec<AutomationRecord>> {
    let sql = automation_select_sql(
        "WHERE deleted_at IS NULL
           AND status = 'active'
           AND health_state = 'ok'
           AND next_run_at IS NOT NULL
           AND next_run_at <= ?1
           AND NOT EXISTS (
               SELECT 1 FROM automation_runs AS run
               WHERE run.automation_id = automations.id
                 AND run.status IN ('queued', 'admitting', 'running', 'waiting_for_approval')
           )
         ORDER BY next_run_at ASC, id ASC
         LIMIT ?2",
    );
    let mut statement = connection.prepare(&sql)?;
    let records = statement
        .query_map(
            params![due_at_or_before, limit.clamp(1, 3) as i64],
            row_to_automation,
        )?
        .collect();
    records
}

/// Atomically persists one scheduled/recovery occurrence and advances the task to a strictly
/// future occurrence. A stale scheduler cannot duplicate an occurrence or overwrite a newer edit.
pub fn enqueue_scheduled_automation_run(
    connection: &mut Connection,
    input: &NewScheduledAutomationRunRecord,
) -> rusqlite::Result<ScheduledAutomationRunEnqueueOutcome> {
    if !matches!(input.trigger_kind.as_str(), "scheduled" | "recovery")
        || input.next_run_at <= input.claimed_at
        || input.claimed_at < 0
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) =
        query_scheduled_occurrence(&transaction, &input.automation_id, input.scheduled_for)?
    {
        transaction.rollback()?;
        return Ok(ScheduledAutomationRunEnqueueOutcome::Existing(existing));
    }
    let Some(task) = query_automation(&transaction, &input.automation_id, false)? else {
        transaction.rollback()?;
        return Ok(ScheduledAutomationRunEnqueueOutcome::NoLongerDue);
    };
    if task.status != StoredAutomationStatus::Active
        || task.config.health_state != "ok"
        || task.revision != input.expected_revision
        || input.config_revision != task.revision
        || task.config.next_run_at != Some(input.scheduled_for)
    {
        transaction.rollback()?;
        return Ok(ScheduledAutomationRunEnqueueOutcome::NoLongerDue);
    }
    if let Some(active) = query_nonterminal_run(&transaction, &input.automation_id)? {
        transaction.rollback()?;
        return Ok(ScheduledAutomationRunEnqueueOutcome::ActiveConflict(active));
    }

    transaction.execute(
        "INSERT INTO automation_runs (
            id, schema_version, automation_id, config_revision, config_snapshot_json,
            trigger_kind, scheduled_for, manual_request_id, status, retry_at,
            admission_attempt, agent_run_id, conversation_id, user_message_id,
            assistant_message_id, report_kind, result_preview, error_code, error_message,
            attention_required_at, attention_read_at, created_at, started_at,
            completed_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, 'queued', NULL,
            0, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL,
            NULL, NULL, ?8, NULL, NULL, ?8
         )",
        params![
            &input.id,
            AUTOMATION_RUN_SCHEMA_VERSION,
            &input.automation_id,
            input.config_revision,
            &input.config_snapshot_json,
            &input.trigger_kind,
            input.scheduled_for,
            input.claimed_at,
        ],
    )?;
    transaction.execute(
        "UPDATE automations
         SET last_scheduled_at = ?1, next_run_at = ?2
         WHERE id = ?3 AND deleted_at IS NULL AND revision = ?4 AND next_run_at = ?1",
        params![
            input.scheduled_for,
            input.next_run_at,
            &input.automation_id,
            input.expected_revision,
        ],
    )?;
    let run = query_run(&transaction, &input.id)?.expect("inserted scheduled automation run");
    insert_event(
        &transaction,
        "run_updated",
        &input.automation_id,
        Some(&input.id),
        Some(run.status_revision),
        r#"{"status":"queued"}"#,
        input.claimed_at,
    )?;
    transaction.commit()?;
    Ok(ScheduledAutomationRunEnqueueOutcome::Enqueued(run))
}

/// Claims queued work, or an expired pre-admission lease after a crash. The short transaction only
/// updates durable queue state; no Agent or external operation may occur before it commits.
pub fn claim_ready_automation_runs(
    connection: &mut Connection,
    now: i64,
    lease_duration_ms: i64,
    limit: usize,
) -> rusqlite::Result<Vec<AutomationRunRecord>> {
    if now < 0 || lease_duration_ms < 1_000 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let limit = limit.clamp(1, 3);
    let lease_expires_at = now.saturating_add(lease_duration_ms.clamp(1_000, 300_000));
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ids = {
        let mut statement = transaction.prepare(
            "SELECT run.id
             FROM automation_runs AS run
             INNER JOIN automations AS task ON task.id = run.automation_id
             WHERE task.deleted_at IS NULL
               AND task.health_state = 'ok'
               AND (
                    (run.status = 'queued' AND COALESCE(run.retry_at, 0) <= ?1)
                    OR (
                        run.status = 'admitting'
                        AND run.admission_expires_at IS NOT NULL
                        AND run.admission_expires_at <= ?1
                    )
               )
             ORDER BY COALESCE(run.retry_at, run.scheduled_for) ASC,
                      run.scheduled_for ASC, run.id ASC
             LIMIT ?2",
        )?;
        let records = statement
            .query_map(params![now, limit as i64], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
    };
    let mut claimed = Vec::with_capacity(ids.len());
    for run_id in ids {
        let token = format!("automation-admission:{}", uuid::Uuid::new_v4());
        let changed = transaction.execute(
            "UPDATE automation_runs
             SET status = 'admitting', status_revision = status_revision + 1,
                 retry_at = NULL, admission_attempt = admission_attempt + 1,
                 admission_token = ?1, admission_expires_at = ?2,
                 error_code = NULL, error_message = NULL, updated_at = MAX(updated_at, ?3)
             WHERE id = ?4
               AND (
                    (status = 'queued' AND COALESCE(retry_at, 0) <= ?3)
                    OR (status = 'admitting' AND admission_expires_at <= ?3)
               )",
            params![token, lease_expires_at, now, run_id],
        )?;
        if changed != 1 {
            continue;
        }
        let run = query_run(&transaction, &run_id)?.expect("claimed automation run");
        insert_event(
            &transaction,
            "run_updated",
            &run.automation_id,
            Some(&run.id),
            Some(run.status_revision),
            r#"{"status":"starting"}"#,
            now,
        )?;
        claimed.push(run);
    }
    transaction.commit()?;
    Ok(claimed)
}

/// Startup-only crash recovery. At this lifecycle point the prior core-server process is known to
/// be gone, so every still-admitting row represents a pre-atomic-admission crash and can be made
/// immediately claimable without waiting for its old process lease.
pub fn recover_automation_admission_leases_on_startup(
    connection: &mut Connection,
    recovered_at: i64,
) -> rusqlite::Result<Vec<AutomationRunRecord>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ids = {
        let mut statement = transaction.prepare(
            "SELECT id FROM automation_runs WHERE status = 'admitting'
             ORDER BY scheduled_for ASC, id ASC",
        )?;
        let records = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
    };
    let mut recovered = Vec::with_capacity(ids.len());
    for run_id in ids {
        let task_deleted = transaction.query_row(
            "SELECT deleted_at IS NOT NULL FROM automations
             WHERE id = (SELECT automation_id FROM automation_runs WHERE id = ?1)",
            [&run_id],
            |row| row.get::<_, bool>(0),
        )?;
        if task_deleted {
            transaction.execute(
                "UPDATE automation_runs
                 SET status = 'cancelled', status_revision = status_revision + 1,
                     retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
                     cancellation_requested_at = MAX(COALESCE(cancellation_requested_at, 0), ?1),
                     error_code = 'automation_deleted',
                     error_message = 'The scheduled task was deleted before it started.',
                     completed_at = MAX(created_at, ?1), updated_at = MAX(updated_at, ?1)
                 WHERE id = ?2 AND status = 'admitting'",
                params![recovered_at, run_id],
            )?;
        } else {
            transaction.execute(
                "UPDATE automation_runs
                 SET status = 'queued', status_revision = status_revision + 1,
                     retry_at = ?1, admission_token = NULL, admission_expires_at = NULL,
                     updated_at = MAX(updated_at, ?1)
                 WHERE id = ?2 AND status = 'admitting'",
                params![recovered_at, run_id],
            )?;
        }
        let run = query_run(&transaction, &run_id)?.expect("recovered automation admission");
        let payload = serde_json::json!({ "status": run.status.as_str(), "recovered": true });
        insert_event(
            &transaction,
            "run_updated",
            &run.automation_id,
            Some(&run.id),
            Some(run.status_revision),
            &payload.to_string(),
            recovered_at,
        )?;
        recovered.push(run);
    }
    transaction.commit()?;
    Ok(recovered)
}

/// Returns an unadmitted claim to the durable queue with a caller-selected absolute retry time.
/// Expected capacity pressure and a busy target conversation are represented here, not as failures.
pub fn defer_automation_run(
    connection: &mut Connection,
    automation_run_id: &str,
    admission_token: &str,
    retry_at: i64,
    deferred_at: i64,
) -> rusqlite::Result<AutomationRunMutationOutcome> {
    if retry_at <= deferred_at {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE automation_runs
         SET status = 'queued', status_revision = status_revision + 1,
             retry_at = ?1, admission_token = NULL, admission_expires_at = NULL,
             updated_at = MAX(updated_at, ?2)
         WHERE id = ?3 AND status = 'admitting' AND admission_token = ?4",
        params![retry_at, deferred_at, automation_run_id, admission_token],
    )?;
    let current = query_run(&transaction, automation_run_id)?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(current));
    }
    let run = current.expect("deferred automation run");
    insert_event(
        &transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        r#"{"status":"queued"}"#,
        deferred_at,
    )?;
    transaction.commit()?;
    Ok(AutomationRunMutationOutcome::Updated(run))
}

/// Terminates a claimed run that failed before atomic HumanRoot admission, so no trace exists yet.
/// The admission token is the authority boundary; bound/running runs must settle from their trace.
#[allow(clippy::too_many_arguments)]
pub fn terminate_unadmitted_automation_run(
    connection: &mut Connection,
    automation_run_id: &str,
    admission_token: &str,
    terminal_status: StoredAutomationRunStatus,
    error_code: &str,
    error_message: &str,
    settled_at: i64,
) -> rusqlite::Result<AutomationRunMutationOutcome> {
    if !matches!(
        terminal_status,
        StoredAutomationRunStatus::Failed | StoredAutomationRunStatus::Cancelled
    ) || error_code.is_empty()
        || error_code.len() > 128
        || error_message.is_empty()
        || error_message.len() > 4_096
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let attention_required = terminal_status == StoredAutomationRunStatus::Failed;
    let changed = transaction.execute(
        "UPDATE automation_runs
         SET status = ?1, status_revision = status_revision + 1,
             retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
             report_kind = COALESCE(report_kind, 'unknown'),
             error_code = ?2, error_message = ?3,
             attention_required_at = CASE
                 WHEN ?5 THEN MAX(COALESCE(attention_required_at, 0), ?4)
                 ELSE attention_required_at
             END,
             completed_at = MAX(created_at, ?4), updated_at = MAX(updated_at, ?4)
         WHERE id = ?6 AND status = 'admitting' AND admission_token = ?7
           AND agent_run_id IS NULL",
        params![
            terminal_status.as_str(),
            error_code,
            error_message,
            settled_at,
            attention_required,
            automation_run_id,
            admission_token,
        ],
    )?;
    let current = query_run(&transaction, automation_run_id)?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(current));
    }
    let run = current.expect("terminated unadmitted automation run");
    transaction.execute(
        "UPDATE automations SET last_run_at = MAX(COALESCE(last_run_at, 0), ?1)
         WHERE id = ?2",
        params![settled_at, &run.automation_id],
    )?;
    let payload = serde_json::json!({ "status": run.status.as_str() });
    insert_event(
        &transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        &payload.to_string(),
        settled_at,
    )?;
    if attention_required {
        insert_event(
            &transaction,
            "attention_changed",
            &run.automation_id,
            Some(&run.id),
            Some(run.status_revision),
            "{}",
            settled_at,
        )?;
    }
    let task_is_live = transaction
        .query_row(
            "SELECT deleted_at IS NULL FROM automations WHERE id = ?1",
            [&run.automation_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if task_is_live {
        let (title, policy) = run_notification_config(&run);
        if terminal_notification_requested(&policy, run.status, "unknown") {
            enqueue_automation_notification_in_transaction(
                &transaction,
                &NewAutomationNotificationRecord {
                    automation_id: run.automation_id.clone(),
                    automation_run_id: Some(run.id.clone()),
                    resource_revision: run.status_revision,
                    notification_kind: "run_result".to_string(),
                    title,
                    body: error_message.to_string(),
                    created_at: settled_at,
                },
            )?;
        }
    }
    transaction.commit()?;
    Ok(AutomationRunMutationOutcome::Updated(run))
}

pub fn get_automation_run(
    connection: &Connection,
    automation_run_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    query_run(connection, automation_run_id)
}

pub fn get_automation_run_by_agent_run_id(
    connection: &Connection,
    agent_run_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql("WHERE agent_run_id = ?1").as_str(),
            [agent_run_id],
            row_to_run,
        )
        .optional()
}

/// Revalidates elevated Automation permission enablement at the same SQLite linearization point
/// used to install the Conversation, message pair, Trace, and run binding.
///
/// `save_ui_preferences` is a SQLite write. Because the caller owns a `BEGIN IMMEDIATE`
/// transaction, a preference revocation that committed before this check is necessarily visible,
/// while a concurrent revocation that commits afterward is ordered after Turn admission. This
/// closes the read-precheck/admission TOCTOU without weakening frozen run permissions.
///
/// When an elevated mode has been revoked, the task update deliberately relies on the canonical
/// Automation triggers to fail every unadmitted run, create task/run attention events, and enqueue
/// the deduplicated configuration-blocked notification in this same transaction.
pub fn revalidate_automation_permission_for_admission_in_transaction(
    transaction: &Transaction<'_>,
    input: &AutomationRunAdmissionInput,
) -> rusqlite::Result<AutomationPermissionAdmissionOutcome> {
    if !matches!(
        input.permission_mode.as_str(),
        "default" | "full" | "custom"
    ) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let Some(current) = query_run(transaction, &input.automation_run_id)? else {
        return Ok(AutomationPermissionAdmissionOutcome::Enabled);
    };
    // Do not let a stale/cancelled caller mutate the task. The regular admission check below will
    // reject it and the caller-owned transaction will roll back any provisional Conversation data.
    if current.status != StoredAutomationRunStatus::Admitting
        || current.admission_token.as_deref() != Some(input.admission_token.as_str())
        || current.config_revision != input.config_revision
        || current.cancellation_requested_at.is_some()
    {
        return Ok(AutomationPermissionAdmissionOutcome::Enabled);
    }
    let permission_enabled = match input.permission_mode.as_str() {
        "default" => true,
        "full" => transaction
            .query_row(
                "SELECT full_permission_enabled FROM ui_preferences WHERE id = 'default'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(true),
        "custom" => transaction
            .query_row(
                "SELECT custom_permission_enabled FROM ui_preferences WHERE id = 'default'",
                [],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(true),
        _ => unreachable!(),
    };
    if permission_enabled {
        return Ok(AutomationPermissionAdmissionOutcome::Enabled);
    }

    let task_updated_at = transaction
        .query_row(
            "SELECT updated_at FROM automations WHERE id = ?1 AND deleted_at IS NULL",
            [&current.automation_id],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    let Some(task_updated_at) = task_updated_at else {
        return Ok(AutomationPermissionAdmissionOutcome::Enabled);
    };
    let blocked_at = input.admitted_at.max(task_updated_at.saturating_add(1));
    let changed = transaction.execute(
        "UPDATE automations
         SET health_state = 'blocked', blocked_code = 'permission_disabled',
             blocked_message = ?1, next_run_at = NULL,
             attention_required_at = MAX(COALESCE(attention_required_at, 0), ?2),
             revision = revision + 1, updated_at = ?2
         WHERE id = ?3 AND deleted_at IS NULL",
        params![
            AUTOMATION_PERMISSION_DISABLED_MESSAGE,
            blocked_at,
            &current.automation_id,
        ],
    )?;
    if changed != 1 {
        return Ok(AutomationPermissionAdmissionOutcome::Enabled);
    }
    let blocked = query_run(transaction, &input.automation_run_id)?
        .expect("permission-blocked automation run remains readable");
    if blocked.status != StoredAutomationRunStatus::Failed
        || blocked.error_code.as_deref() != Some("permission_disabled")
        || blocked.agent_run_id.is_some()
        || blocked.conversation_id.is_some()
        || blocked.user_message_id.is_some()
        || blocked.assistant_message_id.is_some()
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(AutomationPermissionAdmissionOutcome::Blocked)
}

/// Completes the exactly-once Automation side of root Turn admission inside the caller-owned
/// `BEGIN IMMEDIATE` transaction that has already written conversation/messages/trace.
pub fn admit_automation_run_in_transaction(
    transaction: &Transaction<'_>,
    input: &AutomationRunAdmissionInput,
) -> rusqlite::Result<AutomationRunAdmissionOutcome> {
    let current = query_run(transaction, &input.automation_run_id)?;
    let Some(current) = current else {
        return Ok(AutomationRunAdmissionOutcome::Stale(None));
    };
    if current.status == StoredAutomationRunStatus::Cancelled {
        return Ok(AutomationRunAdmissionOutcome::Cancelled(current));
    }
    let same_identity = current.agent_run_id.as_deref() == Some(input.agent_run_id.as_str())
        && current.conversation_id.as_deref() == Some(input.conversation_id.as_str())
        && current.user_message_id.as_deref() == Some(input.user_message_id.as_str())
        && current.assistant_message_id.as_deref() == Some(input.assistant_message_id.as_str())
        && current.config_revision == input.config_revision;
    if same_identity
        && matches!(
            current.status,
            StoredAutomationRunStatus::Running
                | StoredAutomationRunStatus::WaitingForApproval
                | StoredAutomationRunStatus::Completed
                | StoredAutomationRunStatus::Failed
        )
    {
        return Ok(AutomationRunAdmissionOutcome::Replayed(current));
    }
    let task_is_live = transaction.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM automations
             WHERE id = ?1 AND deleted_at IS NULL AND health_state = 'ok'
         )",
        [&current.automation_id],
        |row| row.get::<_, bool>(0),
    )?;
    if !task_is_live {
        let changed = transaction.execute(
            "UPDATE automation_runs
             SET status = 'cancelled', status_revision = status_revision + 1,
                 admission_token = NULL, admission_expires_at = NULL,
                 error_code = 'automation_unavailable',
                 error_message = 'The scheduled task is no longer available.',
                 completed_at = MAX(created_at, ?1), updated_at = MAX(updated_at, ?1)
             WHERE id = ?2 AND status = 'admitting' AND admission_token = ?3",
            params![
                input.admitted_at,
                &input.automation_run_id,
                &input.admission_token
            ],
        )?;
        let cancelled = query_run(transaction, &input.automation_run_id)?
            .expect("automation run remains after task tombstone");
        if changed == 1 {
            insert_event(
                transaction,
                "run_updated",
                &cancelled.automation_id,
                Some(&cancelled.id),
                Some(cancelled.status_revision),
                r#"{"status":"cancelled"}"#,
                input.admitted_at,
            )?;
            return Ok(AutomationRunAdmissionOutcome::Cancelled(cancelled));
        }
        return Ok(AutomationRunAdmissionOutcome::Stale(Some(cancelled)));
    }
    if current.status != StoredAutomationRunStatus::Admitting
        || current.admission_token.as_deref() != Some(input.admission_token.as_str())
        || current.config_revision != input.config_revision
        || current.cancellation_requested_at.is_some()
    {
        return Ok(AutomationRunAdmissionOutcome::Stale(Some(current)));
    }
    let durable_identity_exists = transaction.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM messages
                   WHERE id = ?1 AND conversation_id = ?2 AND role = 'user'),
            EXISTS(SELECT 1 FROM messages
                   WHERE id = ?3 AND conversation_id = ?2 AND role = 'assistant'),
            EXISTS(SELECT 1 FROM conversation_turn_traces
                   WHERE assistant_message_id = ?3 AND conversation_id = ?2 AND run_id = ?4)",
        params![
            &input.user_message_id,
            &input.conversation_id,
            &input.assistant_message_id,
            &input.agent_run_id,
        ],
        |row| {
            Ok((
                row.get::<_, bool>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, bool>(2)?,
            ))
        },
    )?;
    if durable_identity_exists != (true, true, true) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let started_at = input.admitted_at.max(current.created_at);
    let changed = transaction.execute(
        "UPDATE automation_runs
         SET status = 'running', status_revision = status_revision + 1,
             retry_at = NULL, admission_token = NULL, admission_expires_at = NULL,
             agent_run_id = ?1, conversation_id = ?2, user_message_id = ?3,
             assistant_message_id = ?4, started_at = ?5, updated_at = MAX(updated_at, ?5)
         WHERE id = ?6 AND status = 'admitting' AND admission_token = ?7
           AND config_revision = ?8 AND agent_run_id IS NULL
           AND conversation_id IS NULL AND user_message_id IS NULL
           AND assistant_message_id IS NULL",
        params![
            &input.agent_run_id,
            &input.conversation_id,
            &input.user_message_id,
            &input.assistant_message_id,
            started_at,
            &input.automation_run_id,
            &input.admission_token,
            input.config_revision,
        ],
    )?;
    if changed != 1 {
        return Ok(AutomationRunAdmissionOutcome::Stale(query_run(
            transaction,
            &input.automation_run_id,
        )?));
    }
    let run = query_run(transaction, &input.automation_run_id)?.expect("admitted automation run");
    insert_event(
        transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        r#"{"status":"running"}"#,
        input.admitted_at,
    )?;
    Ok(AutomationRunAdmissionOutcome::Admitted(run))
}

pub fn record_automation_report(
    connection: &mut Connection,
    automation_run_id: &str,
    report_kind: &str,
    summary: &str,
    reported_at: i64,
) -> rusqlite::Result<AutomationRunMutationOutcome> {
    if !matches!(report_kind, "no_change" | "important_update" | "completed")
        || summary.is_empty()
        || summary.len() > 2_048
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE automation_runs
         SET report_kind = ?1, result_preview = ?2,
             status_revision = status_revision + 1, updated_at = MAX(updated_at, ?3)
         WHERE id = ?4 AND agent_run_id IS NOT NULL
           AND status IN ('running', 'waiting_for_approval')
           AND report_kind IS NULL",
        params![report_kind, summary, reported_at, automation_run_id],
    )?;
    let current = query_run(&transaction, automation_run_id)?;
    if changed != 1 {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(current));
    }
    let run = current.expect("reported automation run");
    let payload = serde_json::json!({ "status": run.status.as_str(), "reportKind": report_kind });
    insert_event(
        &transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        &payload.to_string(),
        reported_at,
    )?;
    transaction.commit()?;
    Ok(AutomationRunMutationOutcome::Updated(run))
}

/// Projects the durable pending-action state into the Automation run without treating approval as
/// a failure. Entering approval creates attention and an idempotent native-notification request;
/// leaving approval acknowledges that transient attention and suppresses stale pending notices.
pub fn set_automation_run_waiting_for_approval(
    connection: &mut Connection,
    automation_run_id: &str,
    agent_run_id: &str,
    waiting: bool,
    changed_at: i64,
) -> rusqlite::Result<AutomationRunMutationOutcome> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(current) = query_run(&transaction, automation_run_id)? else {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(None));
    };
    if current.agent_run_id.as_deref() != Some(agent_run_id) {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(Some(current)));
    }
    let desired = if waiting {
        StoredAutomationRunStatus::WaitingForApproval
    } else {
        StoredAutomationRunStatus::Running
    };
    if current.status == desired {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Updated(current));
    }
    let expected = if waiting {
        "running"
    } else {
        "waiting_for_approval"
    };
    if waiting {
        let has_pending = transaction.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM agent_pending_actions
                 WHERE run_id = ?1 AND status IN ('pending', 'approved', 'executing')
             )",
            [agent_run_id],
            |row| row.get::<_, bool>(0),
        )?;
        if !has_pending {
            transaction.rollback()?;
            return Ok(AutomationRunMutationOutcome::Stale(Some(current)));
        }
    }
    let changed = if waiting {
        transaction.execute(
            "UPDATE automation_runs
             SET status = 'waiting_for_approval', status_revision = status_revision + 1,
                 attention_required_at = MAX(COALESCE(attention_required_at, 0), ?1),
                 updated_at = MAX(updated_at, ?1)
             WHERE id = ?2 AND status = ?3 AND agent_run_id = ?4",
            params![changed_at, automation_run_id, expected, agent_run_id],
        )?
    } else {
        transaction.execute(
            "UPDATE automation_runs
             SET status = 'running', status_revision = status_revision + 1,
                 attention_read_at = CASE
                     WHEN attention_required_at IS NULL THEN attention_read_at
                     ELSE MAX(COALESCE(attention_read_at, 0), attention_required_at, ?1)
                 END,
                 updated_at = MAX(updated_at, ?1)
             WHERE id = ?2 AND status = ?3 AND agent_run_id = ?4",
            params![changed_at, automation_run_id, expected, agent_run_id],
        )?
    };
    if changed != 1 {
        let current = query_run(&transaction, automation_run_id)?;
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(current));
    }
    let run = query_run(&transaction, automation_run_id)?.expect("updated approval run");
    let payload = serde_json::json!({ "status": run.status.as_str() });
    insert_event(
        &transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        &payload.to_string(),
        changed_at,
    )?;
    insert_event(
        &transaction,
        "attention_changed",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        "{}",
        changed_at,
    )?;
    if waiting {
        let task_is_live = transaction
            .query_row(
                "SELECT deleted_at IS NULL FROM automations WHERE id = ?1",
                [&run.automation_id],
                |row| row.get::<_, bool>(0),
            )
            .optional()?
            .unwrap_or(false);
        if task_is_live {
            let (title, _) = run_notification_config(&run);
            enqueue_automation_notification_in_transaction(
                &transaction,
                &NewAutomationNotificationRecord {
                    automation_id: run.automation_id.clone(),
                    automation_run_id: Some(run.id.clone()),
                    resource_revision: run.status_revision,
                    notification_kind: "approval_required".to_string(),
                    title,
                    body: "This scheduled task is waiting for your approval.".to_string(),
                    created_at: changed_at,
                },
            )?;
        }
    } else {
        transaction.execute(
            "UPDATE automation_notification_outbox
             SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL
             WHERE automation_run_id = ?1 AND notification_kind = 'approval_required'
               AND status = 'pending'",
            [&run.id],
        )?;
    }
    transaction.commit()?;
    Ok(AutomationRunMutationOutcome::Updated(run))
}

/// Settles a bound Automation run only after the durable conversation trace is terminal. This
/// makes process-local Agent events an optimization rather than the source of truth.
pub fn settle_automation_run_from_trace(
    connection: &mut Connection,
    input: &AutomationRunSettlementInput,
) -> rusqlite::Result<AutomationRunMutationOutcome> {
    if !input.terminal_status.is_terminal()
        || input.report_kind.as_deref().is_some_and(|kind| {
            !matches!(
                kind,
                "no_change" | "important_update" | "completed" | "unknown"
            )
        })
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let result_preview = input
        .result_preview
        .as_deref()
        .map(|value| truncate_utf8(value, 8_192));
    let error_code = input
        .error_code
        .as_deref()
        .map(|value| truncate_utf8(value, 128));
    let error_message = input
        .error_message
        .as_deref()
        .map(|value| truncate_utf8(value, 4_096));
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(current) = query_run(&transaction, &input.automation_run_id)? else {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(None));
    };
    if current.agent_run_id.as_deref() != Some(input.agent_run_id.as_str()) {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(Some(current)));
    }
    if current.status.is_terminal() {
        transaction.rollback()?;
        return if current.status == input.terminal_status {
            Ok(AutomationRunMutationOutcome::Updated(current))
        } else {
            Ok(AutomationRunMutationOutcome::Stale(Some(current)))
        };
    }
    let trace_status = transaction
        .query_row(
            "SELECT terminal_status FROM conversation_turn_traces
             WHERE run_id = ?1 AND conversation_id = ?2 AND assistant_message_id = ?3",
            params![
                &input.agent_run_id,
                current.conversation_id.as_deref(),
                current.assistant_message_id.as_deref(),
            ],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if trace_status.as_deref() != Some(input.terminal_status.as_str()) {
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(Some(current)));
    }
    let attention_required = input.terminal_status == StoredAutomationRunStatus::Failed
        || current.report_kind.as_deref() == Some("important_update")
        || input.report_kind.as_deref() == Some("important_update");
    let settled_at = input.settled_at.max(current.created_at);
    let changed = transaction.execute(
        "UPDATE automation_runs
         SET status = ?1, status_revision = status_revision + 1,
             admission_token = NULL, admission_expires_at = NULL, retry_at = NULL,
             report_kind = COALESCE(report_kind, ?2, 'unknown'),
             result_preview = CASE
                 WHEN report_kind IS NOT NULL THEN result_preview
                 ELSE ?3
             END,
             error_code = ?4, error_message = ?5,
             attention_required_at = CASE
                 WHEN ?6 THEN MAX(COALESCE(attention_required_at, 0), ?7)
                 ELSE attention_required_at
             END,
             attention_read_at = CASE
                 WHEN ?6 THEN attention_read_at
                 WHEN attention_required_at IS NULL THEN attention_read_at
                 ELSE MAX(COALESCE(attention_read_at, 0), attention_required_at, ?7)
             END,
             completed_at = ?7, updated_at = MAX(updated_at, ?7)
         WHERE id = ?8 AND agent_run_id = ?9
           AND status IN ('running', 'waiting_for_approval')",
        params![
            input.terminal_status.as_str(),
            input.report_kind.as_deref(),
            result_preview,
            error_code,
            error_message,
            attention_required,
            settled_at,
            &input.automation_run_id,
            &input.agent_run_id,
        ],
    )?;
    if changed != 1 {
        let current = query_run(&transaction, &input.automation_run_id)?;
        transaction.rollback()?;
        return Ok(AutomationRunMutationOutcome::Stale(current));
    }
    let run = query_run(&transaction, &input.automation_run_id)?.expect("settled automation run");
    transaction.execute(
        "UPDATE automations SET last_run_at = MAX(COALESCE(last_run_at, 0), ?1)
         WHERE id = ?2",
        params![settled_at, &run.automation_id],
    )?;
    let payload = serde_json::json!({
        "status": run.status.as_str(),
        "reportKind": run.report_kind.as_deref().unwrap_or("unknown")
    });
    insert_event(
        &transaction,
        "run_updated",
        &run.automation_id,
        Some(&run.id),
        Some(run.status_revision),
        &payload.to_string(),
        settled_at,
    )?;
    if attention_required {
        insert_event(
            &transaction,
            "attention_changed",
            &run.automation_id,
            Some(&run.id),
            Some(run.status_revision),
            "{}",
            settled_at,
        )?;
    }
    let task_is_live = transaction
        .query_row(
            "SELECT deleted_at IS NULL FROM automations WHERE id = ?1",
            [&run.automation_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    if task_is_live {
        let (title, policy) = run_notification_config(&run);
        let report_kind = run.report_kind.as_deref().unwrap_or("unknown");
        if terminal_notification_requested(&policy, run.status, report_kind) {
            let body = run
                .error_message
                .as_deref()
                .or(run.result_preview.as_deref())
                .unwrap_or_else(|| terminal_default_message(run.status));
            enqueue_automation_notification_in_transaction(
                &transaction,
                &NewAutomationNotificationRecord {
                    automation_id: run.automation_id.clone(),
                    automation_run_id: Some(run.id.clone()),
                    resource_revision: run.status_revision,
                    notification_kind: "run_result".to_string(),
                    title,
                    body: truncate_utf8(body, 4_096),
                    created_at: settled_at,
                },
            )?;
        }
    }
    transaction.execute(
        "UPDATE automation_notification_outbox
         SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL
         WHERE automation_run_id = ?1 AND notification_kind = 'approval_required'
           AND status = 'pending'",
        [&run.id],
    )?;
    transaction.commit()?;
    Ok(AutomationRunMutationOutcome::Updated(run))
}

pub fn list_recoverable_automation_runs(
    connection: &Connection,
) -> rusqlite::Result<Vec<AutomationRunRecoveryRecord>> {
    let sql = format!(
        "SELECT run_record.*,
                trace.terminal_status, trace.terminal_error,
                EXISTS(
                    SELECT 1 FROM agent_pending_actions AS pending
                    WHERE pending.run_id = run_record.agent_run_id
                      AND pending.status IN ('pending', 'approved', 'executing')
                ),
                task.deleted_at IS NOT NULL
         FROM ({}) AS run_record
         INNER JOIN automations AS task ON task.id = run_record.automation_id
         LEFT JOIN conversation_turn_traces AS trace
           ON trace.run_id = run_record.agent_run_id
          AND trace.conversation_id = run_record.conversation_id
          AND trace.assistant_message_id = run_record.assistant_message_id
         ORDER BY run_record.created_at ASC, run_record.id ASC",
        run_select_sql(
            "WHERE status IN ('queued', 'admitting', 'running', 'waiting_for_approval')"
        )
    );
    let mut statement = connection.prepare(&sql)?;
    let records = statement
        .query_map([], |row| {
            Ok(AutomationRunRecoveryRecord {
                run: row_to_run(row)?,
                trace_terminal_status: row.get(29)?,
                trace_terminal_error: row.get(30)?,
                has_pending_action: row.get(31)?,
                automation_deleted: row.get(32)?,
            })
        })?
        .collect();
    records
}

pub fn list_cancellation_requested_automation_runs(
    connection: &Connection,
) -> rusqlite::Result<Vec<AutomationRunRecord>> {
    let mut statement = connection.prepare(&run_select_sql(
        "WHERE cancellation_requested_at IS NOT NULL
               AND status IN ('running', 'waiting_for_approval')
             ORDER BY cancellation_requested_at ASC, id ASC",
    ))?;
    let records = statement.query_map([], row_to_run)?.collect();
    records
}

pub fn enqueue_automation_notification(
    connection: &mut Connection,
    input: &NewAutomationNotificationRecord,
) -> rusqlite::Result<AutomationNotificationRecord> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let record = enqueue_automation_notification_in_transaction(&transaction, input)?;
    transaction.commit()?;
    Ok(record)
}

pub fn enqueue_automation_notification_in_transaction(
    transaction: &Transaction<'_>,
    input: &NewAutomationNotificationRecord,
) -> rusqlite::Result<AutomationNotificationRecord> {
    if input.resource_revision <= 0
        || input.title.is_empty()
        || input.title.len() > 512
        || input.body.is_empty()
        || input.body.len() > 4_096
        || !matches!(
            input.notification_kind.as_str(),
            "run_result" | "approval_required" | "configuration_blocked"
        )
        || (input.notification_kind == "configuration_blocked" && input.automation_run_id.is_some())
        || (input.notification_kind != "configuration_blocked" && input.automation_run_id.is_none())
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let notification_id = format!("automation-notification:{}", uuid::Uuid::new_v4());
    let inserted = transaction.execute(
        "INSERT OR IGNORE INTO automation_notification_outbox (
            id, schema_version, automation_id, automation_run_id, resource_revision,
            notification_kind, title, body, status, retry_at, claim_token,
            claim_expires_at, attempt_count, last_error_code, created_at, delivered_at
         ) VALUES (
            ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8,
            NULL, NULL, 0, NULL, ?8, NULL
         )",
        params![
            notification_id,
            &input.automation_id,
            input.automation_run_id.as_deref(),
            input.resource_revision,
            &input.notification_kind,
            &input.title,
            &input.body,
            input.created_at,
        ],
    )?;
    let record = query_automation_notification_by_identity(transaction, input)?
        .ok_or(rusqlite::Error::InvalidQuery)?;
    if inserted == 1 {
        let payload = serde_json::json!({
            "notificationId": record.id,
            "notificationKind": record.notification_kind,
        });
        insert_event(
            transaction,
            "notification_requested",
            &record.automation_id,
            record.automation_run_id.as_deref(),
            Some(record.resource_revision),
            &payload.to_string(),
            input.created_at,
        )?;
    }
    Ok(record)
}

/// Atomically leases a small pending-notification batch. Replaying the same live claim token
/// returns the original batch; an expired process lease is recoverable after restart.
pub fn claim_pending_automation_notifications(
    connection: &mut Connection,
    claim_token: &str,
    now: i64,
    lease_duration_ms: i64,
    limit: usize,
) -> rusqlite::Result<Vec<AutomationNotificationRecord>> {
    if claim_token.is_empty() || claim_token.len() > 256 || now < 0 || lease_duration_ms < 5_000 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing = list_automation_notifications_by_claim(&transaction, claim_token, now)?;
    if !existing.is_empty() {
        transaction.rollback()?;
        return Ok(existing);
    }
    let limit = limit.clamp(1, 10);
    let lease_expires_at = now.saturating_add(lease_duration_ms.clamp(5_000, 300_000));
    let ids = {
        let mut statement = transaction.prepare(
            "SELECT outbox.id
             FROM automation_notification_outbox AS outbox
             INNER JOIN automations AS task ON task.id = outbox.automation_id
             WHERE outbox.status = 'pending'
               AND outbox.retry_at <= ?1
               AND (outbox.claim_token IS NULL OR outbox.claim_expires_at <= ?1)
               AND task.deleted_at IS NULL
             ORDER BY outbox.created_at ASC, outbox.id ASC
             LIMIT ?2",
        )?;
        let records = statement
            .query_map(params![now, limit as i64], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        records
    };
    for notification_id in ids {
        transaction.execute(
            "UPDATE automation_notification_outbox
             SET claim_token = ?1, claim_expires_at = ?2,
                 attempt_count = attempt_count + 1, last_error_code = NULL
             WHERE id = ?3 AND status = 'pending' AND retry_at <= ?4
               AND (claim_token IS NULL OR claim_expires_at <= ?4)",
            params![claim_token, lease_expires_at, notification_id, now],
        )?;
    }
    let claimed = list_automation_notifications_by_claim(&transaction, claim_token, now)?;
    transaction.commit()?;
    Ok(claimed)
}

/// Revalidates a claimed delivery at the last Host-owned boundary before native display. A claim
/// is only a lease; task deletion or approval settlement may suppress it after the batch was
/// returned. Stale semantic claims are durably suppressed here so no later Host can replay them.
pub fn validate_claimed_automation_notification(
    connection: &mut Connection,
    notification_id: &str,
    claim_token: &str,
    now: i64,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    if notification_id.is_empty()
        || notification_id.len() > 256
        || claim_token.is_empty()
        || claim_token.len() > 256
        || now < 0
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(record) = query_automation_notification(&transaction, notification_id)? else {
        transaction.rollback()?;
        return Ok(None);
    };
    let owns_live_claim = record.status == "pending"
        && record.claim_token.as_deref() == Some(claim_token)
        && record
            .claim_expires_at
            .is_some_and(|expires_at| expires_at > now);
    if !owns_live_claim {
        transaction.rollback()?;
        return Ok(None);
    }
    let task_is_live = transaction
        .query_row(
            "SELECT deleted_at IS NULL FROM automations WHERE id = ?1",
            [&record.automation_id],
            |row| row.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(false);
    let semantically_current = match record.notification_kind.as_str() {
        "configuration_blocked" => {
            task_is_live
                && transaction
                    .query_row(
                        "SELECT health_state = 'blocked' AND revision = ?2
                         FROM automations WHERE id = ?1 AND deleted_at IS NULL",
                        params![&record.automation_id, record.resource_revision],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false)
        }
        "approval_required" => {
            if !task_is_live {
                false
            } else if let Some(run_id) = record.automation_run_id.as_deref() {
                transaction
                    .query_row(
                        "SELECT status = 'waiting_for_approval'
                                AND status_revision = ?2
                                AND EXISTS (
                                    SELECT 1 FROM agent_pending_actions AS pending
                                    WHERE pending.run_id = automation_runs.agent_run_id
                                      AND pending.status = 'pending'
                                )
                         FROM automation_runs WHERE id = ?1",
                        params![run_id, record.resource_revision],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false)
            } else {
                false
            }
        }
        "run_result" => {
            if !task_is_live {
                false
            } else if let Some(run_id) = record.automation_run_id.as_deref() {
                transaction
                    .query_row(
                        "SELECT status IN ('completed', 'failed', 'cancelled')
                                AND status_revision = ?2
                         FROM automation_runs WHERE id = ?1",
                        params![run_id, record.resource_revision],
                        |row| row.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false)
            } else {
                false
            }
        }
        _ => false,
    };
    if !semantically_current {
        transaction.execute(
            "UPDATE automation_notification_outbox
             SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL,
                 last_error_code = NULL
             WHERE id = ?1 AND status = 'pending' AND claim_token = ?2",
            params![notification_id, claim_token],
        )?;
        transaction.commit()?;
        return Ok(None);
    }
    transaction.rollback()?;
    Ok(Some(record))
}

pub fn acknowledge_automation_notification_delivered(
    connection: &mut Connection,
    notification_id: &str,
    claim_token: &str,
    delivered_at: i64,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if let Some(existing) = query_automation_notification(&transaction, notification_id)? {
        if existing.status == "delivered" {
            transaction.rollback()?;
            return Ok(Some(existing));
        }
    }
    let changed = transaction.execute(
        "UPDATE automation_notification_outbox
         SET status = 'delivered', delivered_at = MAX(created_at, ?1),
             claim_token = NULL, claim_expires_at = NULL, last_error_code = NULL
         WHERE id = ?2 AND status = 'pending' AND claim_token = ?3",
        params![delivered_at, notification_id, claim_token],
    )?;
    let record = query_automation_notification(&transaction, notification_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn release_automation_notification(
    connection: &mut Connection,
    notification_id: &str,
    claim_token: &str,
    retry_at: i64,
    error_code: &str,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    if error_code.is_empty() || error_code.len() > 128 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE automation_notification_outbox
         SET retry_at = MAX(retry_at, ?1), claim_token = NULL, claim_expires_at = NULL,
             last_error_code = ?2
         WHERE id = ?3 AND status = 'pending' AND claim_token = ?4",
        params![retry_at, error_code, notification_id, claim_token],
    )?;
    let record = query_automation_notification(&transaction, notification_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn suppress_automation_notification(
    connection: &mut Connection,
    notification_id: &str,
    suppressed_at: i64,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE automation_notification_outbox
         SET status = 'suppressed', claim_token = NULL, claim_expires_at = NULL,
             last_error_code = NULL
         WHERE id = ?1 AND status = 'pending' AND created_at <= ?2",
        params![notification_id, suppressed_at],
    )?;
    let record = query_automation_notification(&transaction, notification_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn list_automation_runs(
    connection: &Connection,
    automation_id: &str,
    cursor: Option<&AutomationRunListCursor>,
    limit: usize,
) -> rusqlite::Result<AutomationRunListPage> {
    let limit = limit.clamp(1, 100);
    let cursor_created_at = cursor.map(|value| value.created_at);
    let cursor_id = cursor.map(|value| value.id.as_str());
    let mut statement = connection.prepare(
        "SELECT
            id, schema_version, automation_id, config_revision, config_snapshot_json,
            trigger_kind, scheduled_for, manual_request_id, status, status_revision, retry_at,
            admission_attempt, admission_token, admission_expires_at,
            cancellation_requested_at, agent_run_id, conversation_id, user_message_id,
            assistant_message_id, report_kind, result_preview, error_code, error_message,
            attention_required_at, attention_read_at, created_at, started_at,
            completed_at, updated_at
         FROM automation_runs
         WHERE automation_id = ?1
           AND (
               ?2 IS NULL
               OR created_at < ?2
               OR (created_at = ?2 AND id < ?3)
           )
         ORDER BY created_at DESC, id DESC
         LIMIT ?4",
    )?;
    let mut items = statement
        .query_map(
            params![
                automation_id,
                cursor_created_at,
                cursor_id,
                limit as i64 + 1
            ],
            row_to_run,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|record| AutomationRunListCursor {
            created_at: record.created_at,
            id: record.id.clone(),
        })
    } else {
        None
    };
    Ok(AutomationRunListPage { items, next_cursor })
}

pub fn get_latest_automation_run(
    connection: &Connection,
    automation_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql("WHERE automation_id = ?1 ORDER BY created_at DESC, id DESC LIMIT 1")
                .as_str(),
            [automation_id],
            row_to_run,
        )
        .optional()
}

pub fn get_automation_run_by_manual_request_id(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    query_run_by_manual_request(connection, request_id)
}

pub fn automation_attention_summary(
    connection: &Connection,
) -> rusqlite::Result<AutomationAttentionSummary> {
    let task_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM automations
         WHERE deleted_at IS NULL
           AND attention_required_at IS NOT NULL
           AND (attention_read_at IS NULL OR attention_read_at < attention_required_at)",
        [],
        |row| row.get(0),
    )?;
    let run_count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM automation_runs AS run
         INNER JOIN automations AS task ON task.id = run.automation_id
         WHERE task.deleted_at IS NULL
           AND run.attention_required_at IS NOT NULL
           AND (run.attention_read_at IS NULL OR run.attention_read_at < run.attention_required_at)",
        [],
        |row| row.get(0),
    )?;
    let task_count = nonnegative_u64(task_count);
    let run_count = nonnegative_u64(run_count);
    Ok(AutomationAttentionSummary {
        task_count,
        run_count,
        total_count: task_count.saturating_add(run_count),
        last_event_sequence: last_event_sequence(connection)?,
    })
}

pub fn list_automation_attentions(
    connection: &Connection,
    cursor: Option<&AutomationAttentionListCursor>,
    limit: usize,
) -> rusqlite::Result<AutomationAttentionListPage> {
    let limit = limit.clamp(1, 100);
    let cursor_required_at = cursor.map(|value| value.required_at);
    let cursor_attention_id = cursor.map(|value| value.attention_id.as_str());
    let mut statement = connection.prepare(
        "WITH unread_attention AS (
            SELECT
                'task:' || task.id AS attention_id,
                'configuration_blocked' AS attention_kind,
                task.id AS automation_id,
                NULL AS automation_run_id,
                task.attention_required_at AS required_at,
                task.attention_read_at AS read_at,
                task.blocked_code AS code,
                task.blocked_message AS message
            FROM automations AS task
            WHERE task.deleted_at IS NULL
              AND task.attention_required_at IS NOT NULL
              AND (task.attention_read_at IS NULL OR task.attention_read_at < task.attention_required_at)
            UNION ALL
            SELECT
                'run:' || run.id AS attention_id,
                CASE
                    WHEN run.status = 'waiting_for_approval' THEN 'waiting_for_approval'
                    WHEN run.status IN ('failed', 'cancelled') THEN 'run_failed'
                    ELSE 'important_update'
                END AS attention_kind,
                run.automation_id,
                run.id AS automation_run_id,
                run.attention_required_at AS required_at,
                run.attention_read_at AS read_at,
                run.error_code AS code,
                COALESCE(
                    run.error_message,
                    run.result_preview,
                    CASE WHEN run.status = 'waiting_for_approval'
                         THEN 'This scheduled task is waiting for your approval.' END
                ) AS message
            FROM automation_runs AS run
            INNER JOIN automations AS task ON task.id = run.automation_id
            WHERE task.deleted_at IS NULL
              AND run.attention_required_at IS NOT NULL
              AND (run.attention_read_at IS NULL OR run.attention_read_at < run.attention_required_at)
         )
         SELECT
            attention_id, attention_kind, automation_id, automation_run_id,
            required_at, read_at, code, message
         FROM unread_attention
         WHERE (
            ?1 IS NULL
            OR required_at < ?1
            OR (required_at = ?1 AND attention_id < ?2)
         )
         ORDER BY required_at DESC, attention_id DESC
         LIMIT ?3",
    )?;
    let mut items = statement
        .query_map(
            params![cursor_required_at, cursor_attention_id, limit as i64 + 1],
            |row| {
                Ok(AutomationAttentionRecord {
                    attention_id: row.get(0)?,
                    attention_kind: row.get(1)?,
                    automation_id: row.get(2)?,
                    automation_run_id: row.get(3)?,
                    required_at: row.get(4)?,
                    read_at: row.get(5)?,
                    code: row.get(6)?,
                    message: row.get(7)?,
                })
            },
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if items.len() > limit {
        items.truncate(limit);
        items.last().map(|record| AutomationAttentionListCursor {
            required_at: record.required_at,
            attention_id: record.attention_id.clone(),
        })
    } else {
        None
    };
    Ok(AutomationAttentionListPage { items, next_cursor })
}

pub fn acknowledge_automation_attention(
    connection: &mut Connection,
    automation_id: &str,
    automation_run_id: Option<&str>,
    acknowledged_at: i64,
) -> rusqlite::Result<bool> {
    let transaction = connection.transaction()?;
    let changed = if let Some(run_id) = automation_run_id {
        transaction.execute(
            "UPDATE automation_runs
             SET attention_read_at = MAX(
                    COALESCE(attention_read_at, 0), attention_required_at, ?1
                 ),
                 status_revision = status_revision + 1,
                 updated_at = MAX(updated_at, attention_required_at, ?1)
             WHERE id = ?2 AND automation_id = ?3 AND attention_required_at IS NOT NULL
               AND (attention_read_at IS NULL OR attention_read_at < attention_required_at)",
            params![acknowledged_at, run_id, automation_id],
        )?
    } else {
        transaction.execute(
            "UPDATE automations
             SET attention_read_at = MAX(
                    COALESCE(attention_read_at, 0), attention_required_at, ?1
                 ),
                 revision = revision + 1,
                 updated_at = MAX(updated_at + 1, attention_required_at, ?1)
             WHERE id = ?2 AND deleted_at IS NULL AND attention_required_at IS NOT NULL
               AND (attention_read_at IS NULL OR attention_read_at < attention_required_at)",
            params![acknowledged_at, automation_id],
        )?
    };
    if changed > 0 {
        let revision = if let Some(run_id) = automation_run_id {
            query_run(&transaction, run_id)?.map(|record| record.status_revision)
        } else {
            query_automation(&transaction, automation_id, false)?.map(|record| record.revision)
        };
        insert_event(
            &transaction,
            "attention_changed",
            automation_id,
            automation_run_id,
            revision,
            "{}",
            acknowledged_at,
        )?;
    }
    transaction.commit()?;
    Ok(changed > 0)
}

pub fn acknowledge_automation_attention_id(
    connection: &mut Connection,
    attention_id: &str,
    acknowledged_at: i64,
) -> rusqlite::Result<bool> {
    if let Some(automation_id) = attention_id.strip_prefix("task:") {
        if automation_id.is_empty() {
            return Ok(false);
        }
        return acknowledge_automation_attention(connection, automation_id, None, acknowledged_at);
    }
    let Some(run_id) = attention_id.strip_prefix("run:") else {
        return Ok(false);
    };
    if run_id.is_empty() {
        return Ok(false);
    }
    let automation_id = connection
        .query_row(
            "SELECT automation_id FROM automation_runs WHERE id = ?1",
            [run_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(automation_id) = automation_id else {
        return Ok(false);
    };
    acknowledge_automation_attention(connection, &automation_id, Some(run_id), acknowledged_at)
}

pub fn get_automation_attention_by_id(
    connection: &Connection,
    attention_id: &str,
) -> rusqlite::Result<Option<AutomationAttentionRecord>> {
    if let Some(automation_id) = attention_id.strip_prefix("task:") {
        if automation_id.is_empty() {
            return Ok(None);
        }
        return connection
            .query_row(
                "SELECT
                    'task:' || id, 'configuration_blocked', id, NULL,
                    attention_required_at, attention_read_at, blocked_code, blocked_message
                 FROM automations
                 WHERE id = ?1 AND deleted_at IS NULL AND attention_required_at IS NOT NULL",
                [automation_id],
                row_to_attention,
            )
            .optional();
    }
    let Some(run_id) = attention_id.strip_prefix("run:") else {
        return Ok(None);
    };
    if run_id.is_empty() {
        return Ok(None);
    }
    connection
        .query_row(
            "SELECT
                'run:' || run.id,
                CASE
                    WHEN run.status = 'waiting_for_approval' THEN 'waiting_for_approval'
                    WHEN run.status IN ('failed', 'cancelled') THEN 'run_failed'
                    ELSE 'important_update'
                END,
                run.automation_id, run.id,
                run.attention_required_at, run.attention_read_at, run.error_code,
                COALESCE(
                    run.error_message,
                    run.result_preview,
                    CASE WHEN run.status = 'waiting_for_approval'
                         THEN 'This scheduled task is waiting for your approval.' END
                )
             FROM automation_runs AS run
             INNER JOIN automations AS task ON task.id = run.automation_id
             WHERE run.id = ?1 AND task.deleted_at IS NULL
               AND run.attention_required_at IS NOT NULL",
            [run_id],
            row_to_attention,
        )
        .optional()
}

pub fn acknowledge_automation_attention_record(
    connection: &mut Connection,
    attention_id: &str,
    acknowledged_at: i64,
) -> rusqlite::Result<Option<AutomationAttentionRecord>> {
    let Some(existing) = get_automation_attention_by_id(connection, attention_id)? else {
        return Ok(None);
    };
    if existing
        .read_at
        .is_some_and(|read_at| read_at >= existing.required_at)
    {
        return Ok(Some(existing));
    }
    if !acknowledge_automation_attention_id(connection, attention_id, acknowledged_at)? {
        return get_automation_attention_by_id(connection, attention_id).map(|record| {
            record.filter(|record| {
                record
                    .read_at
                    .is_some_and(|read_at| read_at >= record.required_at)
            })
        });
    }
    get_automation_attention_by_id(connection, attention_id)
}

pub fn list_automation_events_after(
    connection: &Connection,
    after_sequence: i64,
    limit: usize,
) -> rusqlite::Result<Vec<AutomationEventRecord>> {
    let mut statement = connection.prepare(
        "SELECT sequence, schema_version, event_id, event_kind, automation_id,
                automation_run_id, resource_revision, payload_json, occurred_at
         FROM automation_events
         WHERE sequence > ?1
         ORDER BY sequence ASC
         LIMIT ?2",
    )?;
    let events = statement
        .query_map(
            params![after_sequence, limit.clamp(1, 1000) as i64],
            |row| {
                Ok(AutomationEventRecord {
                    sequence: row.get(0)?,
                    schema_version: row.get(1)?,
                    event_id: row.get(2)?,
                    event_kind: row.get(3)?,
                    automation_id: row.get(4)?,
                    automation_run_id: row.get(5)?,
                    resource_revision: row.get(6)?,
                    payload_json: row.get(7)?,
                    occurred_at: row.get(8)?,
                })
            },
        )?
        .collect();
    events
}

pub fn get_latest_automation_event(
    connection: &Connection,
) -> rusqlite::Result<Option<AutomationEventRecord>> {
    connection
        .query_row(
            "SELECT sequence, schema_version, event_id, event_kind, automation_id,
                    automation_run_id, resource_revision, payload_json, occurred_at
             FROM automation_events
             ORDER BY sequence DESC
             LIMIT 1",
            [],
            |row| {
                Ok(AutomationEventRecord {
                    sequence: row.get(0)?,
                    schema_version: row.get(1)?,
                    event_id: row.get(2)?,
                    event_kind: row.get(3)?,
                    automation_id: row.get(4)?,
                    automation_run_id: row.get(5)?,
                    resource_revision: row.get(6)?,
                    payload_json: row.get(7)?,
                    occurred_at: row.get(8)?,
                })
            },
        )
        .optional()
}

pub fn get_latest_automation_event_for(
    connection: &Connection,
    automation_id: &str,
    automation_run_id: Option<&str>,
    event_kind: &str,
    resource_revision: Option<i64>,
) -> rusqlite::Result<Option<AutomationEventRecord>> {
    connection
        .query_row(
            "SELECT sequence, schema_version, event_id, event_kind, automation_id,
                    automation_run_id, resource_revision, payload_json, occurred_at
             FROM automation_events
             WHERE automation_id = ?1
               AND automation_run_id IS ?2
               AND event_kind = ?3
               AND resource_revision IS ?4
             ORDER BY sequence DESC
             LIMIT 1",
            params![
                automation_id,
                automation_run_id,
                event_kind,
                resource_revision
            ],
            |row| {
                Ok(AutomationEventRecord {
                    sequence: row.get(0)?,
                    schema_version: row.get(1)?,
                    event_id: row.get(2)?,
                    event_kind: row.get(3)?,
                    automation_id: row.get(4)?,
                    automation_run_id: row.get(5)?,
                    resource_revision: row.get(6)?,
                    payload_json: row.get(7)?,
                    occurred_at: row.get(8)?,
                })
            },
        )
        .optional()
}

pub fn latest_automation_event_sequence(connection: &Connection) -> rusqlite::Result<i64> {
    last_event_sequence(connection)
}

pub fn list_nonterminal_automation_runs(
    connection: &Connection,
) -> rusqlite::Result<Vec<AutomationRunRecord>> {
    let mut statement = connection.prepare(
        "SELECT
            id, schema_version, automation_id, config_revision, config_snapshot_json,
            trigger_kind, scheduled_for, manual_request_id, status, status_revision, retry_at,
            admission_attempt, admission_token, admission_expires_at,
            cancellation_requested_at, agent_run_id, conversation_id, user_message_id,
            assistant_message_id, report_kind, result_preview, error_code, error_message,
            attention_required_at, attention_read_at, created_at, started_at,
            completed_at, updated_at
         FROM automation_runs
         WHERE status IN ('queued', 'admitting', 'running', 'waiting_for_approval')
         ORDER BY created_at ASC, id ASC",
    )?;
    let runs = statement.query_map([], row_to_run)?.collect();
    runs
}

fn effective_next_run_at(
    status: StoredAutomationStatus,
    config: &AutomationConfigRecord,
) -> Option<i64> {
    if matches!(status, StoredAutomationStatus::Active) && config.health_state == "ok" {
        config.next_run_at
    } else {
        None
    }
}

fn query_automation_by_request(
    transaction: &Transaction<'_>,
    request_id: &str,
) -> rusqlite::Result<Option<AutomationRecord>> {
    transaction
        .query_row(
            automation_select_sql(
                "WHERE create_request_id = ?1 ORDER BY deleted_at IS NULL DESC LIMIT 1",
            )
            .as_str(),
            [request_id],
            row_to_automation,
        )
        .optional()
}

fn query_automation(
    connection: &Connection,
    automation_id: &str,
    include_deleted: bool,
) -> rusqlite::Result<Option<AutomationRecord>> {
    let filter = if include_deleted {
        "WHERE id = ?1"
    } else {
        "WHERE id = ?1 AND deleted_at IS NULL"
    };
    connection
        .query_row(
            automation_select_sql(filter).as_str(),
            [automation_id],
            row_to_automation,
        )
        .optional()
}

fn automation_select_sql(filter: &str) -> String {
    format!(
        "SELECT
            id, schema_version, create_request_id, title, prompt, status,
            health_state, blocked_code, blocked_message, destination_kind,
            target_conversation_id, project_binding_kind, project_id, model_id,
            permission_mode, permission_mode_version, permissions_json, reasoning_json,
            schedule_kind, schedule_json, rrule, timezone, anchor_at, next_run_at,
            last_scheduled_at, last_run_at, notification_policy,
            target_project_snapshot, target_conversation_snapshot, target_model_snapshot,
            target_project_id_snapshot, target_conversation_id_snapshot, target_model_id_snapshot,
            attention_required_at, attention_read_at, revision, created_at, updated_at, deleted_at
         FROM automations {filter}"
    )
}

fn row_to_automation(row: &Row<'_>) -> rusqlite::Result<AutomationRecord> {
    Ok(AutomationRecord {
        id: row.get(0)?,
        schema_version: row.get(1)?,
        create_request_id: row.get(2)?,
        config: AutomationConfigRecord {
            title: row.get(3)?,
            prompt: row.get(4)?,
            health_state: row.get(6)?,
            blocked_code: row.get(7)?,
            blocked_message: row.get(8)?,
            destination_kind: row.get(9)?,
            target_conversation_id: row.get(10)?,
            project_binding_kind: row.get(11)?,
            project_id: row.get(12)?,
            model_id: row.get(13)?,
            permission_mode: row.get(14)?,
            permission_mode_version: row.get(15)?,
            permissions_json: row.get(16)?,
            reasoning_json: row.get(17)?,
            schedule_kind: row.get(18)?,
            schedule_json: row.get(19)?,
            rrule: row.get(20)?,
            timezone: row.get(21)?,
            anchor_at: row.get(22)?,
            next_run_at: row.get(23)?,
            notification_policy: row.get(26)?,
            target_project_snapshot: row.get(27)?,
            target_conversation_snapshot: row.get(28)?,
            target_model_snapshot: row.get(29)?,
            target_project_id_snapshot: row.get(30)?,
            target_conversation_id_snapshot: row.get(31)?,
            target_model_id_snapshot: row.get(32)?,
        },
        status: StoredAutomationStatus::parse(row.get::<_, String>(5)?.as_str())?,
        last_scheduled_at: row.get(24)?,
        last_run_at: row.get(25)?,
        attention_required_at: row.get(33)?,
        attention_read_at: row.get(34)?,
        revision: row.get(35)?,
        created_at: row.get(36)?,
        updated_at: row.get(37)?,
        deleted_at: row.get(38)?,
    })
}

fn query_run(
    connection: &Connection,
    run_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql("WHERE id = ?1").as_str(),
            [run_id],
            row_to_run,
        )
        .optional()
}

fn query_run_by_manual_request(
    connection: &Connection,
    request_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql("WHERE manual_request_id = ?1").as_str(),
            [request_id],
            row_to_run,
        )
        .optional()
}

fn query_scheduled_occurrence(
    connection: &Connection,
    automation_id: &str,
    scheduled_for: i64,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql(
                "WHERE automation_id = ?1 AND scheduled_for = ?2
                   AND trigger_kind IN ('scheduled', 'recovery')
                 ORDER BY created_at ASC LIMIT 1",
            )
            .as_str(),
            params![automation_id, scheduled_for],
            row_to_run,
        )
        .optional()
}

fn query_nonterminal_run(
    connection: &Connection,
    automation_id: &str,
) -> rusqlite::Result<Option<AutomationRunRecord>> {
    connection
        .query_row(
            run_select_sql(
                "WHERE automation_id = ?1
                 AND status IN ('queued', 'admitting', 'running', 'waiting_for_approval')
                 ORDER BY created_at ASC LIMIT 1",
            )
            .as_str(),
            [automation_id],
            row_to_run,
        )
        .optional()
}

fn run_select_sql(filter: &str) -> String {
    format!(
        "SELECT
            id, schema_version, automation_id, config_revision, config_snapshot_json,
            trigger_kind, scheduled_for, manual_request_id, status, status_revision, retry_at,
            admission_attempt, admission_token, admission_expires_at,
            cancellation_requested_at, agent_run_id, conversation_id, user_message_id,
            assistant_message_id, report_kind, result_preview, error_code, error_message,
            attention_required_at, attention_read_at, created_at, started_at,
            completed_at, updated_at
         FROM automation_runs {filter}"
    )
}

fn row_to_run(row: &Row<'_>) -> rusqlite::Result<AutomationRunRecord> {
    Ok(AutomationRunRecord {
        id: row.get(0)?,
        schema_version: row.get(1)?,
        automation_id: row.get(2)?,
        config_revision: row.get(3)?,
        config_snapshot_json: row.get(4)?,
        trigger_kind: row.get(5)?,
        scheduled_for: row.get(6)?,
        manual_request_id: row.get(7)?,
        status: StoredAutomationRunStatus::parse(row.get::<_, String>(8)?.as_str())?,
        status_revision: row.get(9)?,
        retry_at: row.get(10)?,
        admission_attempt: row.get(11)?,
        admission_token: row.get(12)?,
        admission_expires_at: row.get(13)?,
        cancellation_requested_at: row.get(14)?,
        agent_run_id: row.get(15)?,
        conversation_id: row.get(16)?,
        user_message_id: row.get(17)?,
        assistant_message_id: row.get(18)?,
        report_kind: row.get(19)?,
        result_preview: row.get(20)?,
        error_code: row.get(21)?,
        error_message: row.get(22)?,
        attention_required_at: row.get(23)?,
        attention_read_at: row.get(24)?,
        created_at: row.get(25)?,
        started_at: row.get(26)?,
        completed_at: row.get(27)?,
        updated_at: row.get(28)?,
    })
}

fn automation_notification_select_sql(filter: &str) -> String {
    format!(
        "SELECT
            outbox.id, outbox.automation_id, outbox.automation_run_id,
            outbox.resource_revision, outbox.notification_kind, outbox.title, outbox.body,
            outbox.status, outbox.retry_at, outbox.claim_token, outbox.claim_expires_at,
            outbox.attempt_count, outbox.last_error_code, outbox.created_at,
            outbox.delivered_at, run.conversation_id, run.user_message_id,
            run.assistant_message_id
         FROM automation_notification_outbox AS outbox
         LEFT JOIN automation_runs AS run ON run.id = outbox.automation_run_id
         {filter}"
    )
}

fn row_to_automation_notification(row: &Row<'_>) -> rusqlite::Result<AutomationNotificationRecord> {
    Ok(AutomationNotificationRecord {
        id: row.get(0)?,
        automation_id: row.get(1)?,
        automation_run_id: row.get(2)?,
        resource_revision: row.get(3)?,
        notification_kind: row.get(4)?,
        title: row.get(5)?,
        body: row.get(6)?,
        status: row.get(7)?,
        retry_at: row.get(8)?,
        claim_token: row.get(9)?,
        claim_expires_at: row.get(10)?,
        attempt_count: row.get(11)?,
        last_error_code: row.get(12)?,
        created_at: row.get(13)?,
        delivered_at: row.get(14)?,
        conversation_id: row.get(15)?,
        user_message_id: row.get(16)?,
        assistant_message_id: row.get(17)?,
    })
}

fn query_automation_notification(
    connection: &Connection,
    notification_id: &str,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    connection
        .query_row(
            &automation_notification_select_sql("WHERE outbox.id = ?1"),
            [notification_id],
            row_to_automation_notification,
        )
        .optional()
}

fn query_automation_notification_by_identity(
    connection: &Connection,
    input: &NewAutomationNotificationRecord,
) -> rusqlite::Result<Option<AutomationNotificationRecord>> {
    connection
        .query_row(
            &automation_notification_select_sql(
                "WHERE outbox.automation_id = ?1
                   AND outbox.automation_run_id IS ?2
                   AND outbox.notification_kind = ?3
                   AND outbox.resource_revision = ?4
                 ORDER BY outbox.created_at ASC LIMIT 1",
            ),
            params![
                &input.automation_id,
                input.automation_run_id.as_deref(),
                &input.notification_kind,
                input.resource_revision,
            ],
            row_to_automation_notification,
        )
        .optional()
}

fn list_automation_notifications_by_claim(
    connection: &Connection,
    claim_token: &str,
    now: i64,
) -> rusqlite::Result<Vec<AutomationNotificationRecord>> {
    let mut statement = connection.prepare(&automation_notification_select_sql(
        "WHERE outbox.status = 'pending' AND outbox.claim_token = ?1
           AND outbox.claim_expires_at > ?2
         ORDER BY outbox.created_at ASC, outbox.id ASC",
    ))?;
    let records = statement
        .query_map(params![claim_token, now], row_to_automation_notification)?
        .collect();
    records
}

pub fn list_nonterminal_automation_agent_run_ids_for_project(
    connection: &Connection,
    project_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT DISTINCT run.agent_run_id
         FROM automation_runs AS run
         INNER JOIN automations AS task ON task.id = run.automation_id
         LEFT JOIN conversations AS conversation ON conversation.id = run.conversation_id
         WHERE run.status IN ('running', 'waiting_for_approval')
           AND run.agent_run_id IS NOT NULL
           AND (
               conversation.project_id = ?1
               OR (task.project_binding_kind = 'project' AND task.project_id = ?1)
           )
         ORDER BY run.agent_run_id",
    )?;
    let run_ids = statement
        .query_map([project_id], |row| row.get::<_, String>(0))?
        .collect();
    run_ids
}

pub fn list_nonterminal_automation_agent_run_ids_for_conversation(
    connection: &Connection,
    conversation_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut statement = connection.prepare(
        "SELECT agent_run_id FROM automation_runs
         WHERE conversation_id = ?1
           AND status IN ('running', 'waiting_for_approval')
           AND agent_run_id IS NOT NULL
         ORDER BY agent_run_id",
    )?;
    let run_ids = statement
        .query_map([conversation_id], |row| row.get::<_, String>(0))?
        .collect();
    run_ids
}

pub fn list_nonterminal_automation_agent_run_ids_for_messages(
    connection: &Connection,
    conversation_id: &str,
    message_ids: &[String],
) -> rusqlite::Result<Vec<String>> {
    if message_ids.is_empty() {
        return Ok(Vec::new());
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT agent_run_id FROM automation_runs
         WHERE conversation_id = ?
           AND status IN ('running', 'waiting_for_approval')
           AND agent_run_id IS NOT NULL
           AND (user_message_id IN ({placeholders})
                OR assistant_message_id IN ({placeholders}))
         ORDER BY agent_run_id"
    );
    let mut values = Vec::with_capacity(1 + message_ids.len() * 2);
    values.push(conversation_id.to_string());
    values.extend(message_ids.iter().cloned());
    values.extend(message_ids.iter().cloned());
    let mut statement = connection.prepare(&sql)?;
    let run_ids = statement
        .query_map(rusqlite::params_from_iter(values), |row| {
            row.get::<_, String>(0)
        })?
        .collect();
    run_ids
}

fn terminal_notification_requested(
    policy: &str,
    status: StoredAutomationRunStatus,
    report_kind: &str,
) -> bool {
    match policy {
        "all_runs" => true,
        "unsuccessful_only" => matches!(
            status,
            StoredAutomationRunStatus::Failed | StoredAutomationRunStatus::Cancelled
        ),
        "important_updates" => {
            matches!(
                status,
                StoredAutomationRunStatus::Failed | StoredAutomationRunStatus::Cancelled
            ) || matches!(report_kind, "important_update" | "completed" | "unknown")
        }
        _ => false,
    }
}

/// Reads presentation and policy from the immutable run snapshot. Edits to the task row are only
/// for future runs; malformed legacy snapshots fail conservatively to a generic title/all-runs
/// policy instead of borrowing mutable task configuration.
fn run_notification_config(run: &AutomationRunRecord) -> (String, String) {
    let snapshot = serde_json::from_str::<serde_json::Value>(&run.config_snapshot_json).ok();
    let title = snapshot
        .as_ref()
        .and_then(|value| value.get("title"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= 512)
        .map(str::to_string)
        .unwrap_or_else(|| "Scheduled task".to_string());
    let policy = snapshot
        .as_ref()
        .and_then(|value| value.get("notificationPolicy"))
        .and_then(serde_json::Value::as_str)
        .filter(|value| {
            matches!(
                *value,
                "all_runs" | "unsuccessful_only" | "important_updates"
            )
        })
        .unwrap_or("all_runs")
        .to_string();
    (title, policy)
}

fn terminal_default_message(status: StoredAutomationRunStatus) -> &'static str {
    match status {
        StoredAutomationRunStatus::Completed => "The scheduled task completed.",
        StoredAutomationRunStatus::Failed => "The scheduled task failed.",
        StoredAutomationRunStatus::Cancelled => "The scheduled task was cancelled.",
        _ => "The scheduled task was updated.",
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !value.is_char_boundary(boundary) {
        boundary -= 1;
    }
    value[..boundary].to_string()
}

fn row_to_attention(row: &Row<'_>) -> rusqlite::Result<AutomationAttentionRecord> {
    Ok(AutomationAttentionRecord {
        attention_id: row.get(0)?,
        attention_kind: row.get(1)?,
        automation_id: row.get(2)?,
        automation_run_id: row.get(3)?,
        required_at: row.get(4)?,
        read_at: row.get(5)?,
        code: row.get(6)?,
        message: row.get(7)?,
    })
}

fn insert_event(
    transaction: &Transaction<'_>,
    event_kind: &str,
    automation_id: &str,
    automation_run_id: Option<&str>,
    resource_revision: Option<i64>,
    payload_json: &str,
    occurred_at: i64,
) -> rusqlite::Result<()> {
    let event_id = format!("automation-event:{}", uuid::Uuid::new_v4());
    transaction.execute(
        "INSERT INTO automation_events (
            schema_version, event_id, event_kind, automation_id, automation_run_id,
            resource_revision, payload_json, occurred_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            AUTOMATION_EVENT_SCHEMA_VERSION,
            event_id,
            event_kind,
            automation_id,
            automation_run_id,
            resource_revision,
            payload_json,
            occurred_at,
        ],
    )?;
    Ok(())
}

fn unread_attention_count(connection: &Connection) -> rusqlite::Result<u64> {
    let summary = automation_attention_summary(connection)?;
    Ok(summary.total_count)
}

fn last_event_sequence(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM automation_events",
        [],
        |row| row.get(0),
    )
}

fn nonnegative_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests;
