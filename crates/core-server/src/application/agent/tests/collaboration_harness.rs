use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ROOT_CONVERSATION_ID: &str = "conversation-collaboration-harness";
const PROJECT_ID: &str = "project-collaboration-harness";

async fn read_provider_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "provider fixture closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_provider_stream(stream: &mut TcpStream, delta: Value, finish_reason: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let frame = json!({
        "choices": [{ "delta": delta, "finish_reason": null }]
    });
    let finish = json!({
        "choices": [{ "delta": {}, "finish_reason": finish_reason }]
    });
    stream
        .write_all(format!("data: {frame}\n\ndata: {finish}\n\ndata: [DONE]\n\n").as_bytes())
        .await
        .unwrap();
}

async fn write_tool_call(stream: &mut TcpStream, id: &str, name: &str, args: Value) {
    write_provider_stream(
        stream,
        json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [{
                "index": 0,
                "id": id,
                "type": "function",
                "function": {
                    "name": name,
                    "arguments": serde_json::to_string(&args).unwrap()
                }
            }]
        }),
        "tool_calls",
    )
    .await;
}

async fn write_two_tool_calls(
    stream: &mut TcpStream,
    first: (&str, &str, Value),
    second: (&str, &str, Value),
) {
    write_provider_stream(
        stream,
        json!({
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "index": 0,
                    "id": first.0,
                    "type": "function",
                    "function": {
                        "name": first.1,
                        "arguments": serde_json::to_string(&first.2).unwrap()
                    }
                },
                {
                    "index": 1,
                    "id": second.0,
                    "type": "function",
                    "function": {
                        "name": second.1,
                        "arguments": serde_json::to_string(&second.2).unwrap()
                    }
                }
            ]
        }),
        "tool_calls",
    )
    .await;
}

async fn write_text(stream: &mut TcpStream, content: &str) {
    write_provider_stream(
        stream,
        json!({ "role": "assistant", "content": content }),
        "stop",
    )
    .await;
}

fn request_text(request: &Value) -> String {
    fn collect(value: &Value, output: &mut String) {
        match value {
            Value::String(value) => output.push_str(value),
            Value::Array(values) => values.iter().for_each(|value| collect(value, output)),
            Value::Object(object) => {
                if let Some(text) = object.get("text") {
                    collect(text, output);
                } else if let Some(content) = object.get("content") {
                    collect(content, output);
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    let mut output = String::new();
    if let Some(messages) = request["messages"].as_array() {
        for message in messages {
            collect(&message["content"], &mut output);
            output.push('\n');
        }
    }
    output
}

fn tool_results(request: &Value) -> Vec<Value> {
    request["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|message| message["role"] == "tool")
        .filter_map(|message| message["content"].as_str())
        .filter_map(|content| serde_json::from_str(content).ok())
        .collect()
}

fn collaboration_tool_names(request: &Value) -> Vec<String> {
    let mut names = request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str())
        .filter(|name| mycopilot_core::AGENT_COLLABORATION_TOOL_NAMES.contains(name))
        .map(str::to_string)
        .collect::<Vec<_>>();
    names.sort();
    names
}

async fn respond_to_root_request(stream: &mut TcpStream, request: &Value) {
    let results = tool_results(request);
    let child_agent_ids = || {
        results
            .iter()
            .filter_map(|result| result["childAgentId"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
    };
    if results.len() >= 8 {
        write_text(stream, "Collaboration harness chain completed.").await;
    } else if results.len() == 7 {
        write_tool_call(
            stream,
            "call-interrupt",
            "interrupt_agent",
            json!({ "target": child_agent_ids()[1] }),
        )
        .await;
    } else if results.len() == 5 {
        let child_agent_ids = child_agent_ids();
        write_two_tool_calls(
            stream,
            (
                "call-wait-first",
                "wait_agent",
                json!({ "targets": [child_agent_ids[0]], "timeout_ms": 5_000 }),
            ),
            (
                "call-wait-second",
                "wait_agent",
                json!({ "targets": [child_agent_ids[1]], "timeout_ms": 0 }),
            ),
        )
        .await;
    } else if results.len() == 4 {
        write_tool_call(stream, "call-list", "list_agents", json!({})).await;
    } else if results.len() == 3 {
        write_tool_call(
            stream,
            "call-followup",
            "followup_task",
            json!({
                "target": child_agent_ids()[0],
                "message": "Verify the follow-up invariant and report again."
            }),
        )
        .await;
    } else if results.len() == 2 {
        write_tool_call(
            stream,
            "call-send",
            "send_message",
            json!({
                "target": child_agent_ids()[0],
                "message": "Additional evidence is available; do not start a new Turn for this message alone."
            }),
        )
        .await;
    } else if results.len() == 1 {
        write_tool_call(
            stream,
            "call-spawn-model",
            "spawn_agent",
            json!({
                "task_name": "compatibility_review",
                "message": "Inspect the compatibility boundary using the explicitly selected model.",
                "model": "model-2",
                "fork_turns": 1
            }),
        )
        .await;
    } else {
        write_tool_call(
            stream,
            "call-spawn",
            "spawn_agent",
            json!({
                "task_name": "security_review",
                "message": "Inspect the authentication boundary and report evidence.",
                "agent_type": "reviewer",
                "fork_turns": "none"
            }),
        )
        .await;
    }
}

async fn collect_root_until_done(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    root_run_id: &str,
) -> Vec<Value> {
    tokio::time::timeout(Duration::from_secs(15), async {
        let mut events = Vec::new();
        loop {
            let event = receiver.recv().await.expect("Agent event channel closed");
            let done = event["params"]["type"] == "done" && event["params"]["runId"] == root_run_id;
            events.push(event);
            if done {
                return events;
            }
        }
    })
    .await
    .expect("timed out waiting for the root Agent terminal event")
}

async fn wait_for_dispatcher_idle(database_path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let live: i64 = rusqlite::Connection::open(database_path)
                .unwrap()
                .query_row(
                    "SELECT COUNT(*) FROM agent_wake_requests
                     WHERE status IN ('queued', 'claimed', 'running', 'waiting_for_approval')",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            if live == 0 {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("collaboration Dispatcher did not settle its durable queue");
}

async fn wait_for_terminal_wake(
    storage: &StorageService,
    wake_id: &str,
) -> mycopilot_core::AgentWakeRequestRecord {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let wake = storage
                .get_agent_wake(wake_id)
                .unwrap()
                .expect("the durable Wake must remain queryable after dispatch");
            if matches!(
                wake.status,
                mycopilot_core::AgentWakeStatus::Completed
                    | mycopilot_core::AgentWakeStatus::Failed
                    | mycopilot_core::AgentWakeStatus::Cancelled
                    | mycopilot_core::AgentWakeStatus::Interrupted
                    | mycopilot_core::AgentWakeStatus::OutcomeUnknown
            ) {
                return wake;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("recovered durable Wake did not reach a terminal state")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_provider_drives_all_six_tools_through_runtime_host_and_server_services() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_requests = Arc::clone(&requests);
    let child_arrivals = Arc::new(AtomicUsize::new(0));
    let active_children = Arc::new(AtomicUsize::new(0));
    let maximum_active_children = Arc::new(AtomicUsize::new(0));
    let child_ready = Arc::new(tokio::sync::Notify::new());
    let server_child_arrivals = Arc::clone(&child_arrivals);
    let server_active_children = Arc::clone(&active_children);
    let server_maximum_active_children = Arc::clone(&maximum_active_children);
    let server_child_ready = Arc::clone(&child_ready);
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let requests = Arc::clone(&server_requests);
                    let arrivals = Arc::clone(&server_child_arrivals);
                    let active = Arc::clone(&server_active_children);
                    let maximum_active = Arc::clone(&server_maximum_active_children);
                    let ready = Arc::clone(&server_child_ready);
                    tokio::spawn(async move {
                        let request = read_provider_request(&mut stream).await;
                        requests
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .push(request.clone());
                        if request_text(&request).contains("## 子 Agent 协作身份") {
                            let child_text = request_text(&request);
                            let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                            maximum_active.fetch_max(current, Ordering::SeqCst);
                            let notification = ready.notified();
                            let arrival = arrivals.fetch_add(1, Ordering::SeqCst) + 1;
                            if arrival >= 2 {
                                ready.notify_waiters();
                            } else if arrivals.load(Ordering::SeqCst) < 2 {
                                tokio::time::timeout(Duration::from_secs(5), notification)
                                    .await
                                    .expect("two child samples must overlap through the real Dispatcher");
                            }
                            if child_text.contains("compatibility_review") {
                                // Keep the explicitly-model-selected child inside a live Provider
                                // sample. The root's interrupt_agent call must cancel this real
                                // Turn; a fake terminal response would only cover no_active_turn.
                                std::future::pending::<()>().await;
                            }
                            write_text(&mut stream, "Child completed the delegated review.").await;
                            active.fetch_sub(1, Ordering::SeqCst);
                        } else {
                            respond_to_root_request(&mut stream, &request).await;
                        }
                    });
                }
            }
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("collaboration-harness.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: PROJECT_ID.to_string(),
            name: "Collaboration harness".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    settings.api_token = "must-not-enter-selector-directory".to_string();
    let mut second_model = settings.models[0].clone();
    second_model.id = "model-2".to_string();
    second_model.display_name = "Model 2".to_string();
    settings.models.push(second_model);
    storage.save_model_settings(settings).unwrap();
    storage
        .create_agent_template(&mycopilot_core::CreateAgentTemplateInput {
            template_id: "template-reviewer".to_string(),
            project_id: PROJECT_ID.to_string(),
            machine_key: "reviewer".to_string(),
            name: "Reviewer".to_string(),
            description: "Review a delegated boundary".to_string(),
            instructions: "PRIVATE_TEMPLATE_INSTRUCTION: return concrete evidence.".to_string(),
            model_config_id: "model-1".to_string(),
            enabled: true,
        })
        .unwrap();
    assert!(storage
        .get_agent_node_by_conversation(ROOT_CONVERSATION_ID)
        .unwrap()
        .is_none());

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        4,
    )
    .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let input = AgentConversationTurnInput {
        conversation_id: Some(ROOT_CONVERSATION_ID.to_string()),
        project_id: Some(PROJECT_ID.to_string()),
        model_id: "model-1".to_string(),
        context_window_indicator_enabled: true,
        content: "Use Agent collaboration to review authentication.".to_string(),
        attachments: Vec::new(),
        skills: Vec::new(),
        title: Some("Harness root".to_string()),
        user_message_id: Some("user-collaboration-harness".to_string()),
        assistant_message_id: Some("assistant-collaboration-harness".to_string()),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    };
    let turn = service
        .start_conversation_turn(input, notifications)
        .unwrap();
    let events = collect_root_until_done(&mut receiver, &turn.run_id).await;
    let root_done = events
        .iter()
        .find(|event| event["params"]["type"] == "done" && event["params"]["runId"] == turn.run_id)
        .unwrap();
    assert_eq!(root_done["params"]["status"], "completed", "{events:#?}");

    wait_for_dispatcher_idle(&database_path).await;
    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    let _ = stop_sender.send(());
    model_server.await.unwrap();

    let requests = requests
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let root_requests = requests
        .iter()
        .filter(|request| !request_text(request).contains("## 子 Agent 协作身份"))
        .collect::<Vec<_>>();
    let child_requests = requests
        .iter()
        .filter(|request| request_text(request).contains("## 子 Agent 协作身份"))
        .collect::<Vec<_>>();
    assert_eq!(root_requests.len(), 8, "requests={requests:#?}");
    assert!(child_requests.len() >= 2, "requests={requests:#?}");
    assert!(
        maximum_active_children.load(Ordering::SeqCst) >= 2,
        "two different child Agents must overlap at the Provider boundary"
    );

    let mut expected_names = mycopilot_core::AGENT_COLLABORATION_TOOL_NAMES
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    expected_names.sort();
    for request in &requests {
        assert_eq!(collaboration_tool_names(request), expected_names);
    }
    let first_root_text = request_text(root_requests[0]);
    assert!(first_root_text.contains("\"agentType\":\"reviewer\""));
    assert!(first_root_text.contains("\"modelConfigId\":\"model-1\""));
    assert!(first_root_text.contains("\"modelConfigId\":\"model-2\""));
    assert!(!first_root_text.contains("PRIVATE_TEMPLATE_INSTRUCTION"));
    for secret_key in [
        "must-not-enter-selector-directory",
        "apiToken",
        "apiKey",
        "apiUrl",
        "providerProfileConfig",
    ] {
        assert!(!first_root_text.contains(secret_key), "leaked {secret_key}");
    }
    assert!(request_text(child_requests[0]).contains("PRIVATE_TEMPLATE_INSTRUCTION"));

    let final_results = tool_results(root_requests.last().unwrap());
    assert_eq!(final_results.len(), 8, "root tool chain={final_results:#?}");
    assert!(final_results[0]["childAgentId"].is_string());
    assert!(final_results[1]["childAgentId"].is_string());
    assert!(final_results[2]["messageId"].is_string());
    assert!(final_results[3]["messageId"].is_string());
    assert!(final_results[4]["agents"].is_array());
    let wait_result = &final_results[5];
    assert!(wait_result["receiptId"].is_string());
    assert!(wait_result["targets"]
        .as_array()
        .is_some_and(|targets| !targets.is_empty()));
    assert_eq!(
        final_results[6]["errorCode"],
        "agent.collaboration.wait_batch_conflict"
    );
    assert_eq!(final_results[6]["category"], "conflict");
    assert!(final_results[6].get("receiptId").is_none());
    assert!(final_results[7]["targetAgentId"].is_string());
    assert_eq!(final_results[7]["status"], "interrupt_requested");

    let root = storage
        .get_agent_node_by_conversation(ROOT_CONVERSATION_ID)
        .unwrap()
        .expect("trusted Turn construction lazily materializes the root Agent");
    let tree = storage.list_agent_tree(&root.agent_id).unwrap();
    assert_eq!(tree.len(), 3);
    let listed_agents = final_results[4]["agents"].as_array().unwrap();
    for node in &tree {
        let listed = listed_agents
            .iter()
            .find(|summary| summary["agentId"] == node.agent_id)
            .expect("list_agents includes every visible tree node");
        let current_latest_activity = storage
            .latest_agent_collaboration_activity_at(&node.root_agent_id, &node.agent_id)
            .unwrap()
            .unwrap_or(node.updated_at);
        let listed_latest_activity = listed["latestActivityAt"].as_i64().unwrap();
        assert!(listed_latest_activity >= node.updated_at);
        assert!(current_latest_activity >= listed_latest_activity);
        assert!(listed.get("updatedAt").is_none());
    }
    let template_child = tree
        .iter()
        .find(|agent| agent.task_name == "security_review")
        .unwrap();
    let template_child_summary = listed_agents
        .iter()
        .find(|summary| summary["agentId"] == template_child.agent_id)
        .unwrap();
    assert!(
        template_child_summary["latestActivityAt"].as_i64().unwrap() > template_child.updated_at
    );
    assert_eq!(
        template_child
            .template_snapshot
            .as_ref()
            .map(|template| template.machine_key.as_str()),
        Some("reviewer")
    );
    assert_eq!(
        template_child
            .model_snapshot
            .as_ref()
            .unwrap()
            .model_config_id,
        "model-1"
    );
    let explicit_model_child = tree
        .iter()
        .find(|agent| agent.task_name == "compatibility_review")
        .unwrap();
    assert_eq!(explicit_model_child.template_snapshot, None);
    assert_eq!(
        explicit_model_child
            .model_snapshot
            .as_ref()
            .unwrap()
            .model_config_id,
        "model-2"
    );
    assert_ne!(template_child.conversation_id, ROOT_CONVERSATION_ID);
    assert_ne!(
        template_child.conversation_id,
        explicit_model_child.conversation_id
    );
    let interrupted_wake_id: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 ORDER BY sequence ASC LIMIT 1",
            [&explicit_model_child.agent_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        storage
            .get_agent_wake(&interrupted_wake_id)
            .unwrap()
            .unwrap()
            .status,
        mycopilot_core::AgentWakeStatus::Interrupted
    );

    let traces = storage
        .list_conversation_turn_traces(ROOT_CONVERSATION_ID)
        .unwrap();
    // This shared scenario is consumed by the strict Rust/TypeScript protocol tests and the
    // renderer AppShell browser fixture. Keep it anchored to facts produced by this real
    // deterministic Provider -> Runtime -> Host adapter -> application-service chain instead of
    // letting the browser invent a parallel orchestration story.
    let round5_scenario: Value = serde_json::from_str(include_str!(
        "../../../../../../packages/protocol/fixtures/agent-collaboration-round5-scenario-v1.json"
    ))
    .unwrap();
    let expected_tool_calls = round5_scenario["harness"]["toolCalls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap())
        .collect::<Vec<_>>();
    let actual_tool_calls = traces
        .iter()
        .flat_map(|trace| trace.items.iter())
        .filter_map(|item| match item {
            mycopilot_core::ConversationTurnTraceItem::ToolCall { tool, .. } => Some(tool.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(actual_tool_calls, expected_tool_calls);
    assert!(
        maximum_active_children.load(Ordering::SeqCst)
            >= round5_scenario["harness"]["minimumParallelChildren"]
                .as_u64()
                .unwrap() as usize
    );
    let expected_children = round5_scenario["harness"]["children"].as_array().unwrap();
    for expected in expected_children {
        let node = tree
            .iter()
            .find(|node| node.task_name == expected["taskName"].as_str().unwrap())
            .expect("shared Round 5 fixture child must be produced by the real Harness");
        assert_eq!(
            node.model_snapshot.as_ref().unwrap().model_config_id,
            expected["modelConfigId"].as_str().unwrap()
        );
        assert_eq!(
            node.template_snapshot
                .as_ref()
                .map(|template| template.machine_key.as_str()),
            expected["templateMachineKey"].as_str()
        );
    }
    let wait_results = traces
        .iter()
        .flat_map(|trace| trace.items.iter())
        .filter_map(|item| match item {
            mycopilot_core::ConversationTurnTraceItem::ToolResult {
                tool,
                success,
                observation,
                ..
            } if tool == "wait_agent" => Some((*success, observation)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        wait_results.len(),
        2,
        "the precommitted first wait and ordinary failed second wait each append once"
    );
    assert_eq!(
        wait_results
            .iter()
            .filter(|(success, observation)| *success && observation["receiptId"].is_string())
            .count(),
        1,
        "the durable wait receipt must not be reused or double-appended"
    );
    assert_eq!(
        wait_results
            .iter()
            .filter(|(success, observation)| {
                !*success && observation["errorCode"] == "agent.collaboration.wait_batch_conflict"
            })
            .count(),
        1,
        "the second same-batch wait must be a durable ordinary failed ToolResult"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn process_start_dispatcher_recovers_a_queued_child_without_a_new_root_turn() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_requests = Arc::clone(&requests);
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let request = read_provider_request(&mut stream).await;
                    server_requests
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(request);
                    write_text(&mut stream, "Recovered child completed its queued task.").await;
                }
            }
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("collaboration-startup-recovery.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: PROJECT_ID.to_string(),
            name: "Collaboration startup recovery".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let root_conversation_id = "conversation-collaboration-recovery-root";
    let root_agent_id = "agent-collaboration-recovery-root";
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: Some(PROJECT_ID.to_string()),
            model_id: Some("model-1".to_string()),
            title: "Recovery root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: root_agent_id.to_string(),
            conversation_id: root_conversation_id.to_string(),
            creation_request_id: "ensure-collaboration-recovery-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    let child =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage))
            .create_child(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-before-process-restart".to_string(),
                task_name: "startup_recovery".to_string(),
                task: "Complete this task after the next process starts.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();
    assert_eq!(
        child.initial_wake.status,
        mycopilot_core::AgentWakeStatus::Queued
    );

    // This fresh AgentService represents a process that did not observe the transaction which
    // created the queued Wake. Startup must scan SQLite instead of relying on an in-memory event.
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        2,
    )
    .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_collaboration_dispatcher(notifications)
        .unwrap();
    let terminal = wait_for_terminal_wake(&storage, &child.initial_wake.wake_id).await;
    assert_eq!(terminal.status, mycopilot_core::AgentWakeStatus::Completed);
    wait_for_dispatcher_idle(&database_path).await;
    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    let _ = stop_sender.send(());
    model_server.await.unwrap();

    let requests = requests
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    assert_eq!(requests.len(), 1, "only the recovered child should sample");
    assert!(request_text(&requests[0]).contains("## 子 Agent 协作身份"));
    let mut expected_names = mycopilot_core::AGENT_COLLABORATION_TOOL_NAMES
        .iter()
        .map(|name| name.to_string())
        .collect::<Vec<_>>();
    expected_names.sort();
    assert_eq!(collaboration_tool_names(&requests[0]), expected_names);
    assert!(storage
        .list_conversation_turn_traces(root_conversation_id)
        .unwrap()
        .is_empty());
}
