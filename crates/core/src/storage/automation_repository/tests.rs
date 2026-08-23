use super::*;
use crate::storage::migrations;
use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use crate::{ProviderProfileConfig, ProviderProtocolDialect};

fn connection() -> Connection {
    let connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    connection
        .execute_batch(
            "INSERT INTO models (
                id, display_name, api_url_override, api_token_override,
                supports_image, context_window_tokens, provider_profile_config_json,
                provider_connection_revision, provider_protocol_revision,
                input_price, cached_input_price, output_price,
                enabled, position, created_at, updated_at
             ) VALUES (
                'model-a', 'Model A', NULL, NULL,
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
        permission_mode_version: 1,
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
            "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
             VALUES ('project-a', 'Project A', '/project-a', 1, NULL, 1);
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
fn notification_outbox_supports_deduplicated_configuration_attention_without_a_run() {
    let mut connection = connection();
    create(&mut connection, "automation-a", "request-a");
    connection
        .execute(
            "INSERT INTO automation_notification_outbox (
                id, schema_version, automation_id, automation_run_id, resource_revision,
                notification_kind, title, body, status, created_at, delivered_at
             ) VALUES (
                'notification-a', 1, 'automation-a', NULL, 1,
                'configuration_blocked', 'Needs repair', 'Select a new target',
                'pending', 1, NULL
             )",
            [],
        )
        .unwrap();
    assert!(connection
        .execute(
            "INSERT INTO automation_notification_outbox (
                id, schema_version, automation_id, automation_run_id, resource_revision,
                notification_kind, title, body, status, created_at, delivered_at
             ) VALUES (
                'notification-b', 1, 'automation-a', NULL, 1,
                'configuration_blocked', 'Duplicate', 'Must be rejected',
                'pending', 2, NULL
             )",
            [],
        )
        .is_err());
    assert!(connection
        .execute(
            "INSERT INTO automation_notification_outbox (
                id, schema_version, automation_id, automation_run_id, resource_revision,
                notification_kind, title, body, status, created_at, delivered_at
             ) VALUES (
                'notification-c', 1, 'automation-a', NULL, 1,
                'run_result', 'Invalid', 'A run result requires a run',
                'pending', 3, NULL
             )",
            [],
        )
        .is_err());
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

    crate::storage::config_repository::save_model_settings(&mut connection, settings.clone())
        .unwrap();
    let retained = get_automation(&connection, &task.id).unwrap().unwrap();
    assert_eq!(retained.config.health_state, "ok");
    assert_eq!(retained.config.model_id.as_deref(), Some("model-a"));
    assert_eq!(retained.revision, task.revision);

    crate::storage::config_repository::save_model_settings(
        &mut connection,
        ModelSettingsRecord {
            models: Vec::new(),
            ..settings
        },
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
            "INSERT INTO projects (id, name, path, created_at, pinned_at, updated_at)
             VALUES ('project-a', 'Project A', '/project-a', 1, NULL, 1);
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
