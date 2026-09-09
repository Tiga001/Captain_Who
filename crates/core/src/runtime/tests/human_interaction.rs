use super::*;
use crate::human_interaction::HumanInteractionSettings;
use crate::{
    AgentAsyncUserInputAccepted, AgentAsyncUserInputRequest, AgentHumanInteractionIgnoredEvent,
    AgentHumanInteractionRuntimeHost, AgentSamplingBoundaryRequest, AgentUserInputResume,
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
    ignored_receipts:
        Mutex<std::collections::BTreeMap<u64, Vec<AgentHumanInteractionIgnoredEvent>>>,
    ignored_commit_unknown_once: AtomicBool,
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

    fn bind_ignored_events(
        &self,
        request: AgentSamplingBoundaryRequest,
    ) -> AgentResult<Vec<AgentHumanInteractionIgnoredEvent>> {
        let first_sequence = request.expected_next_trace_sequence;
        let batch = request.model_batch_index;
        self.natural_samples.lock().unwrap().push(request);
        let mut receipts = self.ignored_receipts.lock().unwrap();
        if let Some(events) = receipts.get(&batch) {
            return Ok(events.clone());
        }
        let events: Vec<_> = self
            .ignored
            .lock()
            .unwrap()
            .drain(..)
            .enumerate()
            .map(|(index, request_id)| AgentHumanInteractionIgnoredEvent {
                trace_sequence: first_sequence + index as u64,
                event_id: format!("ignored:{request_id}"),
                request_id,
                created_at: 1,
            })
            .collect();
        if !events.is_empty() {
            receipts.insert(batch, events.clone());
            if self
                .ignored_commit_unknown_once
                .swap(false, Ordering::SeqCst)
            {
                return Err(AgentError::new(
                    "simulated committed binding with unknown return",
                ));
            }
        }
        Ok(events)
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

fn with_disabled_collaboration(host: AgentRuntimeHostServices) -> AgentRuntimeHostServices {
    struct UnusedCollaborationExecutor;
    impl crate::AgentCollaborationExecutor for UnusedCollaborationExecutor {
        fn execute(
            &self,
            _: crate::AgentCollaborationInvocation,
            _: crate::AgentCollaborationExecutionControl,
        ) -> crate::AgentCollaborationExecutionFuture {
            panic!("disabled collaboration must never reach its Host executor")
        }
    }

    // Deliberately provide a service despite the disabled policy. The runtime must freeze
    // availability before preparing either an approval or a human-input checkpoint.
    host.with_agent_collaboration(crate::AgentCollaborationRuntimeServices::new(
        Arc::new(UnusedCollaborationExecutor),
        crate::AgentCollaborationCaller {
            agent_id: "root-disabled-collaboration".into(),
            root_agent_id: "root-disabled-collaboration".into(),
            root_conversation_id: "conversation-human".into(),
            parent_agent_id: None,
            conversation_id: "conversation-human".into(),
            project_id: None,
            task_name: crate::ROOT_AGENT_TASK_NAME.into(),
            task_path: "/root".into(),
        },
        crate::AgentCollaborationSelectorDirectory::bounded(
            Vec::new(),
            vec![crate::AgentCollaborationModelSelector {
                model_config_id: "disabled-collaboration-model".into(),
                display_name: "DISABLED_COLLABORATION_DIRECTORY_CANARY".into(),
                capabilities: crate::ModelCapabilities::default(),
            }],
        ),
    ))
    .with_agent_collaboration_policy(Arc::new(
        crate::FrozenAgentCollaborationPolicySource::new(crate::AgentCollaborationSettings {
            enabled: false,
            revision: 2,
            updated_at: 1,
        }),
    ))
}

fn assert_disabled_collaboration_checkpoint(checkpoint: &crate::AgentRunCheckpoint) {
    assert!(checkpoint.collaboration_run_snapshot.is_none());
    assert!(!checkpoint
        .tool_set
        .exposed_tool_names
        .iter()
        .any(|name| crate::AGENT_COLLABORATION_TOOL_NAMES.contains(&name.as_str())));
}

fn assert_disabled_collaboration_requests(
    requests: &[Value],
    profile: crate::AgentContextProfile,
    required_tool: &str,
) {
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["tools"], requests[1]["tools"]);
    for request in requests {
        let names: Vec<_> = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|definition| definition["function"]["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&required_tool));
        assert_eq!(
            names.contains(&"todo_update"),
            profile == crate::AgentContextProfile::Full
        );
        assert!(!names
            .iter()
            .any(|name| crate::AGENT_COLLABORATION_TOOL_NAMES.contains(name)));
        let serialized = request.to_string();
        assert!(!serialized.contains("DISABLED_COLLABORATION_DIRECTORY_CANARY"));
        assert!(serialized.contains("disabled_by_user"));
    }
}

#[tokio::test]
async fn disabled_collaboration_service_allows_command_approval_checkpoint_and_resume() {
    for profile in [
        crate::AgentContextProfile::Full,
        crate::AgentContextProfile::Minimal,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = provider(vec![
            tool_response(vec![tool(
                "command-approval",
                "run_command",
                json!({"command":"printf 'command checkpoint complete\\n'","reason":"Exercise manual command approval."}),
            )]),
            completed_response(),
        ])
        .await;
        let mut base = input(url, dir.path());
        base.prompt_preferences =
            Some(serde_json::from_value(json!({"contextProfile":profile})).unwrap());
        let approval = run(
            base.clone(),
            with_disabled_collaboration(AgentRuntimeHostServices::new()),
        )
        .await;
        assert_eq!(approval.status, AgentRunStatus::WaitingForApproval);
        assert_eq!(approval.proposed_actions.len(), 1);
        assert!(matches!(
            &approval.proposed_actions[0],
            AgentProposedAction::Command { .. }
        ));
        let checkpoint = approval
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
                _ => None,
            })
            .expect("manual command approval must produce a restorable checkpoint");
        assert_disabled_collaboration_checkpoint(&checkpoint);
        let frozen = checkpoint
            .context_items
            .iter()
            .flat_map(|item| &item.tool_calls)
            .find(|call| call.id == checkpoint.pending_tool_call_id)
            .unwrap()
            .clone();
        assert_eq!(frozen.name, "run_command");
        let call_id = frozen.id.clone();
        let mut resumed_input = base;
        resumed_input.messages.clear();
        resumed_input.approval_decision = Some(crate::AgentApprovalDecision {
            action_id: checkpoint
                .pending_action_id
                .clone()
                .unwrap_or_else(|| call_id.clone()),
            status: crate::AgentApprovalDecisionStatus::Approved,
            message: None,
        });
        // The Host supplies the trusted execution receipt at the manual-approval boundary.
        // No real command or remote provider is required to exercise restoration and dispatch.
        resumed_input.tool_continuation = Some(crate::AgentToolContinuation {
            call: AgentToolCall {
                id: frozen.id.clone(),
                tool: frozen.name.clone(),
                args: frozen.args,
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                exact_archive_file: None,
                call_id: frozen.id,
                tool: frozen.name,
                ok: true,
                result: Some(
                    json!({"exitCode":0,"stdout":"command checkpoint complete","stderr":""}),
                ),
                error: None,
            },
        });
        resumed_input.resume_checkpoint = Some(checkpoint);
        let completed = run(
            resumed_input,
            with_disabled_collaboration(AgentRuntimeHostServices::new()),
        )
        .await;
        assert_eq!(completed.status, AgentRunStatus::Completed);
        let requests = server.await.unwrap();
        assert_disabled_collaboration_requests(&requests, profile, "run_command");
        let results: Vec<_> = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
            .collect();
        assert_eq!(results.len(), 1);
        assert!(results[0]["content"]
            .as_str()
            .unwrap()
            .contains("command checkpoint complete"));
    }
}

#[tokio::test]
async fn disabled_collaboration_service_allows_human_input_checkpoint_and_resume() {
    for profile in [
        crate::AgentContextProfile::Full,
        crate::AgentContextProfile::Minimal,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (url, server) = provider(vec![
            tool_response(vec![question("question-disabled-collaboration")]),
            completed_response(),
        ])
        .await;
        let mut base = input(url, dir.path());
        base.prompt_preferences =
            Some(serde_json::from_value(json!({"contextProfile":profile})).unwrap());
        let host = HumanHost::enabled();
        assert_paused(&run(base.clone(), with_disabled_collaboration(host.services())).await);
        let pause = host.take_pause();
        assert_disabled_collaboration_checkpoint(&pause.checkpoint);
        let call_id = pause.call.id.clone();
        let completed = run(
            base,
            with_disabled_collaboration(host.services()).with_user_input_resume(answer(pause, 1)),
        )
        .await;
        assert_eq!(completed.status, AgentRunStatus::Completed);
        assert!(host.pauses.lock().unwrap().is_empty());
        let requests = server.await.unwrap();
        assert_disabled_collaboration_requests(&requests, profile, "request_user_input");
        let results: Vec<_> = requests[1]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
            .collect();
        assert_eq!(results.len(), 1);
        assert!(results[0]["content"]
            .as_str()
            .unwrap()
            .contains("human_interaction_response"));
    }
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
        // Transport user role carries backend-observed history, not a human input or guidance.
        assert_eq!(
            messages
                .iter()
                .filter(|v| v["content"]
                    .as_str()
                    .is_some_and(|text| text.contains("<backend_observed_state>")
                        && text.contains("human_interaction_status")
                        && text.contains("async-request-1")))
                .count(),
            1
        );
    }
    assert_eq!(host.natural_samples.lock().unwrap().len(), 3);
    assert_eq!(
        output
            .conversation_turn_trace
            .as_ref()
            .unwrap()
            .items
            .iter()
            .filter(|item| matches!(item, ConversationTurnTraceItem::BackendState { .. }))
            .count(),
        1
    );
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
fn human_interaction_ignored_event_is_retained_once_and_has_no_guidance() {
    let recorder = Arc::new(Mutex::new(ConversationTraceRecorder::default()));
    let mut context = ContextFrame::new(Vec::new());
    let events = vec![AgentHumanInteractionIgnoredEvent {
        trace_sequence: 0,
        event_id: "ignored:response-1".into(),
        request_id: "request-1".into(),
        created_at: 1,
    }];
    for _ in 0..2 {
        apply_human_interaction_ignored_events(
            &events,
            &mut context,
            &recorder,
            None,
            "assistant-human",
        )
        .unwrap();
    }
    let manifest = context.manifest();
    assert_eq!(manifest.entries.len(), 1);
    assert_eq!(manifest.entries[0].sources, ["backend_state"]);
    assert_eq!(manifest.entries[0].retention, "retained");
    let messages = context.to_messages();
    assert_eq!(
        messages[0].placement(),
        crate::llm::LlmMessagePlacement::BackendStateTimeline
    );
    assert_eq!(
        serde_json::from_str::<Value>(messages[0].content()).unwrap(),
        json!({
            "type":"human_interaction_status", "requestId":"request-1", "status":"ignored"
        })
    );
    let snapshot = recorder.lock().unwrap().snapshot();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.model_context_items.len(), 1);
    assert!(matches!(
        snapshot.items[0],
        ConversationTurnTraceItem::BackendState { .. }
    ));
    let mut invalid = events;
    invalid[0].request_id = " ".into();
    assert!(apply_human_interaction_ignored_events(
        &invalid,
        &mut context,
        &recorder,
        None,
        "assistant-human",
    )
    .is_err());
    assert_eq!(context.manifest().entries.len(), 1);
}

#[tokio::test]
async fn human_interaction_disabled_tools_observe_ignored_batch_after_commit_unknown_binding() {
    let dir = tempfile::tempdir().unwrap();
    let (url, server) = provider(vec![
        tool_response(vec![async_question("q1")]),
        completed_response(),
    ])
    .await;
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        ignored_commit_unknown_once: AtomicBool::new(true),
        ..HumanHost::default()
    });
    let event_host = host.clone();
    let output = AgentRuntime::default().send_chat_with_events_and_cancellation(
        input(url, dir.path()), Some("run-human".into()),
        Some(Arc::new(move |event| {
            if matches!(event, AgentEvent::ToolResult { result, .. } if result.tool == "request_user_input_async") {
                event_host.ignored.lock().unwrap().push("async-request-1".into());
                event_host.enabled.store(false, Ordering::SeqCst);
            }
        })), AgentCancellationToken::new(), Some(host.services()),
    ).await.unwrap();
    assert_eq!(output.status, AgentRunStatus::Completed);
    let requests = server.await.unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["tools"].as_array().unwrap().iter().all(|tool| {
        !matches!(
            tool["function"]["name"].as_str(),
            Some("request_user_input" | "request_user_input_async")
        )
    }));
    let observed = requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["content"].as_str())
        .filter(|content| content.contains("human_interaction_status"))
        .collect::<Vec<_>>();
    assert_eq!(observed.len(), 1);
    assert!(observed[0].contains("<backend_observed_state>"));
    assert!(observed[0].contains("async-request-1"));
    assert!(!observed[0].contains("ignoredRequestIds"));
    let samples = host.natural_samples.lock().unwrap();
    assert_eq!(samples.len(), 3);
    assert_eq!(
        samples[1], samples[2],
        "binding retry must keep the same exact boundary"
    );
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

#[tokio::test]
async fn human_interaction_provider_requests_use_one_policy_snapshot_for_both_tools_and_prompt() {
    struct ChangingPolicy(std::sync::atomic::AtomicUsize);
    impl HumanInteractionPolicySource for ChangingPolicy {
        fn snapshot(&self) -> AgentResult<HumanInteractionSettings> {
            let revision = self.0.fetch_add(1, Ordering::SeqCst);
            Ok(HumanInteractionSettings {
                enabled: revision.is_multiple_of(2),
                revision: revision as u64,
                updated_at: 1,
            })
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let (url, provider) = provider(vec![
        // Malformed/malicious model output may name tools omitted from the current request.
        // Those calls receive ordinary failure results and cannot reach native Host admission.
        tool_response(vec![
            question("unavailable-sync"),
            async_question("unavailable-async"),
        ]),
        tool_response(vec![async_question("available-async")]),
        completed_response(),
    ])
    .await;
    let host = Arc::new(HumanHost {
        enabled: AtomicBool::new(true),
        async_ready: true,
        ..HumanHost::default()
    });
    let source = Arc::new(ChangingPolicy(std::sync::atomic::AtomicUsize::new(0)));
    let output = run(
        input(url, directory.path()),
        host.services()
            .with_human_interaction_policy(source.clone()),
    )
    .await;
    assert_eq!(output.status, AgentRunStatus::Completed);
    assert!(host.pauses.lock().unwrap().is_empty());
    assert_eq!(host.accepted.lock().unwrap().len(), 1);
    let requests = provider.await.unwrap();
    assert_eq!(requests.len(), 3);
    assert_eq!(source.0.load(Ordering::SeqCst), 4); // Initial projection, then one read per sample.
    for (request, available) in requests.iter().zip([false, true, false]) {
        let tools = request["tools"].as_array().unwrap();
        for tool in ["request_user_input", "request_user_input_async"] {
            let definition = tools.iter().find(|entry| entry["function"]["name"] == tool);
            assert_eq!(definition.is_some(), available);
            if let Some(definition) = definition {
                let description = definition["function"]["description"].as_str().unwrap();
                for purpose in ["assistance", "feedback", "actions that require the human"] {
                    assert!(description.contains(purpose), "{tool}: {purpose}");
                }
                assert!(!description.contains("Use only for information or preferences"));
            }
        }
        let message_texts = request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|message| message["content"].as_str())
            .collect::<Vec<_>>();
        let instructions = message_texts
            .iter()
            .find(|text| text.contains("## 人机交互"));
        assert_eq!(instructions.is_some(), available);
        for principle in [
            "需要用户亲自参与的操作",
            "不要求套用固定模板",
            "只作回应能够支持的判断",
        ] {
            if let Some(instructions) = instructions {
                assert!(instructions.contains(principle), "{principle}");
            }
        }
        assert_eq!(
            message_texts
                .iter()
                .any(|text| text.contains("不要求套用固定模板")),
            available,
            "interaction guidance must disappear with the corresponding tools"
        );
    }
    let results = output
        .events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolResult { result, .. } => Some(result),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        results.iter().map(|result| result.ok).collect::<Vec<_>>(),
        [false, false, true]
    );
    let stable = output
        .events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ToolSetChanged {
                stable_revision, ..
            } => Some(stable_revision),
            _ => None,
        })
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        stable.len(),
        1,
        "dynamic settings must preserve the stable tool prefix"
    );
}

#[tokio::test]
async fn human_interaction_provider_cannot_invoke_questions_from_any_descendant_or_automation() {
    struct NoReports;
    impl crate::AutomationReportSink for NoReports {
        fn record(&self, _: crate::AutomationReportKind, _: &str) -> Result<(), String> {
            panic!("identity test must not execute a report")
        }
    }
    // Descendants remain children even when their task path denotes a parent of other agents.
    // Either trusted Automation marker independently suppresses the capability.
    for identity in [
        "child",
        "parent-with-descendants",
        "deep-child",
        "automation-context",
        "automation-sink",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let (url, provider) = provider(vec![
            tool_response(vec![
                question("forbidden-sync"),
                async_question("forbidden-async"),
            ]),
            completed_response(),
        ])
        .await;
        let mut input = input(url, directory.path());
        let host = Arc::new(HumanHost {
            enabled: AtomicBool::new(true),
            async_ready: true,
            ..HumanHost::default()
        });
        let mut services = host.services();
        if identity == "automation-context" {
            let mut preferences: AgentPromptPreferences =
                serde_json::from_value(json!({})).unwrap();
            preferences.automation_execution_context = Some(
                AgentAutomationExecutionContext::new(
                    "automation",
                    "automation-run",
                    1,
                    None,
                    "manual",
                )
                .unwrap(),
            );
            input.prompt_preferences = Some(preferences);
        } else if identity == "automation-sink" {
            services = services.with_automation_report_sink(Arc::new(NoReports));
        } else {
            let parent = if identity == "child" {
                "/root"
            } else {
                "/root/parent/ancestor"
            };
            input.context.as_mut().unwrap().collaboration_identity =
                Some(crate::AgentCollaborationIdentity {
                    agent_id: identity.into(),
                    root_agent_id: "root".into(),
                    root_conversation_id: "root-conversation".into(),
                    parent_agent_id: "parent".into(),
                    parent_task_name: "parent".into(),
                    parent_task_path: parent.into(),
                    conversation_id: "conversation-human".into(),
                    task_name: identity.into(),
                    task_path: format!("{parent}/{identity}"),
                    source_agent_id: "parent".into(),
                    source_kind: crate::AgentMailboxKind::Task,
                    source_task_name: "parent".into(),
                    source_task_path: parent.into(),
                    source_agent_message_id: "task-message".into(),
                    entrusted_task: "Complete a bounded task.".into(),
                    template_instructions: None,
                });
        }
        let output = run(input, services).await;
        assert_eq!(output.status, AgentRunStatus::Completed, "{identity}");
        assert!(host.accepted.lock().unwrap().is_empty(), "{identity}");
        assert!(host.pauses.lock().unwrap().is_empty(), "{identity}");
        assert!(
            host.natural_samples.lock().unwrap().is_empty(),
            "{identity}"
        );
        for request in provider.await.unwrap() {
            assert!(
                !request["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|tool| matches!(
                        tool["function"]["name"].as_str(),
                        Some("request_user_input" | "request_user_input_async")
                    )),
                "{identity}"
            );
            assert!(
                !request["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["content"]
                        .as_str()
                        .is_some_and(|text| text.contains("## 人机交互"))),
                "{identity}"
            );
        }
        let results = output
            .events
            .iter()
            .filter_map(|event| match event {
                AgentEvent::ToolResult { result, .. }
                    if matches!(
                        result.tool.as_str(),
                        "request_user_input" | "request_user_input_async"
                    ) =>
                {
                    Some(result)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(results.len(), 2, "{identity}");
        assert!(results.iter().all(|result| !result.ok), "{identity}");
    }
}
