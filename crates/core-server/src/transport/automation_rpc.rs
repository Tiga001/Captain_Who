use super::*;
use crate::application::automation::{AutomationService, AutomationServiceError};
use mycopilot_protocol_rs::{
    AutomationAttentionAcknowledgeInputDto, AutomationAttentionSummaryInputDto,
    AutomationCreateInputDto, AutomationDeleteInputDto, AutomationGetInputDto,
    AutomationListInputDto, AutomationRunNowInputDto, AutomationRunsListInputDto,
    AutomationSetEnabledInputDto, AutomationUpdateInputDto, AUTOMATION_ERROR_CODE,
};

pub(crate) fn is_automation_request_method(method: &str) -> bool {
    matches!(
        method,
        mycopilot_protocol_rs::AUTOMATION_LIST_METHOD
            | mycopilot_protocol_rs::AUTOMATION_GET_METHOD
            | mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD
            | mycopilot_protocol_rs::AUTOMATION_UPDATE_METHOD
            | mycopilot_protocol_rs::AUTOMATION_SET_ENABLED_METHOD
            | mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD
            | mycopilot_protocol_rs::AUTOMATION_DELETE_METHOD
            | mycopilot_protocol_rs::AUTOMATION_RUNS_LIST_METHOD
            | mycopilot_protocol_rs::AUTOMATION_ATTENTION_SUMMARY_METHOD
            | mycopilot_protocol_rs::AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD
    )
}

pub(crate) fn handle_automation_request(
    storage: &StorageService,
    request: JsonRpcRequest,
) -> Value {
    let id = request.id;
    let service = AutomationService::new(storage);
    match request.method.as_str() {
        mycopilot_protocol_rs::AUTOMATION_LIST_METHOD => {
            parse_and_run(id, request.params, |input| service.list(input))
        }
        mycopilot_protocol_rs::AUTOMATION_GET_METHOD => {
            parse_and_run(id, request.params, |input| service.get(input))
        }
        mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD => {
            parse_and_run(id, request.params, |input| service.create(input))
        }
        mycopilot_protocol_rs::AUTOMATION_UPDATE_METHOD => {
            parse_and_run(id, request.params, |input| service.update(input))
        }
        mycopilot_protocol_rs::AUTOMATION_SET_ENABLED_METHOD => {
            parse_and_run(id, request.params, |input| service.set_enabled(input))
        }
        mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD => {
            parse_and_run(id, request.params, |input| service.run_now(input))
        }
        mycopilot_protocol_rs::AUTOMATION_DELETE_METHOD => {
            parse_and_run(id, request.params, |input| service.delete(input))
        }
        mycopilot_protocol_rs::AUTOMATION_RUNS_LIST_METHOD => {
            parse_and_run(id, request.params, |input| service.list_runs(input))
        }
        mycopilot_protocol_rs::AUTOMATION_ATTENTION_SUMMARY_METHOD => {
            parse_and_run(id, request.params, |input| service.attention_summary(input))
        }
        mycopilot_protocol_rs::AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD => {
            parse_and_run(id, request.params, |input| {
                service.acknowledge_attention(input)
            })
        }
        _ => response_error(Some(id), -32601, "Method not found"),
    }
}

fn parse_and_run<I, O>(
    id: JsonRpcId,
    params: Option<Value>,
    operation: impl FnOnce(I) -> Result<O, AutomationServiceError>,
) -> Value
where
    I: for<'de> Deserialize<'de>,
    O: Serialize,
{
    let input = match parse_params::<I>(params) {
        Ok(input) => input,
        Err(message) => return response_error(Some(id), -32602, message),
    };
    match operation(input) {
        Ok(output) => response_success(id, output),
        Err(error) => automation_error_response(id, error),
    }
}

fn automation_error_response(id: JsonRpcId, error: AutomationServiceError) -> Value {
    serde_json::to_value(error_with_data(
        Some(id),
        i64::from(AUTOMATION_ERROR_CODE),
        error.message(),
        serde_json::to_value(error.data()).expect("automation error data must serialize"),
    ))
    .expect("automation JSON-RPC error response must serialize")
}

pub(crate) fn automation_internal_error_response(id: JsonRpcId) -> Value {
    automation_error_response(
        id,
        AutomationServiceError::internal("automation worker failed"),
    )
}

// Keep the request DTOs referenced here explicitly. Besides documenting the method map this makes
// accidental method/input swaps a compile-time failure instead of a runtime deserialization bug.
const _: fn(AutomationListInputDto) = |_| {};
const _: fn(AutomationGetInputDto) = |_| {};
const _: fn(AutomationCreateInputDto) = |_| {};
const _: fn(AutomationUpdateInputDto) = |_| {};
const _: fn(AutomationSetEnabledInputDto) = |_| {};
const _: fn(AutomationRunNowInputDto) = |_| {};
const _: fn(AutomationDeleteInputDto) = |_| {};
const _: fn(AutomationRunsListInputDto) = |_| {};
const _: fn(AutomationAttentionSummaryInputDto) = |_| {};
const _: fn(AutomationAttentionAcknowledgeInputDto) = |_| {};

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::models::{
        ChatConversationRecord, ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use mycopilot_core::{ProviderProfileConfig, ProviderProtocolDialect};
    use mycopilot_protocol_rs::{
        AutomationDestinationInputDto, AutomationNotificationPolicyDto,
        AutomationPermissionModeDto, AutomationProjectBindingDto, AutomationRunDto,
        AutomationScheduleInputDto, AutomationStatusDto, AutomationTaskDto,
        AUTOMATION_PERMISSION_MODE_VERSION, AUTOMATION_SCHEMA_VERSION,
    };

    fn request<I: Serialize>(storage: &StorageService, method: &str, input: &I) -> Value {
        handle_automation_request(
            storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(9),
                method: method.to_string(),
                params: Some(serde_json::to_value(input).unwrap()),
            },
        )
    }

    fn model_settings() -> ModelSettingsRecord {
        ModelSettingsRecord {
            api_url: "https://provider.example/v1/chat/completions".to_string(),
            api_token: "test-token".to_string(),
            search_mode: "disabled".to_string(),
            tavily_api_key: String::new(),
            models: vec![ModelConfigRecord {
                id: "model-automation".to_string(),
                display_name: "Automation Model".to_string(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(64_000),
                provider_profile_config: ProviderProfileConfig::generic_for_dialect(
                    ProviderProtocolDialect::OpenAiChatCompletions,
                ),
                input_price: "0".to_string(),
                cached_input_price: String::new(),
                output_price: "0".to_string(),
                enabled: true,
            }],
        }
    }

    fn create_input(project_id: &str, request_id: &str) -> AutomationCreateInputDto {
        AutomationCreateInputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            request_id: request_id.to_string(),
            status: AutomationStatusDto::Active,
            title: "Daily brief".to_string(),
            prompt: "Summarize status.\nInclude blockers.".to_string(),
            destination: AutomationDestinationInputDto::NewChat {
                project_binding: AutomationProjectBindingDto::Project,
                project_id: Some(project_id.to_string()),
                model_id: "model-automation".to_string(),
            },
            permission_mode: AutomationPermissionModeDto::Default,
            permission_mode_version: AUTOMATION_PERMISSION_MODE_VERSION,
            schedule: AutomationScheduleInputDto::Daily {
                time_minutes: 540,
                anchor_at: mycopilot_core::storage::now_ms().saturating_sub(60_000),
                timezone: "Asia/Shanghai".to_string(),
            },
            notification_policy: AutomationNotificationPolicyDto::AllRuns,
        }
    }

    #[test]
    fn strict_params_reject_unknown_fields() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        let response = handle_automation_request(
            &storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(1),
                method: mycopilot_protocol_rs::AUTOMATION_LIST_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "schemaVersion": 1,
                    "status": null,
                    "query": null,
                    "cursor": null,
                    "limit": 20,
                    "unexpected": true
                })),
            },
        );
        assert_eq!(response["error"]["code"], -32602);
    }

    #[test]
    fn empty_list_round_trip_uses_public_schema() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        let response = handle_automation_request(
            &storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(2),
                method: mycopilot_protocol_rs::AUTOMATION_LIST_METHOD.to_string(),
                params: Some(serde_json::json!({
                    "schemaVersion": 1,
                    "status": null,
                    "query": null,
                    "cursor": null,
                    "limit": 20
                })),
            },
        );
        assert_eq!(response["result"]["schemaVersion"], 1);
        assert_eq!(response["result"]["tasks"], serde_json::json!([]));
        assert_eq!(response["result"]["counts"]["all"], 0);
    }

    #[tokio::test]
    async fn durable_rpc_lifecycle_enforces_idempotency_cas_and_active_run_admission() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let storage =
            Arc::new(StorageService::open(&temporary.path().join("storage.sqlite")).unwrap());
        storage.save_model_settings(model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-automation".to_string(),
                name: "Automation Project".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();

        let create = create_input("project-automation", "create-request-1");
        let created_response = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
            &create,
        );
        let created: AutomationTaskDto =
            serde_json::from_value(created_response["result"].clone()).unwrap();
        assert_eq!(created.revision, 1);
        assert!(created.next_run_at.is_some());
        assert_eq!(
            created.schedule_summary,
            "Every day at 09:00 (Asia/Shanghai)"
        );

        let retried: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(retried.automation_id, created.automation_id);

        let get = AutomationGetInputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            automation_id: created.automation_id.clone(),
        };
        assert_eq!(
            request(&storage, mycopilot_protocol_rs::AUTOMATION_GET_METHOD, &get)["result"]
                ["automationId"],
            created.automation_id
        );
        let listed = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_LIST_METHOD,
            &AutomationListInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                status: Some(AutomationStatusDto::Active),
                query: Some("daily".to_string()),
                cursor: None,
                limit: 20,
            },
        );
        assert_eq!(listed["result"]["tasks"].as_array().unwrap().len(), 1);

        let conflict = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_UPDATE_METHOD,
            &AutomationUpdateInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: created.automation_id.clone(),
                expected_revision: 0,
                title: create.title.clone(),
                prompt: create.prompt.clone(),
                destination: create.destination.clone(),
                permission_mode: create.permission_mode,
                permission_mode_version: create.permission_mode_version,
                schedule: create.schedule.clone(),
                notification_policy: create.notification_policy,
            },
        );
        assert_eq!(conflict["error"]["code"], AUTOMATION_ERROR_CODE);
        assert_eq!(conflict["error"]["data"]["code"], "revision_conflict");
        assert_eq!(conflict["error"]["data"]["currentRevision"], 1);

        let updated: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_UPDATE_METHOD,
                &AutomationUpdateInputDto {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    automation_id: created.automation_id.clone(),
                    expected_revision: 1,
                    title: "Updated daily brief".to_string(),
                    prompt: create.prompt.clone(),
                    destination: create.destination.clone(),
                    permission_mode: create.permission_mode,
                    permission_mode_version: create.permission_mode_version,
                    schedule: create.schedule.clone(),
                    notification_policy: create.notification_policy,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(updated.revision, 2);

        let paused: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_SET_ENABLED_METHOD,
                &AutomationSetEnabledInputDto {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    automation_id: created.automation_id.clone(),
                    expected_revision: 2,
                    enabled: false,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(paused.status, AutomationStatusDto::Paused);
        assert_eq!(paused.next_run_at, None);

        let resumed: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_SET_ENABLED_METHOD,
                &AutomationSetEnabledInputDto {
                    schema_version: AUTOMATION_SCHEMA_VERSION,
                    automation_id: created.automation_id.clone(),
                    expected_revision: paused.revision,
                    enabled: true,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(resumed.status, AutomationStatusDto::Active);
        assert!(resumed.next_run_at.is_some());

        let run_input = AutomationRunNowInputDto {
            schema_version: AUTOMATION_SCHEMA_VERSION,
            automation_id: created.automation_id.clone(),
            request_id: "manual-request-1".to_string(),
        };
        let run: AutomationRunDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
                &run_input,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(
            run.status,
            mycopilot_protocol_rs::AutomationRunStatusDto::Queued
        );
        let retried_run: AutomationRunDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
                &run_input,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(retried_run.run_id, run.run_id);

        let second: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create_input("project-automation", "create-request-2"),
            )["result"]
                .clone(),
        )
        .unwrap();
        let cross_task_replay = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
            &AutomationRunNowInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: second.automation_id,
                request_id: run_input.request_id.clone(),
            },
        );
        assert_eq!(cross_task_replay["error"]["data"]["code"], "validation");
        assert_eq!(cross_task_replay["error"]["data"]["field"], "requestId");

        let active_conflict = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
            &AutomationRunNowInputDto {
                request_id: "manual-request-2".to_string(),
                ..run_input.clone()
            },
        );
        assert_eq!(
            active_conflict["error"]["data"]["code"],
            "run_already_active"
        );

        let runs = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUNS_LIST_METHOD,
            &AutomationRunsListInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: created.automation_id.clone(),
                cursor: None,
                limit: 20,
            },
        );
        assert_eq!(runs["result"]["runs"].as_array().unwrap().len(), 1);
        assert_eq!(runs["result"]["runs"][0]["status"], "queued");

        let attention = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_ATTENTION_SUMMARY_METHOD,
            &AutomationAttentionSummaryInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                cursor: None,
                limit: 20,
            },
        );
        assert_eq!(attention["result"]["unreadCount"], 0);
        assert_eq!(attention["result"]["items"], serde_json::json!([]));
        let missing_attention = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_ATTENTION_ACKNOWLEDGE_METHOD,
            &AutomationAttentionAcknowledgeInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                attention_id: "task:missing".to_string(),
            },
        );
        assert_eq!(missing_attention["error"]["data"]["code"], "not_found");

        let current: AutomationTaskDto = serde_json::from_value(
            request(&storage, mycopilot_protocol_rs::AUTOMATION_GET_METHOD, &get)["result"].clone(),
        )
        .unwrap();
        let deleted = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_DELETE_METHOD,
            &AutomationDeleteInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: created.automation_id.clone(),
                expected_revision: current.revision,
            },
        );
        assert_eq!(deleted["result"]["automationId"], created.automation_id);
        assert_eq!(
            request(&storage, mycopilot_protocol_rs::AUTOMATION_GET_METHOD, &get)["error"]["data"]
                ["code"],
            "not_found"
        );
        assert_eq!(storage.load_projects().unwrap().len(), 1);
        let events = storage.list_automation_events_after(0, 100).unwrap();
        assert!(events.iter().any(|event| event.event_kind == "created"));
        assert!(events.iter().any(|event| event.event_kind == "updated"));
        assert!(events.iter().any(|event| event.event_kind == "run_updated"));
        assert!(events.iter().any(|event| event.event_kind == "deleted"));

        let (outbound, mut notifications) = tokio::sync::mpsc::unbounded_channel();
        let notifier = tokio::spawn(run_automation_event_notifier(
            Arc::clone(&storage),
            outbound,
            0,
        ));
        let notification =
            tokio::time::timeout(std::time::Duration::from_secs(1), notifications.recv())
                .await
                .unwrap()
                .unwrap();
        notifier.abort();
        let _ = notifier.await;
        assert_eq!(
            notification["method"],
            mycopilot_protocol_rs::AUTOMATION_EVENT_NOTIFICATION_METHOD
        );
        assert_eq!(notification["params"]["kind"], "created");
    }

    #[test]
    fn run_now_honors_current_permission_mode_revocation() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-automation".to_string(),
                name: "Automation Project".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let mut preferences = storage.load_ui_preferences().unwrap();
        preferences.full_permission_enabled = true;
        storage.save_ui_preferences(preferences).unwrap();
        let mut create = create_input("project-automation", "create-full-request");
        create.permission_mode = AutomationPermissionModeDto::Full;
        let task: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create,
            )["result"]
                .clone(),
        )
        .unwrap();

        let mut preferences = storage.load_ui_preferences().unwrap();
        preferences.full_permission_enabled = false;
        storage.save_ui_preferences(preferences).unwrap();
        let response = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
            &AutomationRunNowInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: task.automation_id,
                request_id: "manual-after-revocation".to_string(),
            },
        );
        assert_eq!(response["error"]["data"]["code"], "permission_disabled");
    }

    #[test]
    fn run_now_persists_a_block_when_the_project_path_disappears() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(model_settings()).unwrap();
        storage
            .save_project(ProjectRecord {
                id: "project-automation".to_string(),
                name: "Automation Project".to_string(),
                path: Some(workspace.to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let task: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create_input("project-automation", "create-path-request"),
            )["result"]
                .clone(),
        )
        .unwrap();
        std::fs::remove_dir(&workspace).unwrap();

        let response = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUN_NOW_METHOD,
            &AutomationRunNowInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: task.automation_id.clone(),
                request_id: "manual-missing-path".to_string(),
            },
        );
        assert_eq!(response["error"]["data"]["code"], "target_invalid");

        let current = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_GET_METHOD,
            &AutomationGetInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: task.automation_id.clone(),
            },
        );
        assert_eq!(current["result"]["health"]["state"], "blocked");
        assert_eq!(current["result"]["health"]["code"], "project_path_missing");
        assert_eq!(current["result"]["nextRunAt"], Value::Null);
        assert_eq!(
            current["result"]["attention"]["kind"],
            "configuration_blocked"
        );
        assert!(storage
            .list_automation_events_after(0, 20)
            .unwrap()
            .iter()
            .any(|event| event.event_kind == "attention_changed"));
        let runs = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_RUNS_LIST_METHOD,
            &AutomationRunsListInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: task.automation_id,
                cursor: None,
                limit: 20,
            },
        );
        assert!(runs["result"]["runs"].as_array().unwrap().is_empty());
    }

    #[test]
    fn existing_chat_create_rejects_its_disabled_inherited_model() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        let mut settings = model_settings();
        settings.models[0].enabled = false;
        storage.save_model_settings(settings).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-disabled-model".to_string(),
                project_id: None,
                model_id: Some("model-automation".to_string()),
                title: "Disabled model conversation".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let mut input = create_input("unused-project", "create-existing-disabled");
        input.destination = AutomationDestinationInputDto::ExistingChat {
            conversation_id: "conversation-disabled-model".to_string(),
        };
        input.notification_policy = AutomationNotificationPolicyDto::ImportantUpdates;

        let response = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
            &input,
        );
        assert_eq!(response["error"]["data"]["code"], "target_invalid");
        assert_eq!(
            response["error"]["data"]["field"],
            serde_json::json!("destination")
        );
    }
}
