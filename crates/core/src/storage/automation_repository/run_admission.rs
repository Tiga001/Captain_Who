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

