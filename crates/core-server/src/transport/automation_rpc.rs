use super::*;
use crate::application::automation::{
    AutomationNotificationService, AutomationSchedulerWake, AutomationService,
    AutomationServiceError,
};
use mycopilot_protocol_rs::{
    AutomationAttentionAcknowledgeInputDto, AutomationAttentionSummaryInputDto,
    AutomationCreateInputDto, AutomationDeleteInputDto, AutomationGetInputDto,
    AutomationListInputDto, AutomationNotificationAcknowledgeInputDto,
    AutomationNotificationReleaseInputDto, AutomationNotificationValidateInputDto,
    AutomationNotificationsClaimInputDto, AutomationRunNowInputDto, AutomationRunsListInputDto,
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
            | mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_CLAIM_METHOD
            | mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_VALIDATE_METHOD
            | mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_ACKNOWLEDGE_METHOD
            | mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_RELEASE_METHOD
    )
}

pub(crate) fn handle_automation_request(
    storage: &StorageService,
    request: JsonRpcRequest,
) -> Value {
    handle_automation_request_with_wake(storage, None, request)
}

pub(crate) fn handle_automation_request_with_wake(
    storage: &StorageService,
    scheduler_wake: Option<&AutomationSchedulerWake>,
    request: JsonRpcRequest,
) -> Value {
    let id = request.id;
    let service = scheduler_wake.map_or_else(
        || AutomationService::new(storage),
        |wake| AutomationService::with_scheduler_wake(storage, wake),
    );
    let notification_service = AutomationNotificationService::new(storage);
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
        mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_CLAIM_METHOD => {
            parse_and_run(id, request.params, |input| {
                notification_service.claim(input)
            })
        }
        mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_VALIDATE_METHOD => {
            parse_and_run(id, request.params, |input| {
                notification_service.validate(input)
            })
        }
        mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_ACKNOWLEDGE_METHOD => {
            parse_and_run(id, request.params, |input| {
                notification_service.acknowledge(input)
            })
        }
        mycopilot_protocol_rs::AUTOMATION_NOTIFICATIONS_RELEASE_METHOD => {
            parse_and_run(id, request.params, |input| {
                notification_service.release(input)
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
const _: fn(AutomationNotificationsClaimInputDto) = |_| {};
const _: fn(AutomationNotificationValidateInputDto) = |_| {};
const _: fn(AutomationNotificationAcknowledgeInputDto) = |_| {};
const _: fn(AutomationNotificationReleaseInputDto) = |_| {};

#[cfg(test)]
mod tests {
    use super::*;
    use mycopilot_core::storage::automation_repository::NewAutomationNotificationRecord;
    use mycopilot_core::storage::models::{
        ChatConversationRecord, ModelConfigRecord, ModelSettingsRecord, ProjectRecord,
    };
    use mycopilot_core::{
        AgentLifecycle, EnsureRootAgentInput, ProviderProfileConfig, ProviderProtocolDialect,
    };
    use mycopilot_protocol_rs::{
        AutomationDestinationInputDto, AutomationNotificationPolicyDto,
        AutomationPermissionModeDto, AutomationProjectBindingDto, AutomationRunDto,
        AutomationScheduleInputDto, AutomationStatusDto, AutomationTaskDto,
        NotificationBatchAcknowledgeInputDto, NotificationBatchAcknowledgeOutputDto,
        NotificationBatchReleaseInputDto, NotificationBatchReleaseOutputDto,
        NotificationBatchStatusDto, NotificationBatchValidateInputDto,
        NotificationBatchValidateOutputDto, NotificationBatchesClaimInputDto,
        NotificationBatchesClaimOutputDto, NotificationDeliveryDispositionDto,
        NotificationDeliveryErrorCodeDto, NotificationKindDto, NotificationPriorityDto,
        NotificationSoundLevelDto, AUTOMATION_PERMISSION_MODE_VERSION, AUTOMATION_SCHEMA_VERSION,
        NOTIFICATION_SCHEMA_VERSION,
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

    fn notification_request<I: Serialize>(
        storage: &StorageService,
        method: &str,
        input: &I,
    ) -> Value {
        super::super::notification_rpc::handle_notification_request(
            storage,
            JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                id: JsonRpcId::Number(10),
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
    fn host_notification_rpc_claims_and_acknowledges_the_durable_outbox() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(model_settings()).unwrap();
        let project_id = "project-notification".to_string();
        let project = ProjectRecord {
            id: project_id.clone(),
            name: "Notification project".to_string(),
            path: Some(temporary.path().to_string_lossy().to_string()),
            created_at: mycopilot_core::storage::now_ms(),
            pinned_at: None,
        };
        storage.save_project(project).unwrap();
        let created: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create_input(&project_id, "request-notification"),
            )["result"]
                .clone(),
        )
        .unwrap();
        let created_at = mycopilot_core::storage::now_ms();
        storage
            .enqueue_automation_notification(&NewAutomationNotificationRecord {
                automation_id: created.automation_id.clone(),
                automation_run_id: None,
                resource_revision: i64::try_from(created.revision).unwrap(),
                notification_kind: "configuration_blocked".to_string(),
                title: "Scheduled task needs attention".to_string(),
                body: "Select a valid target.".to_string(),
                created_at,
            })
            .unwrap();

        let claim_input = NotificationBatchesClaimInputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            claim_token: "host-claim-1".to_string(),
            lease_duration_ms: 60_000,
            limit: 10,
        };
        let claimed: NotificationBatchesClaimOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCHES_CLAIM_METHOD,
                &claim_input,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(claimed.claim_token, claim_input.claim_token);
        assert_eq!(claimed.batches.len(), 1);
        assert_eq!(claimed.batches[0].items.len(), 1);
        assert_eq!(
            claimed.batches[0].items[0].kind,
            NotificationKindDto::AutomationConfigurationBlocked
        );
        assert_eq!(
            claimed.batches[0].items[0].automation_id.as_deref(),
            Some(created.automation_id.as_str())
        );
        assert!(claimed.batches[0].items[0].conversation_id.is_none());

        let acknowledged: NotificationBatchAcknowledgeOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCH_ACKNOWLEDGE_METHOD,
                &NotificationBatchAcknowledgeInputDto {
                    schema_version: NOTIFICATION_SCHEMA_VERSION,
                    batch_id: claimed.batches[0].batch_id.clone(),
                    claim_token: claim_input.claim_token,
                    disposition: NotificationDeliveryDispositionDto::Delivered,
                    native_priority: NotificationPriorityDto::ConfigurationBlocked,
                    sound_level_played: NotificationSoundLevelDto::Initial,
                    native_revision: claimed.batches[0].revision,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(acknowledged.batch_id, claimed.batches[0].batch_id);
        assert_eq!(acknowledged.status, NotificationBatchStatusDto::Displayed);

        let next_claim: NotificationBatchesClaimOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCHES_CLAIM_METHOD,
                &NotificationBatchesClaimInputDto {
                    schema_version: NOTIFICATION_SCHEMA_VERSION,
                    claim_token: "host-claim-2".to_string(),
                    lease_duration_ms: 60_000,
                    limit: 10,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert!(next_claim.batches.is_empty());

        let retry_created_at = mycopilot_core::storage::now_ms();
        storage
            .enqueue_automation_notification(&NewAutomationNotificationRecord {
                automation_id: created.automation_id,
                automation_run_id: None,
                resource_revision: i64::try_from(created.revision).unwrap() + 1,
                notification_kind: "configuration_blocked".to_string(),
                title: "Retry notification".to_string(),
                body: "Native delivery failed.".to_string(),
                created_at: retry_created_at,
            })
            .unwrap();
        let retry_claim_token = "host-claim-retry".to_string();
        let retry_claim: NotificationBatchesClaimOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCHES_CLAIM_METHOD,
                &NotificationBatchesClaimInputDto {
                    schema_version: NOTIFICATION_SCHEMA_VERSION,
                    claim_token: retry_claim_token.clone(),
                    lease_duration_ms: 60_000,
                    limit: 10,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(retry_claim.batches.len(), 1);
        let retry_at = retry_created_at + 60_000;
        let released: NotificationBatchReleaseOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCH_RELEASE_METHOD,
                &NotificationBatchReleaseInputDto {
                    schema_version: NOTIFICATION_SCHEMA_VERSION,
                    batch_id: retry_claim.batches[0].batch_id.clone(),
                    claim_token: retry_claim_token,
                    retry_at,
                    error_code: NotificationDeliveryErrorCodeDto::NativeNotificationFailed,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(released.status, NotificationBatchStatusDto::Pending);
        assert!(released.retry_at >= retry_at);
        assert!(released.retry_at <= retry_at + 60_000);
    }

    #[test]
    fn host_notification_rpc_revalidates_the_claim_at_the_native_display_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(model_settings()).unwrap();
        let project_id = "project-notification-validation".to_string();
        storage
            .save_project(ProjectRecord {
                id: project_id.clone(),
                name: "Notification validation project".to_string(),
                path: Some(temporary.path().to_string_lossy().to_string()),
                created_at: mycopilot_core::storage::now_ms(),
                pinned_at: None,
            })
            .unwrap();
        let created: AutomationTaskDto = serde_json::from_value(
            request(
                &storage,
                mycopilot_protocol_rs::AUTOMATION_CREATE_METHOD,
                &create_input(&project_id, "request-notification-validation"),
            )["result"]
                .clone(),
        )
        .unwrap();
        storage
            .block_automation(
                &created.automation_id,
                i64::try_from(created.revision).unwrap(),
                "target_invalid",
                "Choose a valid target.",
            )
            .unwrap();

        let claim_token = "host-validation-claim".to_string();
        let claimed: NotificationBatchesClaimOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCHES_CLAIM_METHOD,
                &NotificationBatchesClaimInputDto {
                    schema_version: NOTIFICATION_SCHEMA_VERSION,
                    claim_token: claim_token.clone(),
                    lease_duration_ms: 60_000,
                    limit: 10,
                },
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(claimed.batches.len(), 1);
        let batch_id = claimed.batches[0].batch_id.clone();
        let validation_input = NotificationBatchValidateInputDto {
            schema_version: NOTIFICATION_SCHEMA_VERSION,
            batch_id: batch_id.clone(),
            claim_token: claim_token.clone(),
        };
        let valid: NotificationBatchValidateOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCH_VALIDATE_METHOD,
                &validation_input,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert_eq!(
            valid.batch.as_ref().map(|item| item.batch_id.as_str()),
            Some(batch_id.as_str())
        );

        let blocked = storage
            .get_automation(&created.automation_id)
            .unwrap()
            .expect("blocked task");
        storage
            .tombstone_automation(&blocked.id, blocked.revision)
            .unwrap();
        let stale: NotificationBatchValidateOutputDto = serde_json::from_value(
            notification_request(
                &storage,
                mycopilot_protocol_rs::NOTIFICATION_BATCH_VALIDATE_METHOD,
                &validation_input,
            )["result"]
                .clone(),
        )
        .unwrap();
        assert!(stale.batch.is_none());
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
                automation_id: task.automation_id.clone(),
                request_id: "manual-after-revocation".to_string(),
            },
        );
        assert_eq!(response["error"]["data"]["code"], "permission_disabled");
        let blocked = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_GET_METHOD,
            &AutomationGetInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                automation_id: task.automation_id.clone(),
            },
        );
        assert_eq!(blocked["result"]["health"]["state"], "blocked");
        assert_eq!(blocked["result"]["health"]["code"], "permission_disabled");
        assert_eq!(blocked["result"]["nextRunAt"], serde_json::Value::Null);
        let attention = request(
            &storage,
            mycopilot_protocol_rs::AUTOMATION_ATTENTION_SUMMARY_METHOD,
            &AutomationAttentionSummaryInputDto {
                schema_version: AUTOMATION_SCHEMA_VERSION,
                cursor: None,
                limit: 20,
            },
        );
        assert_eq!(attention["result"]["unreadCount"], 1);
        assert_eq!(
            attention["result"]["items"][0]["kind"],
            "configuration_blocked"
        );
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

    #[test]
    fn existing_chat_create_rejects_an_inactive_root_agent() {
        let temporary = tempfile::tempdir().unwrap();
        let storage = StorageService::open(&temporary.path().join("storage.sqlite")).unwrap();
        storage.save_model_settings(model_settings()).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-inactive-root".to_string(),
                project_id: None,
                model_id: Some("model-automation".to_string()),
                title: "Inactive root conversation".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let root = storage
            .ensure_root_agent(&EnsureRootAgentInput {
                agent_id: "agent-inactive-root".to_string(),
                conversation_id: "conversation-inactive-root".to_string(),
                creation_request_id: "create-inactive-root-agent".to_string(),
                task_name: "Inactive root".to_string(),
            })
            .unwrap()
            .record()
            .clone();
        storage
            .transition_agent_lifecycle(
                &root.agent_id,
                root.revision,
                AgentLifecycle::Active,
                AgentLifecycle::Disabled,
            )
            .unwrap();

        let mut input = create_input("unused-project", "create-existing-inactive-root");
        input.destination = AutomationDestinationInputDto::ExistingChat {
            conversation_id: "conversation-inactive-root".to_string(),
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
