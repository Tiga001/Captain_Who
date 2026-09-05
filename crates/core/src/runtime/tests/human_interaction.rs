use super::*;
use crate::human_interaction::HumanInteractionSettings;
use crate::{
    AgentAsyncUserInputAccepted, AgentAsyncUserInputRequest, AgentHumanInteractionRuntimeHost,
    AgentHumanInteractionSamplingState, AgentSamplingBoundaryRequest, AgentUserInputResume,
    AgentUserInputSuspension,
};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct HumanHost {
    enabled: AtomicBool,
    pauses: Mutex<Vec<AgentUserInputSuspension>>,
    async_ready: bool,
    accepted: Mutex<Vec<AgentAsyncUserInputRequest>>,
    ignored: Mutex<Vec<String>>,
    natural_samples: Mutex<Vec<AgentSamplingBoundaryRequest>>,
    answer_queue: Option<AgentSteerInputQueue>,
    cancel_after_accept: Option<AgentCancellationToken>,
}

impl HumanHost {
    fn enabled() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(true),
            ..Self::default()
        })
    }

    fn services(self: &Arc<Self>) -> AgentRuntimeHostServices {
        AgentRuntimeHostServices::new()
            .with_human_interaction_policy(self.clone())
            .with_human_interaction_runtime(self.clone())
    }

    fn take_pause(&self) -> AgentUserInputSuspension {
        self.pauses
            .lock()
            .unwrap()
            .pop()
            .expect("durable question pause")
    }
}

impl HumanInteractionPolicySource for HumanHost {
    fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
        let enabled = self.enabled.load(Ordering::SeqCst);
        Ok(HumanInteractionSettings {
            enabled,
            revision: u64::from(!enabled),
            updated_at: 1,
        })
    }
}

impl AgentHumanInteractionRuntimeHost for HumanHost {
    fn async_execution_ready(&self) -> bool {
        self.async_ready
    }

    fn accept_async(
        &self,
        request: AgentAsyncUserInputRequest,
    ) -> AgentResult<AgentAsyncUserInputAccepted> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(AgentError::new("questions disabled"));
        }
        assert_eq!(request.conversation_id, "conversation-human");
        assert_eq!(request.run_id, "run-human");
        assert_eq!(request.assistant_message_id, "assistant-human");
        assert_eq!(
            request.call.approval_status,
            AgentApprovalStatus::NotRequired
        );
        let mut accepted = self.accepted.lock().unwrap();
        let request_id = format!("async-request-{}", accepted.len() + 1);
        if let Some(queue) = &self.answer_queue {
            let response_id = format!("async-response-{}", accepted.len() + 1);
            queue.enqueue(crate::AgentSteerInput {
                guidance_id: response_id.clone(),
                client_message_id: response_id.clone(),
                content: async_answer(&request_id, &response_id),
                created_at: 1,
                attachments: Vec::new(),
                attachment_library: None,
            })?;
        }
        accepted.push(request);
        if let Some(cancellation) = &self.cancel_after_accept {
            cancellation.cancel();
        }
        Ok(AgentAsyncUserInputAccepted { request_id })
    }

    fn natural_sampling_state(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<AgentHumanInteractionSamplingState> {
        self.natural_samples.lock().unwrap().push(request);
        Ok(AgentHumanInteractionSamplingState {
            ignored_request_ids: self.ignored.lock().unwrap().clone(),
        })
    }

    fn suspend(&self, suspension: AgentUserInputSuspension) -> AgentResult<()> {
        if !self.enabled.load(Ordering::SeqCst) {
            return Err(AgentError::new("questions disabled"));
        }
        assert_eq!(
            suspension.call.approval_status,
            AgentApprovalStatus::NotRequired
        );
        assert_eq!(
            suspension.checkpoint.pause_reason,
            crate::AgentRunCheckpointPauseReason::UserInput
        );
        assert!(suspension.checkpoint.pending_action_id.is_none());
        self.pauses.lock().unwrap().push(suspension);
        Ok(())
    }
}

async fn read_request(stream: &mut TcpStream) -> Value {
    let mut bytes = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let count = stream.read(&mut chunk).await.unwrap();
        assert_ne!(count, 0);
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            if bytes.len() >= end + 4 + length {
                return serde_json::from_slice(&bytes[end + 4..end + 4 + length]).unwrap();
            }
        }
    }
}

async fn provider(replies: Vec<Value>) -> (String, tokio::task::JoinHandle<Vec<Value>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let task = tokio::spawn(async move {
        let mut requests = Vec::new();
        for reply in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_request(&mut stream).await);
            let body = serde_json::to_vec(&reply).unwrap();
            let header = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(header.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
        requests
    });
    (url, task)
}

fn tool(id: &str, name: &str, args: Value) -> Value {
    json!({"id":id,"type":"function","function":{"name":name,"arguments":serde_json::to_string(&args).unwrap()}})
}

fn question(id: &str) -> Value {
    tool(
        id,
        "request_user_input",
        json!({"questions":[{"title":"Which direction?","options":["A","B"]}]}),
    )
}

fn async_question(id: &str) -> Value {
    tool(
        id,
        "request_user_input_async",
        json!({"questions":[{"title":"Which direction?","options":["A","B"]}]}),
    )
}

fn async_answer(request_id: &str, response_id: &str) -> String {
    json!({"type":"human_interaction_response", "schemaVersion":1,
        "requestId":request_id,"responseId":response_id,
        "answers":[{"questionId":format!("{request_id}-question"),"question":"Which direction?",
            "kind":"text","answer":"My full answer. ".repeat(1000)}]})
    .to_string()
}

fn tool_response(calls: Vec<Value>) -> Value {
    json!({"choices":[{"message":{"role":"assistant","content":"Checking the details.","tool_calls":calls},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":5,"total_tokens":15}})
}

fn completed_response() -> Value {
    json!({"choices":[{"message":{"role":"assistant","content":"Finished."},"finish_reason":"stop"}],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13}})
}

fn input(url: String, workspace: &std::path::Path) -> AgentChatInput {
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl":url,"apiToken":"unused","model":"test-model","stream":false,"modelCapabilities":crate::ModelCapabilities::default(),
        "contextWindowTokens":128000,"maxTokens":1000,"assistantMessageId":"assistant-human","apiStyle":"open_ai_compatible",
        "context":crate::AgentRunContext {
            conversation_id: Some("conversation-human".into()), project_id: None,
            workspace: Some(crate::AgentWorkspaceContext {
                project_id: None, display_name: None, root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None, permissions: crate::AgentPermissions::default(), collaboration_identity: None,
        },
        "messages":[{"role":"user","content":"Use the evidence and ask about the direction."}]
    })).unwrap();
    freeze_runtime_test_generic_provider(&mut input, "human-input");
    input
}

fn answer(pause: AgentUserInputSuspension, index: usize) -> AgentUserInputResume {
    AgentUserInputResume {
        request_id: format!("request-{index}"),
        response_id: format!("response-{index}"),
        checkpoint: pause.checkpoint,
        continuation: crate::AgentToolContinuation {
            result: AgentToolResult {
                exact_archive_file: None,
                call_id: pause.call.id.clone(),
                tool: pause.call.tool.clone(),
                ok: true,
                result: Some(
                    json!({"type":"human_interaction_response", "schemaVersion":1,
                    "requestId":format!("request-{index}"),"responseId":format!("response-{index}"),
                    "answers":[{"questionId":format!("question-{index}"),"question":"Which direction?","kind":"skipped","answer":"已跳过"}]}),
                ),
                error: None,
            },
            call: pause.call,
        },
    }
}

async fn run(input: AgentChatInput, host: AgentRuntimeHostServices) -> AgentChatOutput {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        AgentRuntime::default().send_chat_with_events_and_cancellation(
            input,
            Some("run-human".into()),
            None,
            AgentCancellationToken::new(),
            Some(host),
        ),
    )
    .await
    .unwrap()
    .unwrap()
}

fn assert_paused(output: &AgentChatOutput) {
    assert_eq!(output.status, AgentRunStatus::WaitingForUserInput);
    assert!(output.proposed_actions.is_empty());
    assert!(output.conversation_turn_trace.is_none());
    assert!(!output
        .events
        .iter()
        .any(|event| matches!(event, AgentEvent::ApprovalRequired { .. })));
    assert!(!output.events.iter().any(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.tool == "request_user_input")));
    let public = serde_json::to_string(output).unwrap();
    assert!(!public.contains("pauseReason"));
    assert!(!public.contains("providerContinuationRefs"));
}

#[tokio::test]
async fn human_interaction_async_accepts_multiple_batches_and_continues_without_answers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("evidence.txt"), "independent evidence").unwrap();
    let (url, server) = provider(vec![
        tool_response(vec![
            async_question("q1"),
            tool("read", "read_file", json!({"path":"evidence.txt"})),
            async_question("q2"),
        ]),
        tool_response(vec![async_question("q3")]),
        completed_response(),
    ])
    .await;
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        ..HumanHost::default()
    });
    let event_host = host.clone();
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input(url, dir.path()),
            Some("run-human".into()),
            Some(Arc::new(move |event| {
                if matches!(event, AgentEvent::ToolResult { result, .. } if result.result.as_ref().is_some_and(|value| value["requestId"] == "async-request-1"))
                {
                    event_host
                        .ignored
                        .lock()
                        .unwrap()
                        .push("async-request-1".into());
                }
            })),
            AgentCancellationToken::new(),
            Some(host.services()),
        )
        .await
        .unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    let accepted_ids = host
        .accepted
        .lock()
        .unwrap()
        .iter()
        .map(|v| v.call.id.clone())
        .collect::<Vec<_>>();
    assert_eq!(accepted_ids.len(), 3);
    assert_eq!(
        accepted_ids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    assert!(host.pauses.lock().unwrap().is_empty());
    let results = output
        .events
        .iter()
        .filter_map(|v| match v {
            AgentEvent::ToolResult { result, .. } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results.iter().map(|v| v.tool.as_str()).collect::<Vec<_>>(),
        [
            "request_user_input_async",
            "read_file",
            "request_user_input_async",
            "request_user_input_async"
        ]
    );
    for (index, result) in results
        .iter()
        .filter(|v| v.tool == "request_user_input_async")
        .enumerate()
    {
        assert!(result.ok);
        assert_eq!(result.call_id, accepted_ids[index]);
        assert_eq!(
            result.result,
            Some(
                json!({"type":"human_interaction_accepted","schemaVersion":1,"status":"accepted","requestId":format!("async-request-{}", index + 1)})
            )
        );
    }
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 3);
    let names = requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(names.contains(&"request_user_input"));
    assert!(names.contains(&"request_user_input_async"));
    assert!(!requests[0].to_string().contains("ignoredRequestIds"));
    assert!(requests[1].to_string().contains("independent evidence"));
    for request in requests.iter().skip(1) {
        let messages = request["messages"].as_array().unwrap();
        // The generic provider transports dynamic System context with its existing user-role
        // adapter. It remains request-only runtime state, never persisted human guidance.
        assert_eq!(
            messages
                .iter()
                .filter(|v| v["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("ignoredRequestIds")
                        && text.contains("async-request-1")))
                .count(),
            1
        );
    }
    assert_eq!(host.natural_samples.lock().unwrap().len(), 3);
    assert!(output
        .conversation_turn_trace
        .as_ref()
        .unwrap()
        .items
        .iter()
        .all(|v| !matches!(v, ConversationTurnTraceItem::UserGuidance { .. })));
    host.ignored.lock().unwrap().push("async-request-2".into());
    assert_eq!(host.natural_samples.lock().unwrap().len(), 3);
}

#[test]
fn human_interaction_ignored_snapshot_is_bounded_request_only_runtime_context() {
    let host = HumanHost::default();
    let request = AgentSamplingBoundaryRequest {
        conversation_id: "conversation-human".into(),
        run_id: "run-human".into(),
        assistant_message_id: "assistant-human".into(),
        model_batch_index: 2,
        expected_next_trace_sequence: 3,
    };
    host.ignored.lock().unwrap().push("ignored-request".into());
    let item =
        crate::runtime::preparation::human_interaction_ignored_context(&host, request.clone())
            .unwrap();
    let frame = crate::context::ContextFrame::new(vec![item]);
    let manifest = frame.manifest();
    assert_eq!(manifest.entries[0].role, "system");
    assert_eq!(manifest.entries[0].sources, ["runtime_guard"]);
    assert_eq!(manifest.entries[0].retention, "request_only");
    for ignored in [
        vec![],
        vec![" ".into()],
        vec!["duplicate".into(), "duplicate".into()],
        (0..2000)
            .map(|i| format!("{i}-{}", "a".repeat(200)))
            .collect(),
    ] {
        *host.ignored.lock().unwrap() = ignored;
        assert!(
            crate::runtime::preparation::human_interaction_ignored_context(&host, request.clone())
                .is_none()
        );
    }
}

#[tokio::test]
async fn human_interaction_async_answers_enter_guidance_once_after_complete_tool_batch() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("evidence.txt"), "independent evidence").unwrap();
    let (url, server) = provider(vec![
        tool_response(vec![
            async_question("q1"),
            tool("read", "read_file", json!({"path":"evidence.txt"})),
            async_question("q2"),
        ]),
        completed_response(),
    ])
    .await;
    let queue = AgentSteerInputQueue::new();
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        answer_queue: Some(queue.clone()),
        ..HumanHost::default()
    });
    let initial_answer = async_answer("prior-request", "prior-response");
    queue
        .enqueue(runtime_steer_input(
            "prior-response",
            "prior-response",
            &initial_answer,
        ))
        .unwrap();
    let output = run(
        input(url, dir.path()),
        host.services().with_steer_input(queue),
    )
    .await;
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert!(host.pauses.lock().unwrap().is_empty());
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["role"] == "user" && v["content"] == initial_answer));
    let messages = requests[1]["messages"].as_array().unwrap();
    let final_tool_index = messages.iter().rposition(|v| v["role"] == "tool").unwrap();
    for index in 1..=2 {
        let content = async_answer(
            &format!("async-request-{index}"),
            &format!("async-response-{index}"),
        );
        let matches = messages
            .iter()
            .enumerate()
            .filter(|(_, v)| v["role"] == "user" && v["content"] == content)
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].0 > final_tool_index);
        let call_id = host.accepted.lock().unwrap()[index - 1].call.id.clone();
        assert_eq!(output.events.iter().filter(|v| matches!(v, AgentEvent::ToolResult { result, .. } if result.call_id == call_id)).count(), 1);
    }
    let trace = output.conversation_turn_trace.as_ref().unwrap();
    let guidance = trace
        .items
        .iter()
        .filter_map(|v| match v {
            ConversationTurnTraceItem::UserGuidance { content, .. } => Some(content),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(guidance.len(), 3);
    assert_eq!(guidance[0], &initial_answer);
    assert_eq!(
        guidance[1],
        &async_answer("async-request-1", "async-response-1")
    );
    assert_eq!(
        guidance[2],
        &async_answer("async-request-2", "async-response-2")
    );
}

#[tokio::test]
async fn human_interaction_async_stop_after_admission_preserves_one_accepted_fact() {
    let dir = tempfile::tempdir().unwrap();
    let (url, server) = provider(vec![tool_response(vec![async_question("q1")])]).await;
    let cancellation = AgentCancellationToken::new();
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        cancel_after_accept: Some(cancellation.clone()),
        ..HumanHost::default()
    });
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input(url, dir.path()),
            Some("run-human".into()),
            None,
            cancellation,
            Some(host.services()),
        )
        .await
        .unwrap();
    assert_eq!(output.status, AgentRunStatus::Cancelled);
    assert_eq!(host.accepted.lock().unwrap().len(), 1);
    assert!(host.pauses.lock().unwrap().is_empty());
    let results = output
        .events
        .iter()
        .filter_map(|v| match v {
            AgentEvent::ToolResult { result, .. } if result.tool == "request_user_input_async" => {
                Some(result)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results.len(), 1);
    assert!(results[0].ok);
    assert_eq!(results[0].result.as_ref().unwrap()["status"], "accepted");
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn human_interaction_async_answer_waits_through_sync_pause_and_restored_tool_queue() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("after.txt"), "work after sync answer").unwrap();
    let (url, server) = provider(vec![
        tool_response(vec![
            async_question("async"),
            question("sync"),
            tool("after", "read_file", json!({"path":"after.txt"})),
        ]),
        completed_response(),
    ])
    .await;
    let queue = AgentSteerInputQueue::new();
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        answer_queue: Some(queue.clone()),
        ..HumanHost::default()
    });
    let base = input(url, dir.path());
    let paused = run(
        base.clone(),
        host.services().with_steer_input(queue.clone()),
    )
    .await;
    assert_eq!(paused.status, AgentRunStatus::WaitingForUserInput);
    let suspension = host.take_pause();
    assert_eq!(suspension.checkpoint.queued_tool_calls.len(), 1);
    assert!(!paused
        .events
        .iter()
        .any(|v| matches!(v, AgentEvent::GuidanceApplied { .. })));
    let output = run(
        base,
        host.services()
            .with_steer_input(queue)
            .with_user_input_resume(answer(suspension, 1)),
    )
    .await;
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(host.accepted.lock().unwrap().len(), 1);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    let messages = requests[1]["messages"].as_array().unwrap();
    assert!(messages
        .iter()
        .any(|v| v.to_string().contains("work after sync answer")));
    let content = async_answer("async-request-1", "async-response-1");
    let guidance_index = messages
        .iter()
        .position(|v| v["role"] == "user" && v["content"] == content)
        .unwrap();
    assert!(guidance_index > messages.iter().rposition(|v| v["role"] == "tool").unwrap());
    let trace = output.conversation_turn_trace.as_ref().unwrap();
    assert_eq!(trace.items.iter().filter(|v| matches!(v, ConversationTurnTraceItem::ToolResult { tool, .. } if tool == "request_user_input_async")).count(), 1);
    assert_eq!(
        trace
            .items
            .iter()
            .filter(|v| matches!(v, ConversationTurnTraceItem::UserGuidance { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn human_interaction_repeated_questions_pause_sequentially_without_replaying_completed_tools()
{
    let dir = tempfile::tempdir().unwrap();
    for (name, data) in [
        ("before.txt", "before-original"),
        ("middle.txt", "middle-evidence"),
        ("after.txt", "after-evidence"),
    ] {
        std::fs::write(dir.path().join(name), data).unwrap();
    }
    let (url, server) = provider(vec![
        tool_response(vec![
            tool(
                "todo",
                "todo_update",
                json!({"items":[{"title":"Gather evidence","status":"in_progress"}]}),
            ),
            tool("before", "read_file", json!({"path":"before.txt"})),
            question("q1"),
            tool("middle", "read_file", json!({"path":"middle.txt"})),
            question("q2"),
            tool("after", "read_file", json!({"path":"after.txt"})),
        ]),
        completed_response(),
    ])
    .await;
    let base = input(url, dir.path());
    let host = HumanHost::enabled();
    let first = run(base.clone(), host.services()).await;
    assert_paused(&first);
    let pause1 = host.take_pause();
    assert_eq!(pause1.checkpoint.queued_tool_calls.len(), 3);
    assert!(pause1.segment_usage.is_some());
    let call1 = pause1.call.id.clone();
    let resume1 = answer(pause1, 1);
    std::fs::write(dir.path().join("before.txt"), "must-not-read-again").unwrap();
    let second = run(
        base.clone(),
        host.services().with_user_input_resume(resume1),
    )
    .await;
    assert_paused(&second);
    let pause2 = host.take_pause();
    assert_eq!(pause2.checkpoint.queued_tool_calls.len(), 1);
    assert!(
        pause2.segment_usage.is_none(),
        "no model sample occurred between the two pauses"
    );
    let call2 = pause2.call.id.clone();
    assert_ne!(call1, call2);
    assert_eq!(
        pause2
            .checkpoint
            .conversation_trace_items
            .iter()
            .filter(|item| matches!(item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call1))
            .count(),
        1
    );
    host.enabled.store(false, Ordering::SeqCst);
    let final_output = run(
        base,
        host.services().with_user_input_resume(answer(pause2, 2)),
    )
    .await;
    assert_eq!(final_output.status, AgentRunStatus::Completed);
    assert!(final_output.todo.is_some());
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[0]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["function"]["name"] == "request_user_input"));
    for request in &requests {
        assert!(!request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["function"]["name"] == "request_user_input_async"));
    }
    assert!(!requests[1]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["function"]["name"] == "request_user_input"));
    let messages = requests[1]["messages"].as_array().unwrap();
    for id in [&call1, &call2] {
        let results: Vec<_> = messages
            .iter()
            .filter(|v| v["role"] == "tool" && v["tool_call_id"] == *id)
            .collect();
        assert_eq!(results.len(), 1);
        assert!(results[0]["content"].as_str().unwrap().contains("skipped"));
    }
    assert_eq!(
        messages
            .iter()
            .filter(|v| v["role"] == "user"
                && v["content"].as_str().is_some_and(
                    |content| content.contains("Use the evidence and ask about the direction.")
                ))
            .count(),
        1
    );
    assert!(!messages.iter().any(|v| v["role"] == "user"
        && v["content"]
            .as_str()
            .is_some_and(|content| content.contains("human_interaction_response")
                || content.contains("\"kind\":\"skipped\""))));
    let serialized = requests[1].to_string();
    assert!(serialized.contains("before-original"));
    assert!(serialized.contains("middle-evidence"));
    assert!(serialized.contains("after-evidence"));
    assert!(!serialized.contains("must-not-read-again"));
}

#[tokio::test]
async fn human_interaction_questions_and_real_approval_alternate_on_one_frozen_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (url, server) = provider(vec![tool_response(vec![
        tool("observe", "read_file", json!({"path":"report.txt"})),
        question("before-approval"),
        tool("write", "apply_patch", json!({"request":{"action":"apply","operation":"create","filePath":"report.txt","content":"draft"}})),
        question("after-approval"),
    ]), completed_response()]).await;
    let mut base = input(url, dir.path());
    base.context.as_mut().unwrap().permissions.write = crate::AgentWritePermission::WorkspaceOnly;
    let host = HumanHost::enabled();
    assert_paused(&run(base.clone(), host.services()).await);
    let question1 = host.take_pause();
    let approval = run(
        base.clone(),
        host.services().with_user_input_resume(answer(question1, 1)),
    )
    .await;
    assert_eq!(approval.status, AgentRunStatus::WaitingForApproval);
    assert!(host.pauses.lock().unwrap().is_empty());
    let checkpoint = approval
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        checkpoint.pause_reason,
        crate::AgentRunCheckpointPauseReason::Approval
    );
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    let frozen = checkpoint
        .context_items
        .iter()
        .flat_map(|v| &v.tool_calls)
        .find(|v| v.id == checkpoint.pending_tool_call_id)
        .unwrap()
        .clone();
    let mut approved_input = base.clone();
    approved_input.approval_decision = Some(crate::AgentApprovalDecision {
        action_id: checkpoint.pending_action_id.clone().unwrap(),
        status: crate::AgentApprovalDecisionStatus::Rejected,
        message: Some("Keep the report unchanged.".into()),
    });
    approved_input.tool_continuation = Some(crate::AgentToolContinuation {
        call: AgentToolCall {
            id: frozen.id.clone(),
            tool: frozen.name.clone(),
            args: frozen.args,
            approval_status: AgentApprovalStatus::Rejected,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: frozen.id,
            tool: frozen.name,
            ok: false,
            result: None,
            error: Some("User rejected the file change.".into()),
        },
    });
    approved_input.resume_checkpoint = Some(checkpoint);
    assert_paused(&run(approved_input, host.services()).await);
    let question2 = host.take_pause();
    assert!(question2.checkpoint.queued_tool_calls.is_empty());
    let final_output = run(
        base,
        host.services().with_user_input_resume(answer(question2, 2)),
    )
    .await;
    assert_eq!(final_output.status, AgentRunStatus::Completed);
    assert!(!dir.path().join("report.txt").exists());
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]
        .to_string()
        .contains("User rejected the file change."));
    let trace = final_output.conversation_turn_trace.unwrap();
    assert_eq!(trace.items.iter().filter(|v| matches!(v, ConversationTurnTraceItem::ToolResult { tool, .. } if tool == "request_user_input")).count(), 2);
}

#[tokio::test]
async fn human_interaction_setting_close_at_dispatch_rejects_new_question_without_waiting() {
    for async_ready in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let tool_name = if async_ready {
            "request_user_input_async"
        } else {
            "request_user_input"
        };
        let (url, server) = provider(vec![
            tool_response(vec![if async_ready {
                async_question("question")
            } else {
                question("question")
            }]),
            completed_response(),
        ])
        .await;
        let host = Arc::new(HumanHost {
            enabled: AtomicBool::new(true),
            async_ready,
            ..HumanHost::default()
        });
        let event_host = host.clone();
        let output = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input(url, dir.path()),
                Some("run-human".into()),
                Some(Arc::new(move |event| {
                    if matches!(event, AgentEvent::ToolCall { call, .. } if call.tool == tool_name)
                    {
                        event_host.enabled.store(false, Ordering::SeqCst);
                    }
                })),
                AgentCancellationToken::new(),
                Some(host.services()),
            )
            .await
            .unwrap();
        assert_eq!(output.status, AgentRunStatus::Completed);
        assert!(host.pauses.lock().unwrap().is_empty());
        assert!(host.accepted.lock().unwrap().is_empty());
        assert!(output.events.iter().any(|v| matches!(v, AgentEvent::ToolResult { result, .. } if result.tool == tool_name && !result.ok)));
        let requests = server.await.unwrap();
        assert!(!requests[1]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| matches!(
                v["function"]["name"].as_str(),
                Some("request_user_input" | "request_user_input_async")
            )));
        assert!(!requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["content"]
                .as_str()
                .is_some_and(|text| text.contains("## 人机交互"))));
    }
}

#[tokio::test]
async fn human_interaction_untrusted_approval_envelope_cannot_publish_an_answer() {
    let dir = tempfile::tempdir().unwrap();
    let (url, server) = provider(vec![tool_response(vec![question("question")])]).await;
    let mut base = input(url, dir.path());
    let host = HumanHost::enabled();
    assert_paused(&run(base.clone(), host.services()).await);
    server.await.unwrap();
    let resume = answer(host.take_pause(), 1);
    for field in ["requestId", "responseId"] {
        let mut changed = resume.clone();
        changed.continuation.result.result.as_mut().unwrap()[field] = json!("foreign-binding");
        assert!(
            install_trusted_user_input_resume(&mut base.clone(), "run-human", changed).is_err()
        );
    }
    let mut changed = resume.clone();
    changed.continuation.result.result.as_mut().unwrap()["answers"][0]["question"] =
        json!("Altered question");
    assert!(install_trusted_user_input_resume(&mut base.clone(), "run-human", changed).is_err());
    base.resume_checkpoint = Some(resume.checkpoint);
    base.tool_continuation = Some(resume.continuation);
    base.approval_decision = Some(crate::AgentApprovalDecision {
        action_id: base
            .resume_checkpoint
            .as_ref()
            .unwrap()
            .pending_tool_call_id
            .clone(),
        status: crate::AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    let observed = Arc::new(AtomicBool::new(false));
    let observer = observed.clone();
    let error = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base,
            Some("run-human".into()),
            None,
            AgentCancellationToken::new(),
            Some(host.services().with_trace_observer(Arc::new(move |_| {
                observer.store(true, Ordering::SeqCst);
                Ok(None)
            }))),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("trusted Host resume port"));
    assert!(!observed.load(Ordering::SeqCst));
}
