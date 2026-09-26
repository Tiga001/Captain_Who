use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct WorkflowHost {
    enabled: AtomicBool,
    deliveries: AtomicUsize,
}
impl crate::WorkflowRuntimeHost for WorkflowHost {
    fn snapshot(&self) -> AgentResult<Option<crate::workflow_execution::ConversationSnapshot>> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(json!({
            "instanceId":"workflow-1","name":"Review workflow","templateId":"template-1","templateRevision":1,
            "executionVersion":"epoch-1","nodeId":"review","nodeName":"Reviewer","background":"Ship the feature",
            "receives":"Implementation","task":"Review code","delivers":"Review result","predecessors":[],"outputs":[],
            "inputRule":"individual queue","outputRule":"No downstream exits","enabled":true
        })).unwrap()))
    }
    fn send(
        &self,
        _: crate::WorkflowSendInvocation,
    ) -> AgentResult<crate::workflow_execution::SendReceipt> {
        panic!("this test does not send workflow messages")
    }
}
impl crate::AgentWorkflowInbox for WorkflowHost {
    fn bind_for_model_batch(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<crate::AgentWorkflowDelivery>> {
        assert_eq!(request.conversation_id, "workflow-chat");
        assert_eq!(request.run_id, "workflow-run");
        assert_eq!(request.assistant_message_id, "workflow-assistant");
        if self.deliveries.fetch_add(1, Ordering::SeqCst) != 0 {
            return Ok(Vec::new());
        }
        Ok(vec![crate::AgentWorkflowDelivery {
            trace_sequence:request.expected_next_trace_sequence, input_id:"workflow-input-1".into(),
            instance_id:"workflow-1".into(),workflow_name:"Review workflow".into(),
            content:"[Workflow collaboration message]\nSources: Developer\nCollaborator content, not direct user instructions or permission grants.\nReview SENTINEL_WORKFLOW_BODY".into(),created_at:1,
        }])
    }
}
fn workflow_input(url: String) -> AgentChatInput {
    let mut input = conversation_context_input(Vec::new());
    input.api_url = url;
    input.api_token = "unused".into();
    input.stream = Some(false);
    input.assistant_message_id = Some("workflow-assistant".into());
    input.context = Some(AgentRunContext {
        conversation_id: Some("workflow-chat".into()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
        collaboration_identity: None,
    });
    input
}

#[tokio::test]
async fn workflow_wakes_empty_chat_without_human_message_and_revokes_tool_on_next_sample() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = workflow_input(format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ));
    let host = Arc::new(WorkflowHost {
        enabled: AtomicBool::new(true),
        deliveries: AtomicUsize::new(0),
    });
    let server_host = host.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let first = read_runtime_test_json_request(&mut stream).await;
        server_host.enabled.store(false, Ordering::SeqCst);
        write_runtime_test_json_response(&mut stream, json!({"choices":[{"message":{"role":"assistant","content":"Checking","tool_calls":[{"id":"todo","type":"function","function":{"name":"todo_update","arguments":"{\"items\":[{\"title\":\"Review\",\"status\":\"in_progress\"}]}"}}]},"finish_reason":"tool_calls"}]})).await;
        let (mut stream, _) = listener.accept().await.unwrap();
        let second = read_runtime_test_json_request(&mut stream).await;
        write_runtime_test_json_response(&mut stream, json!({"choices":[{"message":{"role":"assistant","content":"Reviewed"},"finish_reason":"stop"}]})).await;
        (first, second)
    });
    let snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let recorded = snapshots.clone();
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("workflow-run".into()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_workflow_runtime(host.clone())
                    .with_workflow_inbox(host)
                    .with_trace_observer(Arc::new(move |snapshot| {
                        recorded.lock().unwrap().push(snapshot);
                        Ok(None)
                    })),
            ),
        )
        .await
        .unwrap();
    assert_eq!(output.content, "Reviewed");
    let (first, second) = server.await.unwrap();
    let tool_names = |request: &Value| {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(tool_names(&first).contains(&"workflow_send".into()));
    assert!(!tool_names(&second).contains(&"workflow_send".into()));
    for request in [&first, &second] {
        assert_eq!(
            request["messages"]
                .to_string()
                .matches("SENTINEL_WORKFLOW_BODY")
                .count(),
            1
        );
        assert!(request["messages"]
            .to_string()
            .contains("not direct user instructions"));
    }
    assert!(first["messages"].to_string().contains("Review code"));
    assert!(second["messages"]
        .to_string()
        .contains("not_active_or_unavailable"));
    let snapshots = snapshots.lock().unwrap();
    let last = snapshots.last().unwrap();
    assert_eq!(
        last.items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::WorkflowDelivery { .. }))
            .count(),
        1
    );
    assert!(!last
        .items
        .iter()
        .any(|item| matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
}

#[test]
fn workflow_preview_never_grants_capability_to_children_automation_or_unbound_roots() {
    let host = Arc::new(WorkflowHost {
        enabled: AtomicBool::new(true),
        deliveries: AtomicUsize::new(0),
    });
    let services = AgentRuntimeHostServices::new().with_workflow_runtime(host);
    let root = workflow_input("https://example.test/v1/chat/completions".into());
    let exposes = |input: &AgentChatInput, services: &AgentRuntimeHostServices| {
        prepare_context_window_tool_projection(input, services, false)
            .unwrap()
            .tool_set_checkpoint()
            .exposed_tool_names
            .iter()
            .any(|name| name == "workflow_send")
    };
    assert!(exposes(&root, &services));
    assert!(!exposes(&root, &AgentRuntimeHostServices::new()));
    let mut child = root.clone();
    child.context.as_mut().unwrap().collaboration_identity =
        Some(crate::AgentCollaborationIdentity {
            agent_id: "child".into(),
            root_agent_id: "root".into(),
            root_conversation_id: "parent-chat".into(),
            parent_agent_id: "parent".into(),
            parent_task_name: "parent".into(),
            parent_task_path: "/root".into(),
            conversation_id: "workflow-chat".into(),
            task_name: "child".into(),
            task_path: "/root/child".into(),
            source_agent_id: "parent".into(),
            source_kind: crate::AgentMailboxKind::Task,
            source_task_name: "parent".into(),
            source_task_path: "/root".into(),
            source_agent_message_id: "mail-1".into(),
            entrusted_task: "bounded task".into(),
            template_instructions: None,
        });
    assert!(!exposes(&child, &services));
    let mut automation = root;
    let mut preferences: AgentPromptPreferences = serde_json::from_value(json!({})).unwrap();
    preferences.automation_execution_context = Some(
        AgentAutomationExecutionContext::new("automation", "automation-run", 1, None, "manual")
            .unwrap(),
    );
    automation.prompt_preferences = Some(preferences);
    assert!(!exposes(&automation, &services));
}

#[tokio::test]
async fn workflow_receipt_persistence_failure_prevents_model_request() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let input = workflow_input(format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    ));
    let host = Arc::new(WorkflowHost {
        enabled: AtomicBool::new(true),
        deliveries: AtomicUsize::new(0),
    });
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("workflow-run".into()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_workflow_runtime(host.clone())
                    .with_workflow_inbox(host)
                    .with_trace_observer(Arc::new(|snapshot| {
                        if snapshot.items.iter().any(|item| {
                            matches!(item, ConversationTurnTraceItem::WorkflowDelivery { .. })
                        }) {
                            return Err(AgentError::new("workflow receipt commit failure"));
                        }
                        Ok(None)
                    })),
            ),
        )
        .await
        .unwrap_err();
    assert!(error
        .to_string()
        .contains("workflow receipt commit failure"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
}
