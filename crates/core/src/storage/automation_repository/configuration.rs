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
    if existing.config.health_state == "blocked" && updated.config.health_state == "ok" {
        notification_repository::resolve_notification_events_by_supersession_key_in_transaction(
            &transaction,
            &format!("automation-configuration:{automation_id}"),
            timestamp,
        )?;
    }
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
        notification_repository::resolve_automation_approval_notification_in_transaction(
            transaction,
            &run.id,
            timestamp,
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
    crate::storage::notification_repository::invalidate_notification_events_by_automation_id_in_transaction(
        &transaction,
        automation_id,
        timestamp,
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

