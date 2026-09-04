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

