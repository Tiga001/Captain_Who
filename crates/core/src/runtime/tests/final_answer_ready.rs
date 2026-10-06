use super::*;
use crate::storage::models::{AgentFileChangeRecord, ChatConversationRecord};

fn has_ready(events: &[AgentEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, AgentEvent::FinalAnswerReady { .. }))
}

#[test]
fn final_answer_ready_has_only_run_identity() {
    assert_eq!(
        serde_json::to_value(AgentEvent::FinalAnswerReady {
            run_id: "run-final".to_string(),
        })
        .unwrap(),
        json!({"type":"final_answer_ready","runId":"run-final"})
    );
}

#[tokio::test]
async fn final_answer_ready_is_absent_on_cancellation_output_limit_and_empty_response() {
    for mode in ["cancel", "output_limit", "empty"] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..if mode == "empty" { 2 } else { 1 } {
                let (mut socket, _) = listener.accept().await.unwrap();
                read_runtime_test_json_request(&mut socket).await;
                write_runtime_test_json_response(&mut socket, json!({
                    "choices":[{"message":{"role":"assistant","content":if mode == "empty" { "" } else { "Candidate answer" }},
                        "finish_reason":if mode == "output_limit" { "length" } else { "stop" }}]
                })).await;
            }
        });
        let cancellation = AgentCancellationToken::new();
        let event_cancellation = cancellation.clone();
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let mut input = conversation_context_input(vec![message("user", "Answer the question")]);
        input.api_url = format!("http://{address}/v1/chat/completions");
        input.api_token = "token".to_string();
        // The JSON fallback still commits a stream, allowing cancellation after a
        // complete no-tool response but before the Driver accepts it as final.
        input.stream = Some(mode == "cancel");
        let result = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-final".to_string()),
                Some(Arc::new(move |event| {
                    if mode == "cancel"
                        && matches!(event, AgentEvent::MessageStreamCommitted { .. })
                    {
                        event_cancellation.cancel();
                    }
                    sink.lock().unwrap().push(event);
                })),
                cancellation,
                None,
            )
            .await;
        server.await.unwrap();
        if mode == "cancel" {
            let output = result.unwrap();
            assert_eq!(output.status, AgentRunStatus::Cancelled);
            assert!(!has_ready(&output.events));
        } else {
            assert!(result.is_err(), "{mode} unexpectedly completed");
        }
        assert!(!has_ready(&captured.lock().unwrap()), "{mode}");
    }
}

fn draft_transaction() -> AgentFileChangeRecord {
    AgentFileChangeRecord {
        schema_version: crate::file_change::FILE_CHANGE_SCHEMA_VERSION,
        id: "draft-final".to_string(),
        conversation_id: "conversation-final".to_string(),
        project_id: None,
        run_id: "run-final".to_string(),
        source_tool_name: "apply_patch".to_string(),
        source_tool_call_id: "call-begin".to_string(),
        source_tool_arguments_digest: crate::file_change::proposal_digest(&json!({
            "request":{"action":"begin","operation":"create","filePath":"report.md"}
        }))
        .unwrap(),
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
        observation_id: "fobs-test".to_string(),
        observation_json: "{}".to_string(),
        file_path: "report.md".to_string(),
        operation: "create".to_string(),
        strategy: None,
        status: "drafting".to_string(),
        base_revision: None,
        base_content: String::new(),
        content: String::new(),
        draft_revision: 0,
        next_mutation_index: 0,
        additions: 0,
        deletions: 0,
        line_count: 0,
        byte_count: 0,
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

#[tokio::test]
async fn final_answer_ready_is_absent_for_file_transaction_correction_and_final_guard_failure() {
    for insert_during_sampling in [false, true] {
        let fixture = tempfile::tempdir().unwrap();
        let storage =
            Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-final".to_string(),
                project_id: None,
                model_id: None,
                title: "Final answer test".to_string(),
                messages: Vec::new(),
                created_at: 1,
                updated_at: 1,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        if !insert_during_sampling {
            storage
                .create_agent_file_change(draft_transaction())
                .unwrap();
        }
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server_storage = storage.clone();
        let server = tokio::spawn(async move {
            let requests = if insert_during_sampling {
                1
            } else {
                MAX_RESPONSE_FENCE_CORRECTIONS + 1
            };
            for index in 0..requests {
                let (mut socket, _) = listener.accept().await.unwrap();
                let request = read_runtime_test_json_request(&mut socket).await;
                if insert_during_sampling {
                    server_storage
                        .create_agent_file_change(draft_transaction())
                        .unwrap();
                } else if index > 0 {
                    assert!(request
                        .to_string()
                        .contains("Your preceding text-only response was not shown"));
                }
                write_runtime_test_json_response(&mut socket, json!({
                    "choices":[{"message":{"role":"assistant","content":"Unsettled candidate answer"},"finish_reason":"stop"}]
                })).await;
            }
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = captured.clone();
        let mut input = conversation_context_input(vec![message("user", "Finish the report")]);
        input.api_url = format!("http://{address}/v1/chat/completions");
        input.api_token = "token".to_string();
        input.stream = Some(false);
        input.context = Some(AgentRunContext {
            conversation_id: Some("conversation-final".to_string()),
            project_id: None,
            collaboration_identity: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        });
        let error = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-final".to_string()),
                Some(Arc::new(move |event| sink.lock().unwrap().push(event))),
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_storage(storage)),
            )
            .await
            .unwrap_err();
        server.await.unwrap();
        assert!(
            error.to_string().contains("文件事务") || error.to_string().contains("未结算文件事务")
        );
        let events = captured.lock().unwrap();
        assert!(!has_ready(&events));
        assert!(!events
            .iter()
            .any(|event| matches!(event, AgentEvent::MessageDelta { .. })));
    }
}
