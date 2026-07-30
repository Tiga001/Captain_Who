use super::*;
use mycopilot_core::{
    mcp_normalized_input_schema_identity, mcp_tool_arguments_digest, AgentMcpServerScope,
    AgentMcpToolApproval, AgentMcpToolInvocationIdentity, AgentMcpToolProvenance,
    McpAgentToolAnnotations, McpAgentToolDescriptor, McpApprovedToolInvocation,
    McpToolApprovalRequest, McpToolCatalogContext, McpToolContentBlock, McpToolInvocationFuture,
    McpToolInvocationResult, McpToolInvoker, MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const ARGUMENT_CANARY: &str = "MCP_NEUTRAL_ARGUMENT_CANARY_NOT_DURABLE";
const RESULT_CANARY: &str = "MCP_RESULT_CANARY_LIVE_MODEL_ONLY";

#[derive(Clone, Copy, Default)]
enum ApprovalInvocationBehavior {
    #[default]
    Success,
    BareCancellation,
    TruncatedSuccess,
}

#[derive(Default)]
struct ApprovalLifecycleInvoker {
    prepared: Mutex<HashMap<String, (AgentMcpToolApproval, Value)>>,
    invocation_count: AtomicU64,
    descriptor: Mutex<Option<McpAgentToolDescriptor>>,
    behavior: ApprovalInvocationBehavior,
}

impl ApprovalLifecycleInvoker {
    fn with_descriptor(descriptor: McpAgentToolDescriptor) -> Arc<Self> {
        Self::with_behavior(descriptor, ApprovalInvocationBehavior::Success)
    }

    fn with_behavior(
        descriptor: McpAgentToolDescriptor,
        behavior: ApprovalInvocationBehavior,
    ) -> Arc<Self> {
        Arc::new(Self {
            prepared: Mutex::new(HashMap::new()),
            invocation_count: AtomicU64::new(0),
            descriptor: Mutex::new(Some(descriptor)),
            behavior,
        })
    }
}

impl McpToolInvoker for ApprovalLifecycleInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        Ok(self
            .descriptor
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .into_iter()
            .collect())
    }

    fn prepare_approval(
        &self,
        request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        let (approval, arguments, _) = request.into_parts();
        if mcp_tool_arguments_digest(&arguments)? != approval.identity.arguments_digest {
            return Err(AgentError::new("test approval argument binding changed"));
        }
        self.prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(
                approval.identity.invocation_id.clone(),
                (approval.clone(), arguments),
            );
        Ok(approval)
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&identity.invocation_id);
        Ok(())
    }

    fn revalidate_approved(&self, approval: &AgentMcpToolApproval) -> AgentResult<()> {
        let prepared = self
            .prepared
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some((stored, arguments)) = prepared.get(&approval.identity.invocation_id) else {
            return Err(AgentError::new("test approval payload unavailable"));
        };
        if stored != approval
            || mcp_tool_arguments_digest(arguments)? != approval.identity.arguments_digest
        {
            return Err(AgentError::new("test approval binding changed"));
        }
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        invocation: McpApprovedToolInvocation,
        cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async move {
            if cancellation.is_cancelled() {
                return Err(AgentError::cancelled());
            }
            let prepared = self
                .prepared
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&invocation.approval.identity.invocation_id);
            let Some((stored, arguments)) = prepared else {
                return Err(AgentError::new(
                    "test approval payload was consumed or unavailable",
                ));
            };
            if stored != invocation.approval || arguments != json!({"value": ARGUMENT_CANARY}) {
                return Err(AgentError::new("test approval payload changed"));
            }
            self.invocation_count.fetch_add(1, Ordering::SeqCst);
            if matches!(self.behavior, ApprovalInvocationBehavior::BareCancellation) {
                return Err(AgentError::cancelled());
            }
            Ok(McpToolInvocationResult {
                content: vec![McpToolContentBlock::Text {
                    text: RESULT_CANARY.to_string(),
                }],
                structured_content: Some(json!({"status": "fixture-complete"})),
                is_error: false,
                truncated_at_source: matches!(
                    self.behavior,
                    ApprovalInvocationBehavior::TruncatedSuccess
                ),
            })
        })
    }
}

fn lifecycle_descriptor() -> McpAgentToolDescriptor {
    let input_schema = json!({
        "type": "object",
        "properties": {
            "value": {"type": "string"}
        },
        "required": ["value"]
    });
    let normalized =
        mcp_normalized_input_schema_identity("mcp__approval_fixture__echo", &input_schema).unwrap();
    assert_eq!(
        normalized.normalizer_version,
        MCP_INPUT_SCHEMA_NORMALIZER_VERSION
    );
    McpAgentToolDescriptor {
        provenance: AgentMcpToolProvenance {
            server_id: uuid::Uuid::new_v4().to_string(),
            scope: AgentMcpServerScope::User,
            raw_tool_name: "echo".to_string(),
            model_tool_name: "mcp__approval_fixture__echo".to_string(),
            config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
            registry_revision: 11,
            config_digest: "1".repeat(64),
            catalog_generation: 1,
            catalog_digest: "2".repeat(64),
            catalog_schema_digest: "3".repeat(64),
            schema_digest: normalized.schema_digest,
            schema_normalizer_version: normalized.normalizer_version,
        },
        server_display_name: "Owned approval fixture".to_string(),
        description: Some("Repository-owned approval lifecycle fixture".to_string()),
        input_schema,
        output_schema: None,
        annotations: McpAgentToolAnnotations {
            read_only_hint: Some(true),
            ..Default::default()
        },
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_service_approval_cas_runs_once_and_keeps_mcp_values_out_of_durable_state() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Approval lifecycle complete.").await;
        drop(second);
        (first_request, second_request)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("mcp-approval-lifecycle.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    save_test_pending_provider(
        &storage,
        "mcp-approval-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_descriptor(lifecycle_descriptor());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let _notification_guard = notifications.clone();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-approval-lifecycle".to_string()),
                project_id: None,
                model_id: "mcp-approval-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP approval fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-approval-lifecycle".to_string()),
                assistant_message_id: Some("assistant-mcp-approval-lifecycle".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();

    let approval_required = wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let waiting = wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done"
            && notification["params"]["status"] == "waiting_for_approval"
    })
    .await;
    assert!(!waiting.to_string().contains(ARGUMENT_CANARY));
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::McpToolCall { approval } = &pending[0].action else {
        panic!("expected typed MCP pending action");
    };
    let arguments_digest = approval.identity.arguments_digest.clone();
    assert_eq!(approval.call.args, json!({}));
    let renderer_approval = approval_required.to_string();
    assert!(!renderer_approval.contains(&arguments_digest));
    assert!(!renderer_approval.contains(ARGUMENT_CANARY));
    let renderer_snapshot = serde_json::to_string(&pending[0]).unwrap();
    assert!(!renderer_snapshot.contains(&arguments_digest));
    assert!(!renderer_snapshot.contains(ARGUMENT_CANARY));
    assert!(!waiting.to_string().contains(&arguments_digest));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 0);

    let approved = service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone())
        .unwrap();
    assert_eq!(approved.status, "approved");
    assert!(service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications.clone(),)
        .is_err());

    let completed = wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "done" && notification["params"]["status"] == "completed"
    })
    .await;
    assert!(!completed.to_string().contains(ARGUMENT_CANARY));
    assert!(!completed.to_string().contains(RESULT_CANARY));
    let (first_request, second_request) = model_server.await.unwrap();
    assert!(first_request["tools"].as_array().is_some_and(|tools| tools
        .iter()
        .any(|tool| { tool["function"]["name"].as_str() == Some("mcp__approval_fixture__echo") })));
    let encoded = serde_json::to_string(&second_request).unwrap();
    assert!(encoded.contains(RESULT_CANARY));
    assert!(!encoded.contains(ARGUMENT_CANARY));
    assert_eq!(invoker.invocation_count.load(Ordering::SeqCst), 1);

    drop(service);
    drop(storage);
    for entry in std::fs::read_dir(fixture.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            let bytes = std::fs::read(path).unwrap();
            let database_fragment = String::from_utf8_lossy(&bytes);
            assert!(!database_fragment.contains(ARGUMENT_CANARY));
            assert!(!database_fragment.contains(RESULT_CANARY));
        }
    }
}

async fn run_approved_behavior(behavior: ApprovalInvocationBehavior) -> (Value, Vec<Value>, u64) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model_server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        write_text_stream(&mut second, "Lifecycle fixture complete.").await;
        drop(second);
        second_request
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open(&fixture.path().join("mcp-lifecycle-behavior.sqlite")).unwrap(),
    );
    save_test_pending_provider(
        &storage,
        "mcp-lifecycle-behavior-model",
        &format!("http://{address}/v1/chat/completions"),
        "fixed-test-model-token",
        "disabled",
        "",
    );
    let invoker = ApprovalLifecycleInvoker::with_behavior(lifecycle_descriptor(), behavior);
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>);
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-mcp-lifecycle-behavior".to_string()),
                project_id: None,
                model_id: "mcp-lifecycle-behavior-model".to_string(),
                context_window_indicator_enabled: false,
                content: "Call the owned MCP lifecycle fixture.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-mcp-lifecycle-behavior".to_string()),
                assistant_message_id: Some("assistant-mcp-lifecycle-behavior".to_string()),
                max_tokens: Some(1_024),
                temperature: None,
                prompt_preferences: None,
                permissions: Default::default(),
            },
            notifications.clone(),
        )
        .unwrap();
    wait_for_notification_matching(&mut receiver, |notification| {
        notification["params"]["type"] == "approval_required"
    })
    .await;
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    service
        .approve_action(&turn.run_id, &pending[0].action_id, notifications)
        .unwrap();

    let lifecycle_events = tokio::time::timeout(Duration::from_secs(5), async {
        let mut events = Vec::new();
        loop {
            let notification = receiver.recv().await.expect("notification channel closed");
            if notification["params"]["type"] == "mcp_tool_invocation_state_changed" {
                events.push(notification["params"]["invocation"].clone());
            }
            if notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
            {
                return events;
            }
        }
    })
    .await
    .expect("lifecycle behavior scenario timed out");
    let second_request = model_server.await.unwrap();
    let invocation_count = invoker.invocation_count.load(Ordering::SeqCst);
    (second_request, lifecycle_events, invocation_count)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_cas_bare_cancellation_is_outcome_unknown_and_model_sees_certainty() {
    let (second_request, events, invocation_count) =
        run_approved_behavior(ApprovalInvocationBehavior::BareCancellation).await;
    assert_eq!(invocation_count, 1);
    let encoded = serde_json::to_string(&second_request).unwrap();
    assert!(encoded.contains("mcp.tool_outcome_unknown"));
    assert!(encoded.contains("possibly_dispatched"));
    assert!(!encoded.contains(ARGUMENT_CANARY));

    let states = events
        .iter()
        .map(|event| {
            (
                event["state"].as_str().unwrap_or("<missing>"),
                event["dispatchCertainty"].as_str().unwrap_or("<missing>"),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        states,
        vec![
            ("dispatching", "possibly_dispatched"),
            ("outcome_unknown", "possibly_dispatched"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn authoritative_truncation_reaches_the_terminal_lifecycle_event() {
    let (second_request, events, invocation_count) =
        run_approved_behavior(ApprovalInvocationBehavior::TruncatedSuccess).await;
    assert_eq!(invocation_count, 1);
    let tool_content = second_request["messages"]
        .as_array()
        .and_then(|messages| messages.iter().find(|message| message["role"] == "tool"))
        .and_then(|message| message["content"].as_str())
        .expect("missing live MCP ToolResult in the continuation request");
    assert!(tool_content.contains("\"truncatedAtSource\":true"));
    let terminal = events.last().expect("missing terminal MCP lifecycle event");
    assert_eq!(terminal["state"], "completed");
    assert_eq!(terminal["dispatchCertainty"], "response_received");
    assert_eq!(terminal["outputTruncated"], true);
}

async fn wait_for_notification_matching(
    receiver: &mut tokio::sync::mpsc::UnboundedReceiver<Value>,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    let mut seen = Vec::new();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.expect("notification channel closed");
            if predicate(&notification) {
                return notification;
            }
            seen.push((
                notification["params"]["type"]
                    .as_str()
                    .unwrap_or("<missing>")
                    .to_string(),
                notification["params"]["status"]
                    .as_str()
                    .unwrap_or("<missing>")
                    .to_string(),
                notification["params"]["code"]
                    .as_str()
                    .unwrap_or("<missing>")
                    .to_string(),
            ));
        }
    })
    .await;
    result.unwrap_or_else(|_| {
        panic!("timed out waiting for Agent notification; safe event summary: {seen:?}")
    })
}

async fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "model fixture closed before request completed");
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

async fn write_tool_call_stream(stream: &mut TcpStream) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let tool_frame = json!({
        "choices": [{
            "delta": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "index": 0,
                    "id": "mcp-approval-provider-call",
                    "type": "function",
                    "function": {
                        "name": "mcp__approval_fixture__echo",
                        "arguments": serde_json::to_string(&json!({
                            "value": ARGUMENT_CANARY
                        })).unwrap()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

async fn write_text_stream(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let content_frame = json!({
        "choices": [{
            "delta": {"role": "assistant", "content": content},
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{"delta": {}, "finish_reason": "stop"}]
    });
    stream
        .write_all(
            format!("data: {content_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}
