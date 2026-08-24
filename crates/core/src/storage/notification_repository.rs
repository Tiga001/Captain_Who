use rusqlite::{params, Connection, OptionalExtension, Row, TransactionBehavior};

const MAX_BATCH_ITEMS: usize = 100;
const MAX_NATIVE_DELIVERY_ATTEMPTS: i64 = 5;
const MAX_NATIVE_DELIVERY_RETRY_DELAY_MS: i64 = 60_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationEventRecord {
    pub sequence: i64,
    pub id: String,
    pub notification_kind: String,
    pub source_kind: String,
    pub source_id: String,
    pub run_id: Option<String>,
    pub automation_id: Option<String>,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    pub approval_action_id: Option<String>,
    pub subject_kind: String,
    pub subject_text: String,
    pub priority: String,
    pub dedupe_key: String,
    pub supersession_key: String,
    pub resource_revision: Option<i64>,
    pub batch_id: Option<String>,
    pub seen_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub occurred_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotificationEventRecord {
    pub notification_kind: String,
    pub source_kind: String,
    pub source_id: String,
    pub run_id: Option<String>,
    pub automation_id: Option<String>,
    pub conversation_id: Option<String>,
    pub user_message_id: Option<String>,
    pub assistant_message_id: Option<String>,
    /// Renderer/API action id, not the composite storage primary key.
    pub approval_action_id: Option<String>,
    pub subject_kind: String,
    pub subject_text: String,
    pub dedupe_key: String,
    pub supersession_key: String,
    pub resource_revision: Option<i64>,
    pub occurred_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationBatchRecord {
    pub id: String,
    pub status: String,
    pub revision: i64,
    pub highest_priority: String,
    pub collect_until: i64,
    pub replace_until: i64,
    pub retry_at: i64,
    pub claim_token: Option<String>,
    pub claim_expires_at: Option<i64>,
    pub attempt_count: i64,
    pub last_error_code: Option<String>,
    pub delivered_revision: Option<i64>,
    pub delivered_priority: Option<String>,
    pub sound_level_played: String,
    pub disposition: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    pub displayed_at: Option<i64>,
    pub sealed_at: Option<i64>,
    pub suppressed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationSettingsRecord {
    pub enabled: bool,
    pub sound_enabled: bool,
    pub show_task_content: bool,
    pub human_completed_enabled: bool,
    pub human_failed_enabled: bool,
    pub human_approval_enabled: bool,
    pub human_cancelled_enabled: bool,
    pub revision: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationSettingsUpdate {
    pub enabled: bool,
    pub sound_enabled: bool,
    pub show_task_content: bool,
    pub human_completed_enabled: bool,
    pub human_failed_enabled: bool,
    pub human_approval_enabled: bool,
    pub human_cancelled_enabled: bool,
    pub expected_revision: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationSettingsUpdateOutcome {
    Updated(NotificationSettingsRecord),
    RevisionConflict(NotificationSettingsRecord),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationCounts {
    pub completed: u64,
    pub failed: u64,
    pub cancelled: u64,
    pub approval_required: u64,
    pub important_update: u64,
    pub configuration_blocked: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationSummaryRecord {
    pub unread_count: u64,
    pub unresolved_count: u64,
    pub counts: NotificationCounts,
    pub latest_occurred_at: Option<i64>,
    pub last_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationListCursor {
    pub occurred_at: i64,
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationListPage {
    pub items: Vec<NotificationEventRecord>,
    pub next_cursor: Option<NotificationListCursor>,
    pub unread_count: u64,
    pub last_sequence: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationChangeEventRecord {
    pub sequence: i64,
    pub event_id: String,
    pub event_kind: String,
    pub notification_id: Option<String>,
    pub batch_id: Option<String>,
    pub resource_revision: Option<i64>,
    pub occurred_at: i64,
}

fn priority_for_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "task_completed" | "automation_completed" => Some("completed"),
        "task_cancelled" | "automation_cancelled" => Some("cancelled"),
        "automation_important_update" => Some("important_update"),
        "task_failed" | "automation_failed" => Some("failed"),
        "automation_configuration_blocked" => Some("configuration_blocked"),
        "approval_required" => Some("approval_required"),
        _ => None,
    }
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 256
}

fn validate_new_event(input: &NewNotificationEventRecord) -> rusqlite::Result<&'static str> {
    let Some(priority) = priority_for_kind(&input.notification_kind) else {
        return Err(rusqlite::Error::InvalidQuery);
    };
    if !matches!(input.source_kind.as_str(), "human_root" | "automation")
        || !valid_id(&input.source_id)
        || !matches!(
            input.subject_kind.as_str(),
            "prompt_excerpt" | "automation_title" | "attachment_task"
        )
        || input.subject_text.is_empty()
        || input.subject_text.len() > 512
        || input.dedupe_key.is_empty()
        || input.dedupe_key.len() > 512
        || input.supersession_key.is_empty()
        || input.supersession_key.len() > 512
        || input.occurred_at < 0
        || input.expires_at < input.occurred_at
        || input.resource_revision.is_some_and(|value| value <= 0)
        || input
            .run_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || input
            .automation_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || input
            .conversation_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || input
            .user_message_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || input
            .assistant_message_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || input
            .approval_action_id
            .as_deref()
            .is_some_and(|value| !valid_id(value))
        || (input.source_kind == "automation" && input.automation_id.is_none())
        || (input.source_kind == "human_root" && input.automation_id.is_some())
        || (input.notification_kind == "approval_required" && input.approval_action_id.is_none())
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    Ok(priority)
}

pub fn enqueue_notification_event(
    connection: &mut Connection,
    input: &NewNotificationEventRecord,
) -> rusqlite::Result<NotificationEventRecord> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let record = enqueue_notification_event_in_transaction(&transaction, input)?;
    transaction.commit()?;
    Ok(record)
}

/// Idempotently inserts a notification fact inside an already-owned transaction. This is the
/// primitive HumanRoot terminal and pending-action paths use to make state + notification atomic.
pub fn enqueue_notification_event_in_transaction(
    connection: &Connection,
    input: &NewNotificationEventRecord,
) -> rusqlite::Result<NotificationEventRecord> {
    let priority = validate_new_event(input)?;
    if let Some(existing) = query_notification_event_by_dedupe_key(connection, &input.dedupe_key)? {
        return Ok(existing);
    }
    if let Some(current) =
        query_live_notification_event_by_supersession_key(connection, &input.supersession_key)?
    {
        let current_order = (
            current.resource_revision.unwrap_or(0),
            current.occurred_at,
            i64::from(current.notification_kind != "approval_required"),
        );
        let input_order = (
            input.resource_revision.unwrap_or(0),
            input.occurred_at,
            i64::from(input.notification_kind != "approval_required"),
        );
        if input_order <= current_order {
            return Ok(current);
        }
    }
    let id = format!("notification-event:{}", uuid::Uuid::new_v4());
    connection.execute(
        "INSERT OR IGNORE INTO notification_events (
            id, schema_version, notification_kind, source_kind, source_id, run_id,
            automation_id, conversation_id, user_message_id, assistant_message_id,
            approval_action_id, subject_kind, subject_text, priority, dedupe_key,
            supersession_key, resource_revision, seen_at, resolved_at, occurred_at, expires_at
         ) VALUES (
            ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14,
            ?15, ?16, NULL,
            CASE WHEN ?2 IN (
                'task_completed', 'task_cancelled', 'automation_completed',
                'automation_cancelled', 'automation_important_update'
            ) THEN ?17 ELSE NULL END,
            ?17, ?18
         )",
        params![
            id,
            &input.notification_kind,
            &input.source_kind,
            &input.source_id,
            input.run_id.as_deref(),
            input.automation_id.as_deref(),
            input.conversation_id.as_deref(),
            input.user_message_id.as_deref(),
            input.assistant_message_id.as_deref(),
            input.approval_action_id.as_deref(),
            &input.subject_kind,
            &input.subject_text,
            priority,
            &input.dedupe_key,
            &input.supersession_key,
            input.resource_revision,
            input.occurred_at,
            input.expires_at,
        ],
    )?;
    query_notification_event_by_dedupe_key(connection, &input.dedupe_key)?
        .ok_or(rusqlite::Error::InvalidQuery)
}

pub fn resolve_notification_events_by_supersession_key(
    connection: &mut Connection,
    supersession_key: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = resolve_notification_events_by_supersession_key_in_transaction(
        &transaction,
        supersession_key,
        resolved_at,
    )?;
    transaction.commit()?;
    Ok(changed)
}

pub fn resolve_notification_events_by_supersession_key_in_transaction(
    connection: &Connection,
    supersession_key: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    if supersession_key.is_empty() || supersession_key.len() > 512 || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    resolve_where(
        connection,
        "supersession_key = ?1",
        params![supersession_key],
        resolved_at,
    )
}

pub fn resolve_notification_events_by_approval_action_id(
    connection: &mut Connection,
    approval_action_id: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = resolve_notification_events_by_approval_action_id_in_transaction(
        &transaction,
        approval_action_id,
        resolved_at,
    )?;
    transaction.commit()?;
    Ok(changed)
}

pub fn resolve_notification_events_by_approval_action_id_in_transaction(
    connection: &Connection,
    approval_action_id: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    if !valid_id(approval_action_id) || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    resolve_where(
        connection,
        "approval_action_id = ?1",
        params![approval_action_id],
        resolved_at,
    )
}

pub fn resolve_notification_events_by_run_and_approval_action_id_in_transaction(
    connection: &Connection,
    run_id: &str,
    approval_action_id: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    if !valid_id(run_id) || !valid_id(approval_action_id) || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    resolve_where(
        connection,
        "run_id = ?1 AND approval_action_id = ?2",
        params![run_id, approval_action_id],
        resolved_at,
    )
}

pub fn resolve_notification_events_by_conversation_id_in_transaction(
    connection: &Connection,
    conversation_id: &str,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    if !valid_id(conversation_id) || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    invalidate_where(
        connection,
        "conversation_id = ?1",
        params![conversation_id],
        resolved_at,
    )
}

pub fn resolve_notification_events_by_message_ids_in_transaction(
    connection: &Connection,
    message_ids: &[String],
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    if message_ids.is_empty() {
        return Ok(0);
    }
    if message_ids.len() > 1_000 || message_ids.iter().any(|id| !valid_id(id)) || resolved_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let placeholders = std::iter::repeat_n("?", message_ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT id FROM notification_events
         WHERE seen_at IS NULL AND superseded_at IS NULL
           AND (user_message_id IN ({placeholders}) OR assistant_message_id IN ({placeholders}))"
    );
    let mut values = Vec::<rusqlite::types::Value>::with_capacity(message_ids.len() * 2);
    values.extend(
        message_ids
            .iter()
            .cloned()
            .map(rusqlite::types::Value::Text),
    );
    values.extend(
        message_ids
            .iter()
            .cloned()
            .map(rusqlite::types::Value::Text),
    );
    let ids = connection
        .prepare(&sql)?
        .query_map(rusqlite::params_from_iter(values.iter()), |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    invalidate_ids(connection, &ids, resolved_at)
}

/// Removes all not-yet-seen native delivery facts owned by a deleted automation while retaining
/// their immutable rows for durable diagnostics and audit history.
pub fn invalidate_notification_events_by_automation_id_in_transaction(
    connection: &Connection,
    automation_id: &str,
    invalidated_at: i64,
) -> rusqlite::Result<usize> {
    if !valid_id(automation_id) || invalidated_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    invalidate_where(
        connection,
        "automation_id = ?1",
        params![automation_id],
        invalidated_at,
    )
}

fn resolve_where<P: rusqlite::Params>(
    connection: &Connection,
    predicate: &str,
    params: P,
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    let sql =
        format!("SELECT id FROM notification_events WHERE resolved_at IS NULL AND {predicate}");
    let ids = connection
        .prepare(&sql)?
        .query_map(params, |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    resolve_ids(connection, &ids, resolved_at)
}

fn invalidate_where<P: rusqlite::Params>(
    connection: &Connection,
    predicate: &str,
    params: P,
    invalidated_at: i64,
) -> rusqlite::Result<usize> {
    let sql = format!(
        "SELECT id FROM notification_events
         WHERE seen_at IS NULL AND superseded_at IS NULL AND {predicate}"
    );
    let ids = connection
        .prepare(&sql)?
        .query_map(params, |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    invalidate_ids(connection, &ids, invalidated_at)
}

fn invalidate_ids(
    connection: &Connection,
    ids: &[String],
    invalidated_at: i64,
) -> rusqlite::Result<usize> {
    let mut changed = 0;
    for id in ids {
        let row_changed = connection.execute(
            "UPDATE notification_events
             SET superseded_at = MAX(occurred_at, ?2),
                 resolved_at = COALESCE(resolved_at, MAX(occurred_at, ?2))
             WHERE id = ?1 AND seen_at IS NULL AND superseded_at IS NULL",
            params![id, invalidated_at],
        )?;
        changed += row_changed;
        if row_changed == 1 {
            let batch_id: Option<String> = connection
                .query_row(
                    "SELECT batch_id FROM notification_batch_items
                     WHERE notification_event_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            connection.execute(
                "INSERT INTO notification_change_events (
                    schema_version, event_id, event_kind, notification_id, batch_id,
                    resource_revision, occurred_at
                 ) VALUES (1, ?1, 'resolved', ?2, ?3, NULL, ?4)",
                params![
                    format!("notification-change:{}", uuid::Uuid::new_v4()),
                    id,
                    batch_id,
                    invalidated_at,
                ],
            )?;
        }
    }
    suppress_empty_batches(connection, invalidated_at, "suppressed_deleted")?;
    Ok(changed)
}

fn resolve_ids(
    connection: &Connection,
    ids: &[String],
    resolved_at: i64,
) -> rusqlite::Result<usize> {
    let mut changed = 0;
    for id in ids {
        let row_changed = connection.execute(
            "UPDATE notification_events SET resolved_at = MAX(occurred_at, ?2)
             WHERE id = ?1 AND resolved_at IS NULL",
            params![id, resolved_at],
        )?;
        changed += row_changed;
        if row_changed == 1 {
            let batch_id: Option<String> = connection
                .query_row(
                    "SELECT batch_id FROM notification_batch_items
                     WHERE notification_event_id = ?1",
                    [id],
                    |row| row.get(0),
                )
                .optional()?;
            connection.execute(
                "INSERT INTO notification_change_events (
                    schema_version, event_id, event_kind, notification_id, batch_id,
                    resource_revision, occurred_at
                 ) VALUES (1, ?1, 'resolved', ?2, ?3, NULL, ?4)",
                params![
                    format!("notification-change:{}", uuid::Uuid::new_v4()),
                    id,
                    batch_id,
                    resolved_at,
                ],
            )?;
        }
    }
    suppress_empty_batches(connection, resolved_at, "suppressed_resolved")?;
    Ok(changed)
}

pub fn claim_pending_notification_batches(
    connection: &mut Connection,
    claim_token: &str,
    now: i64,
    lease_duration_ms: i64,
    limit: usize,
) -> rusqlite::Result<Vec<NotificationBatchRecord>> {
    if !valid_id(claim_token) || now < 0 || !(5_000..=300_000).contains(&lease_duration_ms) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if !load_notification_settings(&transaction)?.enabled {
        transaction.execute(
            "UPDATE notification_batches
             SET status = 'suppressed', disposition = 'suppressed_disabled',
                 suppressed_at = MAX(created_at, ?1), claim_token = NULL, claim_expires_at = NULL,
                 updated_at = MAX(updated_at, ?1)
             WHERE status IN ('collecting', 'pending', 'claimed')",
            [now],
        )?;
        transaction.commit()?;
        return Ok(Vec::new());
    }
    transaction.execute(
        "UPDATE notification_batches
         SET status = 'pending', claim_token = NULL, claim_expires_at = NULL
         WHERE status = 'claimed' AND claim_expires_at <= ?1",
        [now],
    )?;
    transaction.execute(
        "UPDATE notification_batches SET status = 'pending'
         WHERE status = 'collecting' AND collect_until <= ?1",
        [now],
    )?;
    transaction.execute(
        "UPDATE notification_batches
         SET status = 'sealed', sealed_at = MAX(created_at, ?1), updated_at = MAX(updated_at, ?1)
         WHERE status = 'displayed' AND replace_until < ?1",
        [now],
    )?;
    suppress_empty_batches(&transaction, now, "suppressed_stale")?;
    let existing = list_notification_batches_by_claim(&transaction, claim_token, now)?;
    if !existing.is_empty() {
        transaction.rollback()?;
        return Ok(existing);
    }
    let lease_expires_at = now.saturating_add(lease_duration_ms);
    let ids = transaction
        .prepare(
            "SELECT batch.id FROM notification_batches AS batch
             WHERE batch.status IN ('collecting', 'pending') AND batch.retry_at <= ?1
               AND EXISTS (
                   SELECT 1 FROM notification_batch_items AS item
                   INNER JOIN notification_events AS event
                       ON event.id = item.notification_event_id
                   WHERE item.batch_id = batch.id AND event.superseded_at IS NULL
                     AND event.seen_at IS NULL
                     AND event.expires_at > ?1
                     AND (
                       event.notification_kind NOT IN ('approval_required', 'automation_configuration_blocked')
                       OR event.resolved_at IS NULL
                     )
                     AND (
                       event.source_kind = 'automation'
                       OR (event.notification_kind = 'task_completed' AND
                           (SELECT human_completed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                       OR (event.notification_kind = 'task_failed' AND
                           (SELECT human_failed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                       OR (event.notification_kind = 'task_cancelled' AND
                           (SELECT human_cancelled_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                       OR (event.notification_kind = 'approval_required' AND
                           (SELECT human_approval_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                     )
               )
             ORDER BY batch.created_at ASC, batch.id ASC LIMIT ?2",
        )?
        .query_map(params![now, limit.clamp(1, 10) as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in ids {
        transaction.execute(
            "UPDATE notification_batches
             SET status = 'claimed', claim_token = ?1, claim_expires_at = ?2,
                 attempt_count = attempt_count + 1, last_error_code = NULL,
                 updated_at = MAX(updated_at, ?3)
             WHERE id = ?4 AND status IN ('collecting', 'pending')",
            params![claim_token, lease_expires_at, now, id],
        )?;
    }
    let records = list_notification_batches_by_claim(&transaction, claim_token, now)?;
    transaction.commit()?;
    Ok(records)
}

pub fn validate_claimed_notification_batch(
    connection: &mut Connection,
    batch_id: &str,
    claim_token: &str,
    now: i64,
) -> rusqlite::Result<Option<NotificationBatchRecord>> {
    if !valid_id(batch_id) || !valid_id(claim_token) || now < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(record) = query_notification_batch(&transaction, batch_id)? else {
        transaction.rollback()?;
        return Ok(None);
    };
    if record.status != "claimed"
        || record.claim_token.as_deref() != Some(claim_token)
        || record.claim_expires_at.is_none_or(|expires| expires <= now)
    {
        transaction.rollback()?;
        return Ok(None);
    }
    let live_count: i64 = transaction.query_row(
        "SELECT COUNT(*) FROM notification_batch_items AS item
         INNER JOIN notification_events AS event ON event.id = item.notification_event_id
         WHERE item.batch_id = ?1 AND event.superseded_at IS NULL AND event.seen_at IS NULL
           AND event.expires_at > ?2
           AND (
             event.notification_kind NOT IN ('approval_required', 'automation_configuration_blocked')
             OR event.resolved_at IS NULL
           )
           AND (
             event.source_kind = 'automation'
             OR (event.notification_kind = 'task_completed' AND
                 (SELECT human_completed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'task_failed' AND
                 (SELECT human_failed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'task_cancelled' AND
                 (SELECT human_cancelled_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'approval_required' AND
                 (SELECT human_approval_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
           )",
        params![batch_id, now],
        |row| row.get(0),
    )?;
    if live_count == 0 || !load_notification_settings(&transaction)?.enabled {
        transaction.execute(
            "UPDATE notification_batches
             SET status = 'suppressed', disposition = ?3, suppressed_at = MAX(created_at, ?1),
                 claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
             WHERE id = ?2 AND status = 'claimed' AND claim_token = ?4",
            params![
                now,
                batch_id,
                if live_count == 0 {
                    "suppressed_stale"
                } else {
                    "suppressed_disabled"
                },
                claim_token,
            ],
        )?;
        transaction.commit()?;
        return Ok(None);
    }
    transaction.rollback()?;
    Ok(Some(record))
}

#[allow(clippy::too_many_arguments)]
pub fn acknowledge_notification_batch(
    connection: &mut Connection,
    batch_id: &str,
    claim_token: &str,
    disposition: &str,
    native_priority: &str,
    sound_level_played: &str,
    native_revision: i64,
    acknowledged_at: i64,
) -> rusqlite::Result<Option<NotificationBatchRecord>> {
    if !valid_id(batch_id)
        || !valid_id(claim_token)
        || !matches!(
            disposition,
            "delivered"
                | "suppressed_foreground"
                | "suppressed_stale"
                | "suppressed_deleted"
                | "suppressed_resolved"
                | "suppressed_disabled"
        )
        || !matches!(sound_level_played, "none" | "initial" | "upgrade")
        || !matches!(
            native_priority,
            "completed"
                | "cancelled"
                | "important_update"
                | "failed"
                | "configuration_blocked"
                | "approval_required"
        )
        || native_revision <= 0
        || acknowledged_at < 0
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = if disposition == "delivered" {
        transaction.execute(
            "UPDATE notification_batches
             SET status = CASE
                     WHEN ?2 < revision THEN 'pending'
                     WHEN replace_until >= ?1 THEN 'displayed'
                     ELSE 'sealed'
                 END,
                 delivered_revision = MIN(revision, ?2), delivered_priority = ?3,
                 sound_level_played = ?4,
                 disposition = 'delivered', displayed_at = COALESCE(displayed_at, MAX(created_at, ?1)),
                 sealed_at = CASE
                     WHEN ?2 >= revision AND replace_until < ?1 THEN MAX(created_at, ?1)
                     ELSE NULL
                 END,
                 claim_token = NULL, claim_expires_at = NULL, last_error_code = NULL,
                 updated_at = MAX(updated_at, ?1)
             WHERE id = ?5 AND status = 'claimed' AND claim_token = ?6",
            params![
                acknowledged_at,
                native_revision,
                native_priority,
                sound_level_played,
                batch_id,
                claim_token
            ],
        )?
    } else {
        transaction.execute(
            "UPDATE notification_batches
             SET status = 'suppressed', disposition = ?1,
                 suppressed_at = MAX(created_at, ?2), claim_token = NULL,
                 claim_expires_at = NULL, last_error_code = NULL,
                 updated_at = MAX(updated_at, ?2)
             WHERE id = ?3 AND status = 'claimed' AND claim_token = ?4",
            params![disposition, acknowledged_at, batch_id, claim_token],
        )?
    };
    let record = query_notification_batch(&transaction, batch_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn release_notification_batch(
    connection: &mut Connection,
    batch_id: &str,
    claim_token: &str,
    retry_at: i64,
    error_code: &str,
) -> rusqlite::Result<Option<NotificationBatchRecord>> {
    if !valid_id(batch_id)
        || !valid_id(claim_token)
        || retry_at < 0
        || error_code.is_empty()
        || error_code.len() > 128
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let Some(current) = query_notification_batch(&transaction, batch_id)? else {
        transaction.rollback()?;
        return Ok(None);
    };
    if current.status != "claimed" || current.claim_token.as_deref() != Some(claim_token) {
        transaction.rollback()?;
        return Ok(None);
    }
    let requested_delay = retry_at.saturating_sub(current.updated_at).max(1_000);
    let retry_multiplier = 1_i64
        .checked_shl(current.attempt_count.saturating_sub(1).min(6) as u32)
        .unwrap_or(64);
    let bounded_delay = requested_delay
        .saturating_mul(retry_multiplier)
        .min(MAX_NATIVE_DELIVERY_RETRY_DELAY_MS);
    let bounded_retry_at = current.updated_at.saturating_add(bounded_delay);
    let exhausted = current.attempt_count >= MAX_NATIVE_DELIVERY_ATTEMPTS;
    let changed = transaction.execute(
        "UPDATE notification_batches
         SET status = CASE WHEN ?3 THEN 'suppressed' ELSE 'pending' END,
             disposition = CASE WHEN ?3 THEN 'suppressed_disabled' ELSE disposition END,
             suppressed_at = CASE WHEN ?3 THEN MAX(created_at, ?1) ELSE NULL END,
             retry_at = MAX(retry_at, ?1), last_error_code = ?2,
             claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
         WHERE id = ?4 AND status = 'claimed' AND claim_token = ?5",
        params![
            bounded_retry_at,
            error_code,
            exhausted,
            batch_id,
            claim_token
        ],
    )?;
    let record = query_notification_batch(&transaction, batch_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn suppress_notification_batch(
    connection: &mut Connection,
    batch_id: &str,
    reason: &str,
    suppressed_at: i64,
) -> rusqlite::Result<Option<NotificationBatchRecord>> {
    if !valid_id(batch_id)
        || !matches!(
            reason,
            "suppressed_foreground"
                | "suppressed_stale"
                | "suppressed_deleted"
                | "suppressed_resolved"
                | "suppressed_disabled"
        )
        || suppressed_at < 0
    {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE notification_batches
         SET status = 'suppressed', disposition = ?1, suppressed_at = MAX(created_at, ?2),
             claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?2)
         WHERE id = ?3 AND status IN ('collecting', 'pending', 'claimed', 'displayed')",
        params![reason, suppressed_at, batch_id],
    )?;
    let record = query_notification_batch(&transaction, batch_id)?;
    transaction.commit()?;
    Ok((changed == 1).then_some(record).flatten())
}

pub fn list_notification_batch_items(
    connection: &Connection,
    batch_id: &str,
    now: i64,
) -> rusqlite::Result<Vec<NotificationEventRecord>> {
    let mut statement = connection.prepare(&notification_event_select_sql(
        "INNER JOIN notification_batch_items AS item ON item.notification_event_id = event.id
         WHERE item.batch_id = ?1 AND event.superseded_at IS NULL AND event.seen_at IS NULL
           AND event.expires_at > ?2
           AND (
             event.notification_kind NOT IN ('approval_required', 'automation_configuration_blocked')
             OR event.resolved_at IS NULL
           )
           AND (
             event.source_kind = 'automation'
             OR (event.notification_kind = 'task_completed' AND
                 (SELECT human_completed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'task_failed' AND
                 (SELECT human_failed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'task_cancelled' AND
                 (SELECT human_cancelled_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
             OR (event.notification_kind = 'approval_required' AND
                 (SELECT human_approval_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
           )
         ORDER BY event.occurred_at ASC, event.id ASC LIMIT ?3",
    ))?;
    let records = statement
        .query_map(
            params![batch_id, now, MAX_BATCH_ITEMS as i64],
            row_to_notification_event,
        )?
        .collect();
    records
}

pub fn list_notifications(
    connection: &Connection,
    cursor: Option<&NotificationListCursor>,
    limit: usize,
    unread_only: bool,
    batch_id: Option<&str>,
) -> rusqlite::Result<NotificationListPage> {
    let mut statement = connection.prepare(&notification_event_select_sql(
        "WHERE event.superseded_at IS NULL
           AND (?1 = 0 OR event.seen_at IS NULL)
           AND (?2 IS NULL OR EXISTS (
               SELECT 1 FROM notification_batch_items AS filter_item
               WHERE filter_item.notification_event_id = event.id
                 AND filter_item.batch_id = ?2
           ))
           AND (
               ?3 IS NULL OR event.occurred_at < ?3
               OR (event.occurred_at = ?3 AND event.id < ?4)
           )
         ORDER BY event.occurred_at DESC, event.id DESC LIMIT ?5",
    ))?;
    let requested = limit.clamp(1, 100);
    let mut items = statement
        .query_map(
            params![
                unread_only,
                batch_id,
                cursor.map(|value| value.occurred_at),
                cursor.map(|value| value.id.as_str()),
                (requested + 1) as i64,
            ],
            row_to_notification_event,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = items.len() > requested;
    items.truncate(requested);
    let next_cursor = has_more
        .then(|| items.last())
        .flatten()
        .map(|item| NotificationListCursor {
            occurred_at: item.occurred_at,
            id: item.id.clone(),
        });
    let summary = notification_summary(connection)?;
    Ok(NotificationListPage {
        items,
        next_cursor,
        unread_count: summary.unread_count,
        last_sequence: summary.last_sequence,
    })
}

pub fn notification_summary(
    connection: &Connection,
) -> rusqlite::Result<NotificationSummaryRecord> {
    connection.query_row(
        "SELECT
            COUNT(*) FILTER (WHERE seen_at IS NULL AND superseded_at IS NULL),
            COUNT(*) FILTER (
                WHERE resolved_at IS NULL AND superseded_at IS NULL
                  AND notification_kind IN (
                    'task_failed', 'automation_failed', 'approval_required',
                    'automation_configuration_blocked'
                )
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind IN ('task_completed', 'automation_completed')
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind IN ('task_failed', 'automation_failed')
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind IN ('task_cancelled', 'automation_cancelled')
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind = 'approval_required'
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind = 'automation_important_update'
            ),
            COUNT(*) FILTER (
                WHERE seen_at IS NULL AND superseded_at IS NULL
                  AND notification_kind = 'automation_configuration_blocked'
            ),
            MAX(occurred_at) FILTER (WHERE superseded_at IS NULL),
            COALESCE((SELECT MAX(sequence) FROM notification_change_events), 0)
         FROM notification_events",
        [],
        |row| {
            Ok(NotificationSummaryRecord {
                unread_count: row.get(0)?,
                unresolved_count: row.get(1)?,
                counts: NotificationCounts {
                    completed: row.get(2)?,
                    failed: row.get(3)?,
                    cancelled: row.get(4)?,
                    approval_required: row.get(5)?,
                    important_update: row.get(6)?,
                    configuration_blocked: row.get(7)?,
                },
                latest_occurred_at: row.get(8)?,
                last_sequence: row.get(9)?,
            })
        },
    )
}

pub fn mark_notification_events_seen(
    connection: &mut Connection,
    event_ids: Option<&[String]>,
    batch_id: Option<&str>,
    all: bool,
    seen_at: i64,
) -> rusqlite::Result<usize> {
    if seen_at < 0 || (!all && event_ids.is_none() && batch_id.is_none()) {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let ids = if all {
        transaction
            .prepare("SELECT id FROM notification_events WHERE seen_at IS NULL")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else if let Some(batch_id) = batch_id {
        transaction
            .prepare(
                "SELECT event.id FROM notification_events AS event
                 INNER JOIN notification_batch_items AS item ON item.notification_event_id = event.id
                 WHERE item.batch_id = ?1 AND event.seen_at IS NULL",
            )?
            .query_map([batch_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        event_ids.unwrap_or_default().to_vec()
    };
    let mut changed = 0;
    for id in &ids {
        let row_changed = transaction.execute(
            "UPDATE notification_events
             SET seen_at = MAX(occurred_at, ?2),
                 resolved_at = CASE
                     WHEN notification_kind IN ('task_failed', 'automation_failed')
                         THEN COALESCE(resolved_at, MAX(occurred_at, ?2))
                     ELSE resolved_at
                 END
             WHERE id = ?1 AND seen_at IS NULL",
            params![id, seen_at],
        )?;
        changed += row_changed;
        if row_changed == 1 {
            transaction.execute(
                "INSERT INTO notification_change_events (
                    schema_version, event_id, event_kind, notification_id, batch_id,
                    resource_revision, occurred_at
                 ) VALUES (1, ?1, 'seen', ?2,
                    (SELECT batch_id FROM notification_batch_items WHERE notification_event_id = ?2),
                    NULL, ?3)",
                params![
                    format!("notification-change:{}", uuid::Uuid::new_v4()),
                    id,
                    seen_at,
                ],
            )?;
        }
    }
    if all {
        transaction.execute(
            "UPDATE notification_batches AS batch
             SET status = 'sealed', sealed_at = MAX(created_at, ?1),
                 claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
             WHERE status IN ('collecting', 'pending', 'claimed', 'displayed')
               AND EXISTS (
                   SELECT 1 FROM notification_batch_items AS item
                   INNER JOIN notification_events AS event
                       ON event.id = item.notification_event_id
                   WHERE item.batch_id = batch.id AND event.seen_at IS NOT NULL
               )",
            [seen_at],
        )?;
    } else if let Some(batch_id) = batch_id {
        transaction.execute(
            "UPDATE notification_batches
             SET status = 'sealed', sealed_at = MAX(created_at, ?1),
                 claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
             WHERE id = ?2 AND status IN ('collecting', 'pending', 'claimed', 'displayed')",
            params![seen_at, batch_id],
        )?;
    } else if !ids.is_empty() {
        let placeholders = std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE notification_batches AS batch
             SET status = 'sealed', sealed_at = MAX(created_at, ?1),
                 claim_token = NULL, claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
             WHERE status IN ('collecting', 'pending', 'claimed', 'displayed')
               AND EXISTS (
                   SELECT 1 FROM notification_batch_items AS item
                   WHERE item.batch_id = batch.id
                     AND item.notification_event_id IN ({placeholders})
               )
               AND NOT EXISTS (
                   SELECT 1 FROM notification_batch_items AS unseen_item
                   INNER JOIN notification_events AS unseen
                       ON unseen.id = unseen_item.notification_event_id
                   WHERE unseen_item.batch_id = batch.id
                     AND unseen.seen_at IS NULL
                     AND unseen.superseded_at IS NULL
                     AND unseen.expires_at > ?1
                     AND (
                         unseen.notification_kind NOT IN (
                             'approval_required', 'automation_configuration_blocked'
                         )
                         OR unseen.resolved_at IS NULL
                     )
               )"
        );
        let mut values = Vec::<rusqlite::types::Value>::with_capacity(ids.len() + 1);
        values.push(rusqlite::types::Value::Integer(seen_at));
        values.extend(ids.iter().cloned().map(rusqlite::types::Value::Text));
        transaction.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
    }
    transaction.commit()?;
    Ok(changed)
}

pub fn load_notification_settings(
    connection: &Connection,
) -> rusqlite::Result<NotificationSettingsRecord> {
    connection.query_row(
        "SELECT enabled, sound_enabled, show_task_content, human_completed_enabled,
                human_failed_enabled, human_approval_enabled, human_cancelled_enabled,
                revision, updated_at
         FROM notification_settings WHERE singleton_id = 1",
        [],
        |row| {
            Ok(NotificationSettingsRecord {
                enabled: row.get(0)?,
                sound_enabled: row.get(1)?,
                show_task_content: row.get(2)?,
                human_completed_enabled: row.get(3)?,
                human_failed_enabled: row.get(4)?,
                human_approval_enabled: row.get(5)?,
                human_cancelled_enabled: row.get(6)?,
                revision: row.get(7)?,
                updated_at: row.get(8)?,
            })
        },
    )
}

pub fn update_notification_settings(
    connection: &mut Connection,
    input: &NotificationSettingsUpdate,
) -> rusqlite::Result<NotificationSettingsUpdateOutcome> {
    if input.expected_revision <= 0 || input.updated_at < 0 {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let changed = transaction.execute(
        "UPDATE notification_settings
         SET enabled = ?1, sound_enabled = ?2, show_task_content = ?3,
             human_completed_enabled = ?4, human_failed_enabled = ?5,
             human_approval_enabled = ?6, human_cancelled_enabled = ?7,
             revision = revision + 1, updated_at = MAX(updated_at + 1, ?8)
         WHERE singleton_id = 1 AND revision = ?9",
        params![
            input.enabled,
            input.sound_enabled,
            input.show_task_content,
            input.human_completed_enabled,
            input.human_failed_enabled,
            input.human_approval_enabled,
            input.human_cancelled_enabled,
            input.updated_at,
            input.expected_revision,
        ],
    )?;
    let settings = load_notification_settings(&transaction)?;
    if changed == 1 {
        if !settings.enabled {
            transaction.execute(
                "UPDATE notification_batches
                 SET status = 'suppressed', disposition = 'suppressed_disabled',
                     suppressed_at = MAX(created_at, ?1), claim_token = NULL,
                     claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
                 WHERE status IN ('collecting', 'pending', 'claimed')",
                [settings.updated_at],
            )?;
        }
        suppress_empty_batches(&transaction, settings.updated_at, "suppressed_disabled")?;
        transaction.execute(
            "INSERT INTO notification_change_events (
                schema_version, event_id, event_kind, notification_id, batch_id,
                resource_revision, occurred_at
             ) VALUES (1, ?1, 'settings_updated', NULL, NULL, ?2, ?3)",
            params![
                format!("notification-change:{}", uuid::Uuid::new_v4()),
                settings.revision,
                settings.updated_at,
            ],
        )?;
        transaction.commit()?;
        Ok(NotificationSettingsUpdateOutcome::Updated(settings))
    } else {
        transaction.rollback()?;
        Ok(NotificationSettingsUpdateOutcome::RevisionConflict(
            settings,
        ))
    }
}

pub fn list_notification_change_events_after(
    connection: &Connection,
    after_sequence: i64,
    limit: usize,
) -> rusqlite::Result<Vec<NotificationChangeEventRecord>> {
    let mut statement = connection.prepare(
        "SELECT sequence, event_id, event_kind, notification_id, batch_id,
                resource_revision, occurred_at
         FROM notification_change_events WHERE sequence > ?1
         ORDER BY sequence ASC LIMIT ?2",
    )?;
    let events = statement
        .query_map(
            params![after_sequence, limit.clamp(1, 1_000) as i64],
            |row| {
                Ok(NotificationChangeEventRecord {
                    sequence: row.get(0)?,
                    event_id: row.get(1)?,
                    event_kind: row.get(2)?,
                    notification_id: row.get(3)?,
                    batch_id: row.get(4)?,
                    resource_revision: row.get(5)?,
                    occurred_at: row.get(6)?,
                })
            },
        )?
        .collect();
    events
}

pub fn latest_notification_change_sequence(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT COALESCE(MAX(sequence), 0) FROM notification_change_events",
        [],
        |row| row.get(0),
    )
}

fn suppress_empty_batches(
    connection: &Connection,
    now: i64,
    reason: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "UPDATE notification_batches AS batch
         SET status = 'suppressed', disposition = ?2,
             suppressed_at = MAX(created_at, ?1), claim_token = NULL,
             claim_expires_at = NULL, updated_at = MAX(updated_at, ?1)
         WHERE status IN ('collecting', 'pending', 'claimed', 'displayed')
           AND NOT EXISTS (
               SELECT 1 FROM notification_batch_items AS item
               INNER JOIN notification_events AS event ON event.id = item.notification_event_id
               WHERE item.batch_id = batch.id AND event.superseded_at IS NULL
                 AND event.seen_at IS NULL
                 AND event.expires_at > ?1
                 AND (
                   event.notification_kind NOT IN ('approval_required', 'automation_configuration_blocked')
                   OR event.resolved_at IS NULL
                 )
                 AND (
                   event.source_kind = 'automation'
                   OR (event.notification_kind = 'task_completed' AND
                       (SELECT human_completed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                   OR (event.notification_kind = 'task_failed' AND
                       (SELECT human_failed_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                   OR (event.notification_kind = 'task_cancelled' AND
                       (SELECT human_cancelled_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                   OR (event.notification_kind = 'approval_required' AND
                       (SELECT human_approval_enabled FROM notification_settings WHERE singleton_id = 1) = 1)
                 )
           )",
        params![now, reason],
    )
}

fn notification_event_select_sql(filter: &str) -> String {
    format!(
        "SELECT event.sequence, event.id, event.notification_kind, event.source_kind,
                event.source_id, event.run_id, event.automation_id, event.conversation_id,
                event.user_message_id, event.assistant_message_id, event.approval_action_id,
                event.subject_kind, event.subject_text, event.priority, event.dedupe_key,
                event.supersession_key, event.resource_revision,
                (SELECT batch_id FROM notification_batch_items WHERE notification_event_id = event.id),
                event.seen_at, event.resolved_at, event.occurred_at, event.expires_at
         FROM notification_events AS event {filter}"
    )
}

fn row_to_notification_event(row: &Row<'_>) -> rusqlite::Result<NotificationEventRecord> {
    Ok(NotificationEventRecord {
        sequence: row.get(0)?,
        id: row.get(1)?,
        notification_kind: row.get(2)?,
        source_kind: row.get(3)?,
        source_id: row.get(4)?,
        run_id: row.get(5)?,
        automation_id: row.get(6)?,
        conversation_id: row.get(7)?,
        user_message_id: row.get(8)?,
        assistant_message_id: row.get(9)?,
        approval_action_id: row.get(10)?,
        subject_kind: row.get(11)?,
        subject_text: row.get(12)?,
        priority: row.get(13)?,
        dedupe_key: row.get(14)?,
        supersession_key: row.get(15)?,
        resource_revision: row.get(16)?,
        batch_id: row.get(17)?,
        seen_at: row.get(18)?,
        resolved_at: row.get(19)?,
        occurred_at: row.get(20)?,
        expires_at: row.get(21)?,
    })
}

fn query_notification_event_by_dedupe_key(
    connection: &Connection,
    dedupe_key: &str,
) -> rusqlite::Result<Option<NotificationEventRecord>> {
    connection
        .query_row(
            &notification_event_select_sql("WHERE event.dedupe_key = ?1"),
            [dedupe_key],
            row_to_notification_event,
        )
        .optional()
}

fn query_live_notification_event_by_supersession_key(
    connection: &Connection,
    supersession_key: &str,
) -> rusqlite::Result<Option<NotificationEventRecord>> {
    connection
        .query_row(
            &notification_event_select_sql(
                "WHERE event.supersession_key = ?1 AND event.superseded_at IS NULL
                 ORDER BY COALESCE(event.resource_revision, 0) DESC,
                    event.occurred_at DESC,
                    (event.notification_kind != 'approval_required') DESC,
                    event.id DESC
                 LIMIT 1",
            ),
            [supersession_key],
            row_to_notification_event,
        )
        .optional()
}

fn notification_batch_select_sql(filter: &str) -> String {
    format!(
        "SELECT id, status, revision, highest_priority, collect_until, replace_until,
                retry_at, claim_token, claim_expires_at, attempt_count, last_error_code,
                delivered_revision, delivered_priority, sound_level_played, disposition,
                created_at, updated_at,
                displayed_at, sealed_at, suppressed_at
         FROM notification_batches {filter}"
    )
}

fn row_to_notification_batch(row: &Row<'_>) -> rusqlite::Result<NotificationBatchRecord> {
    Ok(NotificationBatchRecord {
        id: row.get(0)?,
        status: row.get(1)?,
        revision: row.get(2)?,
        highest_priority: row.get(3)?,
        collect_until: row.get(4)?,
        replace_until: row.get(5)?,
        retry_at: row.get(6)?,
        claim_token: row.get(7)?,
        claim_expires_at: row.get(8)?,
        attempt_count: row.get(9)?,
        last_error_code: row.get(10)?,
        delivered_revision: row.get(11)?,
        delivered_priority: row.get(12)?,
        sound_level_played: row.get(13)?,
        disposition: row.get(14)?,
        created_at: row.get(15)?,
        updated_at: row.get(16)?,
        displayed_at: row.get(17)?,
        sealed_at: row.get(18)?,
        suppressed_at: row.get(19)?,
    })
}

fn query_notification_batch(
    connection: &Connection,
    batch_id: &str,
) -> rusqlite::Result<Option<NotificationBatchRecord>> {
    connection
        .query_row(
            &notification_batch_select_sql("WHERE id = ?1"),
            [batch_id],
            row_to_notification_batch,
        )
        .optional()
}

fn list_notification_batches_by_claim(
    connection: &Connection,
    claim_token: &str,
    now: i64,
) -> rusqlite::Result<Vec<NotificationBatchRecord>> {
    let mut statement = connection.prepare(&notification_batch_select_sql(
        "WHERE status = 'claimed' AND claim_token = ?1 AND claim_expires_at > ?2
         ORDER BY created_at ASC, id ASC",
    ))?;
    let batches = statement
        .query_map(params![claim_token, now], row_to_notification_batch)?
        .collect();
    batches
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        migrations::run_migrations(&connection).unwrap();
        connection
    }

    fn event(kind: &str, dedupe: &str, supersession: &str, at: i64) -> NewNotificationEventRecord {
        NewNotificationEventRecord {
            notification_kind: kind.to_string(),
            source_kind: "human_root".to_string(),
            source_id: "conversation-1".to_string(),
            run_id: Some("run-1".to_string()),
            automation_id: None,
            conversation_id: Some("conversation-1".to_string()),
            user_message_id: Some("user-1".to_string()),
            assistant_message_id: Some("assistant-1".to_string()),
            approval_action_id: (kind == "approval_required").then(|| "action-1".to_string()),
            subject_kind: "prompt_excerpt".to_string(),
            subject_text: "Write tests".to_string(),
            dedupe_key: dedupe.to_string(),
            supersession_key: supersession.to_string(),
            resource_revision: Some(1),
            occurred_at: at,
            expires_at: at + 60_000,
        }
    }

    #[test]
    fn events_are_idempotent_and_microbatched() {
        let mut connection = database();
        let first = enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe-1", "run:1", 1_000),
        )
        .unwrap();
        let replay = enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe-1", "run:1", 1_000),
        )
        .unwrap();
        enqueue_notification_event(
            &mut connection,
            &event("task_failed", "dedupe-2", "run:2", 1_100),
        )
        .unwrap();
        assert_eq!(first.id, replay.id);
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM notification_events", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM notification_batches", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT highest_priority FROM notification_batches",
                    [],
                    |row| row.get::<_, String>(0)
                )
                .unwrap(),
            "failed"
        );
    }

    #[test]
    fn supersession_keeps_only_the_latest_fact_live() {
        let mut connection = database();
        enqueue_notification_event(
            &mut connection,
            &event("approval_required", "approval", "run:1", 1_000),
        )
        .unwrap();
        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "complete", "run:1", 2_000),
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM notification_events WHERE superseded_at IS NULL",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT highest_priority FROM notification_batches",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "completed",
            "an approval superseded by the terminal fact must not leave batch priority elevated",
        );

        let mut failed_connection = database();
        enqueue_notification_event(
            &mut failed_connection,
            &event("task_failed", "failure", "run:1", 1_000),
        )
        .unwrap();
        enqueue_notification_event(
            &mut failed_connection,
            &event("task_completed", "recovered", "run:1", 2_000),
        )
        .unwrap();
        assert_eq!(
            failed_connection
                .query_row(
                    "SELECT highest_priority FROM notification_batches",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "completed",
            "a failed fact superseded by completion must not trigger an upgrade sound",
        );

        let replayed_old = enqueue_notification_event(
            &mut connection,
            &event("approval_required", "approval", "run:1", 1_000),
        )
        .unwrap();
        assert_eq!(replayed_old.dedupe_key, "approval");
        assert_eq!(
            connection
                .query_row(
                    "SELECT dedupe_key FROM notification_events
                     WHERE supersession_key = 'run:1' AND superseded_at IS NULL",
                    [],
                    |row| row.get::<_, String>(0),
                )
                .unwrap(),
            "complete",
            "replaying an older dedupe must not supersede the current fact",
        );

        let mut revision_connection = database();
        let mut revision_two = event("task_completed", "revision-2", "run:revision", 2_000);
        revision_two.resource_revision = Some(2);
        enqueue_notification_event(&mut revision_connection, &revision_two).unwrap();
        let mut late_revision_one = event("task_failed", "revision-1", "run:revision", 3_000);
        late_revision_one.resource_revision = Some(1);
        let current =
            enqueue_notification_event(&mut revision_connection, &late_revision_one).unwrap();
        assert_eq!(current.dedupe_key, "revision-2");
        assert_eq!(
            revision_connection
                .query_row("SELECT COUNT(*) FROM notification_events", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1,
            "an out-of-order lower revision is not a new notification fact",
        );
    }

    #[test]
    fn batch_capacity_splits_high_volume_without_losing_late_priority() {
        let mut connection = database();
        for index in 0..101 {
            enqueue_notification_event(
                &mut connection,
                &event(
                    "task_completed",
                    &format!("completed-{index}"),
                    &format!("run:{index}"),
                    1_000 + index,
                ),
            )
            .unwrap();
        }
        let approval = enqueue_notification_event(
            &mut connection,
            &event("approval_required", "approval-102", "run:102", 1_101),
        )
        .unwrap();

        let claimed =
            claim_pending_notification_batches(&mut connection, "volume-claim", 1_102, 5_000, 10)
                .unwrap();
        assert_eq!(claimed.len(), 2);
        let mut delivered_ids = Vec::new();
        for batch in &claimed {
            let items = list_notification_batch_items(&connection, &batch.id, 1_102).unwrap();
            assert!(items.len() <= MAX_BATCH_ITEMS);
            assert_eq!(batch.revision as usize, items.len());
            delivered_ids.extend(items.into_iter().map(|item| item.id));
        }
        assert_eq!(delivered_ids.len(), 102);
        assert!(delivered_ids.contains(&approval.id));
        assert!(claimed
            .iter()
            .any(|batch| batch.highest_priority == "approval_required"));
    }

    #[test]
    fn batch_claim_ack_and_restart_lease_are_durable() {
        let mut connection = database();
        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe", "run:1", 1_000),
        )
        .unwrap();
        let claimed =
            claim_pending_notification_batches(&mut connection, "claim-a", 3_001, 5_000, 10)
                .unwrap();
        assert_eq!(claimed.len(), 1);
        assert!(
            claim_pending_notification_batches(&mut connection, "claim-b", 4_000, 5_000, 10)
                .unwrap()
                .is_empty()
        );
        let recovered =
            claim_pending_notification_batches(&mut connection, "claim-b", 8_002, 5_000, 10)
                .unwrap();
        assert_eq!(recovered.len(), 1);
        let batch = acknowledge_notification_batch(
            &mut connection,
            &recovered[0].id,
            "claim-b",
            "delivered",
            "completed",
            "initial",
            recovered[0].revision,
            8_003,
        )
        .unwrap()
        .unwrap();
        assert_eq!(batch.status, "displayed");
        assert_eq!(batch.sound_level_played, "initial");
        assert_eq!(batch.delivered_priority.as_deref(), Some("completed"));
    }

    #[test]
    fn collecting_batch_can_be_claimed_immediately_for_low_latency_wake() {
        let mut connection = database();
        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe", "run:1", 1_000),
        )
        .unwrap();
        let claimed =
            claim_pending_notification_batches(&mut connection, "claim", 1_001, 5_000, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, "claimed");
        assert!(claimed[0].collect_until > 1_001);
    }

    #[test]
    fn settings_use_cas_and_disable_pending_delivery() {
        let mut connection = database();
        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe", "run:1", 1_000),
        )
        .unwrap();
        let outcome = update_notification_settings(
            &mut connection,
            &NotificationSettingsUpdate {
                enabled: false,
                sound_enabled: true,
                show_task_content: true,
                human_completed_enabled: true,
                human_failed_enabled: true,
                human_approval_enabled: true,
                human_cancelled_enabled: true,
                expected_revision: 1,
                updated_at: 2_000,
            },
        )
        .unwrap();
        assert!(matches!(
            outcome,
            NotificationSettingsUpdateOutcome::Updated(_)
        ));
        assert!(
            claim_pending_notification_batches(&mut connection, "claim", 3_001, 5_000, 10)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn event_arriving_during_claim_updates_the_same_batch() {
        let mut connection = database();
        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "dedupe-1", "run:1", 1_000),
        )
        .unwrap();
        let claimed =
            claim_pending_notification_batches(&mut connection, "claim", 3_001, 15_000, 10)
                .unwrap();
        assert_eq!(claimed.len(), 1);
        enqueue_notification_event(
            &mut connection,
            &event("task_failed", "dedupe-2", "run:2", 3_100),
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM notification_batches", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
        let current =
            validate_claimed_notification_batch(&mut connection, &claimed[0].id, "claim", 3_101)
                .unwrap()
                .unwrap();
        assert_eq!(current.revision, 2);
        let after_old_native_revision = acknowledge_notification_batch(
            &mut connection,
            &current.id,
            "claim",
            "delivered",
            "completed",
            "initial",
            1,
            3_102,
        )
        .unwrap()
        .unwrap();
        assert_eq!(after_old_native_revision.status, "pending");
        assert_eq!(after_old_native_revision.delivered_revision, Some(1));
    }

    #[test]
    fn seen_and_resolved_are_separate_for_actionable_events() {
        let mut connection = database();
        let failure = enqueue_notification_event(
            &mut connection,
            &event("task_failed", "failure", "run:1", 1_000),
        )
        .unwrap();
        let approval = enqueue_notification_event(
            &mut connection,
            &event("approval_required", "approval", "run:2", 1_100),
        )
        .unwrap();
        mark_notification_events_seen(
            &mut connection,
            Some(&[failure.id.clone(), approval.id.clone()]),
            None,
            false,
            2_000,
        )
        .unwrap();
        let (failure_seen, failure_resolved): (Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT seen_at, resolved_at FROM notification_events WHERE id = ?1",
                [&failure.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let (approval_seen, approval_resolved): (Option<i64>, Option<i64>) = connection
            .query_row(
                "SELECT seen_at, resolved_at FROM notification_events WHERE id = ?1",
                [&approval.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(failure_seen, Some(2_000));
        assert_eq!(failure_resolved, Some(2_000));
        assert_eq!(approval_seen, Some(2_000));
        assert_eq!(approval_resolved, None);
        assert!(claim_pending_notification_batches(
            &mut connection,
            "seen-items-must-not-redeliver",
            2_001,
            5_000,
            10,
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM notification_batches WHERE status = 'sealed'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
        );

        enqueue_notification_event(
            &mut connection,
            &event("task_completed", "later", "run:3", 2_100),
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM notification_batches", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2,
            "interaction seals the old replacement window so later work gets a new batch",
        );
    }

    #[test]
    fn marking_presented_events_seen_preserves_a_concurrently_added_unseen_item() {
        let mut connection = database();
        let presented = enqueue_notification_event(
            &mut connection,
            &event("task_completed", "presented", "run:1", 1_000),
        )
        .unwrap();
        let concurrent = enqueue_notification_event(
            &mut connection,
            &event("task_failed", "concurrent", "run:2", 1_100),
        )
        .unwrap();

        mark_notification_events_seen(&mut connection, Some(&[presented.id]), None, false, 1_200)
            .unwrap();

        let claimed =
            claim_pending_notification_batches(&mut connection, "claim", 1_201, 5_000, 10).unwrap();
        assert_eq!(claimed.len(), 1);
        let live_items = list_notification_batch_items(&connection, &claimed[0].id, 1_201).unwrap();
        assert_eq!(live_items.len(), 1);
        assert_eq!(live_items[0].id, concurrent.id);
        assert_eq!(claimed[0].status, "claimed");
    }

    #[test]
    fn deleting_notification_owners_invalidates_native_delivery_but_keeps_audit_history() {
        let mut conversation_connection = database();
        let conversation_event = enqueue_notification_event(
            &mut conversation_connection,
            &event("task_completed", "conversation-delete", "run:1", 1_000),
        )
        .unwrap();
        let claimed = claim_pending_notification_batches(
            &mut conversation_connection,
            "conversation-claim",
            1_001,
            5_000,
            10,
        )
        .unwrap();
        assert_eq!(claimed.len(), 1);
        resolve_notification_events_by_conversation_id_in_transaction(
            &conversation_connection,
            "conversation-1",
            1_100,
        )
        .unwrap();
        assert!(validate_claimed_notification_batch(
            &mut conversation_connection,
            &claimed[0].id,
            "conversation-claim",
            1_101,
        )
        .unwrap()
        .is_none());
        let history = list_notifications(&conversation_connection, None, 10, false, None).unwrap();
        assert!(history.items.is_empty());
        assert_eq!(history.unread_count, 0);
        assert_eq!(
            conversation_connection
                .query_row(
                    "SELECT COUNT(*) FROM notification_events WHERE id = ?1",
                    [&conversation_event.id],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1,
            "the immutable fact remains stored even though the center omits its invalid target",
        );

        let mut message_connection = database();
        enqueue_notification_event(
            &mut message_connection,
            &event("task_completed", "message-delete", "run:1", 1_000),
        )
        .unwrap();
        resolve_notification_events_by_message_ids_in_transaction(
            &message_connection,
            &["assistant-1".to_string()],
            1_100,
        )
        .unwrap();
        assert!(claim_pending_notification_batches(
            &mut message_connection,
            "message-claim",
            1_101,
            5_000,
            10,
        )
        .unwrap()
        .is_empty());

        let mut automation_connection = database();
        let mut automation_event = event(
            "automation_completed",
            "automation-delete",
            "automation-run:1",
            1_000,
        );
        automation_event.source_kind = "automation".to_string();
        automation_event.source_id = "automation-1".to_string();
        automation_event.automation_id = Some("automation-1".to_string());
        enqueue_notification_event(&mut automation_connection, &automation_event).unwrap();
        invalidate_notification_events_by_automation_id_in_transaction(
            &automation_connection,
            "automation-1",
            1_100,
        )
        .unwrap();
        assert!(claim_pending_notification_batches(
            &mut automation_connection,
            "automation-claim",
            1_101,
            5_000,
            10,
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn repeated_native_failures_use_bounded_backoff_and_eventually_stop_delivery() {
        let mut connection = database();
        let mut durable = event("task_failed", "delivery-failure", "run:1", 1_000);
        durable.expires_at = 1_000_000;
        enqueue_notification_event(&mut connection, &durable).unwrap();

        let mut now = 1_001;
        for attempt in 1..=MAX_NATIVE_DELIVERY_ATTEMPTS {
            let claim_token = format!("claim-{attempt}");
            let claimed =
                claim_pending_notification_batches(&mut connection, &claim_token, now, 5_000, 1)
                    .unwrap();
            assert_eq!(claimed.len(), 1);
            let requested_retry_at = now + 2_500;
            let released = release_notification_batch(
                &mut connection,
                &claimed[0].id,
                &claim_token,
                requested_retry_at,
                "native_notification_failed",
            )
            .unwrap()
            .unwrap();
            assert!(released.retry_at >= requested_retry_at);
            assert!(released.retry_at - released.updated_at <= MAX_NATIVE_DELIVERY_RETRY_DELAY_MS);
            if attempt < MAX_NATIVE_DELIVERY_ATTEMPTS {
                assert_eq!(released.status, "pending");
                now = released.retry_at;
            } else {
                assert_eq!(released.status, "suppressed");
                assert_eq!(released.disposition.as_deref(), Some("suppressed_disabled"));
            }
        }
        assert!(claim_pending_notification_batches(
            &mut connection,
            "claim-after-exhaustion",
            now + MAX_NATIVE_DELIVERY_RETRY_DELAY_MS,
            5_000,
            1,
        )
        .unwrap()
        .is_empty());
        assert_eq!(
            list_notifications(&connection, None, 10, false, None)
                .unwrap()
                .items
                .len(),
            1
        );
    }
}
