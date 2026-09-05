use super::*;
use crate::human_interaction::HumanInteractionSettings;
use crate::{AgentHumanInteractionRuntimeHost, AgentUserInputResume, AgentUserInputSuspension};
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct HumanHost {
    enabled: AtomicBool,
    pauses: Mutex<Vec<AgentUserInputSuspension>>,
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
    let dir = tempfile::tempdir().unwrap();
    let (url, server) = provider(vec![
        tool_response(vec![question("question")]),
        completed_response(),
    ])
    .await;
    let host = HumanHost::enabled();
    let event_host = host.clone();
    let output = AgentRuntime::default().send_chat_with_events_and_cancellation(
        input(url, dir.path()), Some("run-human".into()), Some(Arc::new(move |event| {
            if matches!(event, AgentEvent::ToolCall { call, .. } if call.tool == "request_user_input") {
                event_host.enabled.store(false, Ordering::SeqCst);
            }
        })), AgentCancellationToken::new(), Some(host.services()),
    ).await.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert!(host.pauses.lock().unwrap().is_empty());
    assert!(output.events.iter().any(|v| matches!(v, AgentEvent::ToolResult { result, .. } if result.tool == "request_user_input" && !result.ok)));
    let requests = server.await.unwrap();
    assert!(!requests[1]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["function"]["name"] == "request_user_input"));
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
