use super::*;
use mycopilot_protocol_rs::*;

fn seed_human_question_root(storage: &StorageService) {
    use mycopilot_core::storage::models::{ChatConversationRecord, ChatMessageRecord};
    storage
        .save_conversation(ChatConversationRecord {
            id: "question-chat".into(),
            project_id: None,
            model_id: None,
            title: "Questions".into(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: "question-assistant".into(),
                role: "assistant".into(),
                content: "正在工作".into(),
                created_at: 1,
                status: Some("streaming".into()),
                attachments: vec![],
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: "question-root".into(),
            conversation_id: "question-chat".into(),
            creation_request_id: "question-root-create".into(),
            task_name: "Root".into(),
        })
        .unwrap();
    storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTurnTrace {
                schema_version: mycopilot_core::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: "question-run".into(),
                conversation_id: "question-chat".into(),
                assistant_message_id: "question-assistant".into(),
                terminal_status: mycopilot_core::ConversationTurnTraceTerminalStatus::InProgress,
                terminal_error: None,
                truncated: false,
                items: vec![],
            },
            1,
            1,
        )
        .unwrap();
}

#[test]
fn human_interaction_settlement_rpc_reports_persisted_pending_facts_and_keeps_ignore_silent() {
    use mycopilot_core::human_interaction::*;
    use mycopilot_core::storage::human_interaction_repository::HostHumanInteractionOwner;
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    // Seed after startup reconciliation: this is a currently active Host-owned turn.
    seed_human_question_root(&storage);
    let owner = HostHumanInteractionOwner {
        agent_id: "question-root".into(),
        conversation_id: "question-chat".into(),
        run_id: "question-run".into(),
        assistant_message_id: "question-assistant".into(),
        tool_call_id: "question-call".into(),
    };
    let tool_input = HumanInteractionToolInput {
        questions: vec![
            HumanInteractionQuestionInput {
                title: "输出格式？".into(),
                options: Some(vec!["CSV".into(), "JSON".into()]),
            },
            HumanInteractionQuestionInput {
                title: "其他约束？".into(),
                options: None,
            },
        ],
    };
    let sync = storage
        .create_human_interaction_request(&owner, HumanInteractionMode::Sync, &tool_input)
        .unwrap();
    let ignored = storage
        .create_human_interaction_request(
            &HostHumanInteractionOwner {
                tool_call_id: "question-call-async".into(),
                ..owner
            },
            HumanInteractionMode::Async,
            &tool_input,
        )
        .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let call = |method: &str, params: Value| {
        handle_request(
            &storage,
            &service,
            notifications.clone(),
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: method.into(),
                params: Some(params),
            },
        )
    };
    call(
        HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":false,"expectedRevision":0}),
    );
    receiver.try_recv().unwrap();
    let answers = json!([
        {"kind":"skipped","questionId":sync.questions[1].id},
        {"kind":"option","questionId":sync.questions[0].id,"optionId":sync.questions[0].options.as_ref().unwrap()[0].id}
    ]);
    let submit = json!({"conversationId":sync.conversation_id,"requestId":sync.request_id,"expectedRevision":0,"submissionId":"answer-one","answers":answers});
    let response = call(HUMAN_INTERACTION_SUBMIT_METHOD, submit.clone());
    assert_eq!(response["result"]["status"], "submitted", "{response}");
    assert_eq!(response["result"]["delivery"]["status"], "pending");
    assert_eq!(response["result"]["delivery"]["targetRunId"], Value::Null);
    assert_eq!(
        response["result"]["response"]["answers"][0]["questionId"],
        sync.questions[0].id
    );
    assert_eq!(receiver.try_recv().unwrap()["params"], response["result"]);
    assert_eq!(
        call(HUMAN_INTERACTION_SUBMIT_METHOD, submit)["result"],
        response["result"]
    );
    receiver.try_recv().unwrap();
    let settled_ignore = call(
        HUMAN_INTERACTION_IGNORE_METHOD,
        json!({"conversationId":ignored.conversation_id,"requestId":ignored.request_id,"expectedRevision":0,"submissionId":"ignore-one"}),
    );
    assert_eq!(
        settled_ignore["result"]["status"], "ignored",
        "{settled_ignore}"
    );
    assert_eq!(settled_ignore["result"]["delivery"], Value::Null);
    assert_eq!(settled_ignore["result"]["response"]["answers"], json!([]));
    assert_eq!(
        receiver.try_recv().unwrap()["method"],
        HUMAN_INTERACTION_REQUEST_CHANGED_METHOD
    );
    assert!(
        receiver.try_recv().is_err(),
        "settlement must not emit run/steer events"
    );
    assert_eq!(
        storage
            .load_conversation("question-chat")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        1,
        "foundation does not inject ordinary User messages or fake delivery"
    );
}

#[test]
fn human_interaction_settings_rpc_commits_cas_before_notification_and_rejects_unknown_authority() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&directory.path().join("storage.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let call = |method: &str, params: Value| {
        handle_request(
            &storage,
            &service,
            notifications.clone(),
            JsonRpcRequest {
                jsonrpc: "2.0".into(),
                id: JsonRpcId::Number(1),
                method: method.into(),
                params: Some(params),
            },
        )
    };
    let initial = call(HUMAN_INTERACTION_GET_SETTINGS_METHOD, json!({}));
    assert_eq!(initial["result"]["enabled"], true, "{initial}");
    assert_eq!(initial["result"]["revision"], 0);
    let updated = call(
        HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":false,"expectedRevision":0}),
    );
    assert_eq!(updated["result"]["enabled"], false, "{updated}");
    let notification = receiver.try_recv().unwrap();
    assert_eq!(
        notification["method"],
        HUMAN_INTERACTION_SETTINGS_CHANGED_METHOD
    );
    assert_eq!(notification["params"], updated["result"]);
    assert!(!storage.load_human_interaction_settings().unwrap().enabled);
    let conflict = call(
        HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":true,"expectedRevision":0}),
    );
    assert_eq!(
        conflict["error"]["data"]["code"], "revision_conflict",
        "{conflict}"
    );
    assert!(receiver.try_recv().is_err());
    for (method, params) in [
        (
            HUMAN_INTERACTION_GET_SETTINGS_METHOD,
            json!({"rootAgentId":"fake"}),
        ),
        (
            HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
            json!({"enabled":true,"expectedRevision":1,"promptPreferences":{}}),
        ),
        (
            HUMAN_INTERACTION_SUBMIT_METHOD,
            json!({"conversationId":"c","requestId":"r","expectedRevision":0,"submissionId":"s","answers":[],"checkpoint":{}}),
        ),
        (
            HUMAN_INTERACTION_IGNORE_METHOD,
            json!({"conversationId":"c","requestId":"r","expectedRevision":0,"submissionId":"s","answers":[]}),
        ),
    ] {
        let response = call(method, params);
        assert_eq!(response["error"]["code"], -32602, "{response}");
    }
    let unsafe_revision = call(
        HUMAN_INTERACTION_UPDATE_SETTINGS_METHOD,
        json!({"enabled":true,"expectedRevision":9007199254740992_u64}),
    );
    assert_eq!(unsafe_revision["error"]["data"]["code"], "invalid_input");
    let list = call(
        HUMAN_INTERACTION_LIST_REQUESTS_METHOD,
        json!({"conversationId":"empty-conversation","cursor":null,"limit":100}),
    );
    assert_eq!(list["result"]["items"], json!([]), "{list}");
    let missing = call(
        HUMAN_INTERACTION_SUBMIT_METHOD,
        json!({"conversationId":"empty-conversation","requestId":"unknown","expectedRevision":0,"submissionId":"s","answers":[]}),
    );
    assert!(missing.get("error").is_some(), "{missing}");
    assert!(receiver.try_recv().is_err());
}
