use super::*;
use crate::storage::config_repository;

fn model_settings(include_deleted_model: bool) -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://example.invalid/v1".into(),
        api_token: String::new(),
        search_mode: "disabled".into(),
        tavily_api_key: String::new(),
        models: ["model-a", "model-b"]
            .into_iter()
            .filter(|id| include_deleted_model || *id != "model-a")
            .map(|id| ModelConfigRecord {
                id: id.into(),
                provider_model_id: id.into(),
                display_name: id.into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(4096),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled: true,
            })
            .collect(),
    }
}

fn save_models(connection: &mut Connection, include_deleted_model: bool) -> rusqlite::Result<()> {
    config_repository::save_model_settings(
        connection,
        config_repository::credential_free_fixture(model_settings(include_deleted_model)),
    )
}

#[test]
fn model_settings_delete_preserves_tombstoned_tasks_without_attention() {
    for already_blocked in [false, true] {
        let mut connection = connection();
        save_models(&mut connection, true).unwrap();
        let mut task = create(&mut connection, "automation-a", "request-a");
        if already_blocked {
            task = match block_automation(
                &mut connection,
                &task.id,
                task.revision,
                "permission_disabled",
                "The selected permission mode is disabled.",
            )
            .unwrap()
            {
                AutomationCompareAndSetOutcome::Updated(record) => record,
                outcome => panic!("unexpected block: {outcome:?}"),
            };
        }
        let deleted = match tombstone_automation(&mut connection, &task.id, task.revision).unwrap()
        {
            AutomationCompareAndSetOutcome::Updated(record) => record,
            outcome => panic!("unexpected deletion: {outcome:?}"),
        };
        let mut other_config = config();
        other_config.model_id = Some("model-b".into());
        let other = match create_automation(
            &mut connection,
            &NewAutomationRecord {
                id: "automation-b".into(),
                create_request_id: "request-b".into(),
                status: StoredAutomationStatus::Active,
                config: other_config,
            },
        )
        .unwrap()
        {
            AutomationCreateOutcome::Created(record) => record,
            outcome => panic!("unexpected creation: {outcome:?}"),
        };
        let events = list_automation_events_after(&connection, 0, 100).unwrap();
        let notifications = application_notifications(&connection);

        save_models(&mut connection, false).unwrap();

        let mut expected = deleted;
        expected.config.model_id = None;
        if !already_blocked {
            expected.config.health_state = "blocked".into();
            expected.config.blocked_code = Some("model_missing".into());
            expected.config.blocked_message = Some("The selected model no longer exists.".into());
        }
        assert_eq!(
            query_automation(&connection, &task.id, true).unwrap(),
            Some(expected)
        );
        assert!(get_automation(&connection, &task.id).unwrap().is_none());
        assert_eq!(get_automation(&connection, &other.id).unwrap(), Some(other));
        assert_eq!(
            list_automation_events_after(&connection, 0, 100).unwrap(),
            events
        );
        assert_eq!(application_notifications(&connection), notifications);
        assert!(!connection
            .prepare("PRAGMA foreign_key_check")
            .unwrap()
            .exists([])
            .unwrap());
    }
}

#[test]
fn model_settings_delete_rolls_back_tombstone_preparation_on_fk_failure() {
    let mut connection = connection();
    save_models(&mut connection, true).unwrap();
    let task = create(&mut connection, "automation-a", "request-a");
    tombstone_automation(&mut connection, &task.id, task.revision).unwrap();
    let deleted = query_automation(&connection, &task.id, true).unwrap();
    let settings = config_repository::load_model_settings_snapshot(&mut connection)
        .unwrap()
        .unwrap();
    let events = list_automation_events_after(&connection, 0, 100).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE retained_model_reference (
             model_id TEXT REFERENCES models(id) ON DELETE RESTRICT
         );
         INSERT INTO retained_model_reference VALUES ('model-a');",
        )
        .unwrap();

    assert!(save_models(&mut connection, false).is_err());

    assert_eq!(
        query_automation(&connection, &task.id, true).unwrap(),
        deleted
    );
    let restored = config_repository::load_model_settings_snapshot(&mut connection)
        .unwrap()
        .unwrap();
    assert_eq!(restored.settings, settings.settings);
    assert_eq!(
        restored.configuration_revision,
        settings.configuration_revision
    );
    assert_eq!(
        restored.provider_connection_revisions,
        settings.provider_connection_revisions
    );
    assert_eq!(
        restored.provider_protocol_revisions,
        settings.provider_protocol_revisions
    );
    assert_eq!(
        restored.search_connection_revision,
        settings.search_connection_revision
    );
    assert_eq!(
        list_automation_events_after(&connection, 0, 100).unwrap(),
        events
    );
    connection
        .execute("DELETE FROM retained_model_reference", [])
        .unwrap();
    save_models(&mut connection, false).unwrap();
}
