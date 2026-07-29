use super::*;
use mycopilot_core::image_generation::{
    ImageArtifactStoreConfig, ImageGenerationAdapterRegistry, ImageGenerationConfigurationService,
    ImageGenerationExecutionFailure, ImageGenerationExecutionFailureCode,
    ImageGenerationExecutionLimits, ImageGenerationExecutionPhase, ImageGenerationExecutionReceipt,
    ImageGenerationExecutionService, ImageGenerationExecutionStatus, InMemoryCredentialStore,
    ManagedImageGenerationArtifactStore, SmartMlSeedreamProviderFactory,
    IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
};
use mycopilot_core::storage::image_generation_execution_repository::{
    ImageGenerationExecutionIdentityRecord, ImageGenerationExecutionTerminalUpdate,
    StoredImageGenerationExecutionStatus,
};

#[tokio::test]
async fn restart_pairs_a_durable_image_call_with_its_terminal_journal_receipt() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let conversation_id = "conversation-image-recovery";
    let assistant_message_id = "assistant-image-recovery";
    let run_id = "run-image-recovery";
    let call_id = "call-image-recovery";
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "Image recovery".to_string(),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": run_id,
                        "assistantMessageId": assistant_message_id,
                        "status": "running",
                        "state": {
                            "status": "running",
                            "activeRunId": run_id,
                            "updatedAt": 1
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call_id.to_string(),
            tool: "image_generation".to_string(),
            operation: json!({
                "request": { "operation": "generate", "prompt": "private prompt" },
                "reason": "Create the requested image."
            }),
            provenance: None,
            approval_status: AgentApprovalStatus::NotRequired,
            truncated: false,
        }],
    };
    storage
        .append_in_progress_conversation_turn_trace(&trace, 1, 2)
        .unwrap();

    let execution_id =
        mycopilot_core::agent_image_generation_execution_id(run_id, call_id).unwrap();
    let request_fingerprint = format!("sha256:{}", "a".repeat(64));
    let identity = ImageGenerationExecutionIdentityRecord {
        execution_id: execution_id.as_str().to_string(),
        request_fingerprint: request_fingerprint.clone(),
        safe_request_json: r#"{"schemaVersion":1}"#.to_string(),
        profile_id: "default".to_string(),
        adapter_id: "smartmlSeedream".to_string(),
        profile_revision: 1,
        model_id: "seedream".to_string(),
        operation: "generate".to_string(),
    };
    let claimed = storage.claim_image_generation_execution(&identity).unwrap();
    let mycopilot_core::storage::image_generation_execution_repository::ImageGenerationExecutionClaimOutcome::Claimed(claimed) = claimed else {
        panic!("expected a new image execution journal claim");
    };
    let receipt = ImageGenerationExecutionReceipt {
        schema_version: IMAGE_GENERATION_EXECUTION_RECEIPT_SCHEMA_VERSION,
        execution_id: execution_id.as_str().to_string(),
        request_fingerprint: request_fingerprint.clone(),
        status: ImageGenerationExecutionStatus::OutcomeIndeterminate,
        provider_profile_id: "default".to_string(),
        adapter_id: "smartmlSeedream".to_string(),
        profile_revision: 1,
        model_id: "seedream".to_string(),
        operation: mycopilot_core::image_generation::ImageGenerationOperation::Generate,
        provider_request_id: None,
        http_status: None,
        artifact: None,
        error: Some(ImageGenerationExecutionFailure {
            code: ImageGenerationExecutionFailureCode::ExecutionInterrupted,
            phase: ImageGenerationExecutionPhase::Recovery,
            message: "image generation was interrupted".to_string(),
            recovery: "Inspect provider usage before retrying.".to_string(),
            retryable: false,
            generation_may_have_succeeded: true,
            provider_succeeded: false,
            artifact_commit_may_have_succeeded: false,
            provider_error_code: None,
            artifact_error_code: None,
        }),
        created_at: claimed.created_at,
        completed_at: claimed.created_at,
        duration_ms: 0,
    };
    storage
        .finalize_image_generation_execution(
            execution_id.as_str(),
            &ImageGenerationExecutionTerminalUpdate {
                expected_request_fingerprint: request_fingerprint,
                expected_artifact_sha256: None,
                status: StoredImageGenerationExecutionStatus::OutcomeIndeterminate,
                remote_outcome_unknown: true,
                provider_succeeded: false,
                commit_may_have_succeeded: false,
                provider_request_id: None,
                http_status: None,
                terminal_result_json: serde_json::to_string(&receipt).unwrap(),
            },
        )
        .unwrap();

    let configuration = Arc::new(ImageGenerationConfigurationService::new(
        Arc::clone(&storage),
        Arc::new(InMemoryCredentialStore::default()),
    ));
    let mut adapters = ImageGenerationAdapterRegistry::new();
    adapters
        .register(Arc::new(SmartMlSeedreamProviderFactory::default()))
        .unwrap();
    let artifacts = Arc::new(
        ManagedImageGenerationArtifactStore::new(
            fixture.path().join("image-artifacts"),
            ImageArtifactStoreConfig::default(),
        )
        .unwrap(),
    );
    let execution = Arc::new(
        ImageGenerationExecutionService::new(
            configuration,
            Arc::new(adapters),
            artifacts,
            Arc::clone(&storage),
            ImageGenerationExecutionLimits::default(),
        )
        .unwrap(),
    );
    let service = AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage))
        .unwrap()
        .with_image_generation_execution(execution);

    assert_eq!(
        service
            .reconcile_interrupted_image_generation_tool_audits()
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        service
            .reconcile_interrupted_image_generation_tool_audits()
            .await
            .unwrap(),
        0
    );
    let recovered = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    recovered.validate().unwrap();
    let Some(ConversationTurnTraceItem::ToolResult {
        call_id: recovered_call_id,
        success,
        observation,
        ..
    }) = recovered.items.last()
    else {
        panic!("expected recovered image ToolResult");
    };
    assert_eq!(recovered_call_id, call_id);
    assert!(!success);
    assert_eq!(observation["status"], "outcomeIndeterminate");
    assert_eq!(observation["audit"]["executionId"], execution_id.as_str());
    assert!(observation["failure"]["generationMayHaveSucceeded"]
        .as_bool()
        .unwrap());

    assert_eq!(
        service
            .reconcile_startup_orphaned_conversation_traces()
            .unwrap(),
        1
    );
    let terminal = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    terminal.validate().unwrap();
    assert_eq!(
        terminal.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(terminal.items, recovered.items);
}
