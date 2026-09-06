use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

struct RecordingWorldStateHost {
    state: Mutex<MemoryConversationWorldState>,
    prepared: Mutex<Vec<crate::WorldStateRequestBoundary>>,
    observed: AtomicUsize,
    fail_prepare: bool,
    fail_observed: bool,
}

impl AgentConversationWorldStateHost for RecordingWorldStateHost {
    fn prepare_request(
        &self,
        request: AgentConversationWorldStateRequest,
    ) -> AgentResult<Vec<AnchoredWorldStateRecord>> {
        self.prepared.lock().unwrap().push(request.boundary.clone());
        if self.fail_prepare {
            return Err(AgentError::new("test world-state commit failure"));
        }
        self.state
            .lock()
            .unwrap()
            .prepare(&request.boundary, request.sections)
    }

    fn mark_request_observed(&self, _: &crate::WorldStateRequestBoundary) -> AgentResult<()> {
        self.observed.fetch_add(1, Ordering::SeqCst);
        if self.fail_observed {
            return Err(AgentError::new("test world-state observation failure"));
        }
        self.state.lock().unwrap().mark_observed();
        Ok(())
    }
}

fn input(url: String) -> AgentChatInput {
    let mut input = conversation_context_input(vec![message("user", "Inspect the task")]);
    input.api_url = url;
    input.api_token = "test-token".into();
    input.stream = Some(false);
    input.assistant_message_id = Some("world-assistant".into());
    input.context = Some(AgentRunContext {
        conversation_id: Some("world-conversation".into()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    });
    input
}

struct EnabledHumanPolicy;

impl HumanInteractionPolicySource for EnabledHumanPolicy {
    fn snapshot(&self) -> AgentResult<crate::human_interaction::HumanInteractionSettings> {
        Ok(crate::human_interaction::HumanInteractionSettings {
            enabled: true,
            revision: 1,
            updated_at: 1,
        })
    }
}

#[tokio::test]
async fn preview_readiness_projects_questions_but_never_grants_runtime_execution() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = input(format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ));
    let host = AgentRuntimeHostServices::new()
        .with_human_interaction_policy(Arc::new(EnabledHumanPolicy))
        .with_human_interaction_preview_readiness(true, true);
    let preview = prepare_context_window_tool_projection(&input, &host, false).unwrap();
    assert!(preview
        .tool_set_checkpoint()
        .exposed_tool_names
        .iter()
        .any(|name| name == "request_user_input"));
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(
            &mut stream,
            json!({"choices": [{
                "message": {"role": "assistant", "content": "done"}, "finish_reason": "stop"
            }]}),
        )
        .await;
        request
    });
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("preview-authority".into()),
            None,
            AgentCancellationToken::new(),
            Some(host),
        )
        .await
        .unwrap();
    assert_eq!(output.content, "done");
    let request = server.await.unwrap();
    assert!(request["tools"]
        .as_array()
        .unwrap()
        .iter()
        .all(|tool| !matches!(
            tool["function"]["name"].as_str(),
            Some("request_user_input" | "request_user_input_async")
        )));
    assert!(request["messages"]
        .to_string()
        .contains("execution_unavailable"));
}

#[tokio::test]
async fn conversation_world_state_commit_failure_prevents_network_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = input(format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ));
    let host = Arc::new(RecordingWorldStateHost {
        state: Mutex::new(MemoryConversationWorldState::new(&input).unwrap()),
        prepared: Mutex::new(Vec::new()),
        observed: AtomicUsize::new(0),
        fail_prepare: true,
        fail_observed: false,
    });
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("world-run".into()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_conversation_world_state(host.clone())),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("commit failure"));
    assert_eq!(host.prepared.lock().unwrap().len(), 1);
    assert_eq!(host.observed.load(Ordering::SeqCst), 0);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn conversation_world_state_ack_failure_keeps_usage_and_never_executes_or_replays_response() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = input(format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ));
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(&mut stream, json!({
            "choices": [{"message": {"role": "assistant", "content": "Checking.", "tool_calls": [{
                "id": "todo", "type": "function", "function": {"name": "todo_update", "arguments":
                    "{\"items\":[{\"title\":\"Must not execute\",\"status\":\"in_progress\"}]}"}
            }]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        })).await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(100), listener.accept())
                .await
                .is_err()
        );
        request
    });
    let host = Arc::new(RecordingWorldStateHost {
        state: Mutex::new(MemoryConversationWorldState::new(&input).unwrap()),
        prepared: Mutex::new(Vec::new()),
        observed: AtomicUsize::new(0),
        fail_prepare: false,
        fail_observed: true,
    });
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("world-run".into()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_conversation_world_state(host.clone())),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("observation failure"));
    assert!(error.model_request_interruption().is_some());
    assert_eq!(error.usage().unwrap().total_tokens, Some(15));
    assert_eq!(host.prepared.lock().unwrap().len(), 1);
    assert_eq!(host.observed.load(Ordering::SeqCst), 1);
    assert!(error
        .conversation_turn_trace()
        .unwrap()
        .items
        .iter()
        .all(|item| !matches!(item, ConversationTurnTraceItem::ToolResult { .. })));
    let request = server.await.unwrap();
    let messages = request["messages"].to_string();
    assert!(messages.contains("human.interaction"));
    assert!(!messages.contains("tools.effective"));
    assert!(!messages.contains("stableTools"));
    assert!(!messages.contains("dynamicTools"));
}
