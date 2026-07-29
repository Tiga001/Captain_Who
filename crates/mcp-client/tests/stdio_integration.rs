use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycopilot_mcp_client::{
    InMemoryMcpRegistry, McpCancellationToken, McpCatalogCompleteness, McpConnectionManager,
    McpConnectionState, McpConnector, McpContentBlock, McpEnvBinding, McpErrorKind, McpEvent,
    McpLifecycleKind, McpManagerPolicy, McpPeer, McpPeerNotificationState, McpRegistry,
    McpServerConfig, McpServerId, McpServerScope, McpStdioConfig, McpStdioConnector,
    McpStdioPolicy, McpToolCall, McpTransportConfig, McpTrustLevel,
};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListToolsResult, PaginatedRequestParams,
    ServerCapabilities, ServerInfo, SubscriptionFilter, Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext, SubscriptionSink};
use rmcp::{tool, tool_handler, tool_router, Json, RoleServer, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MODERN_FIXTURE: &str = "--fixture-modern";
const LEGACY_FIXTURE: &str = "--fixture-legacy";
const DYNAMIC_MODERN_FIXTURE: &str = "--fixture-dynamic-modern";
const DYNAMIC_LEGACY_FIXTURE: &str = "--fixture-dynamic-legacy";
const EARLY_EXIT_FIXTURE: &str = "--fixture-early-exit";
const EXIT_AFTER_NEGOTIATION_FIXTURE: &str = "--fixture-exit-after-negotiation";
const STDERR_FIXTURE: &str = "--fixture-stderr-flood";
const STDOUT_FIXTURE: &str = "--fixture-stdout-flood";
const UNCOOPERATIVE_FIXTURE: &str = "--fixture-uncooperative-close";
const UNRESPONSIVE_FIXTURE: &str = "--fixture-unresponsive-after-negotiation";
const PROTOCOL_EOF_FIXTURE: &str = "--fixture-protocol-eof-alive";
const ENV_PARENT: &str = "--fixture-env-parent";
const ENV_PROBE: &str = "--fixture-env-probe";
const FORBIDDEN_TEST_ENV: &str = "MYCOPILOT_MCP_FORBIDDEN_TEST_VALUE";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct EchoInput {
    text: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct AddInput {
    left: i64,
    right: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct AddOutput {
    sum: i64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct StructuredOutput {
    status: String,
    value: i64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SlowInput {
    delay_ms: u64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct SlowStatus {
    started: bool,
    cancelled: bool,
}

#[derive(Debug, Clone)]
struct FixtureServer {
    tool_router: ToolRouter<Self>,
    slow_started: Arc<AtomicBool>,
    slow_cancelled: Arc<AtomicBool>,
}

impl FixtureServer {
    fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            slow_started: Arc::new(AtomicBool::new(false)),
            slow_cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[tool_router(router = tool_router)]
impl FixtureServer {
    #[tool(name = "echo_text", description = "Echo deterministic fixture text")]
    async fn echo_text(&self, Parameters(input): Parameters<EchoInput>) -> String {
        input.text
    }

    #[tool(name = "add_numbers", description = "Add two fixture integers")]
    async fn add_numbers(&self, Parameters(input): Parameters<AddInput>) -> Json<AddOutput> {
        Json(AddOutput {
            sum: input.left + input.right,
        })
    }

    #[tool(
        name = "structured_result",
        description = "Return deterministic structured fixture data"
    )]
    async fn structured_result(&self) -> Json<StructuredOutput> {
        Json(StructuredOutput {
            status: "ok".to_string(),
            value: 7,
        })
    }

    #[tool(
        name = "return_tool_error",
        description = "Return a deterministic tool-level error"
    )]
    async fn return_tool_error(&self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text("fixture tool error")])
    }

    #[tool(
        name = "slow_tool",
        description = "Wait for a controlled fixture duration"
    )]
    async fn slow_tool(
        &self,
        Parameters(input): Parameters<SlowInput>,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> String {
        self.slow_started.store(true, Ordering::SeqCst);
        self.slow_cancelled.store(false, Ordering::SeqCst);
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(input.delay_ms.min(5_000))) => {
                "completed".to_string()
            }
            _ = cancellation.cancelled() => {
                self.slow_cancelled.store(true, Ordering::SeqCst);
                "cancelled".to_string()
            }
        }
    }

    #[tool(
        name = "slow_status",
        description = "Read deterministic in-memory fixture cancellation state"
    )]
    async fn slow_status(&self) -> Json<SlowStatus> {
        Json(SlowStatus {
            started: self.slow_started.load(Ordering::SeqCst),
            cancelled: self.slow_cancelled.load(Ordering::SeqCst),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for FixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("mycopilot-owned-fixture", "1.0.0"))
    }
}

#[derive(Clone)]
struct DynamicFixtureServer {
    modern: bool,
    visible: Arc<AtomicBool>,
    scheduled: Arc<AtomicBool>,
    subscription: Arc<tokio::sync::Mutex<Option<SubscriptionSink>>>,
}

impl DynamicFixtureServer {
    fn new(modern: bool) -> Self {
        Self {
            modern,
            visible: Arc::new(AtomicBool::new(false)),
            scheduled: Arc::new(AtomicBool::new(false)),
            subscription: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    fn schedule_change(&self, peer: rmcp::Peer<RoleServer>) {
        if self.scheduled.swap(true, Ordering::SeqCst) {
            return;
        }
        let fixture = self.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            fixture.visible.store(true, Ordering::SeqCst);
            if fixture.modern {
                let sink = {
                    let mut selected = None;
                    for _ in 0..100 {
                        if let Some(sink) = fixture.subscription.lock().await.clone() {
                            selected = Some(sink);
                            break;
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    selected
                };
                if let Some(sink) = sink {
                    for _ in 0..8 {
                        let _ = sink.notify_tool_list_changed().await;
                    }
                }
            } else {
                for _ in 0..8 {
                    let _ = peer.notify_tool_list_changed().await;
                }
            }
        });
    }
}

impl ServerHandler for DynamicFixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .build(),
        )
        .with_server_info(Implementation::new(
            "mycopilot-owned-dynamic-fixture",
            "1.0.0",
        ))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        self.schedule_change(context.peer);
        let schema = Arc::new(Default::default());
        let mut tools = vec![Tool::new(
            "baseline_tool",
            "Deterministic baseline fixture tool",
            Arc::clone(&schema),
        )];
        if self.visible.load(Ordering::SeqCst) {
            tools.push(Tool::new(
                "dynamic_tool",
                "Deterministic dynamically discovered fixture tool",
                schema,
            ));
        }
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    fn accepted_subscription_filter(
        &self,
        requested: &SubscriptionFilter,
    ) -> Option<SubscriptionFilter> {
        self.modern
            .then(|| requested.supported_by(&self.get_info().capabilities))
    }

    async fn listen(&self, context: SubscriptionContext) -> Result<(), rmcp::ErrorData> {
        if !self.modern {
            return Err(rmcp::ErrorData::method_not_found::<
                rmcp::model::SubscriptionsListenRequestMethod,
            >());
        }
        *self.subscription.lock().await = Some(context.sink().clone());
        context.cancelled().await;
        *self.subscription.lock().await = None;
        Ok(())
    }
}

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some(MODERN_FIXTURE) => serve_fixture(false).await,
        Some(LEGACY_FIXTURE) => serve_fixture(true).await,
        Some(DYNAMIC_MODERN_FIXTURE) => serve_dynamic_fixture(false).await,
        Some(DYNAMIC_LEGACY_FIXTURE) => serve_dynamic_fixture(true).await,
        Some(EARLY_EXIT_FIXTURE) => {}
        Some(EXIT_AFTER_NEGOTIATION_FIXTURE) => {
            let _service = FixtureServer::new()
                .serve(rmcp::transport::stdio())
                .await
                .expect("start exit-after-negotiation owned fixture");
            tokio::time::sleep(Duration::from_millis(300)).await;
            std::process::exit(0);
        }
        Some(STDERR_FIXTURE) => {
            let mut stderr = tokio::io::stderr();
            stderr
                .write_all(&vec![b'x'; 128 * 1024])
                .await
                .expect("write fixed fixture stderr");
            stderr.flush().await.expect("flush fixed fixture stderr");
            serve_fixture(false).await;
        }
        Some(STDOUT_FIXTURE) => {
            let mut stdout = tokio::io::stdout();
            stdout
                .write_all(&vec![b'x'; 4 * 1024])
                .await
                .expect("write fixed fixture stdout");
            stdout.flush().await.expect("flush fixed fixture stdout");
            std::future::pending::<()>().await;
        }
        Some(UNCOOPERATIVE_FIXTURE) => {
            let service = FixtureServer::new()
                .serve(rmcp::transport::stdio())
                .await
                .expect("start uncooperative owned fixture");
            service
                .waiting()
                .await
                .expect("wait for uncooperative fixture transport close");
            std::future::pending::<()>().await;
        }
        Some(UNRESPONSIVE_FIXTURE) => {
            let service = FixtureServer::new()
                .serve(rmcp::transport::stdio())
                .await
                .expect("start unresponsive owned fixture");
            service
                .cancel()
                .await
                .expect("stop fixture protocol reader after negotiation");
            std::future::pending::<()>().await;
        }
        #[cfg(unix)]
        Some(PROTOCOL_EOF_FIXTURE) => {
            let service = FixtureServer::new()
                .serve(rmcp::transport::stdio())
                .await
                .expect("start protocol-EOF owned fixture");
            service
                .cancel()
                .await
                .expect("stop protocol service before closing stdout");
            // SAFETY: this is an isolated repository-owned child fixture. Its
            // only purpose is to model protocol EOF while PID remains alive.
            unsafe {
                libc::close(libc::STDOUT_FILENO);
            }
            std::future::pending::<()>().await;
        }
        Some(ENV_PARENT) => run_environment_parent().await,
        Some(ENV_PROBE) => {
            let present = std::env::var_os(FORBIDDEN_TEST_ENV).is_some();
            std::process::exit(if present { 73 } else { 0 });
        }
        Some(other) if other.starts_with("--fixture-") => {
            panic!("unknown fixture mode: {other}")
        }
        Some(_) | None => run_integration_suite().await,
    }
}

async fn serve_fixture(legacy: bool) {
    if legacy {
        serve_legacy_preamble().await;
    } else {
        let service = FixtureServer::new()
            .serve(rmcp::transport::stdio())
            .await
            .expect("start owned modern fixture");
        service.waiting().await.expect("wait for modern fixture");
    }
}

async fn serve_legacy_preamble() {
    let (stdin, stdout) = legacy_transport().await;
    let service = FixtureServer::new()
        .serve((stdin, stdout))
        .await
        .expect("start owned legacy fixture");
    service.waiting().await.expect("wait for legacy fixture");
}

async fn serve_dynamic_fixture(legacy: bool) {
    let server = DynamicFixtureServer::new(!legacy);
    let service = if legacy {
        let (stdin, stdout) = legacy_transport().await;
        server
            .serve((stdin, stdout))
            .await
            .expect("start owned legacy dynamic fixture")
    } else {
        server
            .serve(rmcp::transport::stdio())
            .await
            .expect("start owned modern dynamic fixture")
    };
    service
        .waiting()
        .await
        .expect("wait for owned dynamic fixture");
}

async fn legacy_transport() -> (tokio::io::Stdin, tokio::io::Stdout) {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .await
        .expect("read discover request");
    let request: Value = serde_json::from_str(&request_line).expect("parse discover request");
    assert_eq!(request["method"], "server/discover");
    let response = json!({
        "jsonrpc": "2.0",
        "id": request["id"],
        "error": {
            "code": -32601,
            "message": "Method not found"
        }
    });
    let mut stdout = tokio::io::stdout();
    stdout
        .write_all(serde_json::to_string(&response).unwrap().as_bytes())
        .await
        .expect("write discover rejection");
    stdout
        .write_all(b"\n")
        .await
        .expect("terminate discover rejection");
    stdout.flush().await.expect("flush discover rejection");
    (reader.into_inner(), stdout)
}

async fn run_integration_suite() {
    modern_discovery_and_tool_round_trip().await;
    legacy_initialize_fallback_and_tool_call().await;
    modern_and_legacy_dynamic_notifications_share_one_signal_api().await;
    manager_debounces_dynamic_tool_refresh().await;
    manager_emits_safe_owned_server_exit_event().await;
    timeout_sends_protocol_cancellation().await;
    explicit_cancellation_reaches_server().await;
    cancellation_stays_bounded_under_transport_backpressure().await;
    early_server_exit_is_structured().await;
    post_negotiation_exit_is_reaped_and_changes_state().await;
    #[cfg(unix)]
    protocol_eof_is_reported_while_the_child_remains_alive().await;
    close_reaps_server_process().await;
    forced_close_terminates_uncooperative_owned_fixture().await;
    stderr_is_continuously_drained_and_bounded().await;
    oversized_protocol_line_is_rejected().await;
    unallowlisted_parent_environment_is_not_inherited().await;
    launch_is_denied_without_explicit_authorization().await;
    invalid_timeout_configuration_is_rejected().await;
    transport_neutral_connector_api_and_stable_types().await;
}

async fn modern_and_legacy_dynamic_notifications_share_one_signal_api() {
    for (mode, lifecycle) in [
        (DYNAMIC_MODERN_FIXTURE, McpLifecycleKind::Discover),
        (DYNAMIC_LEGACY_FIXTURE, McpLifecycleKind::InitializeFallback),
    ] {
        let client = connect_fixture(mode).await;
        assert_eq!(client.protocol_snapshot().lifecycle, lifecycle);
        let mut signals = client.subscribe_signals();
        let initial = signals.snapshot();
        assert_eq!(initial.notification_state, McpPeerNotificationState::Active);
        let first = client
            .list_tools(None)
            .await
            .expect("initial dynamic tools/list");
        assert!(first.tools.iter().any(|tool| tool.name == "baseline_tool"));
        assert!(!first.tools.iter().any(|tool| tool.name == "dynamic_tool"));

        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let changed = signals
                    .changed()
                    .await
                    .expect("dynamic fixture signal channel");
                if changed.tools_revision > initial.tools_revision {
                    break;
                }
            }
        })
        .await
        .expect("dynamic tool notification");
        let refreshed = client
            .list_tools(None)
            .await
            .expect("refreshed dynamic tools/list");
        assert!(
            refreshed
                .tools
                .iter()
                .any(|tool| tool.name == "dynamic_tool"),
            "dynamic tool missing for {lifecycle:?}"
        );
        client.close().await.expect("close dynamic fixture");
    }
}

async fn manager_debounces_dynamic_tool_refresh() {
    let registry = InMemoryMcpRegistry::shared();
    let config = fixture_config(DYNAMIC_MODERN_FIXTURE);
    let server_id = config.id;
    registry
        .add(config)
        .expect("register owned dynamic fixture");
    let events = Arc::new(std::sync::Mutex::new(Vec::<McpEvent>::new()));
    let event_capture = Arc::clone(&events);
    let manager = McpConnectionManager::new(
        registry,
        Arc::new(fixture_connector()),
        Arc::new(move |event: McpEvent| {
            event_capture
                .lock()
                .expect("event capture lock")
                .push(event);
        }),
        McpManagerPolicy {
            notification_debounce: Duration::from_millis(100),
            ..McpManagerPolicy::default()
        },
    )
    .expect("construct dynamic fixture manager");

    let started = manager
        .start(server_id)
        .await
        .expect("start dynamic fixture through manager");
    assert_eq!(started.catalog_generation, 1);
    let first = manager.catalog(server_id).unwrap().unwrap();
    assert_eq!(first.completeness, McpCatalogCompleteness::Complete);
    assert!(!first
        .tools
        .iter()
        .any(|tool| tool.raw_name == "dynamic_tool"));

    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let catalog = manager.catalog(server_id).unwrap().unwrap();
            if catalog.generation == 2
                && catalog
                    .tools
                    .iter()
                    .any(|tool| tool.raw_name == "dynamic_tool")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("manager dynamic catalog refresh");
    tokio::time::sleep(Duration::from_millis(350)).await;
    assert_eq!(
        manager.catalog(server_id).unwrap().unwrap().generation,
        2,
        "notification storm must not advance generation without content changes"
    );
    let generation_two_events = events
        .lock()
        .expect("event capture lock")
        .iter()
        .filter(|event| {
            matches!(
                event,
                McpEvent::CatalogChanged {
                    server_id: event_server,
                    generation: 2,
                    ..
                } if *event_server == server_id
            )
        })
        .count();
    assert_eq!(generation_two_events, 1);
    manager.stop_all().await;
}

async fn manager_emits_safe_owned_server_exit_event() {
    let registry = InMemoryMcpRegistry::shared();
    let config = fixture_config(EXIT_AFTER_NEGOTIATION_FIXTURE);
    let server_id = config.id;
    registry
        .add(config)
        .expect("register owned exiting fixture");
    let events = Arc::new(std::sync::Mutex::new(Vec::<McpEvent>::new()));
    let event_capture = Arc::clone(&events);
    let manager = McpConnectionManager::new(
        registry,
        Arc::new(fixture_connector()),
        Arc::new(move |event: McpEvent| {
            event_capture
                .lock()
                .expect("event capture lock")
                .push(event);
        }),
        McpManagerPolicy::default(),
    )
    .expect("construct exiting fixture manager");
    manager
        .start(server_id)
        .await
        .expect("start owned exiting fixture");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let exited = events
                .lock()
                .expect("event capture lock")
                .iter()
                .any(|event| {
                    matches!(
                        event,
                        McpEvent::ServerExited {
                            server_id: event_server,
                            ..
                        } if *event_server == server_id
                    )
                });
            if exited
                && manager
                    .get_status(server_id)
                    .unwrap()
                    .is_some_and(|status| {
                        status.state == mycopilot_mcp_client::McpServerState::Error
                    })
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("manager observes owned fixture exit");
    manager.stop_all().await;
}

async fn modern_discovery_and_tool_round_trip() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let snapshot = client.protocol_snapshot();
    assert_eq!(snapshot.negotiated_version, "2026-07-28");
    assert_eq!(snapshot.lifecycle, McpLifecycleKind::Discover);
    assert!(snapshot.capabilities.tools);
    assert_eq!(
        snapshot.server.as_ref().map(|server| server.name.as_str()),
        Some("mycopilot-owned-fixture")
    );

    let page = client.list_tools(None).await.expect("list fixture tools");
    let names = page
        .tools
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<BTreeSet<_>>();
    for required in [
        "echo_text",
        "add_numbers",
        "structured_result",
        "return_tool_error",
        "slow_tool",
    ] {
        assert!(names.contains(required), "missing fixture tool {required}");
    }

    let echo = call(&client, "echo_text", json!({"text": "hello"}))
        .await
        .expect("call echo_text");
    assert_eq!(
        echo.content,
        vec![McpContentBlock::Text {
            text: "hello".to_string()
        }]
    );
    assert!(!echo.is_error);

    let addition = call(&client, "add_numbers", json!({"left": 19, "right": 23}))
        .await
        .expect("call add_numbers");
    assert_eq!(addition.structured_content, Some(json!({"sum": 42})));

    let structured = call(&client, "structured_result", json!({}))
        .await
        .expect("call structured_result");
    assert_eq!(
        structured.structured_content,
        Some(json!({"status": "ok", "value": 7}))
    );

    let tool_error = call(&client, "return_tool_error", json!({}))
        .await
        .expect("receive tool-level error result");
    assert!(tool_error.is_error);
    assert_eq!(
        tool_error.content,
        vec![McpContentBlock::Text {
            text: "fixture tool error".to_string()
        }]
    );
    client.close().await.expect("close modern fixture");
}

async fn legacy_initialize_fallback_and_tool_call() {
    let client = connect_fixture(LEGACY_FIXTURE).await;
    let snapshot = client.protocol_snapshot();
    assert_eq!(snapshot.negotiated_version, "2025-11-25");
    assert_eq!(snapshot.lifecycle, McpLifecycleKind::InitializeFallback);
    let page = client
        .list_tools(None)
        .await
        .expect("list tools after initialize fallback");
    assert!(page.tools.iter().any(|tool| tool.name == "echo_text"));
    let echo = call(&client, "echo_text", json!({"text": "legacy"}))
        .await
        .expect("call tool after initialize fallback");
    assert_eq!(
        echo.content,
        vec![McpContentBlock::Text {
            text: "legacy".to_string()
        }]
    );
    client.close().await.expect("close legacy fixture");
}

async fn timeout_sends_protocol_cancellation() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let mut slow = McpToolCall::new("slow_tool", json!({"delay_ms": 2_000}));
    slow.timeout_ms = Some(40);
    let error = client
        .call_tool(slow, McpCancellationToken::new())
        .await
        .expect_err("slow tool should time out");
    assert_eq!(error.kind, McpErrorKind::Timeout);
    wait_for_slow_status(&client, true).await;
    let echo = call(&client, "echo_text", json!({"text": "still-ready"}))
        .await
        .expect("connection remains usable after timeout");
    assert_eq!(
        echo.content,
        vec![McpContentBlock::Text {
            text: "still-ready".to_string()
        }]
    );
    client.close().await.expect("close timeout fixture");
}

async fn explicit_cancellation_reaches_server() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let token = McpCancellationToken::new();
    let call_client = Arc::clone(&client);
    let call_token = token.clone();
    let call_task = tokio::spawn(async move {
        call_client
            .call_tool(
                McpToolCall::new("slow_tool", json!({"delay_ms": 2_000})),
                call_token,
            )
            .await
    });
    wait_for_slow_status(&client, false).await;
    token.cancel();
    let error = call_task
        .await
        .expect("join cancelled tool task")
        .expect_err("slow tool should be cancelled");
    assert_eq!(error.kind, McpErrorKind::Cancelled);
    wait_for_slow_status(&client, true).await;
    client.close().await.expect("close cancellation fixture");
}

async fn cancellation_stays_bounded_under_transport_backpressure() {
    let mut config = fixture_config(UNRESPONSIVE_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect unresponsive owned fixture");
    let cancellation = McpCancellationToken::new();
    let call_client = Arc::clone(&client);
    let call_cancellation = cancellation.clone();
    let call_task = tokio::spawn(async move {
        call_client
            .call_tool(
                McpToolCall::new("echo_text", json!({"text": "x".repeat(2 * 1024 * 1024)})),
                call_cancellation,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_millis(500), call_task)
        .await
        .expect("cancellation must remain bounded")
        .expect("join backpressured call")
        .expect_err("backpressured call must be cancelled");
    assert_eq!(error.kind, McpErrorKind::Cancelled);
    let _ = client.close().await;
}

async fn early_server_exit_is_structured() {
    let error = fixture_connector()
        .connect(&fixture_config(EARLY_EXIT_FIXTURE))
        .await
        .expect_err("early-exit fixture must not negotiate");
    assert_eq!(error.kind, McpErrorKind::ServerExited);
    assert_eq!(error.exit_code, Some(0));
}

async fn post_negotiation_exit_is_reaped_and_changes_state() {
    let client = connect_fixture(EXIT_AFTER_NEGOTIATION_FIXTURE).await;
    for _ in 0..50 {
        if client.connection_state() == McpConnectionState::Failed {
            assert_eq!(
                client
                    .list_tools(None)
                    .await
                    .expect_err("exited server cannot list tools")
                    .kind,
                McpErrorKind::ServerExited
            );
            let _ = client.close().await;
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("post-negotiation process exit was not observed");
}

#[cfg(unix)]
async fn protocol_eof_is_reported_while_the_child_remains_alive() {
    let mut config = fixture_config(PROTOCOL_EOF_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect protocol-EOF owned fixture");
    let mut signals = client.subscribe_signals();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let snapshot = signals.snapshot();
            if snapshot.transport_closed {
                break;
            }
            signals
                .changed()
                .await
                .expect("protocol-EOF signal channel");
        }
    })
    .await
    .expect("protocol EOF must become a manager-visible signal");
    let _ = client.close().await;
}

async fn close_reaps_server_process() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    assert_eq!(client.connection_state(), McpConnectionState::Ready);
    client.close().await.expect("close and reap fixture");
    assert_eq!(client.connection_state(), McpConnectionState::Closed);
    client.close().await.expect("close is idempotent");
}

async fn forced_close_terminates_uncooperative_owned_fixture() {
    let mut config = fixture_config(UNCOOPERATIVE_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect uncooperative owned fixture");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), client.close())
            .await
            .is_err(),
        "first close future should be cancellable while owned shutdown continues"
    );
    client
        .close()
        .await
        .expect("resume, force terminate, and reap uncooperative owned fixture");
    assert_eq!(client.connection_state(), McpConnectionState::Closed);
}

async fn stderr_is_continuously_drained_and_bounded() {
    let policy = McpStdioPolicy {
        stderr_max_retained_bytes: 4 * 1024,
        stderr_rate_limit_bytes_per_second: 2 * 1024,
        ..fixture_policy()
    };
    let client = McpStdioConnector::new(policy)
        .connect(&fixture_config(STDERR_FIXTURE))
        .await
        .expect("stderr flood fixture should negotiate without deadlock");
    let snapshot = client.stderr_snapshot().await;
    assert!(snapshot.retained_bytes <= 4 * 1024);
    assert!(snapshot.truncated);
    assert!(snapshot.dropped_bytes > 0);
    client.close().await.expect("close stderr fixture");
}

async fn oversized_protocol_line_is_rejected() {
    let policy = McpStdioPolicy {
        stdout_max_line_bytes: 1024,
        ..fixture_policy()
    };
    let error = McpStdioConnector::new(policy)
        .connect(&fixture_config(STDOUT_FIXTURE))
        .await
        .expect_err("oversized protocol line must fail negotiation");
    assert!(matches!(
        error.kind,
        McpErrorKind::Negotiation | McpErrorKind::ServerExited
    ));
}

async fn unallowlisted_parent_environment_is_not_inherited() {
    let status =
        tokio::process::Command::new(std::env::current_exe().expect("integration test executable"))
            .arg(ENV_PARENT)
            .env_clear()
            .env(FORBIDDEN_TEST_ENV, "fixed-test-canary")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("run owned environment parent");
    assert!(status.success(), "environment isolation helper failed");
}

async fn run_environment_parent() {
    let error = fixture_connector()
        .connect(&fixture_config(ENV_PROBE))
        .await
        .expect_err("environment probe exits before MCP negotiation");
    if error.kind != McpErrorKind::ServerExited || error.exit_code != Some(0) {
        std::process::exit(74);
    }
}

async fn launch_is_denied_without_explicit_authorization() {
    let error = McpStdioConnector::default()
        .connect(&fixture_config(MODERN_FIXTURE))
        .await
        .expect_err("default connector must deny process launch");
    assert_eq!(error.kind, McpErrorKind::Config);

    let mut untrusted = fixture_config(MODERN_FIXTURE);
    untrusted.trust = McpTrustLevel::Untrusted;
    let error = fixture_connector()
        .connect(&untrusted)
        .await
        .expect_err("untrusted server must not launch");
    assert_eq!(error.kind, McpErrorKind::Config);
}

async fn invalid_timeout_configuration_is_rejected() {
    let mut invalid_config = fixture_config(MODERN_FIXTURE);
    invalid_config.connect_timeout_ms = u64::MAX;
    let error = fixture_connector()
        .connect(&invalid_config)
        .await
        .expect_err("overflowing timeout must fail before process launch");
    assert_eq!(error.kind, McpErrorKind::Config);

    let client = connect_fixture(MODERN_FIXTURE).await;
    let mut call = McpToolCall::new("echo_text", json!({"text": "not-called"}));
    call.timeout_ms = Some(u64::MAX);
    let error = client
        .call_tool(call, McpCancellationToken::new())
        .await
        .expect_err("overflowing tool timeout must be rejected");
    assert_eq!(error.kind, McpErrorKind::Config);
    client
        .close()
        .await
        .expect("close timeout validation fixture");
}

async fn transport_neutral_connector_api_and_stable_types() {
    fn assert_connector<T: mycopilot_mcp_client::McpConnector>() {}
    fn assert_peer<T: mycopilot_mcp_client::McpPeer>() {}
    assert_connector::<McpStdioConnector>();
    assert_peer::<mycopilot_mcp_client::McpClientHandle>();

    let connector = fixture_connector();
    let peer: Arc<dyn McpPeer> = McpConnector::connect(&connector, &fixture_config(MODERN_FIXTURE))
        .await
        .expect("connect through transport-neutral interface");
    peer.close().await.expect("close transport-neutral peer");

    let serialized = serde_json::to_value(McpToolCall::new("echo_text", json!({})))
        .expect("serialize stable call DTO");
    assert_eq!(serialized["name"], "echo_text");
}

async fn connect_fixture(mode: &str) -> Arc<mycopilot_mcp_client::McpClientHandle> {
    fixture_connector()
        .connect(&fixture_config(mode))
        .await
        .expect("connect owned fixture")
}

fn fixture_connector() -> McpStdioConnector {
    McpStdioConnector::new(fixture_policy())
}

fn fixture_policy() -> McpStdioPolicy {
    McpStdioPolicy {
        allowed_programs: BTreeSet::from([
            std::env::current_exe().expect("integration test executable")
        ]),
        ..McpStdioPolicy::default()
    }
}

fn fixture_config(mode: &str) -> McpServerConfig {
    McpServerConfig {
        id: McpServerId::new(),
        display_name: "repository-owned MCP fixture".to_string(),
        scope: McpServerScope::Builtin,
        trust: McpTrustLevel::Builtin,
        enabled: true,
        transport: McpTransportConfig::Stdio(McpStdioConfig {
            program: std::env::current_exe().expect("integration test executable"),
            arguments: vec![mode.to_string()],
            cwd: std::env::current_dir().expect("integration test cwd"),
            environment: Vec::<McpEnvBinding>::new(),
        }),
        connect_timeout_ms: 3_000,
        request_timeout_ms: 2_000,
        shutdown_timeout_ms: 2_000,
    }
}

async fn call(
    client: &Arc<mycopilot_mcp_client::McpClientHandle>,
    name: &str,
    arguments: Value,
) -> Result<mycopilot_mcp_client::McpToolResult, mycopilot_mcp_client::McpError> {
    client
        .call_tool(
            McpToolCall::new(name, arguments),
            McpCancellationToken::new(),
        )
        .await
}

async fn wait_for_slow_status(
    client: &Arc<mycopilot_mcp_client::McpClientHandle>,
    cancelled: bool,
) {
    for _ in 0..50 {
        let status = call(client, "slow_status", json!({}))
            .await
            .expect("read slow status")
            .structured_content
            .expect("slow status structured content");
        if status["started"] == true && status["cancelled"] == cancelled {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("slow tool did not reach expected cancellation state");
}
