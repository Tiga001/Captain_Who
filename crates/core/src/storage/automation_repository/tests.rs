use super::*;
use crate::storage::migrations;
use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use crate::storage::notification_repository::{
    claim_pending_notification_batches, list_notification_batch_items, list_notifications,
    validate_claimed_notification_batch, NotificationBatchRecord, NotificationEventRecord,
};
use crate::{ProviderProfileConfig, ProviderProtocolDialect};

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
        .execute_batch(
            "INSERT INTO models (
                id, provider_model_id, display_name, normalized_display_name,
                api_url_override, api_token_override_ref,
                supports_image, context_window_tokens, provider_profile_config_json,
                provider_connection_revision, provider_protocol_revision,
                input_price, cached_input_price, output_price,
                enabled, position, created_at, updated_at
             ) VALUES (
                'model-a', 'model-a', 'Model A', 'model a', NULL, NULL,
                0, 4096, '{}',
                'provider-connection-v1:test', 'provider-protocol-v1:test',
                '0', '', '0', 1, 0, 1, 1
             );",
        )
        .unwrap();
    connection
}

fn config() -> AutomationConfigRecord {
    AutomationConfigRecord {
        title: "Daily brief".to_string(),
        prompt: "Summarize the project state.".to_string(),
        health_state: "ok".to_string(),
        blocked_code: None,
        blocked_message: None,
        destination_kind: "new_chat".to_string(),
        target_conversation_id: None,
        project_binding_kind: "none".to_string(),
        project_id: None,
        model_id: Some("model-a".to_string()),
        permission_mode: "default".to_string(),
        permission_mode_version: 2,
        permissions_json: r#"{"read":"workspace_only","write":"workspace_only","command":"require_approval","commandSafety":"guarded","patch":"require_approval"}"#.to_string(),
        reasoning_json: Some(r#"{"source":"model_config","effort":"high"}"#.to_string()),
        schedule_kind: "daily".to_string(),
        schedule_json: r#"{"kind":"daily","timeMinutes":540,"timezone":"Asia/Shanghai"}"#
            .to_string(),
        rrule: "FREQ=DAILY;INTERVAL=1".to_string(),
        timezone: "Asia/Shanghai".to_string(),
        anchor_at: 1_700_000_000_000,
        next_run_at: Some(1_700_000_100_000),
        notification_policy: "all_runs".to_string(),
        target_project_snapshot: None,
        target_conversation_snapshot: None,
        target_model_snapshot: Some("Model A".to_string()),
        target_project_id_snapshot: None,
        target_conversation_id_snapshot: None,
        target_model_id_snapshot: None,
    }
}

fn create(connection: &mut Connection, id: &str, request_id: &str) -> AutomationRecord {
    match create_automation(
        connection,
        &NewAutomationRecord {
            id: id.to_string(),
            create_request_id: request_id.to_string(),
            status: StoredAutomationStatus::Active,
            config: config(),
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        AutomationCreateOutcome::Existing(_) => panic!("fixture must create"),
    }
}

fn application_notifications(connection: &Connection) -> Vec<NotificationEventRecord> {
    list_notifications(connection, None, 100, false, None)
        .unwrap()
        .items
}

fn claim_application_notifications(
    connection: &mut Connection,
    claim_token: &str,
    now: i64,
) -> Vec<(NotificationBatchRecord, Vec<NotificationEventRecord>)> {
    claim_pending_notification_batches(connection, claim_token, now, 60_000, 10)
        .unwrap()
        .into_iter()
        .map(|batch| {
            let items = list_notification_batch_items(connection, &batch.id, now).unwrap();
            (batch, items)
        })
        .collect()
}

#[test]
fn create_is_idempotent_and_list_returns_counts_and_events() {
    let mut connection = connection();
    let created = create(&mut connection, "automation-a", "request-a");

    let replay = create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "different-resource-id".to_string(),
            create_request_id: "request-a".to_string(),
            status: StoredAutomationStatus::Active,
            config: config(),
        },
    )
    .unwrap();
    assert_eq!(replay, AutomationCreateOutcome::Existing(created.clone()));

    let page = list_automations(
        &connection,
        &AutomationListInput {
            status: None,
            query: Some("brief".to_string()),
            cursor: None,
            limit: 20,
        },
    )
    .unwrap();
    assert_eq!(page.items, vec![created]);
    assert_eq!(page.total_count, 1);
    assert_eq!(page.active_count, 1);
    assert_eq!(page.paused_count, 0);
    assert_eq!(page.attention_count, 0);
    assert_eq!(page.last_event_sequence, 1);
    assert_eq!(
        list_automation_events_after(&connection, 0, 10)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn list_filters_searches_prompt_and_paginates_without_duplicates() {
    let mut connection = connection();
    let first = create(&mut connection, "automation-a", "request-a");
    let mut second_config = config();
    second_config.title = "Weekly review".to_string();
    second_config.prompt = "Find the special needle in recent work.".to_string();
    let second = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-b".to_string(),
            create_request_id: "request-b".to_string(),
            status: StoredAutomationStatus::Paused,
            config: second_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };

    let searched = list_automations(
        &connection,
        &AutomationListInput {
            status: Some(StoredAutomationStatus::Paused),
            query: Some("NEEDLE".to_string()),
            cursor: None,
            limit: 20,
        },
    )
    .unwrap();
    assert_eq!(searched.items, vec![second.clone()]);
    assert_eq!(searched.total_count, 2);
    assert_eq!(searched.active_count, 1);
    assert_eq!(searched.paused_count, 1);

    let first_page = list_automations(
        &connection,
        &AutomationListInput {
            status: None,
            query: None,
            cursor: None,
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(first_page.items.len(), 1);
    let cursor = first_page.next_cursor.expect("one more page");
    let second_page = list_automations(
        &connection,
        &AutomationListInput {
            status: None,
            query: None,
            cursor: Some(cursor),
            limit: 1,
        },
    )
    .unwrap();
    assert_eq!(second_page.items.len(), 1);
    assert_ne!(first_page.items[0].id, second_page.items[0].id);
    assert!(second_page.items == vec![first] || second_page.items == vec![second]);
}

#[test]
fn config_and_status_updates_use_revision_cas_and_pause_clears_next_run() {
    let mut connection = connection();
    let created = create(&mut connection, "automation-a", "request-a");
    let stale = set_automation_status(
        &mut connection,
        &created.id,
        created.revision + 1,
        StoredAutomationStatus::Paused,
        None,
    )
    .unwrap();
    assert_eq!(
        stale,
        AutomationCompareAndSetOutcome::RevisionConflict(created.clone())
    );

    let paused = match set_automation_status(
        &mut connection,
        &created.id,
        created.revision,
        StoredAutomationStatus::Paused,
        None,
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };
    assert_eq!(paused.status, StoredAutomationStatus::Paused);
    assert_eq!(paused.config.next_run_at, None);

    let mut edited_config = paused.config.clone();
    edited_config.title = "Edited brief".to_string();
    edited_config.next_run_at = Some(1_800_000_000_000);
    let edited = match replace_automation_config(
        &mut connection,
        &paused.id,
        paused.revision,
        &edited_config,
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };
    assert_eq!(edited.config.title, "Edited brief");
    assert_eq!(
        edited.config.next_run_at, None,
        "paused edits cannot arm a task"
    );

    let resumed = match set_automation_status(
        &mut connection,
        &edited.id,
        edited.revision,
        StoredAutomationStatus::Active,
        Some(1_900_000_000_000),
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };
    assert_eq!(resumed.status, StoredAutomationStatus::Active);
    assert_eq!(resumed.config.next_run_at, Some(1_900_000_000_000));
}

#[test]
fn manual_run_enqueue_is_idempotent_and_enforces_one_nonterminal_run() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let input = NewManualAutomationRunRecord {
        id: "automation-run-a".to_string(),
        automation_id: task.id.clone(),
        manual_request_id: "manual-request-a".to_string(),
        scheduled_for: 1_700_000_000_000,
        config_revision: task.revision,
        config_snapshot_json: r#"{"schemaVersion":1,"title":"Daily brief"}"#.to_string(),
        expected_revision: task.revision,
    };
    let stale = enqueue_manual_automation_run(
        &mut connection,
        &NewManualAutomationRunRecord {
            id: "stale-run".to_string(),
            manual_request_id: "stale-request".to_string(),
            expected_revision: task.revision + 1,
            config_revision: task.revision + 1,
            ..input.clone()
        },
    )
    .unwrap();
    assert_eq!(
        stale,
        AutomationRunEnqueueOutcome::RevisionConflict(Box::new(task.clone()))
    );
    let run = match enqueue_manual_automation_run(&mut connection, &input).unwrap() {
        AutomationRunEnqueueOutcome::Enqueued(run) => run,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };
    assert_eq!(run.status, StoredAutomationRunStatus::Queued);

    assert_eq!(
        enqueue_manual_automation_run(&mut connection, &input).unwrap(),
        AutomationRunEnqueueOutcome::Existing(run.clone())
    );
    let conflict = enqueue_manual_automation_run(
        &mut connection,
        &NewManualAutomationRunRecord {
            id: "automation-run-b".to_string(),
            manual_request_id: "manual-request-b".to_string(),
            ..input
        },
    )
    .unwrap();
    assert_eq!(
        conflict,
        AutomationRunEnqueueOutcome::ActiveConflict(run.clone())
    );

    let history = list_automation_runs(&connection, &task.id, None, 10).unwrap();
    assert_eq!(history.items, vec![run.clone()]);
    assert_eq!(
        list_nonterminal_automation_runs(&connection).unwrap(),
        vec![run]
    );
}

#[test]
fn resource_deletion_blocks_tasks_without_deleting_tasks_or_chat_history() {
    let mut connection = connection();
    connection
        .execute_batch(
            "INSERT INTO projects (id, name, created_at, pinned_at, updated_at)
             VALUES ('project-a', 'Project A', 1, NULL, 1);
             INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES ('conversation-a', 'project-a', NULL, 'Target chat', 1, 1, NULL, NULL, NULL);
             INSERT INTO messages (
                id, conversation_id, role, content, status, created_at, position
             ) VALUES ('message-a', 'conversation-a', 'user', 'keep me', 'sent', 1, 0);",
        )
        .unwrap();
    let mut existing_chat_config = config();
    existing_chat_config.destination_kind = "existing_chat".to_string();
    existing_chat_config.target_conversation_id = Some("conversation-a".to_string());
    existing_chat_config.project_binding_kind = "inherit".to_string();
    existing_chat_config.model_id = None;
    existing_chat_config.reasoning_json = None;
    existing_chat_config.target_model_snapshot = None;
    existing_chat_config.target_model_id_snapshot = None;
    existing_chat_config.target_conversation_snapshot = Some("Target chat".to_string());
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-a".to_string(),
            create_request_id: "request-a".to_string(),
            status: StoredAutomationStatus::Active,
            config: existing_chat_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };

    connection
        .execute("DELETE FROM conversations WHERE id = 'conversation-a'", [])
        .unwrap();
    let blocked = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(blocked.status, StoredAutomationStatus::Active);
    assert_eq!(blocked.config.health_state, "blocked");
    assert_eq!(
        blocked.config.blocked_code.as_deref(),
        Some("target_missing")
    );
    assert_eq!(blocked.config.target_conversation_id, None);
    assert_eq!(
        blocked.config.target_conversation_id_snapshot.as_deref(),
        Some("conversation-a")
    );
    assert_eq!(blocked.config.next_run_at, None);
    assert!(blocked.attention_required_at.is_some());
    let events = list_automation_events_after(&connection, 0, 10).unwrap();
    assert!(events.iter().any(|event| {
        event.event_kind == "attention_changed"
            && event.automation_id == "automation-a"
            && event.resource_revision == Some(blocked.revision)
    }));
    let attention_page = list_automation_attentions(&connection, None, 10).unwrap();
    assert_eq!(attention_page.items.len(), 1);
    assert_eq!(attention_page.items[0].attention_id, "task:automation-a");
    let acknowledged = acknowledge_automation_attention_record(
        &mut connection,
        "task:automation-a",
        blocked.attention_required_at.unwrap(),
    )
    .unwrap()
    .unwrap();
    assert!(acknowledged.read_at.unwrap() >= acknowledged.required_at);
    assert_eq!(
        automation_attention_summary(&connection)
            .unwrap()
            .total_count,
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM automations WHERE id = 'automation-a'",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE id = 'message-a'",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        0,
        "the pre-existing chat deletion owns its normal cascading message behavior"
    );
}

#[test]
fn trigger_disabled_resource_invalidation_keeps_important_updates_notifications() {
    let mut connection = connection();
    connection
        .execute_batch(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (
                'conversation-trigger-disabled', NULL, 'model-a', 'Target chat',
                1, 1, NULL, NULL, NULL
             );",
        )
        .unwrap();
    let mut task_config = config();
    task_config.destination_kind = "existing_chat".to_string();
    task_config.target_conversation_id = Some("conversation-trigger-disabled".to_string());
    task_config.project_binding_kind = "inherit".to_string();
    task_config.model_id = None;
    task_config.reasoning_json = None;
    task_config.target_model_snapshot = None;
    task_config.target_model_id_snapshot = None;
    task_config.target_conversation_snapshot = Some("Target chat".to_string());
    task_config.notification_policy = "important_updates".to_string();
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-trigger-disabled".to_string(),
            create_request_id: "request-trigger-disabled".to_string(),
            status: StoredAutomationStatus::Active,
            config: task_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected create outcome: {outcome:?}"),
    };
    let timestamp = now_ms() + 1_000;
    let triggers_were_enabled = connection
        .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap();
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            false,
        )
        .unwrap();
    {
        let transaction = connection.transaction().unwrap();
        invalidate_automations_before_trigger_disabled_conversation_delete(
            &transaction,
            &["conversation-trigger-disabled".to_string()],
            timestamp,
        )
        .unwrap();
        transaction.commit().unwrap();
    }
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER,
            triggers_were_enabled,
        )
        .unwrap();

    let blocked = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(blocked.config.health_state, "blocked");
    let notifications = application_notifications(&connection);
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].notification_kind,
        "automation_configuration_blocked"
    );
    assert_eq!(notifications[0].resource_revision, Some(blocked.revision));
}

#[test]
fn tombstone_hides_task_without_deleting_an_existing_target_chat() {
    let mut connection = connection();
    connection
        .execute_batch(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES ('conversation-a', NULL, NULL, 'Target chat', 1, 1, NULL, NULL, NULL);",
        )
        .unwrap();
    let mut existing_chat_config = config();
    existing_chat_config.destination_kind = "existing_chat".to_string();
    existing_chat_config.target_conversation_id = Some("conversation-a".to_string());
    existing_chat_config.project_binding_kind = "inherit".to_string();
    existing_chat_config.model_id = None;
    existing_chat_config.reasoning_json = None;
    existing_chat_config.target_model_snapshot = None;
    existing_chat_config.target_model_id_snapshot = None;
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-a".to_string(),
            create_request_id: "request-a".to_string(),
            status: StoredAutomationStatus::Active,
            config: existing_chat_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };
    let deleted = tombstone_automation(&mut connection, &task.id, task.revision).unwrap();
    assert!(matches!(
        deleted,
        AutomationCompareAndSetOutcome::Updated(_)
    ));
    assert!(get_automation(&connection, &task.id).unwrap().is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE id = 'conversation-a'",
                [],
                |row| { row.get::<_, i64>(0) }
            )
            .unwrap(),
        1
    );
}

#[test]
fn configuration_block_notifications_follow_all_user_visible_policies() {
    for policy in ["all_runs", "unsuccessful_only", "important_updates"] {
        let mut connection = connection();
        let mut task_config = config();
        task_config.notification_policy = policy.to_string();
        let task = match create_automation(
            &mut connection,
            &NewAutomationRecord {
                id: format!("automation-{policy}"),
                create_request_id: format!("request-{policy}"),
                status: StoredAutomationStatus::Active,
                config: task_config,
            },
        )
        .unwrap()
        {
            AutomationCreateOutcome::Created(record) => record,
            outcome => panic!("unexpected create outcome: {outcome:?}"),
        };

        let blocked = match block_automation(
            &mut connection,
            &task.id,
            task.revision,
            "permission_disabled",
            "The selected permission mode is disabled.",
        )
        .unwrap()
        {
            AutomationCompareAndSetOutcome::Updated(record) => record,
            outcome => panic!("unexpected block outcome: {outcome:?}"),
        };
        let notifications = application_notifications(&connection);

        assert_eq!(notifications.len(), 1, "policy {policy}");
        assert_eq!(
            notifications[0].notification_kind, "automation_configuration_blocked",
            "policy {policy}"
        );
        assert_eq!(
            notifications[0].automation_id.as_deref(),
            Some(task.id.as_str()),
            "policy {policy}"
        );
        assert!(
            notifications[0].run_id.is_none(),
            "configuration attention is task-scoped for policy {policy}"
        );
        assert_eq!(
            notifications[0].resource_revision,
            Some(blocked.revision),
            "policy {policy}"
        );
    }
}

#[test]
fn deleting_a_model_preserves_identity_and_reasoning_while_blocking_the_task() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    assert_eq!(
        task.config.target_model_id_snapshot.as_deref(),
        Some("model-a")
    );
    let reasoning = task.config.reasoning_json.clone();

    connection
        .execute("DELETE FROM models WHERE id = 'model-a'", [])
        .unwrap();

    let blocked = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(blocked.config.health_state, "blocked");
    assert_eq!(
        blocked.config.blocked_code.as_deref(),
        Some("model_missing")
    );
    assert_eq!(blocked.config.model_id, None);
    assert_eq!(
        blocked.config.target_model_id_snapshot.as_deref(),
        Some("model-a")
    );
    assert_eq!(blocked.config.reasoning_json, reasoning);
    assert_eq!(blocked.config.next_run_at, None);
}

#[test]
fn saving_model_settings_updates_existing_ids_without_false_deletion_blocking() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let settings = ModelSettingsRecord {
        api_url: "https://example.invalid/v1".to_string(),
        api_token: "updated-token".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            provider_model_id: "model-a".to_string(),
            display_name: "Renamed Model A".to_string(),
            api_url_override: None,
            api_token_override: None,
            supports_image: false,
            context_window_tokens: Some(8_192),
            provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                ProviderProtocolDialect::OpenAiChatCompletions,
            ),
            input_price: "0".to_string(),
            cached_input_price: String::new(),
            output_price: "0".to_string(),
            enabled: true,
        }],
    };

    crate::storage::config_repository::save_model_settings(
        &mut connection,
        crate::storage::config_repository::credential_free_fixture(settings.clone()),
    )
    .unwrap();
    let retained = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(retained.config.health_state, "ok");
    assert_eq!(retained.config.model_id.as_deref(), Some("model-a"));
    assert_eq!(retained.revision, task.revision);

    crate::storage::config_repository::save_model_settings(
        &mut connection,
        crate::storage::config_repository::credential_free_fixture(ModelSettingsRecord {
            models: Vec::new(),
            ..settings
        }),
    )
    .unwrap();
    let removed = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(removed.config.health_state, "blocked");
    assert_eq!(
        removed.config.blocked_code.as_deref(),
        Some("model_missing")
    );
    assert_eq!(removed.config.model_id, None);
}

#[test]
fn existing_chat_is_blocked_when_its_inherited_model_becomes_unavailable() {
    for disable_model in [false, true] {
        let mut connection = connection();
        connection
            .execute_batch(
                "INSERT INTO conversations (
                    id, project_id, model_id, title, created_at, updated_at,
                    pinned_at, archived_at, unread_at
                 ) VALUES (
                    'conversation-a', NULL, 'model-a', 'Target chat', 1, 1,
                    NULL, NULL, NULL
                 );",
            )
            .unwrap();

        let mut existing_chat_config = config();
        existing_chat_config.destination_kind = "existing_chat".to_string();
        existing_chat_config.target_conversation_id = Some("conversation-a".to_string());
        existing_chat_config.project_binding_kind = "inherit".to_string();
        existing_chat_config.model_id = None;
        existing_chat_config.reasoning_json = None;
        existing_chat_config.target_model_snapshot = None;
        existing_chat_config.target_model_id_snapshot = None;
        existing_chat_config.target_conversation_snapshot = Some("Target chat".to_string());
        let task = match create_automation(
            &mut connection,
            &NewAutomationRecord {
                id: "automation-a".to_string(),
                create_request_id: "request-a".to_string(),
                status: StoredAutomationStatus::Active,
                config: existing_chat_config,
            },
        )
        .unwrap()
        {
            AutomationCreateOutcome::Created(record) => record,
            outcome => panic!("unexpected outcome: {outcome:?}"),
        };

        if disable_model {
            connection
                .execute(
                    "UPDATE models SET enabled = 0, updated_at = updated_at + 1
                     WHERE id = 'model-a'",
                    [],
                )
                .unwrap();
        } else {
            connection
                .execute("DELETE FROM models WHERE id = 'model-a'", [])
                .unwrap();
        }

        let blocked = get_automation(&connection, &task.id).unwrap().unwrap();
        assert_eq!(blocked.status, StoredAutomationStatus::Active);
        assert_eq!(blocked.config.health_state, "blocked");
        assert_eq!(
            blocked.config.blocked_code.as_deref(),
            Some(if disable_model {
                "model_disabled"
            } else {
                "model_missing"
            })
        );
        assert_eq!(
            blocked.config.target_conversation_id.as_deref(),
            Some("conversation-a")
        );
        assert_eq!(
            blocked.config.target_model_id_snapshot.as_deref(),
            Some("model-a")
        );
        assert_eq!(
            blocked.config.target_model_snapshot.as_deref(),
            Some("Model A")
        );
        assert_eq!(blocked.config.next_run_at, None);
        assert!(blocked.attention_required_at.is_some());
        assert!(list_automation_events_after(&connection, 0, 10)
            .unwrap()
            .iter()
            .any(|event| {
                event.event_kind == "attention_changed"
                    && event.automation_id == task.id
                    && event.resource_revision == Some(blocked.revision)
            }));
    }
}

#[test]
fn existing_chat_is_blocked_when_its_inherited_project_is_deleted() {
    let mut connection = connection();
    connection
        .execute_batch(
            "INSERT INTO projects (id, name, created_at, pinned_at, updated_at)
             VALUES ('project-a', 'Project A', 1, NULL, 1);
             INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (
                'conversation-a', 'project-a', 'model-a', 'Target chat', 1, 1,
                NULL, NULL, NULL
             );",
        )
        .unwrap();

    let mut existing_chat_config = config();
    existing_chat_config.destination_kind = "existing_chat".to_string();
    existing_chat_config.target_conversation_id = Some("conversation-a".to_string());
    existing_chat_config.project_binding_kind = "inherit".to_string();
    existing_chat_config.model_id = None;
    existing_chat_config.reasoning_json = None;
    existing_chat_config.target_project_snapshot = None;
    existing_chat_config.target_project_id_snapshot = None;
    existing_chat_config.target_model_snapshot = None;
    existing_chat_config.target_model_id_snapshot = None;
    existing_chat_config.target_conversation_snapshot = Some("Target chat".to_string());
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-a".to_string(),
            create_request_id: "request-a".to_string(),
            status: StoredAutomationStatus::Active,
            config: existing_chat_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected outcome: {outcome:?}"),
    };

    connection
        .execute("DELETE FROM projects WHERE id = 'project-a'", [])
        .unwrap();

    let blocked = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(blocked.status, StoredAutomationStatus::Active);
    assert_eq!(blocked.config.health_state, "blocked");
    assert_eq!(
        blocked.config.blocked_code.as_deref(),
        Some("project_missing")
    );
    assert_eq!(
        blocked.config.target_conversation_id.as_deref(),
        Some("conversation-a")
    );
    assert_eq!(
        blocked.config.target_project_id_snapshot.as_deref(),
        Some("project-a")
    );
    assert_eq!(
        blocked.config.target_project_snapshot.as_deref(),
        Some("Project A")
    );
    assert_eq!(blocked.config.next_run_at, None);
    assert!(blocked.attention_required_at.is_some());
    assert!(list_automation_events_after(&connection, 0, 10)
        .unwrap()
        .iter()
        .any(|event| {
            event.event_kind == "attention_changed"
                && event.automation_id == task.id
                && event.resource_revision == Some(blocked.revision)
        }));
}

fn enqueue_and_claim_manual_run(
    connection: &mut Connection,
    task: &AutomationRecord,
    run_id: &str,
    request_id: &str,
    now: i64,
) -> AutomationRunRecord {
    enqueue_manual_automation_run(
        connection,
        &NewManualAutomationRunRecord {
            id: run_id.to_string(),
            automation_id: task.id.clone(),
            manual_request_id: request_id.to_string(),
            scheduled_for: now,
            config_revision: task.revision,
            config_snapshot_json: serde_json::json!({
                "schemaVersion": 1,
                "title": "Daily brief",
                "notificationPolicy": &task.config.notification_policy,
            })
            .to_string(),
            expected_revision: task.revision,
        },
    )
    .unwrap();
    let mut claimed = claim_ready_automation_runs(connection, now, 30_000, 3).unwrap();
    assert_eq!(claimed.len(), 1);
    claimed.remove(0)
}

fn insert_admitted_turn_projection(
    transaction: &Transaction<'_>,
    conversation_id: &str,
    user_message_id: &str,
    assistant_message_id: &str,
    agent_run_id: &str,
    timestamp: i64,
) {
    transaction
        .execute(
            "INSERT INTO conversations (
                id, project_id, model_id, title, created_at, updated_at,
                pinned_at, archived_at, unread_at
             ) VALUES (?1, NULL, 'model-a', 'Automation chat', ?2, ?2, NULL, NULL, NULL)",
            params![conversation_id, timestamp],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO messages (
                id, conversation_id, role, content, status, input_origin_kind,
                created_at, position
             ) VALUES
                (?1, ?3, 'user', 'Run it', 'sent', 'human', ?5, 0),
                (?2, ?3, 'assistant', '', 'pending', NULL, ?5, 1)",
            params![
                user_message_id,
                assistant_message_id,
                conversation_id,
                agent_run_id,
                timestamp,
            ],
        )
        .unwrap();
    transaction
        .execute(
            "INSERT INTO conversation_turn_traces (
                assistant_message_id, conversation_id, run_id, schema_version,
                terminal_status, terminal_error, truncated, created_at, updated_at, completed_at
             ) VALUES (?1, ?2, ?3, 4, 'in_progress', NULL, 0, ?4, ?4, NULL)",
            params![
                assistant_message_id,
                conversation_id,
                agent_run_id,
                timestamp
            ],
        )
        .unwrap();
}

#[test]
fn due_claim_is_fair_bounded_and_merges_missed_occurrences() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let due = list_due_automations(
        &connection,
        task.config.next_run_at.unwrap() + 86_400_000,
        3,
    )
    .unwrap();
    assert_eq!(
        due.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(),
        ["automation-a"]
    );

    let claimed_at = task.config.next_run_at.unwrap() + 86_400_000;
    let next_future = claimed_at + 60_000;
    let input = NewScheduledAutomationRunRecord {
        id: "scheduled-run-a".to_string(),
        automation_id: task.id.clone(),
        trigger_kind: "recovery".to_string(),
        scheduled_for: task.config.next_run_at.unwrap(),
        config_revision: task.revision,
        config_snapshot_json: r#"{"schemaVersion":1,"title":"Daily brief"}"#.to_string(),
        expected_revision: task.revision,
        next_run_at: next_future,
        claimed_at,
    };
    let run = match enqueue_scheduled_automation_run(&mut connection, &input).unwrap() {
        ScheduledAutomationRunEnqueueOutcome::Enqueued(run) => run,
        outcome => panic!("unexpected claim outcome: {outcome:?}"),
    };
    assert_eq!(run.scheduled_for, task.config.next_run_at.unwrap());
    assert_eq!(run.trigger_kind, "recovery");
    assert_eq!(
        get_automation(&connection, &task.id)
            .unwrap()
            .unwrap()
            .config
            .next_run_at,
        Some(next_future)
    );
    assert_eq!(
        enqueue_scheduled_automation_run(&mut connection, &input).unwrap(),
        ScheduledAutomationRunEnqueueOutcome::Existing(run)
    );
}

#[test]
fn admission_token_and_outer_transaction_make_turn_admission_exactly_once() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let timestamp = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-a",
        "manual-request-a",
        timestamp,
    );
    let token = claimed.admission_token.clone().unwrap();

    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let stale = admit_automation_run_in_transaction(
            &transaction,
            &AutomationRunAdmissionInput {
                automation_run_id: claimed.id.clone(),
                admission_token: "stale-token".to_string(),
                config_revision: task.revision,
                permission_mode: "default".to_string(),
                agent_run_id: "agent-run-a".to_string(),
                conversation_id: "conversation-a".to_string(),
                user_message_id: "user-a".to_string(),
                assistant_message_id: "assistant-a".to_string(),
                admitted_at: timestamp,
            },
        )
        .unwrap();
        assert!(matches!(
            stale,
            AutomationRunAdmissionOutcome::Stale(Some(_))
        ));
        transaction.rollback().unwrap();
    }

    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        insert_admitted_turn_projection(
            &transaction,
            "conversation-a",
            "user-a",
            "assistant-a",
            "agent-run-a",
            timestamp,
        );
        let outcome = admit_automation_run_in_transaction(
            &transaction,
            &AutomationRunAdmissionInput {
                automation_run_id: claimed.id.clone(),
                admission_token: token.clone(),
                config_revision: task.revision,
                permission_mode: "default".to_string(),
                agent_run_id: "agent-run-a".to_string(),
                conversation_id: "conversation-a".to_string(),
                user_message_id: "user-a".to_string(),
                assistant_message_id: "assistant-a".to_string(),
                admitted_at: timestamp,
            },
        )
        .unwrap();
        assert!(matches!(
            outcome,
            AutomationRunAdmissionOutcome::Admitted(_)
        ));
        transaction.rollback().unwrap();
    }

    let after_rollback = get_automation_run(&connection, &claimed.id)
        .unwrap()
        .unwrap();
    assert_eq!(after_rollback.status, StoredAutomationRunStatus::Admitting);
    assert_eq!(
        after_rollback.admission_token.as_deref(),
        Some(token.as_str())
    );
    assert!(after_rollback.agent_run_id.is_none());
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM conversation_turn_traces", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn startup_reclaims_unexpired_admission_and_retry_backoff_is_respected() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let now = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-a",
        "manual-request-a",
        now,
    );
    assert!(claimed.admission_expires_at.unwrap() > now);
    let recoverable = list_recoverable_automation_runs(&connection).unwrap();
    assert_eq!(recoverable.len(), 1);
    assert_eq!(recoverable[0].run.id, claimed.id);
    assert!(recoverable[0].trace_terminal_status.is_none());
    let recovered =
        recover_automation_admission_leases_on_startup(&mut connection, now + 1).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].status, StoredAutomationRunStatus::Queued);

    let claimed_again = claim_ready_automation_runs(&mut connection, now + 1, 30_000, 3)
        .unwrap()
        .remove(0);
    let retry_at = now + 60_000;
    assert!(matches!(
        defer_automation_run(
            &mut connection,
            &claimed_again.id,
            claimed_again.admission_token.as_deref().unwrap(),
            retry_at,
            now + 2,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(_)
    ));
    assert!(
        claim_ready_automation_runs(&mut connection, retry_at - 1, 30_000, 3)
            .unwrap()
            .is_empty()
    );
    let final_claim = claim_ready_automation_runs(&mut connection, retry_at, 30_000, 3)
        .unwrap()
        .remove(0);
    let terminated = terminate_unadmitted_automation_run(
        &mut connection,
        &final_claim.id,
        final_claim.admission_token.as_deref().unwrap(),
        StoredAutomationRunStatus::Failed,
        "admission_failed",
        "The Agent turn could not be started.",
        retry_at + 1,
    )
    .unwrap();
    assert!(matches!(
        terminated,
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::Failed,
            ..
        })
    ));
}

#[test]
fn terminal_trace_settlement_persists_unknown_report_attention_and_deduplicated_outbox() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let timestamp = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-a",
        "manual-request-a",
        timestamp,
    );
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    insert_admitted_turn_projection(
        &transaction,
        "conversation-a",
        "user-a",
        "assistant-a",
        "agent-run-a",
        timestamp,
    );
    assert!(matches!(
        admit_automation_run_in_transaction(
            &transaction,
            &AutomationRunAdmissionInput {
                automation_run_id: claimed.id.clone(),
                admission_token: claimed.admission_token.clone().unwrap(),
                config_revision: task.revision,
                permission_mode: "default".to_string(),
                agent_run_id: "agent-run-a".to_string(),
                conversation_id: "conversation-a".to_string(),
                user_message_id: "user-a".to_string(),
                assistant_message_id: "assistant-a".to_string(),
                admitted_at: timestamp,
            },
        )
        .unwrap(),
        AutomationRunAdmissionOutcome::Admitted(_)
    ));
    transaction.commit().unwrap();
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', updated_at = ?1, completed_at = ?1
             WHERE run_id = 'agent-run-a'",
            [timestamp + 100],
        )
        .unwrap();
    let settlement = AutomationRunSettlementInput {
        automation_run_id: claimed.id.clone(),
        agent_run_id: "agent-run-a".to_string(),
        terminal_status: StoredAutomationRunStatus::Completed,
        report_kind: None,
        result_preview: Some("Finished safely".to_string()),
        error_code: None,
        error_message: None,
        settled_at: timestamp + 100,
    };
    let settled = match settle_automation_run_from_trace(&mut connection, &settlement).unwrap() {
        AutomationRunMutationOutcome::Updated(run) => run,
        outcome => panic!("unexpected settlement: {outcome:?}"),
    };
    assert_eq!(settled.report_kind.as_deref(), Some("unknown"));
    assert_eq!(settled.status, StoredAutomationRunStatus::Completed);
    assert_eq!(
        settle_automation_run_from_trace(&mut connection, &settlement).unwrap(),
        AutomationRunMutationOutcome::Updated(settled.clone())
    );
    let notifications = application_notifications(&connection);
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].conversation_id.as_deref(),
        Some("conversation-a")
    );
    assert_eq!(notifications[0].user_message_id.as_deref(), Some("user-a"));
    assert_eq!(
        notifications[0].assistant_message_id.as_deref(),
        Some("assistant-a")
    );
    assert_eq!(notifications[0].notification_kind, "automation_completed");
    assert_eq!(
        notifications[0].run_id.as_deref(),
        Some(claimed.id.as_str())
    );
}

#[test]
fn terminal_notification_uses_the_run_snapshot_after_task_edits() {
    let mut connection = connection();
    let task = create(
        &mut connection,
        "automation-frozen-notification",
        "request-frozen",
    );
    let timestamp = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-frozen-notification",
        "manual-request-frozen-notification",
        timestamp,
    );
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    insert_admitted_turn_projection(
        &transaction,
        "conversation-frozen-notification",
        "user-frozen-notification",
        "assistant-frozen-notification",
        "agent-run-frozen-notification",
        timestamp,
    );
    assert!(matches!(
        admit_automation_run_in_transaction(
            &transaction,
            &AutomationRunAdmissionInput {
                automation_run_id: claimed.id.clone(),
                admission_token: claimed.admission_token.clone().unwrap(),
                config_revision: task.revision,
                permission_mode: "default".to_string(),
                agent_run_id: "agent-run-frozen-notification".to_string(),
                conversation_id: "conversation-frozen-notification".to_string(),
                user_message_id: "user-frozen-notification".to_string(),
                assistant_message_id: "assistant-frozen-notification".to_string(),
                admitted_at: timestamp,
            },
        )
        .unwrap(),
        AutomationRunAdmissionOutcome::Admitted(_)
    ));
    transaction.commit().unwrap();

    let mut edited_config = task.config.clone();
    edited_config.title = "Edited task title".to_string();
    edited_config.notification_policy = "unsuccessful_only".to_string();
    assert!(matches!(
        replace_automation_config(&mut connection, &task.id, task.revision, &edited_config,)
            .unwrap(),
        AutomationCompareAndSetOutcome::Updated(_)
    ));
    connection
        .execute(
            "UPDATE conversation_turn_traces
             SET terminal_status = 'completed', updated_at = ?1, completed_at = ?1
             WHERE run_id = 'agent-run-frozen-notification'",
            [timestamp + 100],
        )
        .unwrap();
    let settled = match settle_automation_run_from_trace(
        &mut connection,
        &AutomationRunSettlementInput {
            automation_run_id: claimed.id,
            agent_run_id: "agent-run-frozen-notification".to_string(),
            terminal_status: StoredAutomationRunStatus::Completed,
            report_kind: Some("no_change".to_string()),
            result_preview: Some("No material change.".to_string()),
            error_code: None,
            error_message: None,
            settled_at: timestamp + 100,
        },
    )
    .unwrap()
    {
        AutomationRunMutationOutcome::Updated(run) => run,
        outcome => panic!("unexpected settlement outcome: {outcome:?}"),
    };
    assert_eq!(settled.status, StoredAutomationRunStatus::Completed);

    let notifications = application_notifications(&connection);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].subject_text, "Daily brief");
    assert_eq!(notifications[0].notification_kind, "automation_completed");
    assert_eq!(
        get_automation(&connection, &task.id)
            .unwrap()
            .unwrap()
            .config
            .notification_policy,
        "unsuccessful_only"
    );
}

#[test]
fn blocking_a_task_terminalizes_unadmitted_work_and_tombstone_suppresses_notifications() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let now = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-a",
        "manual-request-a",
        now,
    );
    let blocked = match block_automation(
        &mut connection,
        &task.id,
        task.revision,
        "permission_disabled",
        "The selected permission mode is disabled.",
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(task) => task,
        outcome => panic!("unexpected block outcome: {outcome:?}"),
    };
    assert_eq!(
        get_automation_run(&connection, &claimed.id)
            .unwrap()
            .unwrap()
            .status,
        StoredAutomationRunStatus::Failed
    );
    assert!(list_nonterminal_automation_runs(&connection)
        .unwrap()
        .is_empty());
    tombstone_automation(&mut connection, &task.id, blocked.revision).unwrap();
    assert!(application_notifications(&connection).is_empty());
}

#[test]
fn blocking_queued_work_notifies_important_updates_and_does_not_strand_the_run() {
    let mut connection = connection();
    let mut task_config = config();
    task_config.notification_policy = "important_updates".to_string();
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-important-block".to_string(),
            create_request_id: "request-important-block".to_string(),
            status: StoredAutomationStatus::Active,
            config: task_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected create outcome: {outcome:?}"),
    };
    let now = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-important-block",
        "manual-request-important-block",
        now,
    );

    let blocked = match block_automation(
        &mut connection,
        &task.id,
        task.revision,
        "permission_disabled",
        "The selected permission mode is disabled.",
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected block outcome: {outcome:?}"),
    };
    let failed_run = get_automation_run(&connection, &claimed.id)
        .unwrap()
        .unwrap();
    assert_eq!(failed_run.status, StoredAutomationRunStatus::Failed);
    assert_eq!(
        failed_run.error_code.as_deref(),
        Some("permission_disabled")
    );
    assert!(list_nonterminal_automation_runs(&connection)
        .unwrap()
        .is_empty());

    let notifications = application_notifications(&connection);
    assert_eq!(notifications.len(), 1);
    assert_eq!(
        notifications[0].notification_kind,
        "automation_configuration_blocked"
    );
    assert!(notifications[0].run_id.is_none());
    assert_eq!(notifications[0].resource_revision, Some(blocked.revision));
}

#[test]
fn failed_unadmitted_work_emits_a_run_notification_for_important_updates() {
    let mut connection = connection();
    let mut task_config = config();
    task_config.notification_policy = "important_updates".to_string();
    let task = match create_automation(
        &mut connection,
        &NewAutomationRecord {
            id: "automation-important-failure".to_string(),
            create_request_id: "request-important-failure".to_string(),
            status: StoredAutomationStatus::Active,
            config: task_config,
        },
    )
    .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected create outcome: {outcome:?}"),
    };
    let now = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-important-failure",
        "manual-request-important-failure",
        now,
    );
    let failed = match terminate_unadmitted_automation_run(
        &mut connection,
        &claimed.id,
        claimed.admission_token.as_deref().unwrap(),
        StoredAutomationRunStatus::Failed,
        "admission_failed",
        "The Agent turn could not be started.",
        now + 1,
    )
    .unwrap()
    {
        AutomationRunMutationOutcome::Updated(record) => record,
        outcome => panic!("unexpected termination outcome: {outcome:?}"),
    };

    let notifications = application_notifications(&connection);
    assert_eq!(notifications.len(), 1);
    assert_eq!(notifications[0].notification_kind, "automation_failed");
    assert_eq!(notifications[0].run_id.as_deref(), Some(failed.id.as_str()));
}

#[test]
fn structured_report_is_bounded_and_notification_policy_is_not_keyword_based() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let now = now_ms() + 1_000;
    let claimed = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-a",
        "manual-request-a",
        now,
    );
    connection
        .execute(
            "UPDATE automation_runs
             SET status = 'running', status_revision = status_revision + 1,
                 admission_token = NULL, admission_expires_at = NULL,
                 agent_run_id = 'agent-run-a', started_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![now, claimed.id],
        )
        .unwrap();
    let reported = match record_automation_report(
        &mut connection,
        &claimed.id,
        "important_update",
        "A structured update, independent of wording.",
        now + 1,
    )
    .unwrap()
    {
        AutomationRunMutationOutcome::Updated(run) => run,
        outcome => panic!("unexpected report outcome: {outcome:?}"),
    };
    assert_eq!(reported.report_kind.as_deref(), Some("important_update"));
    assert_eq!(
        reported.result_preview.as_deref(),
        Some("A structured update, independent of wording.")
    );
    assert!(matches!(
        record_automation_report(
            &mut connection,
            &claimed.id,
            "no_change",
            "A later tool call cannot downgrade the first durable report.",
            now + 2,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Stale(Some(_))
    ));
    assert!(record_automation_report(
        &mut connection,
        &claimed.id,
        "unknown_kind",
        "not accepted",
        now + 3,
    )
    .is_err());

    connection
        .execute(
            "UPDATE automation_runs
             SET status = 'waiting_for_approval', attention_required_at = ?1,
                 updated_at = ?1
             WHERE id = ?2",
            params![now + 4, claimed.id],
        )
        .unwrap();
    let attentions = list_automation_attentions(&connection, None, 20).unwrap();
    assert_eq!(attentions.items.len(), 1);
    assert_eq!(attentions.items[0].attention_kind, "waiting_for_approval");
    assert_eq!(
        attentions.items[0].message.as_deref(),
        Some("A structured update, independent of wording.")
    );

    assert!(!terminal_notification_requested(
        "important_updates",
        StoredAutomationRunStatus::Completed,
        "no_change"
    ));
    assert!(terminal_notification_requested(
        "important_updates",
        StoredAutomationRunStatus::Completed,
        "important_update"
    ));
    assert!(terminal_notification_requested(
        "important_updates",
        StoredAutomationRunStatus::Completed,
        "completed"
    ));
    assert!(terminal_notification_requested(
        "important_updates",
        StoredAutomationRunStatus::Completed,
        "unknown"
    ));
}

#[test]
fn automation_notifications_use_the_generic_durable_delivery_queue() {
    let mut connection = connection();
    let task = create(&mut connection, "automation-a", "request-a");
    let now = now_ms() + 1_000;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let notification = enqueue_automation_notification_in_transaction(
        &transaction,
        &NewAutomationNotificationRecord {
            automation_id: task.id.clone(),
            automation_run_id: None,
            resource_revision: task.revision,
            notification_kind: "configuration_blocked".to_string(),
            title: "Needs attention".to_string(),
            body: "Choose a valid target.".to_string(),
            created_at: now,
        },
    )
    .unwrap();
    let replay = enqueue_automation_notification_in_transaction(
        &transaction,
        &NewAutomationNotificationRecord {
            automation_id: task.id,
            automation_run_id: None,
            resource_revision: task.revision,
            notification_kind: "configuration_blocked".to_string(),
            title: "Ignored duplicate title".to_string(),
            body: "Ignored duplicate body".to_string(),
            created_at: now + 1,
        },
    )
    .unwrap();
    assert_eq!(replay.id, notification.id);
    assert_eq!(
        notification.notification_kind,
        "automation_configuration_blocked"
    );
    transaction.commit().unwrap();

    let first = crate::storage::notification_repository::claim_pending_notification_batches(
        &mut connection,
        "claim-a",
        now,
        30_000,
        10,
    )
    .unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].highest_priority, "configuration_blocked");
    let retry_at = now + 60_000;
    assert!(
        crate::storage::notification_repository::release_notification_batch(
            &mut connection,
            &first[0].id,
            "claim-a",
            retry_at,
            "native_notification_unavailable",
        )
        .unwrap()
        .is_some()
    );
    assert!(
        crate::storage::notification_repository::claim_pending_notification_batches(
            &mut connection,
            "claim-b",
            retry_at - 1,
            30_000,
            10,
        )
        .unwrap()
        .is_empty()
    );
    let second = crate::storage::notification_repository::claim_pending_notification_batches(
        &mut connection,
        "claim-b",
        retry_at,
        30_000,
        10,
    )
    .unwrap();
    assert_eq!(second[0].id, first[0].id);
    assert_eq!(second[0].attempt_count, 2);
    assert!(
        crate::storage::notification_repository::acknowledge_notification_batch(
            &mut connection,
            &second[0].id,
            "claim-b",
            "delivered",
            "configuration_blocked",
            "initial",
            second[0].revision,
            retry_at + 1,
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn final_notification_validation_suppresses_deleted_repaired_and_settled_claims() {
    let mut connection = connection();
    let base = now_ms() + 10_000;

    let deleted_task = create(
        &mut connection,
        "automation-notification-deleted",
        "request-notification-deleted",
    );
    let deleted_task = match block_automation(
        &mut connection,
        &deleted_task.id,
        deleted_task.revision,
        "target_invalid",
        "Choose a valid target.",
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected block outcome: {outcome:?}"),
    };
    let deleted_claim = claim_application_notifications(&mut connection, "claim-deleted", base);
    assert_eq!(deleted_claim.len(), 1);
    assert_eq!(deleted_claim[0].1.len(), 1);
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &deleted_claim[0].0.id,
        "claim-deleted",
        base + 1,
    )
    .unwrap()
    .is_some());
    assert!(matches!(
        tombstone_automation(&mut connection, &deleted_task.id, deleted_task.revision,).unwrap(),
        AutomationCompareAndSetOutcome::Updated(_)
    ));
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &deleted_claim[0].0.id,
        "claim-deleted",
        base + 2,
    )
    .unwrap()
    .is_none());

    let repaired_task = create(
        &mut connection,
        "automation-notification-repaired",
        "request-notification-repaired",
    );
    let blocked_task = match block_automation(
        &mut connection,
        &repaired_task.id,
        repaired_task.revision,
        "target_invalid",
        "Choose a valid target.",
    )
    .unwrap()
    {
        AutomationCompareAndSetOutcome::Updated(record) => record,
        outcome => panic!("unexpected block outcome: {outcome:?}"),
    };
    let repaired_claim =
        claim_application_notifications(&mut connection, "claim-repaired", base + 3);
    assert_eq!(repaired_claim.len(), 1);
    assert_eq!(repaired_claim[0].1.len(), 1);
    assert_eq!(
        repaired_claim[0].1[0].resource_revision,
        Some(blocked_task.revision),
        "the claim must identify the exact blocked task revision"
    );
    let mut repaired_config = blocked_task.config.clone();
    repaired_config.health_state = "ok".to_string();
    repaired_config.blocked_code = None;
    repaired_config.blocked_message = None;
    repaired_config.next_run_at = Some(base + 120_000);
    assert!(matches!(
        replace_automation_config(
            &mut connection,
            &blocked_task.id,
            blocked_task.revision,
            &repaired_config,
        )
        .unwrap(),
        AutomationCompareAndSetOutcome::Updated(_)
    ));
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &repaired_claim[0].0.id,
        "claim-repaired",
        base + 4,
    )
    .unwrap()
    .is_none());
    let repaired_status: String = connection
        .query_row(
            "SELECT status FROM notification_batches WHERE id = ?1",
            [&repaired_claim[0].0.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(repaired_status, "suppressed");

    let approval_task = create(
        &mut connection,
        "automation-notification-approval",
        "request-notification-approval",
    );
    let approval_claim = enqueue_and_claim_manual_run(
        &mut connection,
        &approval_task,
        "automation-run-notification-approval",
        "manual-request-notification-approval",
        base + 5,
    );
    connection
        .execute(
            "UPDATE automation_runs
             SET status = 'running', status_revision = status_revision + 1,
                 admission_token = NULL, admission_expires_at = NULL,
                 agent_run_id = 'agent-run-notification-approval',
                 started_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![base + 5, approval_claim.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                action_id, run_id, conversation_id, assistant_message_id,
                action_type, tool_name, tool_call_id, status, target_status,
                action_json, agent_input_json, created_at, updated_at
             ) VALUES (
                'action-notification-approval', 'agent-run-notification-approval', NULL, NULL,
                'command', 'run_command', 'call-notification-approval', 'pending', NULL,
                '{}', '{}', ?1, ?1
             )",
            [base + 5],
        )
        .unwrap();
    assert!(matches!(
        set_automation_run_waiting_for_approval(
            &mut connection,
            &approval_claim.id,
            "agent-run-notification-approval",
            true,
            base + 6,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::WaitingForApproval,
            ..
        })
    ));
    let waiting_claim =
        claim_application_notifications(&mut connection, "claim-approval", base + 7);
    assert_eq!(waiting_claim.len(), 1);
    assert_eq!(waiting_claim[0].1.len(), 1);
    assert_eq!(waiting_claim[0].1[0].notification_kind, "approval_required");
    connection
        .execute(
            "UPDATE agent_pending_actions
             SET status = 'approved', updated_at = ?1
             WHERE action_id = 'action-notification-approval' AND status = 'pending'",
            [base + 8],
        )
        .unwrap();
    assert!(matches!(
        set_automation_run_waiting_for_approval(
            &mut connection,
            &approval_claim.id,
            "agent-run-notification-approval",
            false,
            base + 9,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::Running,
            ..
        })
    ));
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &waiting_claim[0].0.id,
        "claim-approval",
        base + 10,
    )
    .unwrap()
    .is_none());
    let approval_status: String = connection
        .query_row(
            "SELECT status FROM notification_batches WHERE id = ?1",
            [&waiting_claim[0].0.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(approval_status, "suppressed");
}

#[test]
fn approval_notification_validation_requires_a_still_pending_durable_action() {
    let mut connection = connection();
    let base = now_ms() + 10_000;
    let task = create(
        &mut connection,
        "automation-notification-approved",
        "request-notification-approved",
    );
    let claimed_run = enqueue_and_claim_manual_run(
        &mut connection,
        &task,
        "automation-run-notification-approved",
        "manual-request-notification-approved",
        base,
    );
    connection
        .execute(
            "UPDATE automation_runs
             SET status = 'running', status_revision = status_revision + 1,
                 admission_token = NULL, admission_expires_at = NULL,
                 agent_run_id = 'agent-run-notification-approved',
                 started_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![base, claimed_run.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                action_id, run_id, conversation_id, assistant_message_id,
                action_type, tool_name, tool_call_id, status, target_status,
                action_json, agent_input_json, created_at, updated_at
             ) VALUES (
                'action-notification-approved', 'agent-run-notification-approved', NULL, NULL,
                'command', 'run_command', 'call-notification-approved', 'pending', NULL,
                '{}', '{}', ?1, ?1
             )",
            [base],
        )
        .unwrap();
    assert!(matches!(
        set_automation_run_waiting_for_approval(
            &mut connection,
            &claimed_run.id,
            "agent-run-notification-approved",
            true,
            base + 1,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::WaitingForApproval,
            ..
        })
    ));
    let claimed_notification =
        claim_application_notifications(&mut connection, "claim-approved", base + 2);
    assert_eq!(claimed_notification.len(), 1);
    assert_eq!(claimed_notification[0].1.len(), 1);
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &claimed_notification[0].0.id,
        "claim-approved",
        base + 3,
    )
    .unwrap()
    .is_some());

    connection
        .execute(
            "UPDATE agent_pending_actions
             SET status = 'approved', updated_at = ?1
             WHERE action_id = 'action-notification-approved' AND status = 'pending'",
            [base + 4],
        )
        .unwrap();
    assert!(matches!(
        set_automation_run_waiting_for_approval(
            &mut connection,
            &claimed_run.id,
            "agent-run-notification-approved",
            false,
            base + 5,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::Running,
            ..
        })
    ));
    assert!(validate_claimed_notification_batch(
        &mut connection,
        &claimed_notification[0].0.id,
        "claim-approved",
        base + 6,
    )
    .unwrap()
    .is_none());
    let notification_status: String = connection
        .query_row(
            "SELECT status FROM notification_batches WHERE id = ?1",
            [&claimed_notification[0].0.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(notification_status, "suppressed");
}

#[test]
fn attention_acknowledgement_covers_all_kinds_with_revisioned_idempotency() {
    let mut connection = connection();
    let base = now_ms() + 1_000;

    let blocked_task = create(
        &mut connection,
        "automation-attention-blocked",
        "request-attention-blocked",
    );
    assert!(matches!(
        block_automation(
            &mut connection,
            &blocked_task.id,
            blocked_task.revision,
            "target_invalid",
            "Choose a valid target.",
        )
        .unwrap(),
        AutomationCompareAndSetOutcome::Updated(_)
    ));

    let failed_task = create(
        &mut connection,
        "automation-attention-failed",
        "request-attention-failed",
    );
    let failed_claim = enqueue_and_claim_manual_run(
        &mut connection,
        &failed_task,
        "automation-run-attention-failed",
        "manual-request-attention-failed",
        base,
    );
    assert!(matches!(
        terminate_unadmitted_automation_run(
            &mut connection,
            &failed_claim.id,
            failed_claim.admission_token.as_deref().unwrap(),
            StoredAutomationRunStatus::Failed,
            "admission_failed",
            "The Agent turn could not be started.",
            base + 1,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(_)
    ));

    let waiting_task = create(
        &mut connection,
        "automation-attention-waiting",
        "request-attention-waiting",
    );
    let waiting_claim = enqueue_and_claim_manual_run(
        &mut connection,
        &waiting_task,
        "automation-run-attention-waiting",
        "manual-request-attention-waiting",
        base + 2,
    );
    connection
        .execute(
            "UPDATE automation_runs
             SET status = 'running', status_revision = status_revision + 1,
                 admission_token = NULL, admission_expires_at = NULL,
                 agent_run_id = 'agent-run-attention-waiting',
                 started_at = ?1, updated_at = ?1
             WHERE id = ?2",
            params![base + 2, waiting_claim.id],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO agent_pending_actions (
                action_id, run_id, conversation_id, assistant_message_id,
                action_type, tool_name, tool_call_id, status, target_status,
                action_json, agent_input_json, created_at, updated_at
             ) VALUES (
                'action-attention-waiting', 'agent-run-attention-waiting', NULL, NULL,
                'command', 'run_command', 'call-attention-waiting', 'pending', NULL,
                '{}', '{}', ?1, ?1
             )",
            [base + 2],
        )
        .unwrap();
    assert!(matches!(
        set_automation_run_waiting_for_approval(
            &mut connection,
            &waiting_claim.id,
            "agent-run-attention-waiting",
            true,
            base + 3,
        )
        .unwrap(),
        AutomationRunMutationOutcome::Updated(AutomationRunRecord {
            status: StoredAutomationRunStatus::WaitingForApproval,
            ..
        })
    ));

    let attention_page = list_automation_attentions(&connection, None, 10).unwrap();
    assert_eq!(attention_page.items.len(), 3);
    assert!(attention_page.items.iter().any(|record| {
        record.attention_id == format!("task:{}", blocked_task.id)
            && record.attention_kind == "configuration_blocked"
    }));
    assert!(attention_page.items.iter().any(|record| {
        record.attention_id == format!("run:{}", failed_claim.id)
            && record.attention_kind == "run_failed"
    }));
    assert!(attention_page.items.iter().any(|record| {
        record.attention_id == format!("run:{}", waiting_claim.id)
            && record.attention_kind == "waiting_for_approval"
    }));

    for attention in &attention_page.items {
        let acknowledged_at = attention.required_at + 10;
        let acknowledged = acknowledge_automation_attention_record(
            &mut connection,
            &attention.attention_id,
            acknowledged_at,
        )
        .unwrap()
        .unwrap();
        assert!(acknowledged.read_at.unwrap() >= acknowledged.required_at);

        let resource_revision = if let Some(run_id) = attention.automation_run_id.as_deref() {
            get_automation_run(&connection, run_id)
                .unwrap()
                .unwrap()
                .status_revision
        } else {
            get_automation(&connection, &attention.automation_id)
                .unwrap()
                .unwrap()
                .revision
        };
        assert!(get_latest_automation_event_for(
            &connection,
            &attention.automation_id,
            attention.automation_run_id.as_deref(),
            "attention_changed",
            Some(resource_revision),
        )
        .unwrap()
        .is_some());

        let sequence_after_first_ack = latest_automation_event_sequence(&connection).unwrap();
        let replay = acknowledge_automation_attention_record(
            &mut connection,
            &attention.attention_id,
            acknowledged_at + 1,
        )
        .unwrap()
        .unwrap();
        assert_eq!(replay, acknowledged);
        assert_eq!(
            latest_automation_event_sequence(&connection).unwrap(),
            sequence_after_first_ack,
            "an ACK replay must not create another event"
        );
        let revision_after_replay = if let Some(run_id) = attention.automation_run_id.as_deref() {
            get_automation_run(&connection, run_id)
                .unwrap()
                .unwrap()
                .status_revision
        } else {
            get_automation(&connection, &attention.automation_id)
                .unwrap()
                .unwrap()
                .revision
        };
        assert_eq!(revision_after_replay, resource_revision);
    }
    assert_eq!(
        automation_attention_summary(&connection)
            .unwrap()
            .total_count,
        0
    );
}
