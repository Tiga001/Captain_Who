use super::*;
use crate::application::human_interaction::HumanInteractionService;
use mycopilot_core::human_interaction::{
    HumanInteractionAnswer, HumanInteractionListInput, HumanInteractionRequestSnapshot,
    HumanInteractionRequestStatus, HumanInteractionSubmitInput,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const NARRATION: &str = "The synthetic question needs an answer 🐈";
const ANSWER: &str = "Keep the exact original task identity.";
const FINAL: &str = "Synthetic human continuation completed.";

async fn human_provider() -> (
    String,
    Arc<Mutex<Vec<Value>>>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        loop {
            let (mut socket, _) = tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => accepted.unwrap(),
            };
            let mut bytes = Vec::new();
            let mut buffer = [0; 8192];
            let request = loop {
                let count = socket.read(&mut buffer).await.unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let length = String::from_utf8_lossy(&bytes[..end])
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    if bytes.len() >= end + 4 + length {
                        break serde_json::from_slice::<Value>(&bytes[end + 4..end + 4 + length])
                            .unwrap();
                    }
                }
            };
            let index = {
                let mut requests = captured.lock().unwrap();
                requests.push(request);
                requests.len() - 1
            };
            assert!(
                index < 2,
                "no duplicated answer or model request is allowed"
            );
            let (delta, reason) = if index == 0 {
                (
                    serde_json::json!({"role":"assistant","content":NARRATION,"tool_calls":[{
                        "index":0,"id":"synthetic-human-question","type":"function","function":{
                            "name":"request_user_input","arguments":serde_json::json!({"questions":[{"title":"Synthetic missing detail"}]}).to_string()
                        }
                    }]}),
                    "tool_calls",
                )
            } else {
                (
                    serde_json::json!({"role":"assistant","content":FINAL}),
                    "stop",
                )
            };
            let frame = serde_json::json!({"choices":[{"delta":delta,"finish_reason":null}]});
            let finish = serde_json::json!({"choices":[{"delta":{},"finish_reason":reason}]});
            let usage = serde_json::json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}});
            let body =
                format!("data: {frame}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n");
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len());
            socket.write_all(response.as_bytes()).await.unwrap();
        }
    });
    (url, requests, stop, task)
}

fn setup_human_storage(
    database: &std::path::Path,
    workspace: &std::path::Path,
    credentials: Arc<mycopilot_core::image_generation::InMemoryCredentialStore>,
    url: String,
) -> Arc<StorageService> {
    let storage =
        Arc::new(StorageService::open_with_model_credentials(database, credentials).unwrap());
    storage
        .save_project(
            mycopilot_core::storage::models::ProjectRecord::with_primary_folder(
                "human-checkpoint-project",
                "Synthetic human checkpoint",
                workspace.to_string_lossy().into_owned(),
                1,
            ),
        )
        .unwrap();
    storage
        .save_model_settings(mycopilot_core::storage::models::ModelSettingsRecord {
            api_url: url,
            api_token: "synthetic-nonbillable".into(),
            search_mode: "disabled".into(),
            tavily_api_key: String::new(),
            models: vec![mycopilot_core::storage::models::ModelConfigRecord {
                id: "human-checkpoint-model".into(),
                provider_model_id: "human-checkpoint-model".into(),
                display_name: "Synthetic human checkpoint".into(),
                api_url_override: None,
                api_token_override: None,
                supports_image: false,
                context_window_tokens: Some(128_000),
                provider_profile_config: crate::test_provider_profile_config(),
                input_price: "0".into(),
                cached_input_price: String::new(),
                output_price: "0".into(),
                enabled: true,
            }],
        })
        .unwrap();
    storage
}

fn human_turn() -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: None,
        project_id: Some("human-checkpoint-project".into()),
        model_id: "human-checkpoint-model".into(),
        context_window_indicator_enabled: false,
        content: "Ask a synthetic question, then continue the same task".into(),
        attachments: vec![],
        folder_references: vec![],
        skills: vec![],
        title: None,
        user_message_id: None,
        assistant_message_id: None,
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions::default(),
    }
}

fn questions(
    storage: &StorageService,
    conversation_id: &str,
) -> Vec<HumanInteractionRequestSnapshot> {
    storage
        .list_human_interaction_requests(&HumanInteractionListInput {
            conversation_id: conversation_id.into(),
            cursor: None,
            limit: 100,
        })
        .unwrap()
        .items
}

async fn wait_for_release(agent: &AgentService, run_id: &str) {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if !agent.cancellations.lock().unwrap().contains_key(run_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the exact Runtime segment must release before reopening its database");
}

async fn wait_for_terminal(storage: &StorageService, assistant: &str) -> ConversationTurnTrace {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(trace) = storage.get_conversation_turn_trace(assistant).unwrap() {
                if trace.terminal_status != ConversationTurnTraceTerminalStatus::InProgress {
                    break trace;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the synthetic human Run must reach a durable terminal state")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn human_wait_without_frontend_reopens_narration_and_resumes_exact_identity_once() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("human-checkpoint.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let (url, requests, stop, provider) = human_provider().await;
    let storage = setup_human_storage(&path, directory.path(), credentials.clone(), url);
    let agent = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, receiver) = crate::transport::outbound_channel();
    let turn = agent
        .start_conversation_turn(human_turn(), notifications)
        .unwrap();
    drop(receiver);
    let question = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(question) = questions(&storage, &turn.conversation_id)
                .into_iter()
                .find(|q| q.status == HumanInteractionRequestStatus::Open)
            {
                break question;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the Core must durably publish its human question without a Renderer");
    wait_for_release(&agent, &turn.run_id).await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "waiting cannot sample without an answer"
    );
    drop(agent);
    drop(storage);
    let storage =
        Arc::new(StorageService::open_with_model_credentials(&path, credentials).unwrap());
    let agent = AgentService::new_authorized_for_test(storage.clone());
    assert!(storage
        .has_sync_human_interaction_wait(&turn.conversation_id)
        .unwrap());
    let loaded = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    let message = loaded
        .messages
        .iter()
        .find(|message| message.id == turn.assistant_message_id)
        .unwrap();
    assert_eq!(
        message.content, NARRATION,
        "human admission must not erase the acknowledged presentation"
    );
    let waiting: Value = serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(waiting["status"], "waiting_for_user_input");
    assert_eq!(waiting["finishReason"], "tool_calls");
    assert!(waiting["coreCheckpoint"]["sequence"].as_u64().unwrap() > 0);
    let submission = HumanInteractionSubmitInput {
        conversation_id: turn.conversation_id.clone(),
        request_id: question.request_id.clone(),
        expected_revision: question.revision,
        submission_id: "human-checkpoint-answer".into(),
        answers: vec![HumanInteractionAnswer::Text {
            question_id: question.questions[0].id.clone(),
            text: ANSWER.into(),
        }],
    };
    let (notifications, receiver) = crate::transport::outbound_channel();
    drop(receiver);
    let human = HumanInteractionService::new(&storage, &agent);
    let accepted = human.submit(submission.clone(), &notifications).unwrap();
    let retried = human.submit(submission, &notifications).unwrap();
    assert_eq!(accepted.response, retried.response);
    let trace = wait_for_terminal(&storage, &turn.assistant_message_id).await;
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert_eq!(trace.run_id, turn.run_id);
    assert_eq!(trace.conversation_id, turn.conversation_id);
    assert_eq!(trace.items.iter().filter(|item| matches!(item,ConversationTurnTraceItem::ToolResult {tool,call_id,..} if tool == "request_user_input" && call_id == &question.tool_call_id)).count(),1);
    wait_for_release(&agent, &turn.run_id).await;
    let _ = stop.send(());
    provider.await.unwrap();
    assert_eq!(requests.lock().unwrap().len(), 2);
    let loaded = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.messages.len(),
        2,
        "a synchronous answer must not invent another user Turn"
    );
    assert_eq!(
        loaded
            .messages
            .iter()
            .find(|message| message.id == turn.assistant_message_id)
            .unwrap()
            .content,
        FINAL
    );
    let usage = storage
        .load_agent_usage_for_owner(
            &turn.run_id,
            &turn.conversation_id,
            &turn.assistant_message_id,
        )
        .unwrap()
        .unwrap();
    assert_eq!(usage.total_tokens, Some(24));
    assert_eq!(usage.billable_request_count, 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_human_checkpoint_barrier_never_publishes_or_leaves_a_suspension() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("human-checkpoint-failure.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let (url, requests, stop, provider) = human_provider().await;
    let storage = setup_human_storage(&path, directory.path(), credentials, url);
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.busy_timeout(Duration::from_secs(10)).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_human_checkpoint BEFORE UPDATE OF agent_run_json ON messages
        WHEN json_extract(NEW.agent_run_json,'$.coreCheckpoint.sequence') > json_extract(OLD.agent_run_json,'$.coreCheckpoint.sequence')
        BEGIN SELECT RAISE(ABORT,'synthetic human checkpoint failure'); END;").unwrap();
    let agent = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let turn = agent
        .start_conversation_turn(human_turn(), notifications)
        .unwrap();
    let trace = wait_for_terminal(&storage, &turn.assistant_message_id).await;
    wait_for_release(&agent, &turn.run_id).await;
    let _ = stop.send(());
    provider.await.unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert!(trace.items.iter().any(|item|matches!(item,ConversationTurnTraceItem::ToolCall {tool,..} if tool == "request_user_input")),"the fixture must reach the human tool boundary");
    assert!(
        questions(&storage, &turn.conversation_id).is_empty(),
        "failed presentation admission cannot create a durable question"
    );
    assert!(!storage
        .has_sync_human_interaction_wait(&turn.conversation_id)
        .unwrap());
    let suspended: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM human_interaction_suspensions WHERE run_id=?1",
            [&turn.run_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        suspended, 0,
        "no hidden waiting, cancelled, or orphaned suspension may be left"
    );
    while let Ok(notification) = receiver.try_recv() {
        assert_ne!(
            notification["method"],
            mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD,
            "a rejected question must not be exposed transiently"
        );
    }
    let usage = storage
        .load_agent_usage_for_owner(
            &turn.run_id,
            &turn.conversation_id,
            &turn.assistant_message_id,
        )
        .unwrap()
        .unwrap();
    let sampled = requests.lock().unwrap().len() as u64;
    assert_eq!(usage.billable_request_count, sampled);
    assert_eq!(usage.total_tokens, Some(sampled * 12));
}
