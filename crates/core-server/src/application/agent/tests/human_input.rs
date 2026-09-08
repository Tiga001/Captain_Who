//! Real Harness/Host tests using a local controlled Provider; no production model is contacted.
use super::*;
use crate::application::human_interaction::HumanInteractionService;
use mycopilot_core::human_interaction::{
    HumanInteractionAnswer, HumanInteractionDeliveryStatus, HumanInteractionListInput,
    HumanInteractionMode, HumanInteractionRequestSnapshot, HumanInteractionRequestStatus,
    HumanInteractionResponseKind, HumanInteractionSubmitInput,
};
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

const ANSWER_TEXT: &str = "Keep the existing public API. human-input-answer-7291";
const QUESTION_TITLE: &str = "Choose a branch [human-input-question-7291]";
const ORIGINAL_REQUEST: &str = "Ask for the missing details, then finish this same task.";

#[derive(Clone)]
enum ProviderReply {
    Questions,
    Command,
    LateWebCalls,
    Complete,
}

pub(super) async fn read_provider_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4096];
    let mut body_start = None;
    let mut request_end = None;
    loop {
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "Provider request closed before its body arrived");
        request.extend_from_slice(&buffer[..count]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let length = String::from_utf8_lossy(&request[..index])
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                    })
                    .unwrap();
                body_start = Some(index + 4);
                request_end = Some(index + 4 + length);
            }
        }
        if request_end.is_some_and(|end| request.len() >= end) {
            return serde_json::from_slice(&request[body_start.unwrap()..request_end.unwrap()])
                .unwrap();
        }
    }
}

async fn write_provider_reply(stream: &mut TcpStream, reply: ProviderReply, index: usize) {
    let (delta, finish_reason) = match reply {
        ProviderReply::Questions => (
            json!({
                "role":"assistant",
                "tool_calls":[{
                    "index":0,"id":format!("provider-question-{index}"),"type":"function",
                    "function":{
                        "name":"request_user_input",
                        "arguments":serde_json::to_string(&json!({"questions":[
                            {"title":QUESTION_TITLE,"options":["main","release"]},
                            {"title":"Describe the constraint"},
                            {"title":"Optional detail"}
                        ]})).unwrap()
                    }
                }]
            }),
            "tool_calls",
        ),
        ProviderReply::Command => (
            json!({
                "role":"assistant",
                "tool_calls":[{
                    "index":0,"id":format!("provider-command-{index}"),"type":"function",
                    "function":{
                        "name":"run_command",
                        "arguments":serde_json::to_string(&json!({
                            "command":"printf human-input-approval",
                            "reason":"Verify an independent approval between human questions"
                        })).unwrap()
                    }
                }]
            }),
            "tool_calls",
        ),
        ProviderReply::LateWebCalls => (
            json!({ "role": "assistant", "tool_calls": [
                { "index": 0, "id": "late-web-search", "type": "function", "function": {
                    "name": "web_search", "arguments": "{\"query\":\"host policy test\"}"
                } },
                { "index": 1, "id": "late-web-fetch", "type": "function", "function": {
                    "name": "web_fetch", "arguments": "{\"url\":\"https://example.com/\"}"
                } }
            ] }),
            "tool_calls",
        ),
        ProviderReply::Complete => (
            json!({"role":"assistant","content":"Completed with the submitted answers."}),
            "stop",
        ),
    };
    let content = json!({"choices":[{"delta":delta,"finish_reason":null}]});
    let finish = json!({"choices":[{"delta":{},"finish_reason":finish_reason}]});
    let usage = json!({"choices":[],"usage":{
        "prompt_tokens":10,"completion_tokens":2,"total_tokens":12
    }});
    stream.write_all(
        format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {content}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n").as_bytes()
    ).await.unwrap();
}

fn turn_input() -> AgentConversationTurnInput {
    AgentConversationTurnInput {
        conversation_id: Some("conversation-human-input".to_string()),
        project_id: Some("project-human-input".to_string()),
        model_id: "model-1".to_string(),
        context_window_indicator_enabled: false,
        content: ORIGINAL_REQUEST.to_string(),
        attachments: Vec::new(),
        skills: Vec::new(),
        title: None,
        user_message_id: Some("user-human-input".to_string()),
        assistant_message_id: Some("assistant-human-input".to_string()),
        max_tokens: None,
        temperature: None,
        prompt_preferences: None,
        permissions: AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            ..AgentPermissions::default()
        },
    }
}

pub(super) async fn wait_for_done(
    receiver: &mut UnboundedReceiver<Value>,
    run_id: &str,
    status: &str,
) -> Value {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notification = receiver
                .recv()
                .await
                .expect("Host notification stream closed");
            let event = &notification["params"];
            assert_ne!(
                event["type"], "error",
                "unexpected Host failure: {notification}"
            );
            if event["type"] == "done" {
                assert_eq!(event["runId"], run_id, "continuation must retain its Run");
                assert_eq!(
                    event["status"], status,
                    "unexpected worker outcome: {notification}"
                );
                return event.clone();
            }
        }
    })
    .await
    .expect("Host did not reach the expected durable Run boundary")
}

fn questions(storage: &StorageService) -> Vec<HumanInteractionRequestSnapshot> {
    storage
        .list_human_interaction_requests(&HumanInteractionListInput {
            conversation_id: "conversation-human-input".to_string(),
            cursor: None,
            limit: 100,
        })
        .unwrap()
        .items
}

fn answers(
    request: &HumanInteractionRequestSnapshot,
    skip_all: bool,
) -> Vec<HumanInteractionAnswer> {
    request
        .questions
        .iter()
        .enumerate()
        .map(|(index, question)| {
            if skip_all || index == 2 {
                HumanInteractionAnswer::Skipped {
                    question_id: question.id.clone(),
                }
            } else if index == 0 {
                HumanInteractionAnswer::Option {
                    question_id: question.id.clone(),
                    option_id: question.options.as_ref().unwrap()[0].id.clone(),
                }
            } else {
                HumanInteractionAnswer::Text {
                    question_id: question.id.clone(),
                    text: ANSWER_TEXT.to_string(),
                }
            }
        })
        .collect()
}

fn assert_user_projection(provider_request: &Value, response_ids: &[String]) {
    let user_messages = provider_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .collect::<Vec<_>>();
    // Host RequestOnly instructions also use the Provider's user role; they are not human Turns.
    // Count the original content and forbid the submitted answer identities/content explicitly.
    assert_eq!(
        user_messages
            .iter()
            .filter(|message| message["content"]
                .as_str()
                .is_some_and(|content| content.contains(ORIGINAL_REQUEST)))
            .count(),
        1,
        "the original human input must appear exactly once"
    );
    for message in user_messages {
        let content = message["content"].to_string();
        assert!(
            !content.contains(ANSWER_TEXT),
            "sync answer became User input"
        );
        assert!(
            !content.contains(QUESTION_TITLE),
            "sync Q+A became User input"
        );
        assert!(
            response_ids.iter().all(|id| !content.contains(id)),
            "sync response identity became User input"
        );
    }
}

fn configure_storage(storage: &StorageService, workspace: &Path, address: &str) {
    storage
        .save_project(ProjectRecord {
            id: "project-human-input".to_string(),
            name: "Human input integration".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
}

async fn controlled_provider(
    replies: Vec<ProviderReply>,
) -> (
    String,
    UnboundedReceiver<Value>,
    tokio::task::JoinHandle<()>,
) {
    controlled_provider_with_gate(replies, None).await
}

async fn controlled_provider_with_gate(
    replies: Vec<ProviderReply>,
    first_response_gate: Option<Arc<Notify>>,
) -> (
    String,
    UnboundedReceiver<Value>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let (captured, requests) = unbounded_channel();
    let provider = tokio::spawn(async move {
        for (index, reply) in replies.into_iter().enumerate() {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
                .await
                .expect("Host did not issue the next authorized Provider request")
                .unwrap();
            captured
                .send(read_provider_request(&mut stream).await)
                .unwrap();
            if index == 0 {
                if let Some(gate) = &first_response_gate {
                    gate.notified().await;
                }
            }
            write_provider_reply(&mut stream, reply, index).await;
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(100), listener.accept())
                .await
                .is_err(),
            "duplicate or stopped answer must not create another Provider request"
        );
    });
    (address, requests, provider)
}

async fn wait_for_worker_release(agent: &AgentService, run_id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if !agent.cancellations.lock().unwrap().contains_key(run_id) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a synchronous wait must release its worker segment before restart");
}

async fn assert_sync_scenario(question_batches: usize, skip_all: bool, approval_between: bool) {
    let mut replies = Vec::new();
    for index in 0..question_batches {
        replies.push(ProviderReply::Questions);
        if approval_between && index == 0 {
            replies.push(ProviderReply::Command);
        }
    }
    replies.push(ProviderReply::Complete);
    let request_count = replies.len() as u64;
    let (address, mut requests, provider) = controlled_provider(replies).await;

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    configure_storage(&storage, fixture.path(), &address);
    let agent = AgentService::new(Arc::clone(&storage));
    let human = HumanInteractionService::new(&storage, &agent);
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications.clone())
        .unwrap();
    let mut submitted = Vec::new();
    let mut model_requests = Vec::new();
    let mut segments = 0_u64;

    for index in 0..question_batches {
        let done = wait_for_done(&mut events, &turn.run_id, "waiting_for_user_input").await;
        segments += 1;
        assert_eq!(done["usage"]["billableRequestCount"], segments);
        assert_eq!(done["usage"]["totalTokens"], segments * 12);
        model_requests.push(requests.recv().await.unwrap());
        assert!(
            requests.try_recv().is_err(),
            "waiting must not sample without an answer"
        );
        assert!(storage
            .has_sync_human_interaction_wait(&turn.conversation_id)
            .unwrap());
        assert!(
            agent.list_pending_actions().is_empty(),
            "human input is not an approval"
        );
        let request = questions(&storage)
            .into_iter()
            .find(|request| request.status == HumanInteractionRequestStatus::Open)
            .unwrap();
        assert_eq!(request.run_id, turn.run_id);
        assert_eq!(request.mode, HumanInteractionMode::Sync);
        assert_eq!(request.questions.len(), 3);
        let mut competing = turn_input();
        competing.user_message_id = Some(format!("competing-user-{index}"));
        competing.assistant_message_id = Some(format!("competing-assistant-{index}"));
        assert!(agent
            .start_conversation_turn(competing, notifications.clone())
            .is_err());

        let submission = HumanInteractionSubmitInput {
            conversation_id: turn.conversation_id.clone(),
            request_id: request.request_id.clone(),
            expected_revision: request.revision,
            submission_id: format!("submission-{index}"),
            answers: answers(&request, skip_all),
        };
        let accepted = human.submit(submission.clone(), &notifications).unwrap();
        let retried = human.submit(submission, &notifications).unwrap();
        assert_eq!(accepted.response, retried.response);
        assert_eq!(accepted.status, HumanInteractionRequestStatus::Submitted);
        assert_eq!(
            accepted.response.as_ref().unwrap().kind,
            HumanInteractionResponseKind::Submitted
        );
        assert_eq!(accepted.response.as_ref().unwrap().answers.len(), 3);
        submitted.push(accepted);

        if approval_between && index == 0 {
            let approval_done =
                wait_for_done(&mut events, &turn.run_id, "waiting_for_approval").await;
            segments += 1;
            assert_eq!(approval_done["usage"]["billableRequestCount"], segments);
            model_requests.push(requests.recv().await.unwrap());
            let pending = agent.list_pending_actions();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].tool_name, "run_command");
            let decision = agent
                .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone())
                .unwrap();
            assert_eq!(decision.agent_output.status, AgentRunStatus::Running);
        }
    }
    let done = wait_for_done(&mut events, &turn.run_id, "completed").await;
    model_requests.push(requests.recv().await.unwrap());
    assert_eq!(done["usage"]["billableRequestCount"], request_count);
    assert_eq!(done["usage"]["totalTokens"], request_count * 12);
    provider.await.unwrap();

    let first_tools = model_requests[0]["tools"].as_array().unwrap();
    assert!(first_tools
        .iter()
        .any(|tool| tool["function"]["name"] == "request_user_input"));
    assert!(first_tools
        .iter()
        .any(|tool| tool["function"]["name"] == "request_user_input_async"));
    let response_ids = submitted
        .iter()
        .map(|request| request.response.as_ref().unwrap().response_id.clone())
        .collect::<Vec<_>>();
    for request in &model_requests {
        assert_user_projection(request, &response_ids);
    }
    let final_messages = model_requests.last().unwrap()["messages"]
        .as_array()
        .unwrap();
    let trace = storage
        .get_conversation_turn_trace("assistant-human-input")
        .unwrap()
        .unwrap();
    let model_log = storage
        .get_conversation_model_context_log("assistant-human-input")
        .unwrap()
        .unwrap();
    for request in &submitted {
        let results = trace
            .items
            .iter()
            .filter_map(|item| match item {
                ConversationTurnTraceItem::ToolResult {
                    call_id,
                    tool,
                    observation,
                    ..
                } if call_id == &request.tool_call_id && tool == "request_user_input" => {
                    Some(observation)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            results.len(),
            1,
            "exactly one durable result per suspended call"
        );
        assert_eq!(results[0]["type"], "human_interaction_response");
        assert_eq!(results[0]["requestId"], request.request_id);
        assert_eq!(
            results[0]["responseId"],
            request.response.as_ref().unwrap().response_id
        );
        let projected_answers = results[0]["answers"].as_array().unwrap();
        assert_eq!(projected_answers.len(), 3);
        for (index, answer) in projected_answers.iter().enumerate() {
            assert_eq!(answer["questionId"], request.questions[index].id);
            assert_eq!(answer["question"], request.questions[index].title);
            if skip_all || index == 2 {
                assert_eq!(answer["kind"], "skipped");
                assert_eq!(answer["answer"], "已跳过");
            } else if index == 0 {
                assert_eq!(answer["kind"], "option");
                assert_eq!(answer["answer"], "main");
                assert_eq!(
                    answer["optionId"],
                    request.questions[index].options.as_ref().unwrap()[0].id
                );
            } else {
                assert_eq!(answer["kind"], "text");
                assert_eq!(answer["answer"], ANSWER_TEXT);
            }
        }
        assert_eq!(
            model_log
                .items
                .iter()
                .filter(|item| item.tool_call_id.as_deref() == Some(request.tool_call_id.as_str()))
                .count(),
            1
        );
        let model_result_count = final_messages
            .iter()
            .filter(|message| {
                message["role"] == "tool"
                    && message["content"]
                        .as_str()
                        .is_some_and(|content| content.contains(&request.request_id))
            })
            .count();
        assert_eq!(
            model_result_count, 1,
            "one original ToolResult enters the resumed request"
        );
    }
    let settled = questions(&storage);
    assert_eq!(settled.len(), question_batches);
    assert!(settled
        .iter()
        .all(|request| request
            .delivery
            .as_ref()
            .is_some_and(
                |delivery| delivery.status == HumanInteractionDeliveryStatus::Applied
                    && delivery.target_run_id.as_deref() == Some(turn.run_id.as_str())
                    && delivery.user_message_id.is_none()
            )));
    let conversation = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        conversation.messages.len(),
        2,
        "display-only sync answers cannot add User rows"
    );
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
    let usage = agent
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(usage.request_count, request_count);
    assert_eq!(usage.input_tokens, Some(request_count * 10));
    assert_eq!(usage.output_tokens, Some(request_count * 2));
    assert_eq!(usage.total_tokens, Some(request_count * 12));
}

#[tokio::test]
async fn sync_human_input_resumes_same_run_once_with_mixed_answers() {
    assert_sync_scenario(1, false, false).await;
}

#[tokio::test]
async fn all_skipped_is_a_formal_sync_response_and_resumes_same_run() {
    assert_sync_scenario(1, true, false).await;
}

#[tokio::test]
async fn consecutive_sync_questions_preserve_one_run_and_cumulative_usage() {
    assert_sync_scenario(2, false, false).await;
}

#[tokio::test]
async fn sync_questions_and_command_approval_alternate_without_crossing_resume_authority() {
    assert_sync_scenario(2, false, true).await;
}

async fn assert_sync_restart(answer_committed_before_restart: bool) {
    let (address, mut requests, provider) =
        controlled_provider(vec![ProviderReply::Questions, ProviderReply::Complete]).await;
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, credentials.clone()).unwrap(),
    );
    configure_storage(&storage, fixture.path(), &address);
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications.clone())
        .unwrap();
    wait_for_done(&mut events, &turn.run_id, "waiting_for_user_input").await;
    wait_for_worker_release(&agent, &turn.run_id).await;
    let original_wire = requests.recv().await.unwrap();
    let original_collaboration = original_wire["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .find(|content| content.contains("<agent_collaboration_directory>"))
        .unwrap()
        .to_string();
    let (_, envelope) = storage
        .load_sync_human_interaction_for_run(&turn.run_id)
        .unwrap()
        .unwrap();
    let encoded_checkpoint = serde_json::to_string(&envelope).unwrap();
    let frozen_input = PersistedAgentResumeInput::decode(&encoded_checkpoint)
        .unwrap()
        .agent_input;
    let frozen_checkpoint = frozen_input.resume_checkpoint.as_ref().unwrap();
    let original_directory = frozen_checkpoint
        .collaboration_run_snapshot
        .as_ref()
        .unwrap()
        .selector_directory
        .clone();
    let mut current_directory = original_directory.clone();
    current_directory.models[0].display_name = "Changed during the synchronous question".into();
    let mut settings = storage.load_model_settings().unwrap().unwrap();
    settings.models[0].display_name = current_directory.models[0].display_name.clone();
    storage.save_model_settings(settings).unwrap();
    let request = questions(&storage).pop().unwrap();
    let submission = HumanInteractionSubmitInput {
        conversation_id: turn.conversation_id.clone(),
        request_id: request.request_id.clone(),
        expected_revision: request.revision,
        submission_id: "submission-across-restart".to_string(),
        answers: answers(&request, false),
    };
    if answer_committed_before_restart {
        // Reproduce a process exit between the submit transaction's commit and Host scheduling.
        // This is the same durable submit entry point used by HumanInteractionService::submit.
        let accepted = storage.submit_human_interaction(&submission).unwrap();
        assert_eq!(
            accepted.delivery.as_ref().unwrap().status,
            HumanInteractionDeliveryStatus::Pending
        );
    }
    drop(events);
    drop(notifications);
    drop(agent);
    drop(storage);

    let storage =
        Arc::new(StorageService::open_with_model_credentials(&database_path, credentials).unwrap());
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
    assert_eq!(
        restarted
            .reconcile_startup_orphaned_conversation_traces()
            .unwrap(),
        0
    );
    assert!(storage
        .has_sync_human_interaction_wait(&turn.conversation_id)
        .unwrap());
    assert!(restarted.pending_actions.lock().unwrap().is_empty());
    assert!(restarted
        .collaboration_run_directories
        .lock()
        .unwrap()
        .is_empty());
    assert_eq!(
        restarted
            .collaboration_directory_for_run(&turn.run_id, &current_directory, None)
            .unwrap(),
        original_directory,
        "a cold synchronous wait must recover its directory from the durable checkpoint"
    );

    // A healthy hot cache does not reload storage on every preview. A cold malformed envelope
    // must fail explicitly instead of silently switching the in-progress run to today's directory.
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    let identity_trigger: String = connection
        .query_row(
            "SELECT sql FROM sqlite_master WHERE name='human_interaction_suspensions_immutable_identity'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    // Fault injection is confined to this temporary database; normal writes reject this corruption.
    connection
        .execute_batch("DROP TRIGGER human_interaction_suspensions_immutable_identity")
        .unwrap();
    connection
        .execute(
            "UPDATE human_interaction_suspensions SET checkpoint_json='{}' WHERE request_id=?1",
            [&request.request_id],
        )
        .unwrap();
    assert_eq!(
        restarted
            .collaboration_directory_for_run(&turn.run_id, &current_directory, None)
            .unwrap(),
        original_directory
    );
    restarted
        .collaboration_run_directories
        .lock()
        .unwrap()
        .clear();
    assert!(restarted
        .collaboration_directory_for_run(&turn.run_id, &current_directory, None)
        .is_err());
    connection
        .execute(
            "UPDATE human_interaction_suspensions SET checkpoint_json=?1 WHERE request_id=?2",
            [&encoded_checkpoint, &request.request_id],
        )
        .unwrap();
    connection.execute_batch(&identity_trigger).unwrap();
    drop(connection);

    restarted
        .collaboration_run_directories
        .lock()
        .unwrap()
        .insert(turn.run_id.clone(), current_directory.clone());
    let mut invalid_checkpoint = frozen_checkpoint.clone();
    invalid_checkpoint.run_id = "another-run".into();
    assert!(restarted
        .collaboration_directory_for_run(
            &turn.run_id,
            &current_directory,
            Some(&invalid_checkpoint)
        )
        .is_err());
    invalid_checkpoint = frozen_checkpoint.clone();
    invalid_checkpoint
        .collaboration_run_snapshot
        .as_mut()
        .unwrap()
        .admitted_wait_model_batches = vec![0];
    assert!(restarted
        .collaboration_directory_for_run(
            &turn.run_id,
            &current_directory,
            Some(&invalid_checkpoint)
        )
        .is_err());
    // The production preview path must replace even a pre-existing speculative cache entry
    // with the validated explicit resume checkpoint before the resumed runtime is attached.
    restarted
        .context_window_tool_projection(&frozen_input, None)
        .unwrap();
    assert_eq!(
        restarted
            .collaboration_directory_for_run(&turn.run_id, &current_directory, None)
            .unwrap(),
        original_directory
    );
    let (notifications, mut events) = unbounded_channel();
    let human = HumanInteractionService::new(&storage, &restarted);
    if answer_committed_before_restart {
        restarted.schedule_ready_human_input_resumes(notifications.clone());
    } else {
        restarted.schedule_ready_human_input_resumes(notifications.clone());
        assert!(
            requests.try_recv().is_err(),
            "startup cannot invent an answer"
        );
        assert_eq!(
            questions(&storage)[0].status,
            HumanInteractionRequestStatus::Open
        );
        human.submit(submission.clone(), &notifications).unwrap();
    }
    let done = wait_for_done(&mut events, &turn.run_id, "completed").await;
    assert_eq!(done["usage"]["billableRequestCount"], 2);
    assert_eq!(done["usage"]["totalTokens"], 24);
    let resumed_request = requests.recv().await.unwrap();
    let resumed_collaboration = resumed_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter(|content| content.contains("<agent_collaboration_directory>"))
        .collect::<Vec<_>>();
    assert_eq!(
        resumed_collaboration,
        vec![original_collaboration.as_str()],
        "the resumed provider request must use the same frozen directory exactly once"
    );
    let retried = human.submit(submission, &notifications).unwrap();
    assert_user_projection(
        &resumed_request,
        std::slice::from_ref(&retried.response.as_ref().unwrap().response_id),
    );
    assert_eq!(
        retried.delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Applied
    );
    provider.await.unwrap();
    let trace = storage
        .get_conversation_turn_trace("assistant-human-input")
        .unwrap()
        .unwrap();
    assert_eq!(trace.run_id, turn.run_id);
    assert_eq!(trace.items.iter().filter(|item| matches!(item, ConversationTurnTraceItem::ToolResult { tool, call_id, .. } if tool == "request_user_input" && call_id == &request.tool_call_id)).count(), 1);
    assert_eq!(
        storage
            .load_conversation(&turn.conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
    let usage = restarted
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(usage.request_count, 2);
    assert_eq!(usage.total_tokens, Some(24));
}

#[tokio::test]
async fn open_sync_question_survives_host_restart_and_accepts_its_answer() {
    assert_sync_restart(false).await;
}

#[tokio::test]
async fn committed_unclaimed_sync_answer_resumes_after_host_restart() {
    assert_sync_restart(true).await;
}

async fn assert_stopped_sync_wait(restart_before_stop: bool) {
    let (address, mut requests, provider) =
        controlled_provider(vec![ProviderReply::Questions]).await;
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, credentials.clone()).unwrap(),
    );
    configure_storage(&storage, fixture.path(), &address);
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications.clone())
        .unwrap();
    wait_for_done(&mut events, &turn.run_id, "waiting_for_user_input").await;
    wait_for_worker_release(&agent, &turn.run_id).await;
    requests.recv().await.unwrap();
    let request = questions(&storage).pop().unwrap();
    let (agent, storage) = if restart_before_stop {
        drop(agent);
        drop(storage);
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&database_path, credentials).unwrap(),
        );
        let restarted =
            AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
        restarted
            .reconcile_startup_orphaned_conversation_traces()
            .unwrap();
        (restarted, storage)
    } else {
        (agent, storage)
    };
    assert!(agent.cancel_run_checked(&turn.run_id).unwrap());
    let stopped = questions(&storage).pop().unwrap();
    assert_eq!(stopped.status, HumanInteractionRequestStatus::Cancelled);
    assert!(HumanInteractionService::new(&storage, &agent)
        .submit(
            HumanInteractionSubmitInput {
                conversation_id: turn.conversation_id.clone(),
                request_id: request.request_id,
                expected_revision: request.revision,
                submission_id: "submission-after-stop".to_string(),
                answers: answers(&stopped, false),
            },
            &notifications
        )
        .is_err());
    agent.schedule_ready_human_input_resumes(notifications);
    provider.await.unwrap();
    assert!(requests.try_recv().is_err());
    assert!(stopped.response.is_none());
    assert!(stopped.delivery.is_none());
    let trace = storage
        .get_conversation_turn_trace("assistant-human-input")
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert_eq!(
        storage
            .load_conversation(&turn.conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
    let usage = agent
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(usage.request_count, 1);
    assert_eq!(usage.total_tokens, Some(12));
}

#[tokio::test]
async fn stopped_sync_wait_rejects_a_late_submission_without_sampling() {
    assert_stopped_sync_wait(false).await;
}

#[tokio::test]
async fn stopped_sync_wait_after_host_restart_rejects_late_submission_without_sampling() {
    assert_stopped_sync_wait(true).await;
}

async fn assert_sync_submission_handoff(before_open_publication: bool) {
    let response_gate = Arc::new(Notify::new());
    let (address, mut requests, provider) = controlled_provider_with_gate(
        vec![ProviderReply::Questions, ProviderReply::Complete],
        Some(Arc::clone(&response_gate)),
    )
    .await;
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    configure_storage(&storage, fixture.path(), &address);
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications.clone())
        .unwrap();

    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let hook = Arc::new(move || {
        let _ = entered_tx.send(());
        // Dropping the sender also releases the worker if a test assertion fails.
        let _ = release_rx
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10));
    });
    if before_open_publication {
        super::super::turn_executor::install_before_waiting_publication_arbitration_hook(
            &turn.run_id,
            hook,
        );
    } else {
        super::super::run_lifecycle::install_before_waiting_persistence_hook(&turn.run_id, hook);
    }
    response_gate.notify_one();
    tokio::task::spawn_blocking(move || entered_rx.recv_timeout(Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("the original worker did not reach the controlled handoff");
    requests.recv().await.unwrap();
    let request = questions(&storage).pop().unwrap();
    let mut observed = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
    let open_was_published = observed.iter().any(|notification| {
        notification["method"] == mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD
            && notification["params"]["status"] == "open"
    });
    assert_eq!(open_was_published, !before_open_publication);
    let submission = HumanInteractionSubmitInput {
        conversation_id: turn.conversation_id.clone(),
        request_id: request.request_id.clone(),
        expected_revision: request.revision,
        submission_id: "submission-at-handoff".to_string(),
        answers: answers(&request, false),
    };
    let submit_agent = agent.clone();
    let submit_storage = Arc::clone(&storage);
    let submit_notifications = notifications.clone();
    let (submit_started_tx, submit_started_rx) = tokio::sync::oneshot::channel();
    let mut submit = tokio::task::spawn_blocking(move || {
        let _ = submit_started_tx.send(());
        HumanInteractionService::new(&submit_storage, &submit_agent)
            .submit(submission, &submit_notifications)
    });
    submit_started_rx.await.unwrap();
    if before_open_publication {
        assert!(
            tokio::time::timeout(Duration::from_millis(75), &mut submit)
                .await
                .is_err(),
            "submit must wait for the committed open snapshot to be published"
        );
        assert_eq!(
            questions(&storage)[0].status,
            HumanInteractionRequestStatus::Open
        );
        assert!(requests.try_recv().is_err());
        release_tx.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(10), submit)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    } else {
        let accepted = tokio::time::timeout(Duration::from_secs(10), submit)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(
            accepted.delivery.as_ref().unwrap().status,
            HumanInteractionDeliveryStatus::Pending
        );
        assert!(agent
            .cancellations
            .lock()
            .unwrap()
            .contains_key(&turn.run_id));
        assert_eq!(
            storage
                .list_sync_human_interaction_ready_resumes()
                .unwrap()
                .len(),
            1
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(75), requests.recv())
                .await
                .is_err(),
            "the answer cannot start another worker before its predecessor retires"
        );
        release_tx.send(()).unwrap();
    }

    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let notification = events.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "handoff failed: {notification}"
            );
            let complete = notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed";
            observed.push(notification);
            if complete {
                break;
            }
        }
    })
    .await
    .expect("the accepted answer did not finish the same Run");
    let mut resumed = false;
    // Events before the hook include the initial Started; only inspect the final sequence after
    // seeing the first waiting state/Done when checking for stale retiring-worker publication.
    let mut initial_wait_seen = false;
    for notification in &observed {
        let event = &notification["params"];
        if event["type"] == "state" && event["state"]["status"] == "waiting_for_user_input" {
            initial_wait_seen = true;
        }
        if initial_wait_seen && event["type"] == "started" {
            resumed = true;
        }
        if event["type"] == "done" && event["status"] == "waiting_for_user_input" {
            assert!(
                !resumed,
                "the retired segment published waiting after its successor started"
            );
            initial_wait_seen = true;
        }
        if event["type"] == "done" {
            assert_eq!(event["runId"], turn.run_id);
        }
    }
    assert!(resumed, "the answer must resume the original Run");
    let request_statuses = observed
        .iter()
        .filter(|notification| {
            notification["method"]
                == mycopilot_protocol_rs::HUMAN_INTERACTION_REQUEST_CHANGED_METHOD
                && notification["params"]["requestId"] == request.request_id
        })
        .map(|notification| notification["params"]["status"].as_str().unwrap())
        .collect::<Vec<_>>();
    let open_index = request_statuses
        .iter()
        .position(|status| *status == "open")
        .unwrap();
    let submitted_index = request_statuses
        .iter()
        .position(|status| *status == "submitted")
        .unwrap();
    assert!(
        open_index < submitted_index,
        "public snapshots must follow commit order: {request_statuses:?}"
    );
    assert!(!request_statuses[submitted_index..].contains(&"open"));
    let final_request = requests.recv().await.unwrap();
    // Done observes the committed trace; the delivery acknowledgement follows during teardown.
    let settled =
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let snapshot = questions(&storage).pop().unwrap();
                if snapshot.delivery.as_ref().is_some_and(|delivery| {
                    delivery.status == HumanInteractionDeliveryStatus::Applied
                }) {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the completed answer did not reach durable Applied status");
    assert_eq!(
        settled.delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Applied
    );
    assert_user_projection(
        &final_request,
        std::slice::from_ref(&settled.response.as_ref().unwrap().response_id),
    );
    provider.await.unwrap();
    let usage = agent
        .get_usage_summary(&AgentUsageSummaryInput {
            range: AgentUsageSummaryRange::All,
            from: None,
            to: None,
        })
        .unwrap();
    assert_eq!(usage.request_count, 2);
    assert_eq!(usage.total_tokens, Some(24));
    let trace = storage
        .get_conversation_turn_trace("assistant-human-input")
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submitted_answer_waits_for_the_old_worker_to_retire_before_resuming() {
    assert_sync_submission_handoff(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_submission_cannot_overtake_the_open_request_notification() {
    assert_sync_submission_handoff(true).await;
}

fn assert_web_search_projection(request: &Value, available: bool, reason: &str) {
    let tools = request["tools"].as_array().unwrap();
    for name in ["web_search", "web_fetch"] {
        assert_eq!(
            tools.iter().any(|tool| tool["function"]["name"] == name),
            available
        );
    }
    let messages = request["messages"].to_string();
    assert_eq!(messages.contains("## 联网搜索"), available);
    assert!(
        messages.contains(reason),
        "request must include the current web policy reason"
    );
    assert!(!request
        .to_string()
        .contains("HOST_SEARCH_CREDENTIAL_CANARY"));
}

#[tokio::test]
async fn web_search_toggle_across_sync_and_approval_pauses_preserves_same_run() {
    use super::web_search_policy::save_search_policy;
    let (address, mut requests, provider) = controlled_provider(vec![
        ProviderReply::Questions,
        ProviderReply::Command,
        ProviderReply::Questions,
        ProviderReply::Complete,
    ])
    .await;
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    configure_storage(&storage, fixture.path(), &address);
    let mut agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications.clone())
        .unwrap();

    for index in 0..2 {
        wait_for_done(&mut events, &turn.run_id, "waiting_for_user_input").await;
        assert_web_search_projection(&requests.recv().await.unwrap(), false, "disabled_by_user");
        save_search_policy(
            &storage,
            "auto",
            CredentialMutation::Replace {
                value: "HOST_SEARCH_CREDENTIAL_CANARY".to_string(),
            },
        );
        let request = questions(&storage)
            .into_iter()
            .find(|request| request.status == HumanInteractionRequestStatus::Open)
            .unwrap();
        HumanInteractionService::new(&storage, &agent)
            .submit(
                HumanInteractionSubmitInput {
                    conversation_id: turn.conversation_id.clone(),
                    request_id: request.request_id.clone(),
                    expected_revision: request.revision,
                    submission_id: format!("web-search-policy-answer-{index}"),
                    answers: answers(&request, true),
                },
                &notifications,
            )
            .unwrap();
        if index == 0 {
            wait_for_done(&mut events, &turn.run_id, "waiting_for_approval").await;
            assert_web_search_projection(&requests.recv().await.unwrap(), true, "available");
            let pending = agent.list_pending_actions();
            assert_eq!(pending.len(), 1);
            let durable = storage.list_pending_agent_actions().unwrap();
            for row in &durable {
                assert!(!row
                    .agent_input_json
                    .contains("HOST_SEARCH_CREDENTIAL_CANARY"));
                assert!(!row.agent_input_json.contains("tavilyApiKey"));
            }
            save_search_policy(&storage, "disabled", CredentialMutation::Clear);
            wait_for_worker_release(&agent, &turn.run_id).await;
            drop(agent);
            agent = AgentService::new(Arc::clone(&storage));
            assert_eq!(
                agent.list_pending_actions().len(),
                1,
                "changed search policy must survive approval restoration"
            );
            agent
                .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone())
                .unwrap();
        }
    }
    let done = wait_for_done(&mut events, &turn.run_id, "completed").await;
    assert_eq!(done["usage"]["billableRequestCount"], 4);
    assert_web_search_projection(&requests.recv().await.unwrap(), true, "available");
    provider.await.unwrap();
    let trace = storage
        .get_conversation_turn_trace("assistant-human-input")
        .unwrap()
        .unwrap();
    assert_eq!(trace.run_id, turn.run_id);
}

#[tokio::test]
async fn web_search_late_model_calls_are_denied_after_committed_off_setting() {
    use super::web_search_policy::save_search_policy;
    let gate = Arc::new(Notify::new());
    let (address, mut requests, provider) = controlled_provider_with_gate(
        vec![ProviderReply::LateWebCalls, ProviderReply::Complete],
        Some(Arc::clone(&gate)),
    )
    .await;
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    configure_storage(&storage, fixture.path(), &address);
    save_search_policy(
        &storage,
        "tavily",
        CredentialMutation::Replace {
            value: "HOST_SEARCH_CREDENTIAL_CANARY".to_string(),
        },
    );
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications)
        .unwrap();
    assert_web_search_projection(&requests.recv().await.unwrap(), true, "available");
    save_search_policy(&storage, "disabled", CredentialMutation::Keep);
    gate.notify_one();
    wait_for_done(&mut events, &turn.run_id, "completed").await;
    let next = requests.recv().await.unwrap();
    assert_web_search_projection(&next, false, "disabled_by_user");
    let results = next["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "tool")
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 2);
    for result in results {
        assert!(result["content"].to_string().contains("disabled_by_user"));
    }
    provider.await.unwrap();
}

#[tokio::test]
async fn web_search_missing_credential_after_sync_restart_keeps_same_run() {
    use super::web_search_policy::save_search_policy;
    let (address, mut requests, provider) =
        controlled_provider(vec![ProviderReply::Questions, ProviderReply::Complete]).await;
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, credentials.clone()).unwrap(),
    );
    configure_storage(&storage, fixture.path(), &address);
    save_search_policy(
        &storage,
        "tavily",
        CredentialMutation::Replace {
            value: "HOST_SEARCH_CREDENTIAL_CANARY".to_string(),
        },
    );
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    let turn = agent
        .start_conversation_turn(turn_input(), notifications)
        .unwrap();
    wait_for_done(&mut events, &turn.run_id, "waiting_for_user_input").await;
    wait_for_worker_release(&agent, &turn.run_id).await;
    assert_web_search_projection(&requests.recv().await.unwrap(), true, "available");
    let request = questions(&storage).pop().unwrap();
    save_search_policy(&storage, "tavily", CredentialMutation::Clear);
    drop(agent);
    drop(storage);
    let storage =
        Arc::new(StorageService::open_with_model_credentials(&database_path, credentials).unwrap());
    let agent = AgentService::new(Arc::clone(&storage));
    let (notifications, mut events) = unbounded_channel();
    HumanInteractionService::new(&storage, &agent)
        .submit(
            HumanInteractionSubmitInput {
                conversation_id: turn.conversation_id.clone(),
                request_id: request.request_id.clone(),
                expected_revision: request.revision,
                submission_id: "web-policy-sync-restart".to_string(),
                answers: answers(&request, true),
            },
            &notifications,
        )
        .unwrap();
    wait_for_done(&mut events, &turn.run_id, "completed").await;
    assert_web_search_projection(
        &requests.recv().await.unwrap(),
        false,
        "configuration_required",
    );
    provider.await.unwrap();
    assert_eq!(
        storage
            .get_conversation_turn_trace("assistant-human-input")
            .unwrap()
            .unwrap()
            .run_id,
        turn.run_id
    );
}
