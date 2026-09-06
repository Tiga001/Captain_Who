use super::*;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct LiveWebPolicy {
    enabled: AtomicBool,
    authorizations: AtomicUsize,
}

impl crate::WebSearchPolicySource for LiveWebPolicy {
    fn snapshot(&self) -> AgentResult<crate::WebSearchPolicySnapshot> {
        Ok(crate::WebSearchPolicySnapshot {
            enabled: self.enabled.load(Ordering::SeqCst),
            credential_ready: true,
        })
    }

    fn authorize_execution(&self) -> AgentResult<crate::WebSearchExecutionCredential> {
        self.authorizations.fetch_add(1, Ordering::SeqCst);
        self.snapshot()?.ensure_available()?;
        panic!("the late web call must be rejected before network admission")
    }
}

#[tokio::test]
async fn web_switch_changes_real_requests_without_changing_stable_prefix_or_replaying_history() {
    let policy = Arc::new(LiveWebPolicy {
        enabled: AtomicBool::new(false),
        authorizations: AtomicUsize::new(0),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_policy = Arc::clone(&policy);
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_runtime_test_json_request(&mut stream).await);
            let reply = match index {
                0 => {
                    server_policy.enabled.store(true, Ordering::SeqCst);
                    json!({ "role": "assistant", "content": "Inspecting the task.",
                        "tool_calls": [{ "id": "progress", "type": "function", "function": {
                            "name": "todo_update",
                            "arguments": json!({"items": [{"title": "Check facts", "status": "in_progress"}]}).to_string()
                        }}]
                    })
                }
                1 => {
                    // This request saw the enabled schema; the user disables it before the
                    // returned tool call reaches Host authorization. No Tavily request is made.
                    server_policy.enabled.store(false, Ordering::SeqCst);
                    json!({ "role": "assistant", "content": null,
                        "tool_calls": [{ "id": "late-web", "type": "function", "function": {
                            "name": "web_search", "arguments": "{\"query\":\"owned test query\"}"
                        }}]
                    })
                }
                _ => json!({"role": "assistant", "content": "Search is now disabled."}),
            };
            write_runtime_test_json_response(&mut stream, json!({
                "choices": [{"message": reply, "finish_reason": if index < 2 {"tool_calls"} else {"stop"}}]
            })).await;
        }
        requests
    });
    let mut input = conversation_context_input(vec![message("user", "Check facts")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    // An enabled legacy input cannot override the live Host's initially disabled setting.
    input.search_config = Some(AgentSearchConfig {
        mode: AgentSearchMode::Auto,
        tavily_api_key: Some("unused-legacy-secret".to_string()),
    });
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        AgentRuntime::default().send_chat_with_events_and_cancellation(
            input,
            Some("live-web-switch".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_web_search_policy(policy.clone())),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let requests = server.await.unwrap();
    assert_eq!(policy.authorizations.load(Ordering::SeqCst), 1);
    for (index, request) in requests.iter().enumerate() {
        let tools = request["tools"].as_array().unwrap();
        for name in ["web_search", "web_fetch"] {
            assert_eq!(
                tools.iter().any(|tool| tool["function"]["name"] == name),
                index == 1
            );
        }
        // This adapter renders non-stable RuntimeGuard context as a user-role request tail.
        // Inspect the wire payload, not only the one stable system message.
        let context = request["messages"].to_string();
        assert_eq!(context.contains("## 联网搜索"), index == 1);
        assert_eq!(context.contains("web_fetch 用于深读"), index == 1);
        assert_eq!(request["messages"][0], requests[0]["messages"][0]);
        assert!(!request.to_string().contains("unused-legacy-secret"));
        let stable = tools
            .iter()
            .filter(|tool| {
                !matches!(
                    tool["function"]["name"].as_str(),
                    Some("web_search" | "web_fetch")
                )
            })
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(stable, *requests[0]["tools"].as_array().unwrap());
    }
    let final_messages = requests[2]["messages"].as_array().unwrap();
    assert!(requests[0].to_string().contains("disabled_by_user"));
    assert!(requests[1].to_string().contains("web.search"));
    assert!(requests[2].to_string().contains("disabled_by_user"));
    assert_eq!(
        final_messages
            .iter()
            .filter(|message| {
                message["role"] == "tool"
                    && message["content"]
                        .as_str()
                        .is_some_and(|content| content.contains("disabled_by_user"))
            })
            .count(),
        1
    );
    assert!(final_messages
        .iter()
        .any(|message| message["content"] == "Inspecting the task."));
}
