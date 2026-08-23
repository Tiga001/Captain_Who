//! Durable storage boundary for user-authored scheduled automations.
//!
//! The repository deliberately stores normalized destination, schedule, permission, and execution
//! snapshots as JSON documents. Application code owns their versioned domain interpretation;
//! SQLite owns idempotency, compare-and-set revisions, non-overlap, and durable event ordering.

use crate::storage::now_ms;
use rusqlite::{params, Connection, OptionalExtension, Row, Transaction};

pub const AUTOMATION_SCHEMA_VERSION: i64 = 1;
pub const AUTOMATION_RUN_SCHEMA_VERSION: i64 = 1;
pub const AUTOMATION_EVENT_SCHEMA_VERSION: i64 = 1;

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
    pub retry_at: Option<i64>,
    pub admission_attempt: i64,
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
    let updated = query_automation(&transaction, automation_id, false)?
        .expect("blocked automation must remain readable");
    transaction.commit()?;
    Ok(AutomationCompareAndSetOutcome::Updated(updated))
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
        Some(input.config_revision),
        r#"{"status":"queued"}"#,
        timestamp,
    )?;
    transaction.commit()?;
    Ok(AutomationRunEnqueueOutcome::Enqueued(run))
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
            trigger_kind, scheduled_for, manual_request_id, status, retry_at,
            admission_attempt, agent_run_id, conversation_id, user_message_id,
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
                'task' AS attention_kind,
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
                'run' AS attention_kind,
                run.automation_id,
                run.id AS automation_run_id,
                run.attention_required_at AS required_at,
                run.attention_read_at AS read_at,
                run.error_code AS code,
                COALESCE(run.error_message, run.result_preview) AS message
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
                 updated_at = MAX(updated_at, attention_required_at, ?1)
             WHERE id = ?2 AND automation_id = ?3 AND attention_required_at IS NOT NULL",
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
             WHERE id = ?2 AND deleted_at IS NULL AND attention_required_at IS NOT NULL",
            params![acknowledged_at, automation_id],
        )?
    };
    if changed > 0 {
        let revision =
            query_automation(&transaction, automation_id, false)?.map(|record| record.revision);
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
                    'task:' || id, 'task', id, NULL,
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
                'run:' || run.id, 'run', run.automation_id, run.id,
                run.attention_required_at, run.attention_read_at, run.error_code,
                COALESCE(run.error_message, run.result_preview)
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
    if !acknowledge_automation_attention_id(connection, attention_id, acknowledged_at)? {
        return Ok(None);
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
            trigger_kind, scheduled_for, manual_request_id, status, retry_at,
            admission_attempt, agent_run_id, conversation_id, user_message_id,
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
            trigger_kind, scheduled_for, manual_request_id, status, retry_at,
            admission_attempt, agent_run_id, conversation_id, user_message_id,
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
        retry_at: row.get(9)?,
        admission_attempt: row.get(10)?,
        agent_run_id: row.get(11)?,
        conversation_id: row.get(12)?,
        user_message_id: row.get(13)?,
        assistant_message_id: row.get(14)?,
        report_kind: row.get(15)?,
        result_preview: row.get(16)?,
        error_code: row.get(17)?,
        error_message: row.get(18)?,
        attention_required_at: row.get(19)?,
        attention_read_at: row.get(20)?,
        created_at: row.get(21)?,
        started_at: row.get(22)?,
        completed_at: row.get(23)?,
        updated_at: row.get(24)?,
    })
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
