//! Async answers cross the real Host admission, Harness boundary and local Provider transport.
use super::*;
use crate::application::human_interaction::HumanInteractionService;
use mycopilot_core::human_interaction::{
    HumanInteractionAnswer, HumanInteractionDeliveryStatus, HumanInteractionIgnoreInput,
    HumanInteractionListInput, HumanInteractionMode, HumanInteractionRequestSnapshot,
    HumanInteractionRequestStatus, HumanInteractionResponseDisplay, HumanInteractionSubmitInput,
};
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const CONVERSATION: &str = "conversation-async-human-input";
const ASSISTANT: &str = "assistant-async-human-input";
const ORIGINAL: &str = "Ask asynchronously, continue independently, and incorporate the answers.";
const ANSWER: &str = "Preserve the public API. async-answer-96325";

enum Reply {
    Async(usize),
    Sync,
    Approval,
    Complete,
    RejectRequest,
}

async fn read_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    loop {
        let mut buffer = [0_u8; 4096];
        let count = stream.read(&mut buffer).await.unwrap();
        assert!(count > 0, "Provider closed before sending its request");
        bytes.extend_from_slice(&buffer[..count]);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let length = String::from_utf8_lossy(&bytes[..header_end])
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let body_start = header_end + 4;
            if bytes.len() >= body_start + length {
                return serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap();
            }
        }
    }
}

async fn write_reply(stream: &mut TcpStream, reply: Reply, request_index: usize) {
    if matches!(reply, Reply::RejectRequest) {
        let body = "{\"error\":{\"message\":\"controlled non-retryable request failure\",\"type\":\"invalid_request_error\"}}";
        stream.write_all(format!("HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        return;
    }
    let (delta, reason) = match reply {
        Reply::RejectRequest => unreachable!("handled above"),
        Reply::Complete => (
            json!({"role":"assistant","content":"Independent work finished."}),
            "stop",
        ),
        Reply::Async(count) => (
            json!({"role":"assistant","tool_calls":(0..count).map(|index| json!({
            "index":index,"id":format!("async-question-{request_index}-{index}"),"type":"function",
            "function":{"name":"request_user_input_async","arguments":serde_json::to_string(&json!({
                "questions":[
                    {"title":format!("Select output [{request_index}/{index}]"),"options":["CSV","JSON"]},
                    {"title":"Describe the compatibility constraint"},
                    {"title":"Optional explanation"}
                ]
            })).unwrap()}
        })).collect::<Vec<_>>()}),
            "tool_calls",
        ),
        Reply::Sync => (
            json!({"role":"assistant","tool_calls":[{
                "index":0,"id":format!("sync-question-{request_index}"),"type":"function",
                "function":{"name":"request_user_input","arguments":serde_json::to_string(&json!({
                    "questions":[{"title":"Required blocking detail"}]
                })).unwrap()}
            }]}),
            "tool_calls",
        ),
        Reply::Approval => (
            json!({"role":"assistant","tool_calls":[{
                "index":0,"id":format!("approval-{request_index}"),"type":"function",
                "function":{"name":"run_command","arguments":serde_json::to_string(&json!({
                    "command":"printf async-human-approval","reason":"Verify independent command approval"
                })).unwrap()}
            }]}),
            "tool_calls",
        ),
    };
    let content = json!({"choices":[{"delta":delta,"finish_reason":null}]});
    let finish = json!({"choices":[{"delta":{},"finish_reason":reason}]});
    let usage =
        json!({"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}});
    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {content}\n\ndata: {finish}\n\ndata: {usage}\n\ndata: [DONE]\n\n").as_bytes()).await.unwrap();
}

struct Fixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    credentials: Arc<mycopilot_core::image_generation::InMemoryCredentialStore>,
    storage: Arc<StorageService>,
    agent: AgentService,
    notifications: UnboundedSender<Value>,
    events: UnboundedReceiver<Value>,
    requests: UnboundedReceiver<Value>,
    replies: UnboundedSender<Reply>,
    provider: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.provider.abort();
    }
}

impl Fixture {
    async fn new() -> Self {
        let directory = tempdir().unwrap();
        let database = directory.path().join("storage.sqlite");
        let credentials =
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&database, credentials.clone()).unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (captured, requests) = unbounded_channel();
        let (replies, mut pending_replies) = unbounded_channel();
        let provider = tokio::spawn(async move {
            let mut index = 0;
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                captured.send(read_request(&mut stream).await).unwrap();
                let Some(reply) = pending_replies.recv().await else {
                    return;
                };
                write_reply(&mut stream, reply, index).await;
                index += 1;
            }
        });
        storage
            .save_project(ProjectRecord {
                id: "project-async-human-input".to_string(),
                name: "Async human input".to_string(),
                path: Some(directory.path().to_string_lossy().into_owned()),
                created_at: 1,
                pinned_at: None,
            })
            .unwrap();
        let mut settings = test_model_settings();
        settings.api_url = format!("http://{address}/v1/chat/completions");
        storage.save_model_settings(settings).unwrap();
        let agent = AgentService::new(Arc::clone(&storage));
        let (notifications, events) = unbounded_channel();
        Self {
            _directory: directory,
            database,
            credentials,
            storage,
            agent,
            notifications,
            events,
            requests,
            replies,
            provider,
        }
    }

    fn start(&self) -> AgentConversationTurnOutput {
        self.start_with_suffix("")
    }

    fn start_with_suffix(&self, suffix: &str) -> AgentConversationTurnOutput {
        self.start_with_content(suffix, ORIGINAL.to_string())
    }

    fn start_with_content(&self, suffix: &str, content: String) -> AgentConversationTurnOutput {
        self.agent
            .start_conversation_turn(
                AgentConversationTurnInput {
                    conversation_id: Some(CONVERSATION.to_string()),
                    project_id: Some("project-async-human-input".to_string()),
                    model_id: "model-1".to_string(),
                    context_window_indicator_enabled: false,
                    content,
                    attachments: Vec::new(),
                    skills: Vec::new(),
                    title: None,
                    user_message_id: Some(format!("user-async-human-input{suffix}")),
                    assistant_message_id: Some(format!("{ASSISTANT}{suffix}")),
                    max_tokens: None,
                    temperature: None,
                    prompt_preferences: None,
                    permissions: AgentPermissions {
                        write: AgentWritePermission::WorkspaceOnly,
                        command: AgentCommandPermission::RequireApproval,
                        command_safety: AgentCommandSafetyPolicy::FullAccess,
                        ..AgentPermissions::default()
                    },
                },
                self.notifications.clone(),
            )
            .unwrap()
    }

    fn batches(&self) -> Vec<HumanInteractionRequestSnapshot> {
        self.storage
            .list_human_interaction_requests(&HumanInteractionListInput {
                conversation_id: CONVERSATION.to_string(),
                cursor: None,
                limit: 100,
            })
            .unwrap()
            .items
    }

    fn submission(
        request: &HumanInteractionRequestSnapshot,
        skip_all: bool,
    ) -> HumanInteractionSubmitInput {
        HumanInteractionSubmitInput {
            conversation_id: CONVERSATION.to_string(),
            request_id: request.request_id.clone(),
            expected_revision: request.revision,
            submission_id: format!("submit-{}", request.request_id),
            answers: request
                .questions
                .iter()
                .enumerate()
                .map(|(index, question)| {
                    if skip_all || index == 2 {
                        HumanInteractionAnswer::Skipped {
                            question_id: question.id.clone(),
                        }
                    } else if let Some(options) = &question.options {
                        HumanInteractionAnswer::Option {
                            question_id: question.id.clone(),
                            option_id: options[0].id.clone(),
                        }
                    } else {
                        HumanInteractionAnswer::Text {
                            question_id: question.id.clone(),
                            text: ANSWER.to_string(),
                        }
                    }
                })
                .collect(),
        }
    }

    fn submit(
        &self,
        request: &HumanInteractionRequestSnapshot,
        skip_all: bool,
    ) -> HumanInteractionRequestSnapshot {
        let service = HumanInteractionService::new(&self.storage, &self.agent);
        let input = Self::submission(request, skip_all);
        let accepted = service.submit(input.clone(), &self.notifications).unwrap();
        let repeated = service.submit(input, &self.notifications).unwrap();
        assert_eq!(
            accepted.response, repeated.response,
            "one immutable response per submission identity"
        );
        assert_eq!(accepted.status, HumanInteractionRequestStatus::Submitted);
        accepted
    }

    fn ignore(&self, request: &HumanInteractionRequestSnapshot) -> HumanInteractionRequestSnapshot {
        let service = HumanInteractionService::new(&self.storage, &self.agent);
        let input = HumanInteractionIgnoreInput {
            conversation_id: CONVERSATION.to_string(),
            request_id: request.request_id.clone(),
            expected_revision: request.revision,
            submission_id: format!("ignore-{}", request.request_id),
        };
        let accepted = service.ignore(input.clone(), &self.notifications).unwrap();
        assert_eq!(
            service.ignore(input, &self.notifications).unwrap(),
            accepted,
            "retrying an ignore must preserve its original settlement"
        );
        assert_eq!(accepted.status, HumanInteractionRequestStatus::Ignored);
        assert!(accepted.delivery.is_none());
        assert!(accepted.response.as_ref().unwrap().answers.is_empty());
        accepted
    }

    async fn request(&mut self) -> Value {
        tokio::time::timeout(Duration::from_secs(10), self.requests.recv())
            .await
            .expect("expected authorized Provider sampling")
            .expect("Provider fixture closed")
    }

    fn reply(&self, reply: Reply) {
        self.replies.send(reply).unwrap();
    }

    async fn done(&mut self, status: &str) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let event = self.events.recv().await.unwrap()["params"].clone();
                if status != "failed" {
                    assert_ne!(event["type"], "error", "unexpected Host failure: {event}");
                }
                if event["type"] == "done" {
                    assert_eq!(event["status"], status, "unexpected Host boundary: {event}");
                    return event;
                }
            }
        })
        .await
        .expect("expected Host terminal or suspension notification")
    }

    async fn released(&self, run_id: &str) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self
                .agent
                .cancellations
                .lock()
                .unwrap()
                .contains_key(run_id)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("worker did not release its segment");
    }

    async fn applied(&self, count: usize) -> Vec<HumanInteractionRequestSnapshot> {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let applied = self
                    .batches()
                    .into_iter()
                    .filter(|request| {
                        request.delivery.as_ref().is_some_and(|delivery| {
                            delivery.status == HumanInteractionDeliveryStatus::Applied
                        })
                    })
                    .collect::<Vec<_>>();
                if applied.len() == count {
                    return applied;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("formal answers were not durably applied")
    }

    async fn no_request(&mut self) {
        assert!(
            tokio::time::timeout(Duration::from_millis(100), self.requests.recv())
                .await
                .is_err(),
            "ignored, suspended or duplicate input must not schedule a model request"
        );
    }

    fn usage(&self, count: u64) {
        let usage = self
            .agent
            .get_usage_summary(&AgentUsageSummaryInput {
                range: AgentUsageSummaryRange::All,
                from: None,
                to: None,
            })
            .unwrap();
        assert_eq!(usage.request_count, count);
        assert_eq!(usage.input_tokens, Some(count * 10));
        assert_eq!(usage.output_tokens, Some(count * 2));
        assert_eq!(usage.total_tokens, Some(count * 12));
    }

    fn messages(&self) -> Vec<ChatMessageRecord> {
        self.storage
            .load_conversation(CONVERSATION)
            .unwrap()
            .unwrap()
            .messages
    }

    fn restart(&mut self) {
        let storage = Arc::new(
            StorageService::open_with_model_credentials(&self.database, self.credentials.clone())
                .unwrap(),
        );
        let agent =
            AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&storage)).unwrap();
        agent
            .reconcile_startup_orphaned_conversation_traces()
            .unwrap();
        self.agent = agent;
        self.storage = storage;
        let (notifications, events) = unbounded_channel();
        self.notifications = notifications;
        self.events = events;
        self.agent
            .schedule_human_input_deliveries(self.notifications.clone());
    }
}

fn response_id(request: &HumanInteractionRequestSnapshot) -> &str {
    &request.response.as_ref().unwrap().response_id
}

fn user_displays(request: &Value) -> Vec<HumanInteractionResponseDisplay> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["role"] == "user")
        .filter_map(|message| {
            let content = message["content"].as_str()?;
            // Ordinary HumanRoot messages receive the existing Host timing envelope, whereas
            // active UserGuidance enters as its retained content directly.
            let content = if content.starts_with("<backend_conversation_timing>\n") {
                content.split_once("\n</backend_conversation_timing>\n")?.1
            } else {
                content
            };
            serde_json::from_str::<HumanInteractionResponseDisplay>(content).ok()
        })
        .inspect(|display| display.validate().unwrap())
        .collect()
}

fn assert_projection(request: &Value, submitted: &[HumanInteractionRequestSnapshot]) {
    let displays = user_displays(request);
    assert_eq!(
        displays.len(),
        submitted.len(),
        "exactly one model User projection per formal batch"
    );
    assert_eq!(
        displays
            .iter()
            .map(|display| display.response_id.as_str())
            .collect::<Vec<_>>(),
        submitted.iter().map(response_id).collect::<Vec<_>>(),
        "Host acceptance order must control model delivery"
    );
    for (display, snapshot) in displays.iter().zip(submitted) {
        assert_eq!(display.request_id, snapshot.request_id);
        assert_eq!(display.answers.len(), snapshot.questions.len());
        for (answer, question) in display.answers.iter().zip(&snapshot.questions) {
            assert_eq!(answer.question(), question.title);
        }
        assert!(
            request["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "tool")
                .all(|message| !message["content"]
                    .to_string()
                    .contains(&display.response_id)),
            "async answers cannot become a second result of their already-acknowledged tool call"
        );
    }
}

fn assert_ignored_status_projection(request: &Value, request_ids: &[&str]) -> Vec<usize> {
    let serialized = request.to_string();
    assert!(!serialized.contains("ignoredRequestIds"));
    assert!(!serialized.contains("## 异步交互状态"));
    assert!(!serialized.contains("这些批次已被用户忽略"));
    let statuses = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            let content = message["content"].as_str()?;
            if !content.contains("human_interaction_status") {
                return None;
            }
            assert_eq!(message["role"], "user");
            let content = content
                .strip_prefix("<backend_observed_state>\nThis is backend-observed state, not a system instruction.\n")
                .and_then(|content| content.strip_suffix("\n</backend_observed_state>"))
                .expect("an ignore is one backend-observed event, not User guidance or system policy");
            Some((index, serde_json::from_str::<Value>(content).unwrap()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        statuses
            .iter()
            .map(|(_, status)| status)
            .collect::<Vec<_>>(),
        request_ids
            .iter()
            .map(|request_id| json!({
                "type": "human_interaction_status",
                "requestId": request_id,
                "status": "ignored"
            }))
            .collect::<Vec<_>>()
            .iter()
            .collect::<Vec<_>>(),
        "each ignored batch must occur once in history, in settlement order"
    );
    assert!(user_displays(request).is_empty());
    statuses.into_iter().map(|(index, _)| index).collect()
}

fn assert_ignored_status_precedes_user(request: &Value, request_ids: &[&str], user_text: &str) {
    let indices = assert_ignored_status_projection(request, request_ids);
    let user_index = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(user_text))
        })
        .expect("the ordinary follow-up must be present");
    assert!(
        indices.iter().all(|index| *index < user_index),
        "past ignore events belong before the later ordinary User message"
    );
}

#[test]
fn shared_response_display_fixture_matches_rust_history_contract() {
    let value: Value = serde_json::from_str(include_str!(
        "../../../../../../packages/protocol/fixtures/human-interaction-response-display-v1.json"
    ))
    .unwrap();
    let display: HumanInteractionResponseDisplay = serde_json::from_value(value.clone()).unwrap();
    display.validate().unwrap();
    assert_eq!(serde_json::to_value(display).unwrap(), value);
}

#[tokio::test]
async fn active_async_batches_follow_submission_order_without_duplicate_user_rows_or_usage() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    let first = fixture.request().await;
    assert!(first["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|tool| tool["function"]["name"] == "request_user_input_async"));
    fixture.reply(Reply::Async(3));
    let independent = fixture.request().await;
    assert!(user_displays(&independent).is_empty());
    let requests = fixture.batches();
    assert_eq!(requests.len(), 3);
    assert!(requests
        .windows(2)
        .all(|pair| pair[0].sequence > pair[1].sequence));
    let submitted = requests
        .iter()
        .enumerate()
        .map(|(index, request)| fixture.submit(request, index == 1))
        .collect::<Vec<_>>();
    fixture.reply(Reply::Complete);
    let continuation = fixture.request().await;
    assert_projection(&continuation, &submitted);
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_eq!(done["runId"], turn.run_id);
    assert_eq!(done["usage"]["billableRequestCount"], 3);
    let applied = fixture.applied(3).await;
    assert!(applied
        .iter()
        .all(
            |request| request.delivery.as_ref().is_some_and(|delivery| delivery
                .target_run_id
                .as_deref()
                == Some(turn.run_id.as_str())
                && delivery.user_message_id.is_none())
        ));
    assert_eq!(
        fixture.messages().len(),
        2,
        "active answers are assistant UserGuidance history, never extra User rows"
    );
    let trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    let guidance = trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::UserGuidance { content, .. } => {
                Some(serde_json::from_str::<HumanInteractionResponseDisplay>(content).unwrap())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        guidance
            .iter()
            .map(|display| display.response_id.as_str())
            .collect::<Vec<_>>(),
        submitted.iter().map(response_id).collect::<Vec<_>>()
    );
    fixture.usage(3);
    fixture.no_request().await;
    fixture.released(&turn.run_id).await;
    let followup = fixture.start_with_suffix("-followup");
    assert_ne!(followup.run_id, turn.run_id);
    let replayed = fixture.request().await;
    assert_projection(&replayed, &submitted);
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.usage(4);
    assert_eq!(fixture.messages().len(), 4);
    fixture.no_request().await;
}

async fn assert_idle_answer(skip_all: bool, restart: Option<bool>) {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;
    let request = fixture.batches().pop().unwrap();
    let submitted = if restart == Some(true) {
        let accepted = fixture
            .storage
            .submit_human_interaction(&Fixture::submission(&request, skip_all))
            .unwrap();
        assert_eq!(
            accepted.delivery.as_ref().unwrap().status,
            HumanInteractionDeliveryStatus::Pending
        );
        fixture.restart();
        accepted
    } else {
        if restart == Some(false) {
            fixture.restart();
            fixture.no_request().await;
        }
        fixture.submit(&request, skip_all)
    };
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    if skip_all {
        assert!(
            serde_json::to_value(user_displays(&continuation)[0].clone()).unwrap()["answers"]
                .as_array()
                .unwrap()
                .iter()
                .all(|answer| answer["kind"] == "skipped" && answer["answer"] == "已跳过")
        );
    }
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_ne!(
        done["runId"], turn.run_id,
        "idle answer starts a new HumanRoot Run"
    );
    let applied = fixture.applied(1).await.pop().unwrap();
    let delivery = applied.delivery.as_ref().unwrap();
    assert_eq!(delivery.target_run_id.as_deref(), done["runId"].as_str());
    let messages = fixture.messages();
    assert_eq!(
        messages.len(),
        4,
        "one User + assistant pair for an idle answer"
    );
    let message = messages
        .iter()
        .find(|message| Some(message.id.as_str()) == delivery.user_message_id.as_deref())
        .unwrap();
    assert_eq!(message.role, "user");
    let display: HumanInteractionResponseDisplay = serde_json::from_str(&message.content).unwrap();
    display.validate().unwrap();
    assert_eq!(display.response_id, response_id(&submitted));
    let repeat = fixture.submit(&request, skip_all);
    assert_eq!(repeat.response, submitted.response);
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn async_answer_after_natural_completion_starts_one_human_root_turn() {
    assert_idle_answer(false, None).await;
}

#[tokio::test]
async fn answer_accepted_while_active_chooses_one_idle_route_after_natural_terminal_race() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    let request = fixture.batches().pop().unwrap();
    // Freeze the real durable interval between acceptance and its scheduling hint. The original
    // worker reaches its terminal fence before the coordinator observes this response.
    let submitted = fixture
        .storage
        .submit_human_interaction(&Fixture::submission(&request, false))
        .unwrap();
    assert_eq!(
        submitted.delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;
    fixture
        .agent
        .schedule_human_input_deliveries(fixture.notifications.clone());
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_ne!(done["runId"], turn.run_id);
    let applied = fixture.applied(1).await.pop().unwrap();
    assert_eq!(
        applied.delivery.as_ref().unwrap().target_run_id.as_deref(),
        done["runId"].as_str()
    );
    assert!(applied.delivery.unwrap().user_message_id.is_some());
    let original_trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert!(!original_trace
        .items
        .iter()
        .any(|item| matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn all_skipped_async_batch_is_a_formal_answer_that_starts_one_idle_turn() {
    assert_idle_answer(true, None).await;
}

#[tokio::test]
async fn open_async_batch_survives_restart_after_its_original_run_completed() {
    assert_idle_answer(false, Some(false)).await;
}

#[tokio::test]
async fn committed_async_answer_is_delivered_once_after_restart_before_dispatch() {
    assert_idle_answer(false, Some(true)).await;
}

#[tokio::test]
async fn ignore_one_async_batch_does_not_deliver_or_wake_and_leaves_other_batches_open() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(2));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;
    let batches = fixture.batches();
    let input = HumanInteractionIgnoreInput {
        conversation_id: CONVERSATION.to_string(),
        request_id: batches[0].request_id.clone(),
        expected_revision: batches[0].revision,
        submission_id: "ignore-current-batch".to_string(),
    };
    let human = HumanInteractionService::new(&fixture.storage, &fixture.agent);
    let ignored = human.ignore(input.clone(), &fixture.notifications).unwrap();
    assert_eq!(
        human.ignore(input, &fixture.notifications).unwrap(),
        ignored
    );
    assert_eq!(ignored.status, HumanInteractionRequestStatus::Ignored);
    assert!(ignored.delivery.is_none());
    assert!(ignored.response.as_ref().unwrap().answers.is_empty());
    fixture
        .agent
        .schedule_human_input_deliveries(fixture.notifications.clone());
    fixture.no_request().await;
    assert_eq!(
        fixture.batches()[1].status,
        HumanInteractionRequestStatus::Open
    );
    assert_eq!(fixture.messages().len(), 2);
    fixture.usage(2);
}

#[tokio::test]
async fn active_ignored_batches_enter_natural_samples_once_and_remain_in_history_after_restart() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    assert_ignored_status_projection(&fixture.request().await, &[]);
    fixture.reply(Reply::Async(2));
    assert_ignored_status_projection(&fixture.request().await, &[]);
    let batches = fixture.batches();
    let first = fixture.ignore(&batches[1]);
    fixture.no_request().await;

    // The next tool batch creates a natural sampling boundary. Ignoring must not create one.
    fixture.reply(Reply::Async(1));
    let after_first = fixture.request().await;
    let first_indices = assert_ignored_status_projection(&after_first, &[&first.request_id]);
    let first_last_tool = after_first["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rposition(|message| message["role"] == "tool")
        .unwrap();
    assert!(first_indices[0] > first_last_tool);
    let still_open = fixture.batches()[0].clone();
    let second = fixture.ignore(&batches[0]);
    fixture.ignore(&batches[1]);
    fixture.no_request().await;

    fixture.reply(Reply::Async(1));
    let after_second = fixture.request().await;
    let indices =
        assert_ignored_status_projection(&after_second, &[&first.request_id, &second.request_id]);
    let last_tool = after_second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rposition(|message| message["role"] == "tool")
        .unwrap();
    assert!(
        indices[0] < last_tool && indices[1] > last_tool,
        "an earlier event must keep its historical position when a later ignore is appended"
    );
    assert_eq!(
        fixture
            .batches()
            .iter()
            .find(|request| request.request_id == still_open.request_id)
            .unwrap()
            .status,
        HumanInteractionRequestStatus::Open
    );
    fixture.reply(Reply::Complete);
    assert_eq!(fixture.done("completed").await["runId"], turn.run_id);
    fixture.released(&turn.run_id).await;
    fixture.no_request().await;
    assert_eq!(fixture.messages().len(), 2);
    let trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert!(trace
        .items
        .iter()
        .all(|item| !matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
    fixture.usage(4);

    fixture.restart();
    fixture.no_request().await;
    let followup_text = "Ordinary follow-up after naturally observed ignore events.";
    fixture.start_with_content("-ignore-followup", followup_text.to_string());
    let replayed = fixture.request().await;
    assert_ignored_status_precedes_user(
        &replayed,
        &[&first.request_id, &second.request_id],
        followup_text,
    );
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(5);
    fixture.no_request().await;
}

#[tokio::test]
async fn idle_ignored_batches_survive_restart_without_waking_and_precede_the_next_user() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(3));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;

    let batches = fixture.batches();
    // Deliberately differ from the descending question-list order.
    let first = fixture.ignore(&batches[2]);
    let second = fixture.ignore(&batches[0]);
    fixture
        .agent
        .schedule_human_input_deliveries(fixture.notifications.clone());
    fixture.no_request().await;
    assert_eq!(fixture.messages().len(), 2);
    fixture.usage(2);

    fixture.restart();
    fixture.no_request().await;
    assert_eq!(fixture.ignore(&batches[2]), first);
    assert_eq!(fixture.ignore(&batches[0]), second);
    assert_eq!(
        fixture
            .batches()
            .iter()
            .find(|request| request.request_id == batches[1].request_id)
            .unwrap()
            .status,
        HumanInteractionRequestStatus::Open
    );
    fixture.no_request().await;
    assert_eq!(fixture.messages().len(), 2);
    let followup_text = "Ordinary follow-up after idle ignore settlements and restart.";
    let followup = fixture.start_with_content("-idle-ignore", followup_text.to_string());
    assert_ne!(followup.run_id, turn.run_id);
    let replayed = fixture.request().await;
    assert_ignored_status_precedes_user(
        &replayed,
        &[&first.request_id, &second.request_id],
        followup_text,
    );
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn ignore_during_the_final_request_does_not_extend_the_run_and_replays_after_restart() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    let batch = fixture.batches().pop().unwrap();
    let ignored = fixture.ignore(&batch);
    fixture.no_request().await;
    fixture.reply(Reply::Complete);
    assert_eq!(fixture.done("completed").await["runId"], turn.run_id);
    fixture.released(&turn.run_id).await;
    fixture.no_request().await;
    assert_eq!(fixture.messages().len(), 2);
    fixture.usage(2);

    // No later sampling boundary observed the event in its original Run.
    fixture.restart();
    fixture.no_request().await;
    assert_eq!(fixture.ignore(&batch), ignored);
    let followup_text = "Ordinary follow-up after an ignore during the prior final request.";
    fixture.start_with_content("-terminal-ignore", followup_text.to_string());
    let replayed = fixture.request().await;
    assert_ignored_status_precedes_user(&replayed, &[&ignored.request_id], followup_text);
    let ignored_index = assert_ignored_status_projection(&replayed, &[&ignored.request_id])[0];
    let final_index = replayed["messages"]
        .as_array()
        .unwrap()
        .iter()
        .position(|message| {
            message["role"] == "assistant" && message["content"] == "Independent work finished."
        })
        .unwrap();
    assert!(
        final_index < ignored_index,
        "the final request did not observe the ignore: its completed reply must precede the event"
    );
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn ignored_history_after_compaction_is_ordinary_history_and_is_not_reinjected() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(2));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;

    let prefix = fixture
        .storage
        .prepare_context_compaction_prefix(CONVERSATION, &ContextJournalCursor::message(ASSISTANT))
        .unwrap();
    fixture
        .storage
        .commit_context_compaction_prefix(
            &prefix,
            test_compaction_draft(
                &prefix,
                "summary-before-ignore",
                "SUMMARY_BEFORE_IGNORE",
                200,
                8,
            ),
            ASSISTANT,
        )
        .unwrap();
    let batches = fixture.batches();
    let first = fixture.ignore(&batches[1]);
    let second = fixture.ignore(&batches[0]);
    fixture.no_request().await;
    assert_eq!(
        fixture
            .storage
            .get_active_context_compaction_summary(CONVERSATION)
            .unwrap()
            .unwrap()
            .id,
        "summary-before-ignore",
        "a later postlude cannot invalidate an already covered completion"
    );
    fixture.restart();
    let trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    let first_sequence = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::BackendState {
                sequence, content, ..
            } if serde_json::from_str::<Value>(content).unwrap()["requestId"]
                == first.request_id =>
            {
                Some(*sequence)
            }
            _ => None,
        })
        .unwrap();
    // Compact one ordinary event and leave the later event after the cursor. No special state
    // retention may reintroduce the first request ID from its durable ignored response.
    let next = fixture
        .storage
        .prepare_context_compaction_prefix(
            CONVERSATION,
            &ContextJournalCursor::trace_item(ASSISTANT, first_sequence),
        )
        .unwrap();
    fixture
        .storage
        .commit_context_compaction_prefix(
            &next,
            test_compaction_draft(
                &next,
                "summary-after-ignore",
                "SUMMARY_AFTER_ONE_IGNORE",
                200,
                8,
            ),
            ASSISTANT,
        )
        .unwrap();
    fixture.restart();
    fixture.no_request().await;
    let followup = "Normal follow-up after compacting an ignored event.";
    fixture.start_with_content("-compacted-ignore", followup.to_string());
    let request = fixture.request().await;
    assert_ignored_status_precedes_user(&request, &[&second.request_id], followup);
    assert!(request.to_string().contains("SUMMARY_AFTER_ONE_IGNORE"));
    assert!(!request.to_string().contains(&first.request_id));
    assert!(!request.to_string().contains("Independent work finished."));
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.usage(3);
    assert_eq!(fixture.messages().len(), 4);
    fixture.no_request().await;
}

async fn assert_suspended_async_delivery(sync: bool, queued_before_pause: bool) {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    let asynchronous = fixture
        .batches()
        .into_iter()
        .find(|request| request.mode == HumanInteractionMode::Async)
        .unwrap();
    let before_pause = queued_before_pause.then(|| fixture.submit(&asynchronous, false));
    let previous_guidance = fixture.storage.list_queued_agent_run_guidances().unwrap();
    if queued_before_pause {
        assert_eq!(previous_guidance.len(), 1);
        assert_eq!(
            fixture.batches()[0].delivery.as_ref().unwrap().status,
            HumanInteractionDeliveryStatus::Bound
        );
    }
    fixture.reply(if sync { Reply::Sync } else { Reply::Approval });
    fixture
        .done(if sync {
            "waiting_for_user_input"
        } else {
            "waiting_for_approval"
        })
        .await;
    fixture.released(&turn.run_id).await;
    let submitted = before_pause.unwrap_or_else(|| fixture.submit(&asynchronous, false));
    fixture.no_request().await;
    assert_eq!(
        fixture
            .batches()
            .into_iter()
            .find(|request| request.request_id == asynchronous.request_id)
            .unwrap()
            .delivery
            .unwrap()
            .status,
        if sync {
            HumanInteractionDeliveryStatus::Pending
        } else {
            HumanInteractionDeliveryStatus::Bound
        },
        "approval retains the inbox while a synchronous pause keeps answers pending"
    );
    if sync {
        let blocking = fixture
            .batches()
            .into_iter()
            .find(|request| request.mode == HumanInteractionMode::Sync)
            .unwrap();
        fixture.submit(&blocking, false);
    } else {
        let pending = fixture.agent.list_pending_actions();
        assert_eq!(pending.len(), 1);
        fixture
            .agent
            .approve_action(
                &turn.run_id,
                &pending[0].action_id,
                fixture.notifications.clone(),
            )
            .unwrap();
    }
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_eq!(done["runId"], turn.run_id);
    fixture.applied(if sync { 2 } else { 1 }).await;
    if queued_before_pause {
        let old = fixture
            .storage
            .load_agent_run_guidance(&previous_guidance[0].guidance_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            old.status,
            if sync {
                AgentGuidanceStatus::Rejected
            } else {
                AgentGuidanceStatus::Applied
            }
        );
        let trace = fixture
            .storage
            .get_conversation_turn_trace(ASSISTANT)
            .unwrap()
            .unwrap();
        let applied_ids = trace
            .items
            .iter()
            .filter_map(|item| match item {
                ConversationTurnTraceItem::UserGuidance { guidance_id, .. } => Some(guidance_id),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(applied_ids.len(), 1);
        if sync {
            assert_ne!(applied_ids[0], &previous_guidance[0].guidance_id);
        } else {
            assert_eq!(applied_ids[0], &previous_guidance[0].guidance_id);
        }
    }
    assert_eq!(fixture.messages().len(), 2);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn async_submission_waits_for_command_approval_then_enters_the_same_run() {
    assert_suspended_async_delivery(false, false).await;
}

#[tokio::test]
async fn async_submission_waits_for_sync_answer_then_enters_the_same_run() {
    assert_suspended_async_delivery(true, false).await;
}

#[tokio::test]
async fn async_guidance_queued_before_approval_retains_identity_after_same_run_resume() {
    assert_suspended_async_delivery(false, true).await;
}

#[tokio::test]
async fn async_guidance_queued_before_sync_pause_is_rebound_once_after_same_run_resume() {
    assert_suspended_async_delivery(true, true).await;
}

#[tokio::test]
async fn stop_cancels_already_accepted_async_delivery_before_dispatch_and_prevents_restart_wake() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    fixture.reply(Reply::Sync);
    fixture.done("waiting_for_user_input").await;
    fixture.released(&turn.run_id).await;
    let asynchronous = fixture
        .batches()
        .into_iter()
        .find(|request| request.mode == HumanInteractionMode::Async)
        .unwrap();
    let accepted = fixture
        .storage
        .submit_human_interaction(&Fixture::submission(&asynchronous, false))
        .unwrap();
    assert_eq!(
        accepted.delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    assert!(fixture.agent.cancel_run_checked(&turn.run_id).unwrap());
    fixture
        .agent
        .schedule_human_input_deliveries(fixture.notifications.clone());
    fixture.no_request().await;
    let cancelled = fixture
        .batches()
        .into_iter()
        .find(|request| request.request_id == asynchronous.request_id)
        .unwrap();
    assert_eq!(cancelled.status, HumanInteractionRequestStatus::Submitted);
    assert_eq!(
        cancelled.delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Cancelled
    );
    fixture.restart();
    fixture.no_request().await;
    let repeated = fixture.submit(&asynchronous, false);
    assert_eq!(
        repeated.delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Cancelled
    );
    fixture.no_request().await;
    assert_eq!(fixture.messages().len(), 2);
    fixture.usage(2);
}

#[tokio::test]
async fn new_explicit_async_answer_after_stop_is_new_user_intent_and_starts_one_turn() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    fixture.reply(Reply::Sync);
    fixture.done("waiting_for_user_input").await;
    fixture.released(&turn.run_id).await;
    assert!(fixture.agent.cancel_run_checked(&turn.run_id).unwrap());
    fixture.no_request().await;
    let asynchronous = fixture
        .batches()
        .into_iter()
        .find(|request| request.mode == HumanInteractionMode::Async)
        .unwrap();
    assert_eq!(asynchronous.status, HumanInteractionRequestStatus::Open);
    let submitted = fixture.submit(&asynchronous, false);
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_ne!(done["runId"], turn.run_id);
    fixture.applied(1).await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn async_answer_waits_for_real_manual_compaction_and_drains_after_its_completion() {
    let mut fixture = Fixture::new().await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let generated = super::provider_transition::provider_transition_generator("model-1");
    let ready = Arc::clone(&entered);
    let unblock = Arc::clone(&release);
    let generator: ContextCompactionSummaryGenerator = Arc::new(move |request, cancellation| {
        let generated = Arc::clone(&generated);
        let ready = Arc::clone(&ready);
        let unblock = Arc::clone(&unblock);
        Box::pin(async move {
            ready.notify_one();
            unblock.notified().await;
            let mut output = generated(request, cancellation).await?;
            output.observation.actual_usage = Some(mycopilot_core::ModelRequestActualUsage {
                raw: AgentUsage {
                    input_tokens: Some(10),
                    output_tokens: Some(2),
                    total_tokens: Some(12),
                    billable_request_count: Some(1),
                    output_thinking_tokens: None,
                    cached_input_tokens: None,
                    cache_creation_input_tokens: None,
                },
                normalized_input_tokens: Some(10),
                normalization: mycopilot_core::ModelRequestUsageNormalization::OpenAiInputTokens,
            });
            Ok(output)
        })
    });
    fixture.agent = fixture
        .agent
        .clone()
        .with_context_compaction_summary_generator(generator);
    let turn = fixture.start_with_content("", ORIGINAL.repeat(200));
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&turn.run_id).await;
    let request = fixture.batches().pop().unwrap();
    let operation = fixture
        .agent
        .start_manual_context_compaction(
            AgentManualContextCompactionStartInput {
                conversation_id: CONVERSATION.to_string(),
                request_id: "compact-before-async-delivery".to_string(),
            },
            fixture.notifications.clone(),
        )
        .unwrap();
    assert_eq!(operation.status, "running");
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    let submitted = fixture.submit(&request, false);
    fixture.no_request().await;
    assert_eq!(
        fixture.batches()[0].delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Pending
    );
    assert!(fixture
        .agent
        .ensure_no_manual_context_compaction(CONVERSATION)
        .is_err());
    assert_eq!(
        fixture.messages().len(),
        2,
        "no worker does not make a compacting conversation idle"
    );
    release.notify_one();
    // Completion itself must drain the pending response; no frontend event or explicit retry.
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    fixture.reply(Reply::Complete);
    let done = fixture.done("completed").await;
    assert_ne!(done["runId"], turn.run_id);
    let compacted = fixture
        .agent
        .get_manual_context_compaction_status(AgentManualContextCompactionStatusInput {
            conversation_id: CONVERSATION.to_string(),
            operation_id: Some(operation.operation_id),
        })
        .unwrap()
        .operations
        .pop()
        .unwrap();
    assert_eq!(compacted.status, "completed", "{:?}", compacted.error);
    assert!(!compacted.is_busy);
    fixture.applied(1).await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(4); // Three real model transports plus one separately owned compaction request.
    fixture.no_request().await;
}

#[tokio::test]
async fn failed_idle_answer_is_settled_without_restart_and_a_later_batch_can_continue() {
    let mut fixture = Fixture::new().await;
    let original = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(2));
    fixture.request().await;
    fixture.reply(Reply::Complete);
    fixture.done("completed").await;
    fixture.released(&original.run_id).await;
    let batches = fixture.batches();
    let first = fixture.submit(&batches[0], false);
    let failed_request = fixture.request().await;
    assert_projection(&failed_request, std::slice::from_ref(&first));
    fixture.reply(Reply::RejectRequest);
    let failed = fixture.done("failed").await;
    assert_ne!(failed["runId"], original.run_id);
    fixture.released(failed["runId"].as_str().unwrap()).await;
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let request = fixture
                .batches()
                .into_iter()
                .find(|request| request.request_id == first.request_id)
                .unwrap();
            if request.delivery.as_ref().unwrap().status == HumanInteractionDeliveryStatus::Failed {
                return request;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("terminal model failure must settle its executing receipt before restart");
    assert_eq!(settled.status, HumanInteractionRequestStatus::Submitted);
    let usage_input = AgentUsageSummaryInput {
        range: AgentUsageSummaryRange::All,
        from: None,
        to: None,
    };
    let before_retry = fixture.agent.get_usage_summary(&usage_input).unwrap();
    assert_eq!(
        fixture.submit(&batches[0], false).delivery.unwrap().status,
        HumanInteractionDeliveryStatus::Failed
    );
    fixture
        .agent
        .schedule_human_input_deliveries(fixture.notifications.clone());
    fixture.no_request().await;
    let after_retry = fixture.agent.get_usage_summary(&usage_input).unwrap();
    assert_eq!(after_retry.request_count, before_retry.request_count);
    assert_eq!(after_retry.total_tokens, before_retry.total_tokens);
    let second = fixture.submit(&batches[1], false);
    let resumed = fixture.request().await;
    assert_projection(&resumed, &[first, second]);
    fixture.reply(Reply::Complete);
    let completed = fixture.done("completed").await;
    assert_ne!(completed["runId"], failed["runId"]);
    fixture.applied(1).await;
    assert_eq!(
        fixture.messages().len(),
        6,
        "each formal batch owns exactly one User and one assistant row"
    );
    let after_second = fixture.agent.get_usage_summary(&usage_input).unwrap();
    assert_eq!(after_second.request_count, before_retry.request_count + 1);
    assert_eq!(
        after_second.total_tokens,
        before_retry.total_tokens.map(|tokens| tokens + 12)
    );
    fixture.no_request().await;
}

#[tokio::test]
async fn pre_runtime_approval_failure_releases_occupancy_and_drains_pending_async_answer() {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(1));
    fixture.request().await;
    fixture.reply(Reply::Approval);
    fixture.done("waiting_for_approval").await;
    fixture.released(&turn.run_id).await;
    let request = fixture.batches().pop().unwrap();
    let submitted = fixture.submit(&request, false);
    fixture.no_request().await;
    assert_eq!(
        fixture.batches()[0].delivery.as_ref().unwrap().status,
        HumanInteractionDeliveryStatus::Bound
    );

    // Reuse the existing pre-Runtime continuation failure test entrance: the real Host pending
    // action and its checkpoint remain valid. Only its reconstructed Skill snapshot is invalid.
    // Restoration fails before Runtime or command execution can begin.
    let pending = fixture
        .agent
        .pending_actions
        .lock()
        .unwrap()
        .values()
        .find(|record| record.snapshot.run_id == turn.run_id)
        .unwrap()
        .clone();
    fixture
        .agent
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = fixture.agent.pending_actions.lock().unwrap()[&pending.storage_id].clone();
    fixture
        .agent
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    let mut resumed = approved.agent_input.clone();
    let checkpoint = resumed.resume_checkpoint.as_mut().unwrap();
    if let Some(skills) = checkpoint
        .extension_snapshots
        .iter_mut()
        .find(|snapshot| snapshot.extension_id == "skills")
    {
        skills.version = u32::MAX;
    } else {
        checkpoint
            .extension_snapshots
            .push(mycopilot_core::AgentExtensionSnapshot {
                extension_id: "skills".to_string(),
                version: u32::MAX,
                state: json!({}),
            });
    }
    fixture
        .agent
        .run_action_continuation(
            approved,
            resumed,
            fixture.notifications.clone(),
            PendingActionStatus::Completed,
            None,
        )
        .await;
    let failed = fixture.done("failed").await;
    assert_eq!(failed["runId"], turn.run_id);
    let trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    let terminal_pending = fixture
        .storage
        .get_pending_agent_action(&pending.storage_id)
        .unwrap()
        .unwrap();
    assert_eq!(terminal_pending.status, "failed");
    let original_usage = fixture
        .storage
        .load_agent_usage_for_owner(&turn.run_id, CONVERSATION, ASSISTANT)
        .unwrap()
        .unwrap();
    assert_eq!(original_usage.status.as_deref(), Some("failed"));
    assert!(original_usage
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Skill extension version")));

    // The terminal helper itself must drain the saved response, without a submit retry or a
    // frontend scheduling hint. No Provider request was made by the failed continuation.
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&submitted));
    fixture.reply(Reply::Complete);
    let completed = fixture.done("completed").await;
    assert_ne!(completed["runId"], turn.run_id);
    let applied = fixture.applied(1).await.pop().unwrap();
    assert_eq!(
        applied.delivery.as_ref().unwrap().target_run_id.as_deref(),
        completed["runId"].as_str()
    );
    assert!(applied.delivery.unwrap().user_message_id.is_some());
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

async fn assert_pre_runtime_sync_revocation(explicit_stop: bool) {
    let mut fixture = Fixture::new().await;
    let turn = fixture.start();
    fixture.request().await;
    fixture.reply(Reply::Async(2));
    fixture.request().await;
    fixture.reply(Reply::Sync);
    fixture.done("waiting_for_user_input").await;
    fixture.released(&turn.run_id).await;
    let batches = fixture.batches();
    let asynchronous = batches
        .iter()
        .filter(|request| request.mode == HumanInteractionMode::Async)
        .cloned()
        .collect::<Vec<_>>();
    let blocking = batches
        .iter()
        .find(|request| request.mode == HumanInteractionMode::Sync)
        .unwrap();
    let submitted = fixture.submit(&asynchronous[0], false);
    fixture.no_request().await;

    // The old worker has retired. Revoke the new claim at the last storage execution fence,
    // after reconstruction but before any resumed Tool or Provider request can run.
    let service = fixture.agent.clone();
    let run_id = turn.run_id.clone();
    super::super::turn_executor::install_before_human_resume_execution_hook(
        &turn.run_id,
        Arc::new(move || {
            if explicit_stop {
                assert!(service.cancel_run_checked(&run_id).unwrap());
            } else {
                service
                    .storage
                    .cancel_sync_human_interactions_for_run(&run_id)
                    .unwrap();
            }
        }),
    );
    fixture.submit(blocking, true);
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = fixture.events.recv().await {
            if event["params"]["code"] == "human_input_resume_failed" {
                return;
            }
        }
        panic!("missing refused sync resume event");
    })
    .await
    .expect("sync resume refusal did not settle");
    assert!(
        !fixture
            .agent
            .active_runs
            .lock()
            .unwrap()
            .contains_key(&turn.run_id),
        "a refused resume must not leave an accepting queue without a worker"
    );
    fixture.released(&turn.run_id).await;
    let trace = fixture
        .storage
        .get_conversation_turn_trace(ASSISTANT)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );

    let expected = if explicit_stop {
        fixture.no_request().await;
        let cancelled = fixture
            .batches()
            .into_iter()
            .find(|request| request.request_id == submitted.request_id)
            .unwrap();
        assert_eq!(
            cancelled.delivery.unwrap().status,
            HumanInteractionDeliveryStatus::Cancelled
        );
        // This later answer is explicit user intent after Stop. A stale accepting queue would
        // steal it from the new-root route and leave it stranded until the next process restart.
        fixture.submit(&asynchronous[1], false)
    } else {
        submitted
    };
    let continuation = fixture.request().await;
    assert_projection(&continuation, std::slice::from_ref(&expected));
    fixture.reply(Reply::Complete);
    let completed = fixture.done("completed").await;
    assert_ne!(completed["runId"], turn.run_id);
    fixture.applied(1).await;
    assert_eq!(fixture.messages().len(), 4);
    fixture.usage(3);
    fixture.no_request().await;
}

#[tokio::test]
async fn revoked_sync_resume_claim_retires_control_and_drains_pending_async_answer() {
    assert_pre_runtime_sync_revocation(false).await;
}

#[tokio::test]
async fn stop_before_sync_resume_execution_retires_control_and_allows_later_explicit_answer() {
    assert_pre_runtime_sync_revocation(true).await;
}

fn assert_question_contract(request: &Value, enabled: bool) {
    for tool in ["request_user_input", "request_user_input_async"] {
        assert_eq!(
            request["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["function"]["name"] == tool),
            enabled,
        );
    }
    assert_eq!(
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["content"]
                .as_str()
                .is_some_and(|text| text.contains("## 人机交互"))),
        enabled,
    );
}

#[tokio::test]
async fn live_host_setting_close_rejects_in_flight_model_questions_then_reopens_next_snapshot() {
    for synchronous in [false, true] {
        let mut fixture = Fixture::new().await;
        fixture.start();
        assert_question_contract(&fixture.request().await, true);
        HumanInteractionService::new(&fixture.storage, &fixture.agent)
            .update_settings(
                mycopilot_core::human_interaction::HumanInteractionSettingsUpdate {
                    enabled: false,
                    expected_revision: 0,
                },
                &fixture.notifications,
            )
            .unwrap();
        // The Provider saw an enabled schema, but durable admission must recheck the later
        // setting transaction. Neither mode may publish a question or suspend the worker.
        fixture.reply(if synchronous {
            Reply::Sync
        } else {
            Reply::Async(1)
        });
        assert_question_contract(&fixture.request().await, false);
        assert!(fixture.batches().is_empty());
        HumanInteractionService::new(&fixture.storage, &fixture.agent)
            .update_settings(
                mycopilot_core::human_interaction::HumanInteractionSettingsUpdate {
                    enabled: true,
                    expected_revision: 1,
                },
                &fixture.notifications,
            )
            .unwrap();
        // Reopening the live policy cannot authorize a call absent from this model request.
        fixture.reply(if synchronous {
            Reply::Sync
        } else {
            Reply::Async(1)
        });
        assert_question_contract(&fixture.request().await, true);
        assert!(fixture.batches().is_empty());
        fixture.reply(Reply::Complete);
        fixture.done("completed").await;
        let messages = fixture.messages();
        let trace = fixture
            .storage
            .get_conversation_turn_trace(&messages[1].id)
            .unwrap()
            .unwrap();
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { success: false, .. }
                ))
                .count(),
            2
        );
        fixture.usage(3);
        fixture.no_request().await;
    }
}

#[tokio::test]
async fn disabled_host_setting_still_delivers_previously_admitted_sync_and_async_answers() {
    for synchronous in [false, true] {
        let mut fixture = Fixture::new().await;
        let turn = fixture.start();
        fixture.request().await;
        fixture.reply(if synchronous {
            Reply::Sync
        } else {
            Reply::Async(1)
        });
        if synchronous {
            fixture.done("waiting_for_user_input").await;
            fixture.released(&turn.run_id).await;
        } else {
            fixture.request().await;
        }
        let request = fixture.batches().pop().unwrap();
        HumanInteractionService::new(&fixture.storage, &fixture.agent)
            .update_settings(
                mycopilot_core::human_interaction::HumanInteractionSettingsUpdate {
                    enabled: false,
                    expected_revision: 0,
                },
                &fixture.notifications,
            )
            .unwrap();
        let answer = fixture.submit(&request, true);
        if !synchronous {
            fixture.reply(Reply::Complete);
        }
        let continuation = fixture.request().await;
        assert_question_contract(&continuation, false);
        if synchronous {
            let results = continuation["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|message| message["role"] == "tool")
                .filter_map(|message| {
                    serde_json::from_str::<HumanInteractionResponseDisplay>(
                        message["content"].as_str()?,
                    )
                    .ok()
                })
                .collect::<Vec<_>>();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].response_id, response_id(&answer));
            assert!(user_displays(&continuation).is_empty());
        } else {
            assert_projection(&continuation, std::slice::from_ref(&answer));
        }
        fixture.reply(Reply::Complete);
        let completed = fixture.done("completed").await;
        assert_eq!(completed["runId"], turn.run_id);
        fixture.applied(1).await;
        assert_eq!(fixture.messages().len(), 2);
        fixture.usage(if synchronous { 2 } else { 3 });
        fixture.no_request().await;
    }
}
