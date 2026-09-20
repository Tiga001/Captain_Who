use super::*;
use mycopilot_core::storage::models::{
    AgentFileChangeRecord, ChatConversationMetaRecord, ModelConfigRecord, ModelSettingsRecord,
    ProjectRecord,
};
use mycopilot_core::{
    AgentForkTurns, AgentInputAttachment, AgentInputAttachmentKind, CreateChildAgentInput,
    EnsureRootAgentInput, ProviderProfileConfig, ProviderProtocolDialect,
};

fn model_settings() -> ModelSettingsRecord {
    ModelSettingsRecord {
        api_url: "https://provider.example/v1/chat/completions".to_string(),
        api_token: "fixture-secret".to_string(),
        search_mode: "disabled".to_string(),
        tavily_api_key: String::new(),
        models: vec![ModelConfigRecord {
            id: "model-a".to_string(),
            provider_model_id: "model-a".to_string(),
            display_name: "Model A".to_string(),
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

fn request(
    storage: &StorageService,
    service: &AgentService,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    handle_request(
        storage,
        service,
        notifications,
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id: JsonRpcId::Number(1),
            method: method.to_string(),
            params: Some(params),
        },
    )
}

fn draft(id: &str, conversation_id: &str) -> AgentFileChangeRecord {
    AgentFileChangeRecord {
        schema_version: 1,
        id: id.to_string(),
        conversation_id: conversation_id.to_string(),
        project_id: Some("project-a".to_string()),
        run_id: format!("run-{id}"),
        source_tool_name: "apply_patch".to_string(),
        source_tool_call_id: format!("call-{id}"),
        source_tool_arguments_digest: format!("digest-{id}"),
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
        observation_id: "fobs_transport_fixture".to_string(),
        observation_json: "{}".to_string(),
        file_path: format!("{id}.txt"),
        operation: "create".to_string(),
        strategy: None,
        status: "drafting".to_string(),
        base_revision: None,
        base_content: String::new(),
        content: "durable draft content".to_string(),
        draft_revision: 0,
        next_mutation_index: 0,
        additions: 1,
        deletions: 0,
        line_count: 1,
        byte_count: 21,
        mutation_count: 0,
        stats_final: false,
        summary: None,
        final_action_id: None,
        final_action_arguments_digest: None,
        final_permission_revision: None,
        final_tool_set_revision: None,
        final_provider_wire_revision: None,
        created_at: 1,
        updated_at: 1,
        expires_at: i64::MAX,
    }
}

fn attachment(storage: &StorageService, id: &str) -> AgentInputAttachment {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(1, 1)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let bytes = bytes.into_inner();
    let token = storage
        .begin_attachment_import(mycopilot_core::AttachmentImportInput {
            id: id.to_string(),
            kind: AgentInputAttachmentKind::Image,
            name: format!("{id}.png"),
            mime_type: Some("image/png".to_string()),
            size_bytes: bytes.len() as u64,
        })
        .unwrap();
    storage
        .append_attachment_import(
            &token,
            0,
            &base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &bytes),
        )
        .unwrap();
    storage.finish_attachment_import(&token).unwrap()
}

#[test]
fn ordinary_user_rpc_cannot_read_or_mutate_a_child_conversation() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("rpc.sqlite")).unwrap());
    storage.save_model_settings(model_settings()).unwrap();
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "project-a".to_string(),
            "Project A".to_string(),
            directory.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    storage
        .save_conversation_meta(ChatConversationMetaRecord {
            id: "conversation-root".to_string(),
            project_id: Some("project-a".to_string()),
            model_id: Some("model-a".to_string()),
            title: "Root".to_string(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .save_conversation_meta(ChatConversationMetaRecord {
            id: "conversation-legacy".to_string(),
            project_id: Some("project-a".to_string()),
            model_id: Some("model-a".to_string()),
            title: "Legacy".to_string(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: "agent-root".to_string(),
            conversation_id: "conversation-root".to_string(),
            creation_request_id: "ensure-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let child = storage
        .create_child_agent(&CreateChildAgentInput {
            parent_agent_id: "agent-root".to_string(),
            creation_request_id: "spawn-child".to_string(),
            task_name: "review".to_string(),
            task: "Review and report.".to_string(),
            template_machine_key: None,
            explicit_model_id: None,
            reasoning_effort: None,
            fork_turns: AgentForkTurns::None,
        })
        .unwrap();
    let child_conversation = child.agent.conversation_id;
    storage
        .create_agent_file_change(draft("draft-root", "conversation-root"))
        .unwrap();
    storage
        .create_agent_file_change(draft("draft-legacy", "conversation-legacy"))
        .unwrap();
    storage
        .create_agent_file_change(draft("draft-child", &child_conversation))
        .unwrap();
    for (conversation_id, attachment_id) in [
        ("conversation-root", "attachment-root"),
        ("conversation-legacy", "attachment-legacy"),
        (child_conversation.as_str(), "attachment-child"),
    ] {
        storage
            .save_input_attachments(
                conversation_id,
                &format!("message-{attachment_id}"),
                Some("project-a"),
                &[attachment(&storage, attachment_id)],
                2,
            )
            .unwrap();
    }
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));

    let history = request(
        &storage,
        &service,
        STORAGE_LOAD_CONVERSATIONS_METHOD,
        serde_json::Value::Null,
    );
    let visible_conversations = history["result"].as_array().unwrap();
    assert_eq!(visible_conversations.len(), 2, "{history}");
    assert!(visible_conversations
        .iter()
        .all(|conversation| conversation["id"] != child_conversation));
    let search = request(
        &storage,
        &service,
        SEARCH_SEARCH_CHATS_METHOD,
        serde_json::json!({ "query": "Review and report", "limit": 20 }),
    );
    assert_eq!(search["result"].as_array().unwrap().len(), 0, "{search}");

    let attempts = [
        (
            STORAGE_LOAD_CONVERSATION_METHOD,
            serde_json::json!({ "conversationId": child_conversation }),
        ),
        (
            STORAGE_SAVE_CONVERSATION_META_METHOD,
            serde_json::json!({
                "id": child_conversation,
                "projectId": "project-a",
                "modelId": "model-a",
                "title": "forged child title",
                "createdAt": 1,
                "updatedAt": 2,
                "pinnedAt": null,
                "archivedAt": null,
                "unreadAt": null
            }),
        ),
        (
            STORAGE_UPSERT_CHAT_MESSAGES_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "messages": [{
                    "id": "forged-user",
                    "role": "user",
                    "content": "forged",
                    "createdAt": 2,
                    "status": "sent",
                    "attachments": [],
                    "agentRunJson": null,
                    "uiStateJson": null
                }],
                "positionOffset": 0
            }),
        ),
        (
            STORAGE_SAVE_CHAT_MESSAGE_STATE_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "message": {
                    "id": "forged-user",
                    "content": "forged",
                    "status": "sent",
                    "agentRunJson": null
                }
            }),
        ),
        (
            STORAGE_SAVE_CHAT_MESSAGE_UI_STATE_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "message": { "id": "forged-user", "uiStateJson": "{}" }
            }),
        ),
        (
            STORAGE_SAVE_COMPOSER_DRAFT_METHOD,
            serde_json::json!({
                "draft": {
                    "scopeId": child_conversation,
                    "message": "forged",
                    "permissionMode": "default",
                    "permissionModeVersion": 2,
                    "modelId": "model-a",
                    "projectId": "project-a",
                    "attachmentsJson": "[]",
                    "skillsJson": "[]",
                    "queuedMessagesJson": "[]",
                    "updatedAt": 2
                }
            }),
        ),
        (
            STORAGE_SAVE_COMPOSER_DRAFT_MESSAGE_METHOD,
            serde_json::json!({
                "scopeId": child_conversation,
                "message": "forged",
                "updatedAt": 3
            }),
        ),
        (
            STORAGE_DELETE_CHAT_MESSAGES_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "messageIds": ["forged-user"]
            }),
        ),
        (
            STORAGE_FORK_CONVERSATION_METHOD,
            serde_json::json!({
                "requestId": "fork-child",
                "sourceConversationId": child_conversation,
                "forkPoint": {
                    "kind": "assistant_reply",
                    "assistantMessageId": "missing"
                }
            }),
        ),
        (
            AGENT_START_CONVERSATION_TURN_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "projectId": "project-a",
                "modelId": "model-a",
                "content": "forged user Turn"
            }),
        ),
        (
            AGENT_STEER_RUN_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "expectedRunId": "forged-run",
                "clientMessageId": "forged-guidance",
                "content": "forged guidance"
            }),
        ),
        (
            AGENT_PREFLIGHT_PROVIDER_TRANSITION_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "targetModelId": "model-a"
            }),
        ),
        (
            AGENT_START_PROVIDER_TRANSITION_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "targetModelId": "model-a",
                "transitionToken": "forged-transition"
            }),
        ),
        (
            AGENT_GET_PROVIDER_TRANSITION_STATUS_METHOD,
            serde_json::json!({
                "conversationId": child_conversation
            }),
        ),
        (
            STORAGE_DELETE_CONVERSATION_METHOD,
            serde_json::json!({ "conversationId": child_conversation }),
        ),
        (
            AGENT_COMMAND_SESSIONS_LIST_METHOD,
            serde_json::json!({ "conversationId": child_conversation }),
        ),
        (
            AGENT_COMMAND_SESSIONS_GET_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "sessionId": "cmd_01J00000000000000000000000"
            }),
        ),
        (
            AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
            serde_json::json!({
                "conversationId": child_conversation,
                "projectId": "project-a",
                "modelId": "model-a"
            }),
        ),
        (
            AGENT_READ_FILE_CHANGE_METHOD,
            serde_json::json!({ "transactionId": "draft-child" }),
        ),
        (
            AGENT_GET_FILE_CHANGE_DIFF_METHOD,
            serde_json::json!({ "transactionId": "draft-child" }),
        ),
        (
            STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD,
            serde_json::json!({ "attachmentId": "attachment-child" }),
        ),
        (
            STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
            serde_json::json!({ "attachmentIds": ["attachment-child"] }),
        ),
    ];

    for (method, params) in attempts {
        let response = request(&storage, &service, method, params);
        assert_eq!(response["error"]["code"], -32000, "{method}: {response}");
        assert!(
            response["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("子 Agent Conversation 是只读观察视图")),
            "{method}: {response}"
        );
    }

    let observer = request(
        &storage,
        &service,
        AGENT_COLLABORATION_LOAD_OBSERVER_CONVERSATION_METHOD,
        serde_json::json!({
            "rootConversationId": "conversation-root",
            "conversationId": child_conversation
        }),
    );
    assert_eq!(
        observer["result"]["messages"][0]["inputOrigin"]["kind"], "agent",
        "the authorized observer path must retain the durable parent-Agent actor: {observer}"
    );
    assert_eq!(
        observer["result"]["messages"][0]["inputOrigin"]["senderAgentId"],
        "agent-root"
    );
    assert_eq!(
        observer["result"]["messages"][0]["inputOrigin"]["sourceAgentMessageId"],
        child.task_message.message_id
    );

    for method in [
        AGENT_READ_FILE_CHANGE_METHOD,
        AGENT_GET_FILE_CHANGE_DIFF_METHOD,
    ] {
        let exact_observer = request(
            &storage,
            &service,
            method,
            serde_json::json!({
                "transactionId": "draft-child",
                "observerRootConversationId": "conversation-root"
            }),
        );
        assert!(
            exact_observer.get("result").is_some(),
            "exact root+child observer authority must permit read-only draft details: {exact_observer}"
        );

        let forged_observer = request(
            &storage,
            &service,
            method,
            serde_json::json!({
                "transactionId": "draft-child",
                "observerRootConversationId": "conversation-legacy"
            }),
        );
        assert_eq!(
            forged_observer["error"]["code"], -32000,
            "{forged_observer}"
        );
    }

    let loaded = storage
        .load_conversation(&child_conversation)
        .unwrap()
        .unwrap();
    assert_eq!(loaded.title, "review");
    assert!(loaded
        .messages
        .iter()
        .all(|message| message.id != "forged-user"));

    for conversation_id in ["conversation-root", "conversation-legacy"] {
        let sessions = request(
            &storage,
            &service,
            AGENT_COMMAND_SESSIONS_LIST_METHOD,
            serde_json::json!({ "conversationId": conversation_id }),
        );
        assert_eq!(sessions["result"]["sessions"], serde_json::json!([]));

        let context = request(
            &storage,
            &service,
            AGENT_GET_CONTEXT_WINDOW_SNAPSHOT_METHOD,
            serde_json::json!({
                "conversationId": conversation_id,
                "projectId": "project-a",
                "modelId": "model-a"
            }),
        );
        assert!(
            context.get("result").is_some(),
            "{conversation_id}: {context}"
        );
    }

    for draft_id in ["draft-root", "draft-legacy"] {
        let content = request(
            &storage,
            &service,
            AGENT_READ_FILE_CHANGE_METHOD,
            serde_json::json!({ "transactionId": draft_id }),
        );
        assert_eq!(content["result"]["content"], "durable draft content");

        let diff = request(
            &storage,
            &service,
            AGENT_GET_FILE_CHANGE_DIFF_METHOD,
            serde_json::json!({ "transactionId": draft_id }),
        );
        assert!(diff["result"]["patch"].as_str().is_some());
    }

    for method in [
        AGENT_READ_FILE_CHANGE_METHOD,
        AGENT_GET_FILE_CHANGE_DIFF_METHOD,
    ] {
        let old_shape = request(
            &storage,
            &service,
            method,
            serde_json::json!({ "draftId": "draft-root" }),
        );
        assert!(
            old_shape.get("error").is_some(),
            "old draftId RPC shape must fail closed for {method}"
        );
        let extra = request(
            &storage,
            &service,
            method,
            serde_json::json!({
                "transactionId": "draft-root",
                "draftId": "draft-root"
            }),
        );
        assert!(
            extra.get("error").is_some(),
            "extra legacy RPC field must fail closed for {method}"
        );
    }

    for attachment_id in ["attachment-root", "attachment-legacy"] {
        let image = request(
            &storage,
            &service,
            STORAGE_LOAD_ATTACHMENT_IMAGE_METHOD,
            serde_json::json!({ "attachmentId": attachment_id }),
        );
        assert_eq!(image["result"]["id"], attachment_id);

        let inputs = request(
            &storage,
            &service,
            STORAGE_LOAD_INPUT_ATTACHMENTS_METHOD,
            serde_json::json!({ "attachmentIds": [attachment_id] }),
        );
        assert_eq!(inputs["result"][0]["id"], attachment_id);
    }
}
