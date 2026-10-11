use super::*;

mod admission_leases;
use crate::storage::models::{ModelConfigRecord, ModelSettingsRecord};
use std::sync::{mpsc, Arc};
use std::time::Duration;

fn fixture() -> (tempfile::TempDir, Arc<StorageService>) {
    let directory = tempfile::tempdir().unwrap();
    let service = StorageService::open_with_model_credentials(
        &directory.path().join("automations.sqlite"),
        Arc::new(crate::image_generation::InMemoryCredentialStore::default()),
    )
    .unwrap();
    service
        .save_model_settings(ModelSettingsRecord {
            api_url: "https://fixture.invalid/v1".into(),
            api_token: "fixture-token".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "fixture-model".into(),
                provider_model_id: "fixture-model".into(),
                display_name: "Fixture Model".into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: crate::ProviderProfileConfig::generic_for_dialect(
                    crate::ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled: true,
            }],
        })
        .unwrap();
    (directory, Arc::new(service))
}

fn input(id: &str) -> NewAutomationRecord {
    NewAutomationRecord {
        id: id.into(),
        create_request_id: format!("request-{id}"),
        status: StoredAutomationStatus::Active,
        config: AutomationConfigRecord {
            title: "Scheduled task".into(),
            prompt: "Synthetic test only".into(),
            health_state: "ok".into(),
            blocked_code: None,
            blocked_message: None,
            destination_kind: "new_chat".into(),
            target_conversation_id: None,
            project_binding_kind: "none".into(),
            project_id: None,
            model_id: Some("fixture-model".into()),
            permission_mode: "default".into(),
            permission_mode_version: 2,
            permissions_json: serde_json::to_string(&crate::AgentPermissions::default()).unwrap(),
            reasoning_json: Some(r#"{"source":"model_config","effort":"high"}"#.into()),
            schedule_kind: "daily".into(),
            schedule_json: r#"{"kind":"daily","timeMinutes":540,"timezone":"UTC"}"#.into(),
            rrule: "FREQ=DAILY;INTERVAL=1".into(),
            timezone: "UTC".into(),
            anchor_at: 1,
            next_run_at: Some(2),
            notification_policy: "all_runs".into(),
            target_project_snapshot: None,
            target_conversation_snapshot: None,
            target_model_snapshot: None,
            target_project_id_snapshot: None,
            target_conversation_id_snapshot: None,
            target_model_id_snapshot: None,
        },
    }
}

fn list_input() -> AutomationListInput {
    AutomationListInput {
        status: None,
        query: None,
        cursor: None,
        limit: 100,
    }
}

#[test]
fn automation_reads_use_wal_readers_while_writer_is_busy() {
    let (_directory, service) = fixture();
    service.create_automation(&input("task")).unwrap();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let writer_service = service.clone();
    let writer = std::thread::spawn(move || {
        writer_service
            .state
            .write_sync(1024, StorageWriteClass::Normal, move |connection| {
                let transaction = connection
                    .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                    .unwrap();
                transaction
                    .execute(
                        "UPDATE automations SET title='uncommitted' WHERE id='task'",
                        [],
                    )
                    .unwrap();
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
                transaction.commit().unwrap();
            })
            .unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let read_service = service.clone();
    let (done, result) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        done.send(read_service.list_automations(&list_input()))
            .unwrap();
    });
    let page = result.recv_timeout(Duration::from_secs(2));
    release.send(()).unwrap();
    writer.join().unwrap();
    reader.join().unwrap();
    let page = page
        .expect("WAL reads must not queue behind the writer")
        .unwrap();
    assert_eq!(page.items[0].config.title, "Scheduled task");
    assert_eq!(page.total_count, 1);
    assert_eq!(page.last_event_sequence, 1);
    assert_eq!(
        service
            .get_automation("task")
            .unwrap()
            .unwrap()
            .config
            .title,
        "uncommitted"
    );
}

#[test]
fn automation_page_rows_counts_and_event_cursor_share_one_snapshot() {
    let (_directory, service) = fixture();
    service.create_automation(&input("task-0")).unwrap();
    let writer_service = service.clone();
    let writer = std::thread::spawn(move || {
        for index in 1..40 {
            writer_service
                .create_automation(&input(&format!("task-{index}")))
                .unwrap();
        }
    });
    for _ in 0..80 {
        let page = service.list_automations(&list_input()).unwrap();
        assert_eq!(page.items.len() as u64, page.total_count);
        assert_eq!(page.active_count, page.total_count);
        assert_eq!(page.paused_count, 0);
        assert_eq!(page.last_event_sequence as u64, page.total_count);
    }
    writer.join().unwrap();
    assert_eq!(
        service.list_automations(&list_input()).unwrap().total_count,
        40
    );
}

#[test]
fn automation_failed_domain_write_publishes_neither_row_nor_event() {
    let (_directory, service) = fixture();
    service
        .state
        .write_sync(1024, StorageWriteClass::Normal, |connection| {
            connection.execute_batch(
                "CREATE TRIGGER reject_automation_event BEFORE INSERT ON automation_events
             WHEN NEW.automation_id = 'bad'
             BEGIN SELECT RAISE(ABORT, 'injected event failure'); END;",
            )
        })
        .unwrap()
        .unwrap();
    assert!(service
        .create_automation(&input("bad"))
        .unwrap_err()
        .contains("injected event failure"));
    assert!(service.get_automation("bad").unwrap().is_none());
    assert_eq!(service.latest_automation_event_sequence().unwrap(), 0);
    service.create_automation(&input("good")).unwrap();
    assert!(service.get_automation("good").unwrap().is_some());
    assert_eq!(service.latest_automation_event_sequence().unwrap(), 1);
}

#[test]
fn production_automation_claim_lease_starts_after_waiting_for_writer() {
    let (_directory, service) = fixture();
    service.create_automation(&input("task")).unwrap();
    service
        .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
            id: "run".into(),
            automation_id: "task".into(),
            manual_request_id: "manual-request".into(),
            scheduled_for: 1,
            config_revision: 1,
            config_snapshot_json: "{}".into(),
            expected_revision: 1,
        })
        .unwrap();
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let writer_service = service.clone();
    let writer = std::thread::spawn(move || {
        writer_service
            .state
            .write_sync(1024, StorageWriteClass::Normal, move |_| {
                started.send(()).unwrap();
                wait.recv_timeout(Duration::from_secs(5)).unwrap();
            })
            .unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let claim_service = service.clone();
    let (done, result) = mpsc::channel();
    let claim = std::thread::spawn(move || {
        done.send(claim_service.claim_ready_automation_runs_now(30_000, 1))
            .unwrap();
    });
    assert!(matches!(
        result.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    let released_at = now_ms();
    release.send(()).unwrap();
    writer.join().unwrap();
    let runs = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    claim.join().unwrap();
    assert_eq!(runs.len(), 1);
    assert!(runs[0].admission_expires_at.unwrap() >= released_at + 30_000);
    // Deterministic callers retain explicit-time reclaim behavior.
    let expires_at = runs[0].admission_expires_at.unwrap();
    assert!(service
        .claim_ready_automation_runs(expires_at - 1, 30_000, 1)
        .unwrap()
        .is_empty());
    let reclaimed = service
        .claim_ready_automation_runs(expires_at, 30_000, 1)
        .unwrap();
    assert_eq!(reclaimed[0].admission_expires_at, Some(expires_at + 30_000));
    assert_ne!(reclaimed[0].admission_token, runs[0].admission_token);
}

#[test]
fn automation_admission_estimate_counts_config_text_and_snapshots() {
    let mut config = input("task").config;
    let before = automation_config_bytes(&config);
    config.prompt.push_str("long prompt");
    config.permissions_json.push_str("permissions");
    config
        .reasoning_json
        .as_mut()
        .unwrap()
        .push_str("reasoning");
    config.target_model_snapshot = Some("model snapshot".into());
    assert_eq!(
        automation_config_bytes(&config) - before,
        "long prompt".len() + "permissions".len() + "reasoning".len() + "model snapshot".len()
    );
}
