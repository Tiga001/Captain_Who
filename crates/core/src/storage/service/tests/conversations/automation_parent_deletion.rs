use super::*;
use crate::storage::automation_repository::{
    AutomationCompareAndSetOutcome, AutomationConfigRecord, AutomationCreateOutcome,
    AutomationListInput, AutomationRecord, AutomationRunEnqueueOutcome, AutomationRunRecord,
    NewAutomationRecord, NewManualAutomationRunRecord, StoredAutomationRunStatus,
    StoredAutomationStatus,
};
use rusqlite::types::Value;
use std::collections::BTreeMap;

fn create_task(
    service: &StorageService,
    id: &str,
    project_id: &str,
    conversation_id: Option<&str>,
) -> AutomationRecord {
    let existing_chat = conversation_id.is_some();
    match service
        .create_automation(&NewAutomationRecord {
            id: id.to_string(),
            create_request_id: format!("request-{id}"),
            status: StoredAutomationStatus::Active,
            config: AutomationConfigRecord {
                title: format!("Follow {id}"),
                prompt: "Report important changes.".to_string(),
                health_state: "ok".to_string(),
                blocked_code: None,
                blocked_message: None,
                destination_kind: if existing_chat {
                    "existing_chat"
                } else {
                    "new_chat"
                }
                .to_string(),
                target_conversation_id: conversation_id.map(str::to_string),
                project_binding_kind: if existing_chat { "inherit" } else { "project" }.to_string(),
                project_id: (!existing_chat).then(|| project_id.to_string()),
                model_id: (!existing_chat).then(|| "model-1".to_string()),
                permission_mode: "default".to_string(),
                permission_mode_version: 2,
                permissions_json: "{}".to_string(),
                reasoning_json: (!existing_chat).then(|| "{}".to_string()),
                schedule_kind: "daily".to_string(),
                schedule_json: r#"{"kind":"daily","timeMinutes":540,"timezone":"Asia/Shanghai"}"#
                    .to_string(),
                rrule: "FREQ=DAILY;INTERVAL=1".to_string(),
                timezone: "Asia/Shanghai".to_string(),
                anchor_at: 1_700_000_000_000,
                next_run_at: Some(1_700_000_100_000),
                notification_policy: "all_runs".to_string(),
                target_project_snapshot: Some(format!("Original {project_id}")),
                target_conversation_snapshot: conversation_id.map(|id| format!("Original {id}")),
                target_model_snapshot: Some("Original model name".to_string()),
                target_project_id_snapshot: Some(project_id.to_string()),
                target_conversation_id_snapshot: conversation_id.map(str::to_string),
                target_model_id_snapshot: Some("model-1".to_string()),
            },
        })
        .unwrap()
    {
        AutomationCreateOutcome::Created(record) => record,
        outcome => panic!("unexpected create outcome: {outcome:?}"),
    }
}

fn tombstone_with_history(
    service: &StorageService,
    task: &AutomationRecord,
) -> AutomationRunRecord {
    let run_id = format!("run-{}", task.id);
    assert!(matches!(
        service
            .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
                id: run_id.clone(),
                automation_id: task.id.clone(),
                manual_request_id: format!("manual-{}", task.id),
                scheduled_for: 1_700_000_100_000,
                config_revision: task.revision,
                config_snapshot_json: serde_json::json!({
                    "schemaVersion": 1,
                    "title": task.config.title,
                    "targetProjectId": task.config.target_project_id_snapshot,
                    "targetConversationId": task.config.target_conversation_id_snapshot,
                    "targetModelId": task.config.target_model_id_snapshot,
                })
                .to_string(),
                expected_revision: task.revision,
            })
            .unwrap(),
        AutomationRunEnqueueOutcome::Enqueued(_)
    ));
    assert!(matches!(
        service
            .tombstone_automation(&task.id, task.revision)
            .unwrap(),
        AutomationCompareAndSetOutcome::Updated(_)
    ));
    let run = service.get_automation_run(&run_id).unwrap().unwrap();
    assert_eq!(run.status, StoredAutomationRunStatus::Cancelled);
    assert_eq!(run.error_code.as_deref(), Some("automation_deleted"));
    run
}

fn stored_task(service: &StorageService, id: &str) -> BTreeMap<String, Value> {
    let connection = service.state.connection().unwrap();
    let mut statement = connection
        .prepare("SELECT * FROM automations WHERE id = ?1")
        .unwrap();
    let columns = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect::<Vec<_>>();
    statement
        .query_row([id], |row| {
            columns
                .iter()
                .enumerate()
                .map(|(index, name)| Ok((name.clone(), row.get(index)?)))
                .collect()
        })
        .unwrap()
}

fn assert_foreign_keys_and_triggers(service: &StorageService) {
    let connection = service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    assert!(connection
        .db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)
        .unwrap());
}

fn parent_deletion_preserves_tombstoned_automations(graph_bound: bool, delete_project: bool) {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    // new_chat tasks require an actual model FK, unlike an inherited existing_chat model.
    service
        .state
        .connection()
        .unwrap()
        .execute_batch(
            "INSERT INTO models (
                id, provider_model_id, display_name, normalized_display_name,
                supports_image, context_window_tokens, provider_profile_config_json,
                provider_connection_revision, provider_protocol_revision,
                input_price, cached_input_price, output_price,
                enabled, position, created_at, updated_at
             ) VALUES (
                'model-1', 'model-1', 'Model 1', 'model 1',
                0, 4096, '{}', 'provider-connection-v1:test', 'provider-protocol-v1:test',
                '0', '', '0', 1, 0, 1, 1
             );",
        )
        .unwrap();
    for (id, project_id) in [
        ("conversation-deleted", "project-1"),
        ("conversation-unrelated", "project-2"),
    ] {
        service
            .save_conversation(conversation(id, Some(project_id), &format!("message-{id}")))
            .unwrap();
    }
    if graph_bound {
        bind_agent_root(&service, "agent-deleted", "conversation-deleted");
        let mut child = conversation("conversation-child", Some("project-1"), "unused");
        child.messages.clear();
        service.save_conversation(child).unwrap();
        service
            .create_agent_node(&crate::CreateAgentNodeInput {
                agent_id: "agent-child".to_string(),
                root_agent_id: "agent-deleted".to_string(),
                parent_agent_id: "agent-deleted".to_string(),
                conversation_id: "conversation-child".to_string(),
                creation_request_id: "create-agent-child".to_string(),
                task_name: "child".to_string(),
                task_path: "/root/child".to_string(),
                template_snapshot: None,
                model_snapshot: crate::AgentModelSelectionSnapshot {
                    model_config_id: "model-1".to_string(),
                    display_name: "Model 1".to_string(),
                    supports_image: false,
                    effective_context_window_tokens: 4096,
                    model_settings_configuration_revision: "settings-1".to_string(),
                    provider_connection_revision: "provider-connection-v1:test".to_string(),
                    provider_protocol_revision: "provider-protocol-v1:test".to_string(),
                },
            })
            .unwrap();
    }
    service
        .save_composer_draft(composer_draft(
            "conversation-deleted",
            Some("project-1"),
            "Preserve on rollback",
        ))
        .unwrap();

    let mut targets = vec![create_task(
        &service,
        "task-conversation",
        "project-1",
        Some("conversation-deleted"),
    )];
    if delete_project {
        targets.push(create_task(&service, "task-project", "project-1", None));
    }
    if graph_bound {
        targets.push(create_task(
            &service,
            "task-child",
            "project-1",
            Some("conversation-child"),
        ));
    }
    let histories = targets
        .iter()
        .map(|task| tombstone_with_history(&service, task))
        .collect::<Vec<_>>();
    let unrelated_active = create_task(
        &service,
        "task-unrelated-active",
        "project-2",
        Some("conversation-unrelated"),
    );
    let unrelated_deleted = create_task(
        &service,
        "task-unrelated-deleted",
        "project-2",
        Some("conversation-unrelated"),
    );
    let unrelated_history = tombstone_with_history(&service, &unrelated_deleted);
    let before = targets
        .iter()
        .map(|task| stored_task(&service, &task.id))
        .collect::<Vec<_>>();
    let unrelated_before = stored_task(&service, &unrelated_deleted.id);
    let event_sequence = service.latest_automation_event_sequence().unwrap();
    let notification_sequence = service.latest_notification_change_sequence().unwrap();
    let notification_count = service
        .state
        .connection()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM notification_events", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    let conversations_before = service.load_conversation("conversation-deleted").unwrap();
    let drafts_before = service.load_composer_drafts().unwrap();

    let delete_parent = || {
        if delete_project {
            service.delete_project("project-1")
        } else {
            service.delete_conversation("conversation-deleted")
        }
    };
    // A real FK restriction also fails the Agent tree path, which disables SQL triggers.
    // Removing the guard then retries exactly the same delete against the rolled-back state.
    let (parent_table, parent_id) = if delete_project {
        ("projects", "project-1")
    } else {
        ("conversations", "conversation-deleted")
    };
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE test_parent_deletion_guard (
                    parent_id TEXT REFERENCES {parent_table}(id) ON DELETE RESTRICT
                 );"
            ))
            .unwrap();
        connection
            .execute(
                "INSERT INTO test_parent_deletion_guard (parent_id) VALUES (?1)",
                [parent_id],
            )
            .unwrap();
    }
    let guarded_error = delete_parent().unwrap_err();
    for (task, original) in targets.iter().zip(&before) {
        assert_eq!(&stored_task(&service, &task.id), original);
    }
    for history in histories.iter().chain([&unrelated_history]) {
        assert_eq!(
            service.get_automation_run(&history.id).unwrap().as_ref(),
            Some(history)
        );
    }
    assert_eq!(
        serde_json::to_value(service.load_conversation("conversation-deleted").unwrap()).unwrap(),
        serde_json::to_value(conversations_before).unwrap()
    );
    assert_eq!(
        serde_json::to_value(service.load_composer_drafts().unwrap()).unwrap(),
        serde_json::to_value(drafts_before).unwrap()
    );
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-1"));
    if graph_bound {
        assert!(service.get_agent_node("agent-deleted").unwrap().is_some());
        assert!(service.get_agent_node("agent-child").unwrap().is_some());
        assert!(service
            .load_conversation("conversation-child")
            .unwrap()
            .is_some());
    }
    assert_eq!(
        service.latest_automation_event_sequence().unwrap(),
        event_sequence
    );
    assert_eq!(
        service.latest_notification_change_sequence().unwrap(),
        notification_sequence
    );
    assert_foreign_keys_and_triggers(&service);
    service
        .state
        .connection()
        .unwrap()
        .execute_batch("DROP TABLE test_parent_deletion_guard")
        .unwrap();

    delete_parent().unwrap();
    assert!(
        guarded_error.to_ascii_lowercase().contains("foreign key"),
        "{guarded_error}"
    );
    for ((task, original), history) in targets.iter().zip(before).zip(&histories) {
        let mut after = stored_task(&service, &task.id);
        assert_eq!(after["health_state"], Value::Text("blocked".to_string()));
        assert_eq!(
            after["blocked_code"],
            Value::Text(
                if task.config.project_binding_kind == "project" {
                    "project_missing"
                } else {
                    "target_missing"
                }
                .to_string()
            )
        );
        assert!(matches!(&after["blocked_message"], Value::Text(message) if !message.is_empty()));
        assert_eq!(after["target_conversation_id"], Value::Null);
        if delete_project {
            assert_eq!(after["project_id"], Value::Null);
        }
        assert_eq!(after["next_run_at"], Value::Null);
        assert_ne!(after["deleted_at"], Value::Null);
        let mut preserved = original;
        for field in [
            "health_state",
            "blocked_code",
            "blocked_message",
            "target_conversation_id",
            "project_id",
        ] {
            after.remove(field);
            preserved.remove(field);
        }
        // Includes deleted_at, status, schedule, attention and all six identity snapshots.
        assert_eq!(after, preserved);
        assert!(service.get_automation(&task.id).unwrap().is_none());
        assert_eq!(
            service.get_automation_run(&history.id).unwrap().as_ref(),
            Some(history)
        );
        assert_eq!(
            service
                .list_automation_runs(&task.id, None, 10)
                .unwrap()
                .items,
            vec![history.clone()]
        );
    }
    assert_eq!(
        service.get_automation(&unrelated_active.id).unwrap(),
        Some(unrelated_active.clone())
    );
    assert_eq!(
        stored_task(&service, &unrelated_deleted.id),
        unrelated_before
    );
    assert_eq!(
        service.get_automation_run(&unrelated_history.id).unwrap(),
        Some(unrelated_history)
    );
    assert_eq!(
        service
            .list_automations(&AutomationListInput {
                status: None,
                query: None,
                cursor: None,
                limit: 10,
            })
            .unwrap()
            .items,
        vec![unrelated_active.clone()]
    );
    assert_eq!(
        service.list_due_automations(i64::MAX, 10).unwrap(),
        vec![unrelated_active]
    );
    assert!(service
        .claim_ready_automation_runs(i64::MAX - 60_000, 30_000, 10)
        .unwrap()
        .is_empty());
    assert_eq!(
        service.latest_automation_event_sequence().unwrap(),
        event_sequence
    );
    assert_eq!(
        service.latest_notification_change_sequence().unwrap(),
        notification_sequence
    );
    assert_eq!(
        service
            .state
            .connection()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM notification_events", [], |row| row
                .get::<_, i64>(0),)
            .unwrap(),
        notification_count
    );
    assert!(service
        .load_conversation("conversation-deleted")
        .unwrap()
        .is_none());
    assert!(service
        .load_conversation("conversation-unrelated")
        .unwrap()
        .is_some());
    assert!(service.load_composer_drafts().unwrap().is_empty());
    assert_eq!(
        service
            .load_projects()
            .unwrap()
            .iter()
            .any(|project| project.id == "project-1"),
        !delete_project
    );
    assert!(service
        .load_projects()
        .unwrap()
        .iter()
        .any(|project| project.id == "project-2"));
    if graph_bound {
        assert!(service.get_agent_node("agent-deleted").unwrap().is_none());
        assert!(service.get_agent_node("agent-child").unwrap().is_none());
        assert!(service
            .load_conversation("conversation-child")
            .unwrap()
            .is_none());
    }
    assert_foreign_keys_and_triggers(&service);
}

#[test]
fn conversation_deletion_preserves_tombstoned_automations() {
    parent_deletion_preserves_tombstoned_automations(false, false);
}

#[test]
fn project_deletion_preserves_tombstoned_automations() {
    parent_deletion_preserves_tombstoned_automations(false, true);
}

#[test]
fn graph_bound_conversation_deletion_preserves_tombstoned_automations() {
    parent_deletion_preserves_tombstoned_automations(true, false);
}

#[test]
fn graph_bound_project_deletion_preserves_tombstoned_automations() {
    parent_deletion_preserves_tombstoned_automations(true, true);
}
