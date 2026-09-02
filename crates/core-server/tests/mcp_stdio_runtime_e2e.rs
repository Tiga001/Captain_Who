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
    ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey,
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
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

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
        std::fs::write(fixture_pid_marker(&marker), std::process::id().to_string())
            .expect("write repository-owned fixture PID marker");
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
    persistent_registry_core_rpc_stdio_fixture_chain().await;
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
    #[cfg(unix)]
    let fixture_pid = read_fixture_pid(&call_marker);

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

    let provider_profile =
        ProviderProfileConfig::generic_for_dialect(ProviderProtocolDialect::OpenAiChatCompletions);
    let provider_configuration_revision = Some("provider-protocol-v1:mcp-stdio-e2e".to_string());
    let provider_protocol_key = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        "fixed-model-fixture",
        provider_configuration_revision.clone(),
    )
    .expect("freeze the current Generic OpenAI provider protocol");
    let input = AgentChatInput {
        api_url: format!("http://{model_address}/v1/chat/completions"),
        api_token: "fixed-model-fixture-token".to_string(),
        provider_configuration_revision,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(provider_profile),
        provider_protocol_key: Some(provider_protocol_key),
        model_config_id: None,
        model: "fixed-model-fixture".to_string(),
        model_capabilities: ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(4_096),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
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
    #[cfg(unix)]
    assert_process_reaped(fixture_pid);
    assert_eq!(
        manager
            .get_status(server_id)
            .expect("read stopped fixture status")
            .expect("fixture remains registered")
            .active_call_count,
        0
    );
}

async fn persistent_registry_core_rpc_stdio_fixture_chain() {
    let fixture_dir = tempfile::tempdir().expect("create persistent MCP fixture directory");
    let database = fixture_dir.path().join("storage.sqlite");
    let call_marker = fixture_dir.path().join("management-call-marker");
    let executable = std::env::current_exe().expect("resolve repository-owned fixture executable");

    let mut first_host = CoreRpcHarness::spawn(&database, fixture_dir.path()).await;
    let added = first_host
        .request(
            "mcp.server.add",
            json!({
                "schemaVersion": 1,
                "displayName": "repository-owned persistent fixture",
                "transport": "stdio",
                "executable": executable.to_string_lossy(),
                "arguments": [
                    FIXTURE_MODE,
                    call_marker.to_string_lossy()
                ],
                "cwd": fixture_dir.path().to_string_lossy(),
                "approvalMode": "prompt"
            }),
        )
        .await;
    assert!(added.get("error").is_none(), "MCP add must succeed");
    let added_server = &added["result"]["server"];
    assert_eq!(added_server["enabled"], false);
    assert_eq!(added_server["trust"], "untrusted");
    assert_eq!(added_server["launchAuthorizationState"], "required");
    let server_id = required_string(added_server, "serverId");

    let preview = first_host
        .request(
            "mcp.server.authorizeLaunch.prepare",
            mutation_params(added_server),
        )
        .await;
    assert!(
        preview.get("error").is_none(),
        "launch authorization preview must succeed"
    );
    let preview_result = &preview["result"];
    assert_eq!(
        preview_result["serverId"].as_str(),
        Some(server_id.as_str())
    );
    assert_eq!(
        preview_result["arguments"],
        json!([FIXTURE_MODE, call_marker.to_string_lossy().into_owned()])
    );

    let committed = first_host
        .request(
            "mcp.server.authorizeLaunch.commit",
            json!({
                "schemaVersion": 1,
                "authorizationId": required_string(preview_result, "authorizationId"),
                "precondition": preview_result["precondition"].clone()
            }),
        )
        .await;
    assert!(
        committed.get("error").is_none(),
        "launch authorization commit must succeed"
    );
    let authorized_server = &committed["result"]["server"];
    assert_eq!(authorized_server["enabled"], false);
    assert_eq!(authorized_server["trust"], "userApproved");
    assert_eq!(authorized_server["launchAuthorizationState"], "authorized");

    let enabled = first_host
        .request("mcp.server.enable", mutation_params(authorized_server))
        .await;
    assert!(enabled.get("error").is_none(), "enable must succeed");
    let enabled_server = &enabled["result"]["server"];
    assert_eq!(enabled_server["enabled"], true);

    let started = first_host
        .request("mcp.server.start", mutation_params(enabled_server))
        .await;
    assert!(
        started.get("error").is_none(),
        "authorized fixture start must succeed"
    );
    let started_server = &started["result"]["server"];
    assert_eq!(started_server["state"], "ready");
    #[cfg(unix)]
    let first_fixture_pid = read_fixture_pid(&call_marker);

    let tools = first_host
        .request(
            "mcp.catalog.tools",
            json!({
                "schemaVersion": 1,
                "serverId": server_id,
                "limit": 100
            }),
        )
        .await;
    assert!(
        tools.get("error").is_none(),
        "fixture tool discovery must succeed"
    );
    assert!(tools["result"]["tools"]
        .as_array()
        .is_some_and(|tools| tools.iter().any(|tool| {
            tool["rawName"].as_str() == Some("echo_text")
                && tool.get("inputSchema").is_none()
                && tool.get("annotations").is_none()
                && tool.get("_meta").is_none()
        })));
    assert!(
        !call_marker.exists(),
        "management discovery must never invoke an MCP tool"
    );

    let refreshed = first_host
        .request("mcp.catalog.refresh", mutation_params(started_server))
        .await;
    assert!(
        refreshed.get("error").is_none(),
        "fixture Catalog refresh must succeed"
    );
    let persisted_identity = (
        required_string(started_server, "serverId"),
        required_string(started_server, "configEpoch"),
        required_string(started_server, "configDigest"),
        started_server["registryRevision"]
            .as_u64()
            .expect("persisted Registry revision"),
    );
    first_host.shutdown().await;
    #[cfg(unix)]
    assert_process_reaped(first_fixture_pid);

    let mut restarted_host = CoreRpcHarness::spawn(&database, fixture_dir.path()).await;
    let restored = restarted_host
        .request(
            "mcp.server.get",
            json!({
                "schemaVersion": 1,
                "serverId": persisted_identity.0
            }),
        )
        .await;
    assert!(
        restored.get("error").is_none(),
        "persisted server must survive Host restart"
    );
    let restored_server = &restored["result"]["server"];
    assert_eq!(
        restored_server["serverId"].as_str(),
        Some(persisted_identity.0.as_str())
    );
    assert_eq!(
        restored_server["configEpoch"].as_str(),
        Some(persisted_identity.1.as_str())
    );
    assert_eq!(
        restored_server["configDigest"].as_str(),
        Some(persisted_identity.2.as_str())
    );
    assert_eq!(
        restored_server["registryRevision"].as_u64(),
        Some(persisted_identity.3)
    );
    assert_eq!(restored_server["enabled"], true);
    assert_eq!(restored_server["launchAuthorizationState"], "authorized");

    // `start_enabled` may already be negotiating. A duplicate typed start is
    // deliberately coalesced by the Manager and gives this E2E an authoritative
    // Ready boundary without timing sleeps.
    let restored_started = restarted_host
        .request("mcp.server.start", mutation_params(restored_server))
        .await;
    assert!(
        restored_started.get("error").is_none(),
        "restored exact authorization must start the fixture"
    );
    let restored_started_server = &restored_started["result"]["server"];
    assert_eq!(restored_started_server["state"], "ready");
    #[cfg(unix)]
    let restored_fixture_pid = read_fixture_pid(&call_marker);

    let restarted = restarted_host
        .request(
            "mcp.server.restart",
            mutation_params(restored_started_server),
        )
        .await;
    assert!(
        restarted.get("error").is_none(),
        "fixture restart must succeed"
    );
    let restarted_server = &restarted["result"]["server"];
    assert_eq!(restarted_server["state"], "ready");
    #[cfg(unix)]
    let restarted_fixture_pid = read_fixture_pid(&call_marker);
    #[cfg(unix)]
    {
        assert_ne!(
            restored_fixture_pid, restarted_fixture_pid,
            "restart must replace the repository-owned fixture process"
        );
        assert_process_reaped(restored_fixture_pid);
    }

    let deleted = restarted_host
        .request("mcp.server.delete", mutation_params(restarted_server))
        .await;
    assert!(
        deleted.get("error").is_none(),
        "fixture deletion and process cleanup must succeed"
    );
    assert_eq!(deleted["result"]["server"]["enabled"], false);
    assert_eq!(deleted["result"]["server"]["state"], "disabled");
    #[cfg(unix)]
    assert_process_reaped(restarted_fixture_pid);

    let list = restarted_host
        .request("mcp.server.list", json!({ "schemaVersion": 1 }))
        .await;
    assert_eq!(list["result"]["servers"], json!([]));
    assert!(
        list["result"]["registryRevision"]
            .as_u64()
            .is_some_and(|revision| revision > persisted_identity.3),
        "global Registry revision must remain monotonic across restart"
    );
    assert!(
        !call_marker.exists(),
        "management lifecycle must not bypass per-tool approval"
    );
    restarted_host.shutdown().await;
}

struct CoreRpcHarness {
    child: Child,
    stdin: ChildStdin,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    stderr: tokio::task::JoinHandle<Vec<u8>>,
    next_id: u64,
}

impl CoreRpcHarness {
    async fn spawn(database: &Path, isolated_root: &Path) -> Self {
        let home = isolated_root.join("home");
        let app_data = isolated_root.join("app-data");
        let xdg_data = isolated_root.join("xdg-data");
        std::fs::create_dir_all(&home).expect("create isolated test home");
        std::fs::create_dir_all(&app_data).expect("create isolated test app data");
        std::fs::create_dir_all(&xdg_data).expect("create isolated test XDG data");
        let mut child = Command::new(env!("CARGO_BIN_EXE_core-server"))
            .env_clear()
            .env("MYCOPILOT_STORAGE_DB", database)
            .env("HOME", home)
            .env("APPDATA", app_data)
            .env("XDG_DATA_HOME", xdg_data)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn repository core-server");
        let stdin = child.stdin.take().expect("core-server stdin");
        let stdout = BufReader::new(child.stdout.take().expect("core-server stdout")).lines();
        let child_stderr = child.stderr.take().expect("core-server stderr");
        let stderr = tokio::spawn(async move {
            let mut bytes = Vec::new();
            child_stderr
                .take((64 * 1024 + 1) as u64)
                .read_to_end(&mut bytes)
                .await
                .expect("read bounded test core-server stderr");
            bytes
        });
        let mut harness = Self {
            child,
            stdin,
            stdout,
            stderr,
            next_id: 1,
        };
        let ping = harness.request("core.ping", json!({})).await;
        assert_eq!(ping["result"]["message"], "pong");
        harness
    }

    async fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).expect("test RPC ID capacity");
        let request = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.stdin
            .write_all(format!("{request}\n").as_bytes())
            .await
            .expect("write core-server JSON-RPC request");
        self.stdin
            .flush()
            .await
            .expect("flush core-server JSON-RPC request");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let line = tokio::time::timeout_at(deadline, self.stdout.next_line())
                .await
                .expect("core-server response before test deadline")
                .expect("read core-server stdout")
                .expect("core-server stdout remained open");
            let response: Value =
                serde_json::from_str(&line).expect("decode core-server JSON-RPC output");
            if response["id"].as_u64() == Some(id) {
                return response;
            }
            assert!(
                response.get("id").is_none() && response.get("method").is_some(),
                "unexpected core-server JSON-RPC response"
            );
        }
    }

    async fn shutdown(mut self) {
        let response = self.request("core.shutdown", json!({})).await;
        assert!(
            response.get("error").is_none(),
            "core shutdown must succeed"
        );
        drop(self.stdin);
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), self.child.wait())
            .await
            .expect("core-server exits before shutdown deadline")
            .expect("wait for core-server");
        let stderr = self.stderr.await.expect("join core-server stderr reader");
        assert!(status.success(), "core-server must exit successfully");
        assert!(
            stderr.len() <= 64 * 1024,
            "test core-server stderr exceeded the safe retained limit"
        );
    }
}

fn mutation_params(server: &Value) -> Value {
    json!({
        "schemaVersion": 1,
        "serverId": required_string(server, "serverId"),
        "precondition": {
            "expectedRegistryRevision": server["registryRevision"]
                .as_u64()
                .expect("server Registry revision"),
            "expectedConfigEpoch": required_string(server, "configEpoch"),
            "expectedConfigDigest": required_string(server, "configDigest")
        }
    })
}

fn required_string(value: &Value, key: &str) -> String {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string field {key}"))
        .to_string()
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

fn fixture_pid_marker(marker: &Path) -> PathBuf {
    marker.with_extension("pid")
}

#[cfg(unix)]
fn read_fixture_pid(marker: &Path) -> libc::pid_t {
    std::fs::read_to_string(fixture_pid_marker(marker))
        .expect("read repository-owned fixture PID marker")
        .parse()
        .expect("parse repository-owned fixture PID")
}

#[cfg(unix)]
fn assert_process_reaped(pid: libc::pid_t) {
    // SAFETY: signal 0 performs only an existence/permission probe. The PID
    // came from this repository-owned fixture process.
    let result = unsafe { libc::kill(pid, 0) };
    assert_eq!(
        result, -1,
        "repository-owned fixture PID {pid} still exists"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH),
        "repository-owned fixture PID {pid} was not fully reaped"
    );
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
