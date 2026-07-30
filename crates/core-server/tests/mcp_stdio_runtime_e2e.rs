use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mycopilot_core::{
    send_chat_with_host_services, AgentApiStyle, AgentCancellationToken, AgentChatInput,
    AgentChatMessage, AgentEventEmitter, AgentMcpToolApproval, AgentProposedAction,
    AgentRunContext, AgentRunStatus, AgentRuntimeHostServices, McpApprovedToolInvocation,
    McpToolCatalogContext, McpToolContentBlock, McpToolInvoker, McpToolRuntime, ModelCapabilities,
};
use mycopilot_core_server::adapters::mcp_runtime::McpRuntimeBridge;
use mycopilot_mcp_client::{
    InMemoryMcpRegistry, McpApprovalMode, McpConnectionManager, McpConnector, McpEnvBinding,
    McpManagerPolicy, McpRegistry, McpServerConfig, McpServerId, McpServerScope, McpStdioConfig,
    McpStdioConnector, McpStdioPolicy, McpTransportConfig, McpTrustLevel,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::{tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const FIXTURE_MODE: &str = "--mycopilot-owned-mcp-fixture";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EchoInput {
    text: String,
}

#[derive(Debug, Clone)]
struct OwnedFixtureServer {
    tool_router: ToolRouter<Self>,
    call_marker: PathBuf,
}

impl OwnedFixtureServer {
    fn new(call_marker: PathBuf) -> Self {
        Self {
            tool_router: Self::tool_router(),
            call_marker,
        }
    }
}

#[tool_router(router = tool_router)]
impl OwnedFixtureServer {
    #[tool(
        name = "echo_text",
        description = "Echo fixed repository-owned fixture input",
        annotations(read_only_hint = true)
    )]
    async fn echo_text(
        &self,
        rmcp::handler::server::wrapper::Parameters(input): rmcp::handler::server::wrapper::Parameters<EchoInput>,
    ) -> String {
        let mut marker = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.call_marker)
            .expect("open repository-owned fixture call marker");
        marker
            .write_all(b"called\n")
            .expect("write repository-owned fixture call marker");
        input.text
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for OwnedFixtureServer {}

#[tokio::main]
async fn main() {
    mycopilot_core_server::mcp_trace_safety::install_mcp_safe_tracing()
        .expect("repository fixture must install the MCP-safe tracing policy");
    let mut args = std::env::args_os();
    let _executable = args.next();
    if args.next().as_deref() == Some(std::ffi::OsStr::new(FIXTURE_MODE)) {
        let marker = args
            .next()
            .map(PathBuf::from)
            .expect("owned fixture marker path");
        let service = OwnedFixtureServer::new(marker)
            .serve(rmcp::transport::stdio())
            .await
            .expect("start repository-owned MCP stdio fixture");
        service
            .waiting()
            .await
            .expect("wait for repository-owned MCP stdio fixture");
        return;
    }

    runtime_tool_registry_bridge_manager_stdio_fixture_chain().await;
}

async fn runtime_tool_registry_bridge_manager_stdio_fixture_chain() {
    let fixture_dir = tempfile::tempdir().expect("create fixture temp directory");
    let call_marker = fixture_dir.path().join("mcp-call-marker");
    let executable = std::env::current_exe().expect("resolve repository-owned test executable");

    let server_id = McpServerId::new();
    let registry = InMemoryMcpRegistry::shared();
    registry
        .add(McpServerConfig {
            id: server_id,
            display_name: "repository-owned stdio fixture".to_string(),
            scope: McpServerScope::Project {
                project_id: "project-mcp-stdio-e2e".to_string(),
            },
            trust: McpTrustLevel::Managed,
            approval_mode: McpApprovalMode::Prompt,
            enabled: true,
            transport: McpTransportConfig::Stdio(McpStdioConfig {
                program: executable.clone(),
                arguments: vec![
                    FIXTURE_MODE.to_string(),
                    call_marker.to_string_lossy().into_owned(),
                ],
                cwd: fixture_dir.path().to_path_buf(),
                environment: Vec::<McpEnvBinding>::new(),
            }),
            connect_timeout_ms: 3_000,
            request_timeout_ms: 2_000,
            shutdown_timeout_ms: 2_000,
        })
        .expect("register repository-owned MCP fixture");
    let connector: Arc<dyn McpConnector> = Arc::new(McpStdioConnector::new(McpStdioPolicy {
        allowed_programs: BTreeSet::from([executable]),
        ..McpStdioPolicy::default()
    }));
    let manager = Arc::new(
        McpConnectionManager::without_events(registry, connector, McpManagerPolicy::default())
            .expect("construct MCP connection manager"),
    );
    manager
        .start(server_id)
        .await
        .expect("start repository-owned MCP fixture");

    let bridge = Arc::new(McpRuntimeBridge::new(Arc::clone(&manager)));
    let catalog_context = McpToolCatalogContext {
        project_id: Some("project-mcp-stdio-e2e".to_string()),
    };
    let descriptor = bridge
        .catalog(&catalog_context)
        .expect("capture MCP catalog")
        .into_iter()
        .find(|tool| tool.provenance.raw_tool_name == "echo_text")
        .expect("owned echo_text fixture tool");
    let model_tool_name = descriptor.provenance.model_tool_name;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind repository-owned model fixture");
    let model_address = listener.local_addr().expect("model fixture address");
    let response_tool_name = model_tool_name.clone();
    let model_fixture = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.expect("accept model request");
        let request = read_json_request(&mut stream).await;
        assert!(request["tools"]
            .as_array()
            .is_some_and(|tools| tools.iter().any(|tool| {
                tool["function"]["name"].as_str() == Some(response_tool_name.as_str())
            })));
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "owned-provider-call",
                            "type": "function",
                            "function": {
                                "name": response_tool_name,
                                "arguments": "{\"text\":\"runtime-to-stdio\"}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;
    });

    let input = AgentChatInput {
        api_url: format!("http://{model_address}/v1/chat/completions"),
        api_token: "fixed-model-fixture-token".to_string(),
        provider_configuration_revision: None,
        model: "fixed-model-fixture".to_string(),
        model_capabilities: ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(4_096),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-mcp-stdio-e2e".to_string()),
            project_id: Some("project-mcp-stdio-e2e".to_string()),
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-mcp-stdio-e2e".to_string()),
        context_compaction_summary: None,
        goal: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![AgentChatMessage {
            message_id: Some("user-mcp-stdio-e2e".to_string()),
            role: "user".to_string(),
            content: "Call the repository-owned MCP fixture.".to_string(),
            created_at: Some(1),
            conversation_turn_trace: None,
            conversation_model_context_items: Vec::new(),
        }],
    };
    let invoker: Arc<dyn McpToolInvoker> = bridge.clone();
    let host_services = AgentRuntimeHostServices::new().with_mcp_tools(
        McpToolRuntime::capture_for_context(invoker, catalog_context),
    );
    let emitter: AgentEventEmitter = Arc::new(|_| {});
    let output = send_chat_with_host_services(
        input,
        "run-mcp-stdio-e2e".to_string(),
        emitter,
        AgentCancellationToken::new(),
        host_services,
    )
    .await
    .expect("Agent Runtime should prepare one MCP approval");
    model_fixture.await.expect("join model fixture");

    assert_eq!(output.status, AgentRunStatus::WaitingForApproval);
    assert!(
        !call_marker.exists(),
        "MCP fixture must not be invoked before approval"
    );
    let approval = only_mcp_approval(&output.proposed_actions);
    assert_eq!(approval.identity.provenance.raw_tool_name, "echo_text");
    assert_eq!(approval.call.args, json!({}));
    bridge
        .revalidate_approved(&approval)
        .expect("approval-time preflight should succeed before the dispatch boundary");
    assert!(
        !call_marker.exists(),
        "approval-time revalidation must not dispatch the MCP call"
    );

    let result = bridge
        .invoke_approved(
            McpApprovedToolInvocation {
                approval: approval.clone(),
            },
            AgentCancellationToken::new(),
        )
        .await
        .expect("approved invocation should traverse the real stdio MCP transport");
    assert!(result.content.iter().any(|block| {
        matches!(
            block,
            McpToolContentBlock::Text { text } if text == "runtime-to-stdio"
        )
    }));
    assert_eq!(fixture_call_count(&call_marker), 1);

    let replay = bridge
        .invoke_approved(
            McpApprovedToolInvocation { approval },
            AgentCancellationToken::new(),
        )
        .await
        .expect_err("the exact approval payload is consumable only once");
    assert_eq!(replay.code(), Some("mcp.approval_payload_unavailable"));
    assert_eq!(fixture_call_count(&call_marker), 1);

    let shutdown = manager.stop_all().await;
    assert!(shutdown.iter().all(|result| result.error.is_none()));
    assert_eq!(
        manager
            .get_status(server_id)
            .expect("read stopped fixture status")
            .expect("fixture remains registered")
            .active_call_count,
        0
    );
}

fn only_mcp_approval(actions: &[AgentProposedAction]) -> AgentMcpToolApproval {
    match actions {
        [AgentProposedAction::McpToolCall { approval }] => approval.as_ref().clone(),
        other => panic!("expected one typed MCP approval, got {other:?}"),
    }
}

fn fixture_call_count(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .expect("read repository-owned fixture call marker")
        .lines()
        .count()
}

async fn read_json_request(stream: &mut TcpStream) -> serde_json::Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream
            .read(&mut buffer)
            .await
            .expect("read model fixture request");
        assert!(read > 0, "model fixture connection closed early");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .expect("model fixture content length");
                body_start = Some(header_end + 4);
                expected_length = Some(header_end + 4 + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(
        &request[body_start.expect("request body start")..expected_length.expect("request length")],
    )
    .expect("decode model fixture request")
}

async fn write_json_response(stream: &mut TcpStream, body: serde_json::Value) {
    let body = serde_json::to_vec(&body).expect("encode model fixture response");
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream
        .write_all(headers.as_bytes())
        .await
        .expect("write model fixture headers");
    stream
        .write_all(&body)
        .await
        .expect("write model fixture body");
}
