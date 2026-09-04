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
        notification_repository::resolve_automation_approval_notification_in_transaction(
            &transaction,
            &run.id,
            changed_at,
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
    notification_repository::resolve_automation_approval_notification_in_transaction(
        &transaction,
        &run.id,
        settled_at,
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

pub fn enqueue_automation_notification_in_transaction(
    transaction: &Transaction<'_>,
    input: &NewAutomationNotificationRecord,
) -> rusqlite::Result<NotificationEventRecord> {
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
    let run = input
        .automation_run_id
        .as_deref()
        .map(|run_id| query_run(transaction, run_id))
        .transpose()?
        .flatten();
    if input.automation_run_id.is_some() && run.is_none() {
        return Err(rusqlite::Error::InvalidQuery);
    }
    let notification_kind = match input.notification_kind.as_str() {
        "approval_required" => "approval_required",
        "configuration_blocked" => "automation_configuration_blocked",
        "run_result" => match run.as_ref().map(|record| record.status) {
            Some(StoredAutomationRunStatus::Failed) => "automation_failed",
            Some(StoredAutomationRunStatus::Cancelled) => "automation_cancelled",
            Some(StoredAutomationRunStatus::Completed)
                if run
                    .as_ref()
                    .and_then(|record| record.report_kind.as_deref())
                    == Some("important_update") =>
            {
                "automation_important_update"
            }
            Some(StoredAutomationRunStatus::Completed) => "automation_completed",
            _ => return Err(rusqlite::Error::InvalidQuery),
        },
        _ => return Err(rusqlite::Error::InvalidQuery),
    };
    let identity = input
        .automation_run_id
        .as_deref()
        .unwrap_or(&input.automation_id);
    let dedupe_key = format!(
        "automation:{}:{}:{}",
        input.notification_kind, identity, input.resource_revision
    );
    let supersession_key = input.automation_run_id.as_deref().map_or_else(
        || format!("automation-configuration:{}", input.automation_id),
        |run_id| format!("automation-run:{run_id}"),
    );
    let event = NewNotificationEventRecord {
        notification_kind: notification_kind.to_string(),
        source_kind: "automation".to_string(),
        source_id: input.automation_id.clone(),
        run_id: input.automation_run_id.clone(),
        automation_id: Some(input.automation_id.clone()),
        conversation_id: run
            .as_ref()
            .and_then(|record| record.conversation_id.clone()),
        user_message_id: run
            .as_ref()
            .and_then(|record| record.user_message_id.clone()),
        assistant_message_id: run
            .as_ref()
            .and_then(|record| record.assistant_message_id.clone()),
        approval_action_id: None,
        subject_kind: "automation_title".to_string(),
        subject_text: input.title.clone(),
        dedupe_key: dedupe_key.clone(),
        supersession_key,
        resource_revision: Some(input.resource_revision),
        occurred_at: input.created_at,
        expires_at: input.created_at.saturating_add(
            if matches!(
                input.notification_kind.as_str(),
                "approval_required" | "configuration_blocked"
            ) {
                2_592_000_000
            } else {
                604_800_000
            },
        ),
    };
    let existed = transaction.query_row(
        "SELECT EXISTS(SELECT 1 FROM notification_events WHERE dedupe_key = ?1)",
        [&dedupe_key],
        |row| row.get::<_, bool>(0),
    )?;
    // Agent-tree deletion deliberately disables every SQLite trigger for the destructive graph
    // mutation. Notification projection is nevertheless part of this durable write: enable
    // triggers only for the notification INSERT, then restore the caller's connection setting before
    // any resource row is deleted. This keeps the generic application outbox atomic without
    // re-enabling the unrelated deletion triggers around the destructive statements.
    let triggers_were_enabled =
        transaction.db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)?;
    if !triggers_were_enabled {
        transaction.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            true,
        )?;
    }
    let insert_result =
        notification_repository::enqueue_notification_event_in_transaction(transaction, &event);
    if !triggers_were_enabled {
        transaction.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            false,
        )?;
    }
    let record = insert_result?;
    let inserted_notification_id = (!existed)
        .then(|| {
            transaction
                .query_row(
                    "SELECT id FROM notification_events WHERE dedupe_key = ?1",
                    [&dedupe_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
        })
        .transpose()?
        .flatten();
    if let Some(notification_id) = inserted_notification_id {
        let payload = serde_json::json!({
            "notificationId": notification_id,
            "notificationKind": input.notification_kind,
        });
        insert_event(
            transaction,
            "notification_requested",
            &input.automation_id,
            input.automation_run_id.as_deref(),
            Some(input.resource_revision),
            &payload.to_string(),
            input.created_at,
        )?;
    }
    Ok(record)
}

