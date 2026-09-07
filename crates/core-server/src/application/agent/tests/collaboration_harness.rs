use super::*;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

include!("collaboration_wait_terminal.rs");

const ROOT_CONVERSATION_ID: &str = "conversation-collaboration-harness";
const PROJECT_ID: &str = "project-collaboration-harness";

fn first_runtime_tool_call_id(run_id: &str, provider_call_id: &str) -> String {
    const HASH_DOMAIN: &[u8] = b"mycopilot:model-tool-call-id:v1";
    const RESPONSE_DOMAIN: &[u8] = b"model-response";

    fn hash_field(hasher: &mut Sha256, value: &[u8]) {
        hasher.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
        hasher.update(value);
    }

    let mut hasher = Sha256::new();
    hash_field(&mut hasher, HASH_DOMAIN);
    hash_field(&mut hasher, RESPONSE_DOMAIN);
    hash_field(&mut hasher, run_id.as_bytes());
    hash_field(&mut hasher, &0_u64.to_be_bytes());
    hash_field(&mut hasher, &0_u64.to_be_bytes());
    hash_field(&mut hasher, provider_call_id.as_bytes());
    format!("tc1_{}", URL_SAFE_NO_PAD.encode(hasher.finalize()))
}

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
    let usage = json!({
        "choices": [],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    });
    stream
        .write_all(
            format!("data: {frame}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n")
                .as_bytes(),
        )
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
    let terminal = tokio::time::timeout(Duration::from_secs(10), async {
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
    .await;
    match terminal {
        Ok(wake) => wake,
        Err(_) => {
            let wake = storage
                .get_agent_wake(wake_id)
                .unwrap()
                .expect("the timed-out Wake must remain queryable");
            let trace = wake
                .assistant_message_id
                .as_deref()
                .and_then(|message_id| storage.get_conversation_turn_trace(message_id).unwrap());
            let sessions = trace
                .as_ref()
                .map(|trace| {
                    storage
                        .list_agent_command_sessions(&trace.conversation_id, 16)
                        .unwrap()
                })
                .unwrap_or_default();
            panic!(
                "recovered durable Wake did not reach a terminal state: wake={wake:?}, trace={trace:?}, sessions={sessions:?}"
            );
        }
    }
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
    let child_ready = Arc::new(tokio::sync::Barrier::new(2));
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
                            let arrival = arrivals.fetch_add(1, Ordering::SeqCst) + 1;
                            if arrival <= 2 {
                                tokio::time::timeout(Duration::from_secs(5), ready.wait())
                                    .await
                                    .expect("two child samples must overlap through the real Dispatcher");
                            }
                            if child_text.contains("compatibility_review") {
                                // Keep the explicitly-model-selected child inside a live Provider
                                // sample. The root's interrupt_agent call must cancel this real
                                // Turn; a fake terminal response would only cover no_active_turn.
                                std::future::pending::<()>().await;
                            }
                            let child_result = if child_text.contains("security_review") {
                                format!(
                                    "Child completed the delegated review with evidence. {}",
                                    "evidence ".repeat(3_000)
                                )
                            } else {
                                "Child completed the delegated review.".to_string()
                            };
                            write_text(&mut stream, &child_result).await;
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
    second_model.provider_model_id = "model-2".to_string();
    second_model.display_name = "Model 2".to_string();
    second_model.supports_image = true;
    settings.models.push(second_model);
    storage.save_model_settings(settings).unwrap();
    storage
        .create_agent_template(&mycopilot_core::CreateAgentTemplateInput {
            template_id: "template-reviewer".to_string(),
            machine_key: "reviewer".to_string(),
            name: "Reviewer".to_string(),
            description: "Review a delegated boundary".to_string(),
            instructions: "PRIVATE_TEMPLATE_INSTRUCTION: return concrete evidence.".to_string(),
            model_config_id: "model-1".to_string(),
            enabled: true,
        })
        .unwrap();
    storage
        .set_agent_template_project_assignment(PROJECT_ID, "template-reviewer", true)
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
    assert!(events
        .iter()
        .all(|event| { event["params"]["code"] != "conversation_trace_persistence_failed" }));

    wait_for_dispatcher_idle(&database_path).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if service.turn_concurrency_gate().active() == 0
                && storage
                    .list_in_progress_conversation_turn_traces()
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("terminal collaboration run leaked a durable Turn or concurrency permit");
    assert!(!service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(&turn.run_id));
    let root_usage = storage
        .load_agent_usage_for_owner(
            &turn.run_id,
            ROOT_CONVERSATION_ID,
            "assistant-collaboration-harness",
        )
        .unwrap()
        .expect("large precommitted wait terminalization must persist root Usage");
    assert!(root_usage.total_tokens.is_some_and(|tokens| tokens > 0));
    assert!(root_usage.billable_request_count > 0);
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
    assert!(first_root_text.contains("\"defaultModelCapabilities\":{\"imageInput\":false}"));
    assert!(first_root_text.contains(
        "\"modelConfigId\":\"model-2\",\"displayName\":\"Model 2\",\"capabilities\":{\"imageInput\":true}"
    ));
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
    let template_child_request = child_requests
        .iter()
        .find(|request| request_text(request).contains("security_review"))
        .expect("the template-selected child request must be present");
    assert!(request_text(template_child_request).contains("PRIVATE_TEMPLATE_INSTRUCTION"));

    let final_results = tool_results(root_requests.last().unwrap());
    assert_eq!(final_results.len(), 8, "root tool chain={final_results:#?}");
    assert!(final_results[0]["childAgentId"].is_string());
    assert!(final_results[1]["childAgentId"].is_string());
    assert_eq!(final_results[0]["modelCapabilities"]["imageInput"], false);
    assert_eq!(final_results[1]["modelCapabilities"]["imageInput"], true);
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
    // A child can only exist after a trusted root Turn admitted the spawn Tool. Reproduce that
    // production invariant so startup recovery exercises the queued child, rather than an
    // impossible graph whose parent never committed an effective-permission snapshot.
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "startup-recovery-root",
        AgentPermissions::default(),
    );
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
    assert_eq!(
        terminal.status,
        mycopilot_core::AgentWakeStatus::Completed,
        "startup recovery terminal error: {:?}",
        terminal.terminal_error
    );
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
    let root_traces = storage
        .list_conversation_turn_traces(root_conversation_id)
        .unwrap();
    assert_eq!(root_traces.len(), 1, "startup cannot open a new root Turn");
    assert_eq!(
        root_traces[0].assistant_message_id,
        "assistant-permission-seed-startup-recovery-root"
    );
    assert_eq!(
        root_traces[0].terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_root_run_cancellation_stops_running_and_queued_descendants() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (child_arrived_sender, child_arrived_receiver) = tokio::sync::oneshot::channel();
    let (stop_sender, stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_provider_request(&mut stream).await;
        assert!(request_text(&request).contains("## 子 Agent 协作身份"));
        let _ = child_arrived_sender.send(());
        let _ = stop_receiver.await;
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("collaboration-root-stop.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: PROJECT_ID.to_string(),
            name: "Collaboration root stop".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    let root_conversation_id = "conversation-collaboration-root-stop";
    let root_agent_id = "agent-collaboration-root-stop";
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: Some(PROJECT_ID.to_string()),
            model_id: Some("model-1".to_string()),
            title: "Root stop".to_string(),
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
            creation_request_id: "ensure-collaboration-root-stop".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "root-stop",
        AgentPermissions::default(),
    );
    let root_run_id = "run-collaboration-root-stop";
    let root_assistant_message_id = "assistant-collaboration-root-stop";
    storage
        .upsert_chat_messages(
            root_conversation_id,
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: root_assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 2,
                status: Some("streaming".to_string()),
                attachments: Vec::new(),
                agent_run_json: None,
                ui_state_json: None,
            }],
            0,
        )
        .unwrap();
    assert!(storage
        .append_in_progress_conversation_turn_trace(
            &mycopilot_core::ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: root_run_id.to_string(),
                conversation_id: root_conversation_id.to_string(),
                assistant_message_id: root_assistant_message_id.to_string(),
                terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
                terminal_error: None,
                truncated: false,
                items: Vec::new(),
            },
            2,
            2,
        )
        .unwrap());
    let factory =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage));
    let running_child = factory
        .create_child_with_expected_selector_from_run(
            &mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-running-before-root-stop".to_string(),
                task_name: "running_child".to_string(),
                task: "Remain inside the provider request until cancelled.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            },
            None,
            None,
            root_run_id,
        )
        .unwrap();
    let queued_child = factory
        .create_child_with_expected_selector_from_run(
            &mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-queued-before-root-stop".to_string(),
                task_name: "queued_child".to_string(),
                task: "This Wake must be cancelled before admission.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            },
            None,
            None,
            root_run_id,
        )
        .unwrap();

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
    tokio::time::timeout(Duration::from_secs(10), child_arrived_receiver)
        .await
        .expect("the first child never reached the Provider")
        .expect("the child arrival signal was dropped");

    let root_cancellation = AgentCancellationToken::new();
    service.register_cancellation(root_run_id, root_cancellation.clone());
    service.register_active_run_control(
        root_run_id,
        root_conversation_id,
        root_assistant_message_id,
        Some(PROJECT_ID),
        ModelCapabilities::default(),
        AgentPermissions::default(),
    );

    assert!(service.cancel_run(root_run_id));
    assert!(root_cancellation.is_cancelled());
    assert_eq!(
        storage
            .get_agent_wake(&queued_child.initial_wake.wake_id)
            .unwrap()
            .unwrap()
            .status,
        mycopilot_core::AgentWakeStatus::Cancelled
    );
    let durable_stop = storage
        .get_agent_tree_run_stop(root_run_id)
        .unwrap()
        .expect("root stop must be durable");
    assert_eq!(durable_stop.root_agent_id, root_agent_id);
    assert_eq!(durable_stop.root_run_id, root_run_id);

    // Stop-first scheduling is rejected inside the same SQLite write transaction. No child
    // identity, message, or Wake can escape if the process exits immediately after this call.
    assert!(factory
        .create_child_with_expected_selector_from_run(
            &mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-after-root-stop".to_string(),
                task_name: "late_child".to_string(),
                task: "This late Wake must never be committed.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            },
            None,
            None,
            root_run_id,
        )
        .is_err());

    assert!(
        crate::application::agent_collaboration::AgentMessagingService::new(Arc::clone(&storage))
            .follow_up_from_run(
                &mycopilot_core::SendAgentMessageRequest {
                    sender_agent_id: root_agent_id.to_string(),
                    recipient_agent_id: queued_child.agent.agent_id.clone(),
                    request_id: "followup-after-root-stop".to_string(),
                    content: "This late follow-up Wake must never be committed.".to_string(),
                },
                root_run_id,
            )
            .is_err()
    );

    let running_terminal =
        wait_for_terminal_wake(&storage, &running_child.initial_wake.wake_id).await;
    assert_eq!(
        running_terminal.status,
        mycopilot_core::AgentWakeStatus::Interrupted,
        "running descendant cancellation failed: {:?}",
        running_terminal.terminal_error
    );
    wait_for_dispatcher_idle(&database_path).await;
    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .iter()
        .all(|trace| trace.run_id == root_run_id));
    storage
        .reconcile_orphaned_in_progress_conversation_turn_traces(
            &std::collections::HashSet::new(),
            mycopilot_core::storage::now_ms(),
        )
        .unwrap();
    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());
    service.unregister_cancellation_if_current(root_run_id, &root_cancellation);
    assert!(service.agent_tree_run_is_stopped(root_run_id).unwrap());
    assert!(!service
        .agent_tree_run_is_stopped("run-collaboration-root-stop-future")
        .unwrap());

    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    let _ = stop_sender.send(());
    model_server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_agent_stops_a_child_waiting_on_a_handed_off_command_session() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_requests = Arc::clone(&requests);
    let interrupt_gate = Arc::new(tokio::sync::Notify::new());
    let server_interrupt_gate = Arc::clone(&interrupt_gate);
    let (session_sender, session_receiver) = tokio::sync::oneshot::channel::<String>();
    let session_sender = Arc::new(Mutex::new(Some(session_sender)));
    let server_session_sender = Arc::clone(&session_sender);
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let requests = Arc::clone(&server_requests);
                    let interrupt_gate = Arc::clone(&server_interrupt_gate);
                    let session_sender = Arc::clone(&server_session_sender);
                    tokio::spawn(async move {
                        let request = read_provider_request(&mut stream).await;
                        requests
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .push(request.clone());
                        let results = tool_results(&request);
                        if request_text(&request).contains("## 子 Agent 协作身份") {
                            if results.is_empty() {
                                write_tool_call(
                                    &mut stream,
                                    "call-child-long-command",
                                    "run_command",
                                    json!({
                                        "command": "sleep 30",
                                        "reason": "hold a real managed command Session open until interrupted"
                                    }),
                                )
                                .await;
                                return;
                            }

                            let running_receipt = results
                                .iter()
                                .find(|result| {
                                    result["status"] == "running"
                                        && result["continueWith"]["tool"] == "command_session"
                                })
                                .expect("child continuation must contain the running command receipt");
                            let session_id = running_receipt["sessionId"]
                                .as_str()
                                .expect("running command receipt must own a Session")
                                .to_string();
                            session_sender
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .take()
                                .expect("the child Session is reported exactly once")
                                .send(session_id.clone())
                                .expect("the test must still be waiting for the child Session");
                            write_tool_call(
                                &mut stream,
                                "call-child-command-wait",
                                "command_session",
                                json!({ "sessionId": session_id, "action": "wait" }),
                            )
                            .await;
                            return;
                        }

                        match results.len() {
                            0 => {
                                write_tool_call(
                                    &mut stream,
                                    "call-spawn-command-child",
                                    "spawn_agent",
                                    json!({
                                        "task_name": "command_wait_child",
                                        "message": "Start the requested long command, then wait for its Session result.",
                                        "fork_turns": "none"
                                    }),
                                )
                                .await;
                            }
                            1 => {
                                let child_agent_id = results[0]["childAgentId"]
                                    .as_str()
                                    .expect("spawn result must contain the child Agent identity")
                                    .to_string();
                                interrupt_gate.notified().await;
                                write_tool_call(
                                    &mut stream,
                                    "call-interrupt-command-child",
                                    "interrupt_agent",
                                    json!({ "target": child_agent_id }),
                                )
                                .await;
                            }
                            2 => {
                                assert_eq!(results[1]["status"], "interrupt_requested");
                                write_text(&mut stream, "The delegated command child was interrupted.")
                                    .await;
                            }
                            count => panic!("unexpected root collaboration result count: {count}"),
                        }
                    });
                }
            }
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture
        .path()
        .join("collaboration-command-interrupt.sqlite");
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let project_id = "project-collaboration-command-interrupt";
    let root_conversation_id = "conversation-collaboration-command-interrupt";
    storage
        .save_project(ProjectRecord {
            id: project_id.to_string(),
            name: "Collaboration command interrupt".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let mut service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        2,
    )
    .unwrap();
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(20),
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(root_conversation_id.to_string()),
                project_id: Some(project_id.to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Delegate a command lifecycle check, then interrupt the child."
                    .to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: Some("Command interruption root".to_string()),
                user_message_id: Some("user-collaboration-command-interrupt".to_string()),
                assistant_message_id: Some("assistant-collaboration-command-interrupt".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::AutoApprove,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications,
        )
        .unwrap();

    let session_id = tokio::time::timeout(Duration::from_secs(10), session_receiver)
        .await
        .expect("child never handed off its long-running command")
        .expect("child provider dropped the Session identity");
    let child_run_id = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notification = receiver.recv().await.expect("Agent event channel closed");
            if notification["params"]["type"] == "tool_call"
                && notification["params"]["call"]["tool"] == "command_session"
            {
                break notification["params"]["runId"]
                    .as_str()
                    .expect("child ToolCall carries its Run identity")
                    .to_string();
            }
        }
    })
    .await
    .expect("child never entered command_session wait");
    let running_session: (String, String) = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT origin_run_id, status
             FROM agent_command_sessions WHERE session_id = ?1",
            [&session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(running_session.0, child_run_id);
    assert_eq!(running_session.1, "running");
    let child_cancellation = service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&child_run_id)
        .cloned()
        .expect("the live child Runtime must retain its registered cancellation token");

    interrupt_gate.notify_one();
    let root_events = collect_root_until_done(&mut receiver, &turn.run_id).await;
    let root_done = root_events
        .iter()
        .find(|event| event["params"]["type"] == "done" && event["params"]["runId"] == turn.run_id)
        .expect("root Agent must complete after dispatching interrupt_agent");
    assert_eq!(root_done["params"]["status"], "completed");
    assert!(
        child_cancellation.is_cancelled(),
        "interrupt_agent must signal the exact live child Runtime token"
    );

    let root = storage
        .get_agent_node_by_conversation(root_conversation_id)
        .unwrap()
        .expect("root Agent must exist");
    let child = storage
        .list_agent_tree(&root.agent_id)
        .unwrap()
        .into_iter()
        .find(|agent| agent.task_name == "command_wait_child")
        .expect("spawned command child must exist");
    let child_wake_id: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT wake_id FROM agent_wake_requests
             WHERE agent_id = ?1 AND run_id = ?2",
            [&child.agent_id, &child_run_id],
            |row| row.get(0),
        )
        .unwrap();
    let terminal_wake = wait_for_terminal_wake(&storage, &child_wake_id).await;
    assert_eq!(
        terminal_wake.status,
        mycopilot_core::AgentWakeStatus::Interrupted,
        "child Wake did not settle as interrupted: {:?}",
        terminal_wake.terminal_error
    );

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let session = storage
                .load_agent_command_session(&child.conversation_id, &session_id)
                .unwrap()
                .expect("the handed-off Session remains durably inspectable");
            let trace = storage
                .get_conversation_turn_trace(
                    terminal_wake
                        .assistant_message_id
                        .as_deref()
                        .expect("admitted Wake owns an assistant message"),
                )
                .unwrap()
                .expect("child Turn keeps its durable Trace");
            if session.snapshot.status == AgentCommandSessionStatus::Interrupted
                && trace.terminal_status == ConversationTurnTraceTerminalStatus::Cancelled
                && service.turn_concurrency_gate().active() == 0
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("interrupt_agent did not settle Session, Trace, and concurrency ownership");

    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());
    let parent_wake_count: i64 = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM agent_wake_requests WHERE agent_id = ?1",
            [&root.agent_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        parent_wake_count, 0,
        "child settlement must not wake the root"
    );

    let request_count_after_settlement = requests
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .len();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        request_count_after_settlement,
        "interrupted child settlement must not start another Provider request"
    );
    assert_eq!(request_count_after_settlement, 5);

    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    let _ = stop_sender.send(());
    model_server.await.unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChildApprovalInterruptWindow {
    PendingStore,
    PublicationArbitration,
    WaitingPublication,
    WaitingPersistence,
}

impl ChildApprovalInterruptWindow {
    fn advances_approval(self) -> bool {
        matches!(self, Self::WaitingPublication | Self::WaitingPersistence)
    }
}

async fn assert_child_approval_handoff_linearizes(window: ChildApprovalInterruptWindow) {
    const PROVIDER_CALL_ID: &str = "call-child-approval-handoff";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_requests = Arc::clone(&requests);
    let (provider_arrived_sender, provider_arrived_receiver) = tokio::sync::oneshot::channel();
    let (continuation_arrived_sender, continuation_arrived_receiver) =
        tokio::sync::oneshot::channel();
    let provider_release = Arc::new(tokio::sync::Notify::new());
    let server_provider_release = Arc::clone(&provider_release);
    let server_window = window;
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        let mut provider_arrived_sender = Some(provider_arrived_sender);
        let mut continuation_arrived_sender = Some(continuation_arrived_sender);
        let mut request_index = 0_usize;
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let request = read_provider_request(&mut stream).await;
                    assert!(request_text(&request).contains("## 子 Agent 协作身份"));
                    server_requests
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .push(request.clone());
                    match request_index {
                        0 => {
                            provider_arrived_sender
                                .take()
                                .expect("the approval child makes exactly one initial request")
                                .send(())
                                .expect("the test must still be waiting for the Provider request");
                            server_provider_release.notified().await;
                            let command = if server_window.advances_approval() {
                                "printf 'approval advanced\\n'"
                            } else {
                                "sleep 30"
                            };
                            write_tool_call(
                                &mut stream,
                                PROVIDER_CALL_ID,
                                "run_command",
                                json!({
                                    "command": command,
                                    "reason": "hold the Runtime at the manual approval handoff"
                                }),
                            )
                            .await;
                        }
                        1 if server_window.advances_approval() => {
                            assert!(tool_results(&request).iter().any(|result| {
                                result["status"] == "exited" && result["exitCode"] == 0
                            }));
                            continuation_arrived_sender
                                .take()
                                .expect("the approved child starts exactly one continuation")
                                .send(())
                                .expect("the test must still be waiting for the continuation");
                            write_text(&mut stream, "Approved child continuation completed.").await;
                        }
                        count => panic!("unexpected approval child Provider request {count}"),
                    }
                    request_index += 1;
                }
            }
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("collaboration-approval-handoff.sqlite");
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let project_id = "project-collaboration-approval-handoff";
    let root_conversation_id = "conversation-collaboration-approval-handoff-root";
    let root_agent_id = "agent-collaboration-approval-handoff-root";
    storage
        .save_project(ProjectRecord {
            id: project_id.to_string(),
            name: "Collaboration approval handoff".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("model-1".to_string()),
            title: "Approval handoff root".to_string(),
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
            creation_request_id: "ensure-collaboration-approval-handoff-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "approval-handoff",
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            ..AgentPermissions::default()
        },
    );
    let child =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage))
            .create_child(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-approval-handoff-child".to_string(),
                task_name: "approval_handoff_child".to_string(),
                task: "Request the command and wait for explicit approval.".to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_collaboration_dispatcher(notifications.clone())
        .unwrap();

    tokio::time::timeout(Duration::from_secs(10), provider_arrived_receiver)
        .await
        .expect("child Runtime never reached the fake Provider")
        .expect("the Provider arrival sender was dropped");

    let handoff_wake = storage
        .get_agent_wake(&child.initial_wake.wake_id)
        .unwrap()
        .expect("the admitted child Wake remains durable");
    let child_run_id = handoff_wake
        .run_id
        .clone()
        .expect("the approval handoff must occur after exact Turn admission");
    let assistant_message_id = handoff_wake
        .assistant_message_id
        .clone()
        .expect("the admitted child Wake owns an assistant message");
    let action_id = first_runtime_tool_call_id(&child_run_id, PROVIDER_CALL_ID);
    let (hook_entered_sender, hook_entered_receiver) = std::sync::mpsc::sync_channel(0);
    let (hook_release_sender, hook_release_receiver) = std::sync::mpsc::sync_channel(0);
    let hook_release_receiver = Arc::new(Mutex::new(hook_release_receiver));
    let hook_receiver = Arc::clone(&hook_release_receiver);
    let hook = Arc::new(move || {
        let _ = hook_entered_sender.send(());
        let _ = hook_receiver
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .recv();
    });
    match window {
        ChildApprovalInterruptWindow::PendingStore => {
            super::super::turn_executor::install_before_pending_action_store_hook(&action_id, hook);
        }
        ChildApprovalInterruptWindow::PublicationArbitration => {
            super::super::turn_executor::install_before_approval_publication_arbitration_hook(
                &action_id, hook,
            );
        }
        ChildApprovalInterruptWindow::WaitingPublication => {
            super::super::turn_executor::install_before_waiting_publication_arbitration_hook(
                &child_run_id,
                hook,
            );
        }
        ChildApprovalInterruptWindow::WaitingPersistence => {
            super::super::run_lifecycle::install_before_waiting_persistence_hook(
                &child_run_id,
                hook,
            );
        }
    }
    provider_release.notify_one();
    tokio::task::spawn_blocking(move || {
        hook_entered_receiver.recv_timeout(Duration::from_secs(10))
    })
    .await
    .expect("approval handoff observer task panicked")
    .expect("child Runtime never reached the selected approval handoff");

    let child_cancellation = service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&child_run_id)
        .cloned()
        .expect("the child Runtime token must remain registered during the handoff");
    let events_before_cancellation =
        std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    let pending_was_visible_before_interrupt = service
        .list_pending_actions()
        .iter()
        .any(|pending| pending.run_id == child_run_id && pending.action_id == action_id);

    if window.advances_approval() {
        assert!(
            pending_was_visible_before_interrupt,
            "the selected waiting hook must run after pending publication"
        );
        assert!(
            events_before_cancellation.iter().any(|event| {
                event["params"]["runId"] == child_run_id
                    && event["params"]["type"] == "approval_required"
            }),
            "the approval must be published before advancing it: {events_before_cancellation:#?}"
        );

        let storage_id = pending_action_storage_id(&child_run_id, &action_id);
        let decision = service
            .decide_root_projected_approval(
                root_conversation_id,
                &storage_id,
                ProjectedApprovalDecision::Approve,
                None,
                notifications,
            )
            .expect("the root must advance the real child approval");
        assert!(decision.accepted);
        tokio::time::timeout(Duration::from_secs(10), continuation_arrived_receiver)
            .await
            .expect("approved child continuation never reached the Provider")
            .expect("approved child continuation sender was dropped");
        hook_release_sender
            .send(())
            .expect("the old waiting segment hook must still await release");

        let terminal_wake = wait_for_terminal_wake(&storage, &child.initial_wake.wake_id).await;
        assert_eq!(
            terminal_wake.status,
            mycopilot_core::AgentWakeStatus::Completed,
            "the approved continuation must own the final child outcome: {:?}",
            terminal_wake.terminal_error
        );
        wait_for_dispatcher_idle(&database_path).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if service.turn_concurrency_gate().active() == 0
                    && storage
                        .list_in_progress_conversation_turn_traces()
                        .unwrap()
                        .is_empty()
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("approved continuation did not release Trace and concurrency ownership");

        let trace = storage
            .get_conversation_turn_trace(&assistant_message_id)
            .unwrap()
            .expect("the approved child Turn keeps its durable Trace");
        trace.validate().unwrap();
        assert_eq!(
            trace.terminal_status,
            ConversationTurnTraceTerminalStatus::Completed
        );
        let conversation = storage
            .load_conversations()
            .unwrap()
            .into_iter()
            .find(|conversation| conversation.id == child.agent.conversation_id)
            .expect("the approved child conversation remains queryable");
        let assistant = conversation
            .messages
            .iter()
            .find(|message| message.id == assistant_message_id)
            .expect("the approved child assistant message remains queryable");
        assert_eq!(assistant.status.as_deref(), Some("sent"));
        assert_eq!(assistant.content, "Approved child continuation completed.");

        let durable_pending: (String, Option<String>) = rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(durable_pending.0, "completed");
        assert_eq!(durable_pending.1.as_deref(), Some("completed"));
        assert!(service.list_pending_actions().is_empty());

        let usage = storage
            .load_agent_usage_for_owner(
                &child_run_id,
                &child.agent.conversation_id,
                &assistant_message_id,
            )
            .unwrap()
            .expect("the approved child Turn keeps its Usage row");
        assert_eq!(usage.status.as_deref(), Some("completed"));
        assert!(usage.completed_at.is_some());
        assert_eq!(usage.input_tokens, Some(20));
        assert_eq!(usage.output_tokens, Some(10));
        assert_eq!(usage.total_tokens, Some(30));
        assert_eq!(usage.billable_request_count, 2);
        assert_eq!(service.turn_concurrency_gate().active(), 0);
        assert!(storage
            .list_in_progress_conversation_turn_traces()
            .unwrap()
            .is_empty());

        tokio::time::sleep(Duration::from_millis(50)).await;
        let events_after_approval =
            std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
        assert!(events_after_approval.iter().all(|event| {
            let params = &event["params"];
            params["runId"] != child_run_id
                || !matches!(params["type"].as_str(), Some("state" | "done"))
                || params["status"] != "waiting_for_approval"
        }), "advanced approval published a stale WaitingForApproval event: {events_after_approval:#?}");
        assert_eq!(
            requests
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len(),
            2,
            "the approved child must make exactly one continuation request"
        );

        assert!(service
            .shutdown_collaboration_dispatcher()
            .await
            .unwrap()
            .is_some());
        let _ = stop_sender.send(());
        model_server.await.unwrap();
        return;
    }

    let interrupt = service.interrupt_agent_wake_run(&child_run_id);
    let token_was_cancelled = child_cancellation.is_cancelled();
    hook_release_sender
        .send(())
        .expect("the approval handoff hook must still be waiting for release");

    assert_eq!(
        handoff_wake.status,
        mycopilot_core::AgentWakeStatus::Running,
        "the interrupt must land before the Dispatcher observes WaitingForApproval"
    );
    assert_eq!(
        pending_was_visible_before_interrupt,
        window == ChildApprovalInterruptWindow::PublicationArbitration,
        "the selected hook must prove its exact side of pending-action publication"
    );
    assert!(service.list_pending_actions().is_empty());
    assert!(
        interrupt
            .expect("child interruption must not fail")
            .turn_termination_confirmed(),
        "the exact live child Runtime token must confirm the interruption"
    );
    assert!(token_was_cancelled);

    let terminal_wake = wait_for_terminal_wake(&storage, &child.initial_wake.wake_id).await;
    assert_eq!(
        terminal_wake.status,
        mycopilot_core::AgentWakeStatus::Interrupted,
        "the cancelled approval handoff must not strand the child Wake: {:?}",
        terminal_wake.terminal_error
    );
    wait_for_dispatcher_idle(&database_path).await;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if service.turn_concurrency_gate().active() == 0
                && storage
                    .list_in_progress_conversation_turn_traces()
                    .unwrap()
                    .is_empty()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("approval cancellation did not release Trace and concurrency ownership");

    let trace = storage
        .get_conversation_turn_trace(&assistant_message_id)
        .unwrap()
        .expect("the interrupted child Turn keeps its durable Trace");
    trace.validate().unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult {
            call_id,
            approval_status: AgentApprovalStatus::Rejected,
            ..
        } if call_id == &action_id
    )));

    let conversation = storage
        .load_conversations()
        .unwrap()
        .into_iter()
        .find(|conversation| conversation.id == child.agent.conversation_id)
        .expect("the child conversation remains queryable");
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .expect("the child assistant message remains queryable");
    assert_eq!(assistant.status.as_deref(), Some("sent"));
    assert_eq!(assistant.content, "");

    let durable_pending: (String, Option<String>) = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            [pending_action_storage_id(&child_run_id, &action_id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(durable_pending.0, "cancelled");
    assert_eq!(durable_pending.1.as_deref(), Some("cancelled"));
    assert!(service.list_pending_actions().is_empty());

    let usage = storage
        .load_agent_usage_for_owner(
            &child_run_id,
            &child.agent.conversation_id,
            &assistant_message_id,
        )
        .unwrap()
        .expect("the child Turn keeps its Usage row");
    assert_eq!(usage.status.as_deref(), Some("cancelled"));
    assert!(usage.completed_at.is_some());
    assert_eq!(service.turn_concurrency_gate().active(), 0);
    assert!(storage
        .list_in_progress_conversation_turn_traces()
        .unwrap()
        .is_empty());

    tokio::time::sleep(Duration::from_millis(50)).await;
    let events_after_cancellation =
        std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events_before_cancellation.iter().all(|event| {
        event["params"]["runId"] != child_run_id
            || event["params"]["status"] != "waiting_for_approval"
    }));
    assert!(events_after_cancellation.iter().all(|event| {
        let params = &event["params"];
        params["runId"] != child_run_id
            || !matches!(params["type"].as_str(), Some("state" | "done"))
            || params["status"] != "waiting_for_approval"
    }), "cancelled child published a stale WaitingForApproval event: {events_after_cancellation:#?}");
    assert!(events_after_cancellation.iter().all(|event| {
        event["params"]["runId"] != child_run_id || event["params"]["type"] != "approval_required"
    }));
    assert_eq!(
        requests
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1,
        "the cancelled approval must not start a Provider continuation"
    );

    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    let _ = stop_sender.send(());
    model_server.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_agent_closes_a_child_approval_handoff_before_pending_store() {
    assert_child_approval_handoff_linearizes(ChildApprovalInterruptWindow::PendingStore).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn interrupt_agent_closes_a_child_approval_handoff_before_publication_arbitration() {
    assert_child_approval_handoff_linearizes(ChildApprovalInterruptWindow::PublicationArbitration)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advanced_child_approval_prevents_the_old_segment_from_rewriting_waiting() {
    assert_child_approval_handoff_linearizes(ChildApprovalInterruptWindow::WaitingPersistence)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn advanced_child_approval_retires_obsolete_waiting_notifications() {
    assert_child_approval_handoff_linearizes(ChildApprovalInterruptWindow::WaitingPublication)
        .await;
}
