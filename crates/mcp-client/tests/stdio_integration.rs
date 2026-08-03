use std::collections::BTreeSet;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use mycopilot_mcp_client::{
    InMemoryMcpRegistry, McpCancellationToken, McpCatalogCompleteness, McpCatalogIssue,
    McpCatalogToolCall, McpConnectionManager, McpConnectionState, McpConnector, McpContentBlock,
    McpDispatchCertainty, McpDispatchPhase, McpDispatchTracker, McpEnvBinding, McpErrorKind,
    McpEvent, McpLifecycleKind, McpManagerPolicy, McpOutcomeUnknownReason, McpPeer,
    McpPeerNotificationState, McpRegistry, McpServerConfig, McpServerId, McpServerScope,
    McpServerState, McpStdioConfig, McpStdioConnector, McpStdioPolicy, McpToolCall,
    McpTransportConfig, McpTrustLevel,
};
use rmcp::handler::server::{router::tool::ToolRouter, wrapper::Parameters};
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, ListToolsResult, PaginatedRequestParams,
    ProgressNotificationParam, RequestMetaObject, ServerCapabilities, ServerInfo,
    SubscriptionFilter, Tool,
};
use rmcp::service::{RequestContext, SubscriptionContext, SubscriptionSink};
use rmcp::{tool, tool_handler, tool_router, Json, RoleServer, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const MODERN_FIXTURE: &str = "--fixture-modern";
const LEGACY_FIXTURE: &str = "--fixture-legacy";
const PYTHON_LEGACY_FIXTURE: &str = "--fixture-python-legacy-invalid-params";
const PYTHON_LEGACY_BAD_INITIALIZE_FIXTURE: &str = "--fixture-python-legacy-invalid-initialize";
const NON_LEGACY_INVALID_PARAMS_FIXTURE: &str = "--fixture-non-legacy-invalid-params";
const DYNAMIC_MODERN_FIXTURE: &str = "--fixture-dynamic-modern";
const DYNAMIC_LEGACY_FIXTURE: &str = "--fixture-dynamic-legacy";
const PAGED_FIXTURE: &str = "--fixture-paged";
const REPEATED_CURSOR_FIXTURE: &str = "--fixture-repeated-cursor";
const LARGE_CATALOG_FIXTURE: &str = "--fixture-large-catalog";
const OVER_LIMIT_CATALOG_FIXTURE: &str = "--fixture-over-limit-catalog";
const EARLY_EXIT_FIXTURE: &str = "--fixture-early-exit";
const EXIT_AFTER_NEGOTIATION_FIXTURE: &str = "--fixture-exit-after-negotiation";
const STDERR_FIXTURE: &str = "--fixture-stderr-flood";
const STDOUT_FIXTURE: &str = "--fixture-stdout-flood";
const MALFORMED_JSON_FIXTURE: &str = "--fixture-malformed-json";
const INVALID_UTF8_FIXTURE: &str = "--fixture-invalid-utf8";
const UNCOOPERATIVE_FIXTURE: &str = "--fixture-uncooperative-close";
#[cfg(unix)]
const UNCOOPERATIVE_DYNAMIC_FIXTURE: &str = "--fixture-uncooperative-dynamic-close";
#[cfg(unix)]
const TERM_AWARE_FIXTURE: &str = "--fixture-term-aware-close";
const UNRESPONSIVE_FIXTURE: &str = "--fixture-unresponsive-after-negotiation";
const PROTOCOL_EOF_FIXTURE: &str = "--fixture-protocol-eof-alive";
#[cfg(unix)]
const FORKED_DESCENDANT_FIXTURE: &str = "--fixture-forked-descendant";
const ENV_PARENT: &str = "--fixture-env-parent";
const ENV_PROBE: &str = "--fixture-env-probe";
const FORBIDDEN_TEST_ENV: &str = "MYCOPILOT_MCP_FORBIDDEN_TEST_VALUE";
const STRESS_SUITE: &str = "--stress-suite";
const NOTIFICATION_STORM_COUNT: usize = 512;

#[cfg(unix)]
static TERM_RECEIVED: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
static FIXTURE_DESCENDANT_PID: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
extern "C" fn record_sigterm(_signal: libc::c_int) {
    TERM_RECEIVED.store(true, Ordering::SeqCst);
}

#[cfg(unix)]
fn spawn_owned_fixture_descendant() {
    // Install SIGTERM ignore before fork so the post-fork child only performs async-signal-safe
    // syscalls. The leader restores its prior disposition immediately after fork.
    // SAFETY: this runs only inside the isolated repository-owned fixture process.
    let previous = unsafe { libc::signal(libc::SIGTERM, libc::SIG_IGN) };
    assert_ne!(previous, libc::SIG_ERR, "install fixture SIGTERM handler");
    // SAFETY: the child branch calls only close/pause after a multithreaded fork and never returns
    // to Rust or Tokio. It exists solely to exercise process-group containment.
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        // SAFETY: these are the inherited standard descriptors in the isolated fixture child.
        unsafe {
            libc::close(libc::STDIN_FILENO);
            libc::close(libc::STDOUT_FILENO);
            libc::close(libc::STDERR_FILENO);
            loop {
                libc::pause();
            }
        }
    }
    // SAFETY: restore the leader's original disposition before serving MCP.
    unsafe {
        libc::signal(libc::SIGTERM, previous);
    }
    assert!(pid > 0, "fork repository-owned fixture descendant");
    FIXTURE_DESCENDANT_PID.store(pid, Ordering::SeqCst);
}

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

#[derive(Debug, Serialize, schemars::JsonSchema)]
struct FixtureProcessOutput {
    pid: u32,
}

#[cfg(unix)]
#[derive(Debug, Serialize, schemars::JsonSchema)]
struct FixtureDescendantOutput {
    pid: i32,
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
    #[tool(
        name = "echo_text",
        description = "Echo deterministic fixture text",
        annotations(read_only_hint = true)
    )]
    async fn echo_text(&self, Parameters(input): Parameters<EchoInput>) -> String {
        input.text
    }

    #[tool(
        name = "add_numbers",
        description = "Add two fixture integers",
        annotations(read_only_hint = true)
    )]
    async fn add_numbers(&self, Parameters(input): Parameters<AddInput>) -> Json<AddOutput> {
        Json(AddOutput {
            sum: input.left + input.right,
        })
    }

    #[tool(
        name = "structured_result",
        description = "Return deterministic structured fixture data",
        annotations(read_only_hint = true)
    )]
    async fn structured_result(&self) -> Json<StructuredOutput> {
        Json(StructuredOutput {
            status: "ok".to_string(),
            value: 7,
        })
    }

    #[tool(
        name = "fixture_process_id",
        description = "Return the repository-owned fixture process identifier",
        annotations(read_only_hint = true)
    )]
    async fn fixture_process_id(&self) -> Json<FixtureProcessOutput> {
        Json(FixtureProcessOutput {
            pid: std::process::id(),
        })
    }

    #[cfg(unix)]
    #[tool(
        name = "fixture_descendant_pid",
        description = "Return the repository-owned descendant process identifier",
        annotations(read_only_hint = true)
    )]
    async fn fixture_descendant_pid(&self) -> Json<FixtureDescendantOutput> {
        Json(FixtureDescendantOutput {
            pid: FIXTURE_DESCENDANT_PID.load(Ordering::SeqCst),
        })
    }

    #[tool(
        name = "return_tool_error",
        description = "Return a deterministic tool-level error",
        annotations(read_only_hint = true)
    )]
    async fn return_tool_error(&self) -> CallToolResult {
        CallToolResult::error(vec![ContentBlock::text("fixture tool error")])
    }

    #[tool(
        name = "return_protocol_error",
        description = "Return a deterministic JSON-RPC tool error",
        annotations(read_only_hint = true)
    )]
    async fn return_protocol_error(&self) -> Result<String, rmcp::ErrorData> {
        Err(rmcp::ErrorData::internal_error(
            "SERVER_PRIVATE_ERROR_CANARY",
            None,
        ))
    }

    #[tool(
        name = "oversized_text_result",
        description = "Return repository-owned text beyond the Host raw-result budget",
        annotations(read_only_hint = true)
    )]
    async fn oversized_text_result(&self) -> String {
        "x".repeat(4 * 1024 * 1024)
    }

    #[tool(
        name = "oversized_structured_result",
        description = "Return repository-owned structured content beyond the Host budget",
        annotations(read_only_hint = true)
    )]
    async fn oversized_structured_result(&self) -> CallToolResult {
        CallToolResult::structured(json!({"payload": "x".repeat(9 * 1024)}))
    }

    #[tool(
        name = "oversized_image_result",
        description = "Return repository-owned encoded image data beyond the per-block budget",
        annotations(read_only_hint = true)
    )]
    async fn oversized_image_result(&self) -> CallToolResult {
        CallToolResult::success(vec![ContentBlock::image(
            "A".repeat(1024 * 1024 + 4),
            "image/png",
        )])
    }

    #[tool(
        name = "too_many_content_blocks",
        description = "Return more repository-owned content blocks than the Host accepts",
        annotations(read_only_hint = true)
    )]
    async fn too_many_content_blocks(&self) -> CallToolResult {
        CallToolResult::success(
            (0..129)
                .map(|_| ContentBlock::text("owned"))
                .collect::<Vec<_>>(),
        )
    }

    #[tool(
        name = "progress_until_timeout",
        description = "Emit controlled progress without completing before the Host deadline",
        annotations(read_only_hint = true)
    )]
    async fn progress_until_timeout(
        &self,
        Parameters(input): Parameters<SlowInput>,
        meta: RequestMetaObject,
        peer: rmcp::Peer<RoleServer>,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<String, rmcp::ErrorData> {
        let progress_token = meta
            .get_progress_token()
            .ok_or_else(|| rmcp::ErrorData::invalid_params("progress token required", None))?;
        let steps = input.delay_ms.min(5_000).div_ceil(10);
        for step in 0..steps {
            tokio::select! {
                _ = cancellation.cancelled() => return Ok("cancelled".to_string()),
                _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
            let _ = peer
                .notify_progress(
                    ProgressNotificationParam::new(progress_token.clone(), step as f64)
                        .with_total(steps as f64)
                        .with_message("owned fixture progress"),
                )
                .await;
        }
        Ok("completed".to_string())
    }

    #[tool(
        name = "slow_tool",
        description = "Wait for a controlled fixture duration",
        annotations(read_only_hint = true)
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
        description = "Read deterministic in-memory fixture cancellation state",
        annotations(read_only_hint = true)
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
                    for _ in 0..NOTIFICATION_STORM_COUNT {
                        let _ = sink.notify_tool_list_changed().await;
                    }
                }
            } else {
                for _ in 0..NOTIFICATION_STORM_COUNT {
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
            format!("fixture-pid:{}", std::process::id()),
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

#[derive(Clone)]
struct PagedFixtureServer {
    repeat_cursor: bool,
}

impl ServerHandler for PagedFixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("mycopilot-owned-paged-fixture", "1.0.0"),
        )
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let cursor = request.and_then(|request| request.cursor);
        let schema = Arc::new(Default::default());
        match cursor.as_deref() {
            None => Ok(ListToolsResult {
                tools: vec![Tool::new(
                    "page_one_tool",
                    "Deterministic first wire page",
                    Arc::clone(&schema),
                )],
                next_cursor: Some("owned-page-two".to_string()),
                ..Default::default()
            }),
            Some("owned-page-two") => Ok(ListToolsResult {
                tools: vec![Tool::new(
                    "page_two_tool",
                    "Deterministic second wire page",
                    schema,
                )],
                next_cursor: self.repeat_cursor.then(|| "owned-page-two".to_string()),
                ..Default::default()
            }),
            Some(_) => Err(rmcp::ErrorData::invalid_params(
                "unknown repository fixture cursor",
                None,
            )),
        }
    }
}

#[derive(Clone)]
struct LargeCatalogFixtureServer {
    tool_count: usize,
}

impl ServerHandler for LargeCatalogFixtureServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("mycopilot-owned-large-catalog-fixture", "1.0.0"),
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let schema = Arc::new(Default::default());
        let tools = (0..self.tool_count)
            .map(|index| {
                Tool::new(
                    format!("owned_tool_{index:04}"),
                    "Deterministic large-catalog fixture tool",
                    Arc::clone(&schema),
                )
            })
            .collect();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }
}

#[tokio::main]
async fn main() {
    match std::env::args().nth(1).as_deref() {
        Some(MODERN_FIXTURE) => serve_fixture(false).await,
        Some(LEGACY_FIXTURE) => serve_fixture(true).await,
        Some(PYTHON_LEGACY_FIXTURE) => serve_python_legacy_preamble().await,
        Some(PYTHON_LEGACY_BAD_INITIALIZE_FIXTURE) => reject_python_discover_and_initialize().await,
        Some(NON_LEGACY_INVALID_PARAMS_FIXTURE) => {
            let (_stdin, _stdout) = reject_discover(
                -32602,
                "A modern server rejected malformed discovery parameters",
                Some(json!({"reason": "owned non-legacy fixture"})),
            )
            .await;
            std::future::pending::<()>().await;
        }
        Some(DYNAMIC_MODERN_FIXTURE) => serve_dynamic_fixture(false).await,
        Some(DYNAMIC_LEGACY_FIXTURE) => serve_dynamic_fixture(true).await,
        Some(PAGED_FIXTURE) => serve_paged_fixture(false).await,
        Some(REPEATED_CURSOR_FIXTURE) => serve_paged_fixture(true).await,
        Some(LARGE_CATALOG_FIXTURE) => serve_large_catalog_fixture(1024).await,
        Some(OVER_LIMIT_CATALOG_FIXTURE) => serve_large_catalog_fixture(1025).await,
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
        Some(MALFORMED_JSON_FIXTURE) => {
            let mut stdout = tokio::io::stdout();
            stdout
                .write_all(b"not-json\n")
                .await
                .expect("write malformed owned fixture JSON");
            stdout.flush().await.expect("flush malformed fixture JSON");
            std::future::pending::<()>().await;
        }
        Some(INVALID_UTF8_FIXTURE) => {
            let mut stdout = tokio::io::stdout();
            stdout
                .write_all(&[0xff, b'\n'])
                .await
                .expect("write invalid UTF-8 owned fixture frame");
            stdout.flush().await.expect("flush invalid UTF-8 frame");
            std::future::pending::<()>().await;
        }
        Some(UNCOOPERATIVE_FIXTURE) => {
            #[cfg(unix)]
            // SAFETY: this repository-owned child fixture intentionally ignores
            // SIGTERM so the connector's final SIGKILL/reap path is exercised.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
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
        #[cfg(unix)]
        Some(UNCOOPERATIVE_DYNAMIC_FIXTURE) => {
            // SAFETY: this repository-owned child fixture intentionally ignores SIGTERM so close
            // must settle both its active notification reader and final SIGKILL/reap path.
            unsafe {
                libc::signal(libc::SIGTERM, libc::SIG_IGN);
            }
            let service = DynamicFixtureServer::new(true)
                .serve(rmcp::transport::stdio())
                .await
                .expect("start uncooperative dynamic owned fixture");
            service
                .waiting()
                .await
                .expect("wait for uncooperative dynamic fixture transport close");
            std::future::pending::<()>().await;
        }
        #[cfg(unix)]
        Some(TERM_AWARE_FIXTURE) => {
            TERM_RECEIVED.store(false, Ordering::SeqCst);
            // SAFETY: installs a minimal async-signal-safe handler in the
            // isolated repository-owned fixture process.
            unsafe {
                libc::signal(
                    libc::SIGTERM,
                    record_sigterm as *const () as libc::sighandler_t,
                );
            }
            let service = FixtureServer::new()
                .serve(rmcp::transport::stdio())
                .await
                .expect("start TERM-aware owned fixture");
            service
                .waiting()
                .await
                .expect("wait for TERM-aware fixture transport close");
            while !TERM_RECEIVED.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let mut stderr = tokio::io::stderr();
            stderr
                .write_all(b"owned fixture received SIGTERM\n")
                .await
                .expect("write TERM fixture diagnostic");
            stderr.flush().await.expect("flush TERM fixture diagnostic");
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
        #[cfg(unix)]
        Some(FORKED_DESCENDANT_FIXTURE) => {
            spawn_owned_fixture_descendant();
            serve_fixture(false).await;
        }
        Some(ENV_PARENT) => run_environment_parent().await,
        Some(ENV_PROBE) => {
            let present = std::env::var_os(FORBIDDEN_TEST_ENV).is_some();
            std::process::exit(if present { 73 } else { 0 });
        }
        Some(STRESS_SUITE) => run_stress_suite().await,
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
    let (stdin, stdout) = reject_discover(-32601, "Method not found", None).await;
    let service = FixtureServer::new()
        .serve((stdin, stdout))
        .await
        .expect("start owned legacy fixture");
    service.waiting().await.expect("wait for legacy fixture");
}

async fn serve_python_legacy_preamble() {
    let (stdin, stdout) = reject_discover(
        -32602,
        "Invalid request parameters",
        Some(Value::String(String::new())),
    )
    .await;
    let service = FixtureServer::new()
        .serve((stdin, stdout))
        .await
        .expect("start owned Python-legacy fixture");
    service
        .waiting()
        .await
        .expect("wait for Python-legacy fixture");
}

async fn reject_python_discover_and_initialize() {
    let (stdin, mut stdout) = reject_discover(
        -32602,
        "Invalid request parameters",
        Some(Value::String(String::new())),
    )
    .await;
    let mut reader = BufReader::new(stdin);
    let mut initialize_line = String::new();
    reader
        .read_line(&mut initialize_line)
        .await
        .expect("read compatibility initialize request");
    let initialize: Value =
        serde_json::from_str(&initialize_line).expect("parse compatibility initialize request");
    assert_eq!(initialize["method"], "initialize");
    let response = json!({
        "jsonrpc": "2.0",
        "id": initialize["id"],
        "error": {
            "code": -32602,
            "message": "Invalid request parameters",
            "data": ""
        }
    });
    stdout
        .write_all(serde_json::to_string(&response).unwrap().as_bytes())
        .await
        .expect("write initialize rejection");
    stdout
        .write_all(b"\n")
        .await
        .expect("terminate initialize rejection");
    stdout.flush().await.expect("flush initialize rejection");
    std::future::pending::<()>().await;
}

async fn serve_dynamic_fixture(legacy: bool) {
    let server = DynamicFixtureServer::new(!legacy);
    let service = if legacy {
        let (stdin, stdout) = reject_discover(-32601, "Method not found", None).await;
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

async fn serve_paged_fixture(repeat_cursor: bool) {
    let service = PagedFixtureServer { repeat_cursor }
        .serve(rmcp::transport::stdio())
        .await
        .expect("start owned paged fixture");
    service.waiting().await.expect("wait for paged fixture");
}

async fn serve_large_catalog_fixture(tool_count: usize) {
    let service = LargeCatalogFixtureServer { tool_count }
        .serve(rmcp::transport::stdio())
        .await
        .expect("start owned large-catalog fixture");
    service
        .waiting()
        .await
        .expect("wait for large-catalog fixture");
}

async fn reject_discover(
    code: i64,
    message: &str,
    data: Option<Value>,
) -> (tokio::io::Stdin, tokio::io::Stdout) {
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
            "code": code,
            "message": message,
            "data": data
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
    json_rpc_tool_error_is_an_authoritative_response().await;
    direct_stdio_result_limit_is_enforced().await;
    direct_stdio_structured_media_and_block_limits_are_enforced().await;
    manager_preserves_authoritative_json_rpc_error_certainty().await;
    manager_rejects_a_real_stdio_result_beyond_the_raw_byte_limit().await;
    progress_never_extends_the_host_deadline().await;
    legacy_initialize_fallback_and_tool_call().await;
    python_sdk_invalid_params_discover_falls_back_once().await;
    initialize_invalid_params_after_compatibility_fallback_is_terminal().await;
    unrelated_invalid_params_does_not_downgrade_lifecycle().await;
    real_stdio_tool_pagination_and_repeated_cursor_protection().await;
    modern_and_legacy_dynamic_notifications_share_one_signal_api().await;
    manager_debounces_dynamic_tool_refresh().await;
    manager_emits_safe_owned_server_exit_event().await;
    timeout_sends_protocol_cancellation().await;
    cancellation_before_dispatch_is_definitely_not_dispatched().await;
    explicit_cancellation_reaches_server().await;
    cancellation_stays_bounded_under_transport_backpressure().await;
    early_server_exit_is_structured().await;
    post_negotiation_exit_is_reaped_and_changes_state().await;
    #[cfg(unix)]
    protocol_eof_is_reported_while_the_child_remains_alive().await;
    close_reaps_server_process().await;
    concurrent_close_callers_share_one_cleanup_result().await;
    forced_close_terminates_uncooperative_owned_fixture().await;
    #[cfg(unix)]
    dropped_handle_force_kills_and_reaps_uncooperative_owned_fixture().await;
    #[cfg(unix)]
    close_settles_notification_and_reader_tasks_before_reporting_closed().await;
    #[cfg(unix)]
    graceful_close_uses_term_before_kill().await;
    #[cfg(unix)]
    normal_shutdown_terminates_forked_descendant().await;
    #[cfg(unix)]
    exited_leader_still_terminates_forked_descendant().await;
    manager_shutdown_force_reaps_uncooperative_owned_fixture().await;
    stderr_is_continuously_drained_and_bounded().await;
    oversized_protocol_line_is_rejected().await;
    malformed_and_non_utf8_protocol_frames_are_rejected().await;
    unallowlisted_parent_environment_is_not_inherited().await;
    shell_looking_arguments_remain_literal_argv().await;
    launch_is_denied_without_explicit_authorization().await;
    invalid_timeout_configuration_is_rejected().await;
    transport_neutral_connector_api_and_stable_types().await;
}

async fn run_stress_suite() {
    let started = tokio::time::Instant::now();
    stdio_start_stop_100_cycles().await;
    manager_restarts_and_concurrent_refresh_remain_bounded().await;
    stop_and_remove_during_real_calls_settle_without_replay().await;
    crashed_server_can_be_reconfigured_and_restarted().await;
    large_real_catalog_honors_the_1024_tool_boundary().await;
    eprintln!(
        "owned MCP stress suite completed in {} ms",
        started.elapsed().as_millis()
    );
}

async fn stdio_start_stop_100_cycles() {
    for cycle in 0..100 {
        let client = connect_fixture(MODERN_FIXTURE).await;
        #[cfg(unix)]
        let pid = fixture_process_id(&client).await;
        client
            .close()
            .await
            .unwrap_or_else(|error| panic!("close owned cycle {cycle}: {error}"));
        assert_eq!(client.connection_state(), McpConnectionState::Closed);
        #[cfg(unix)]
        assert_child_was_reaped(pid);
    }
}

async fn manager_restarts_and_concurrent_refresh_remain_bounded() {
    let registry = InMemoryMcpRegistry::shared();
    let primary = fixture_config(MODERN_FIXTURE);
    let primary_id = primary.id;
    registry.add(primary).expect("register restart fixture");
    let manager = McpConnectionManager::without_events(
        Arc::clone(&registry) as Arc<dyn McpRegistry>,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct restart fixture manager");
    manager
        .start(primary_id)
        .await
        .expect("start restart fixture");
    for iteration in 0..25 {
        let status = manager
            .restart(primary_id)
            .await
            .unwrap_or_else(|error| panic!("restart iteration {iteration}: {error}"));
        assert_eq!(status.state, McpServerState::Ready);
        assert_eq!(status.active_call_count, 0);
    }
    manager.stop_all().await;

    let registry = InMemoryMcpRegistry::shared();
    let mut ids = Vec::new();
    for index in 0..8 {
        let mut config = fixture_config(MODERN_FIXTURE);
        config.display_name = format!("owned concurrent fixture {index}");
        ids.push(config.id);
        registry.add(config).expect("register concurrent fixture");
    }
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct concurrent fixture manager");
    let results = manager.start_enabled().await;
    assert_eq!(results.len(), ids.len());
    assert!(results.iter().all(|result| {
        result.error.is_none()
            && result
                .status
                .as_ref()
                .is_some_and(|status| status.state == McpServerState::Ready)
    }));
    let mut refreshes = tokio::task::JoinSet::new();
    for server_id in ids.iter().copied() {
        let manager = manager.clone();
        refreshes.spawn(async move { manager.refresh(server_id).await });
    }
    while let Some(result) = refreshes.join_next().await {
        result
            .expect("join concurrent real refresh")
            .expect("refresh concurrent real fixture");
    }
    let stopped = manager.stop_all().await;
    assert_eq!(stopped.len(), ids.len());
    assert_eq!(manager.active_call_count().unwrap(), 0);
    for server_id in ids {
        assert_eq!(
            manager.get_status(server_id).unwrap().unwrap().state,
            McpServerState::Disabled
        );
    }
}

async fn stop_and_remove_during_real_calls_settle_without_replay() {
    for remove in [false, true] {
        let registry = InMemoryMcpRegistry::shared();
        let server = fixture_config(MODERN_FIXTURE);
        let server_id = server.id;
        registry.add(server).expect("register active-call fixture");
        let manager = McpConnectionManager::without_events(
            registry,
            Arc::new(fixture_connector()),
            McpManagerPolicy::default(),
        )
        .expect("construct active-call fixture manager");
        manager
            .start(server_id)
            .await
            .expect("start active-call fixture");
        #[cfg(unix)]
        let fixture_pid = manager
            .call_catalog_tool(
                catalog_call_for_manager(
                    &manager,
                    server_id,
                    "fixture_process_id",
                    json!({}),
                    None,
                ),
                McpCancellationToken::new(),
            )
            .await
            .expect("read owned active-call fixture pid")
            .structured_content
            .and_then(|value| value.get("pid").and_then(Value::as_u64))
            .and_then(|pid| libc::pid_t::try_from(pid).ok())
            .expect("owned fixture returns a valid pid");
        let call = catalog_call_for_manager(
            &manager,
            server_id,
            "slow_tool",
            json!({"delay_ms": 5_000}),
            Some(5_000),
        );
        let call_manager = manager.clone();
        let task = tokio::spawn(async move {
            call_manager
                .call_catalog_tool(call, McpCancellationToken::new())
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if manager.active_call_count().unwrap() == 1 {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("real stdio call must enter active registry");

        if remove {
            assert!(manager
                .remove_server(server_id)
                .await
                .expect("remove active real fixture")
                .is_some());
        } else {
            manager
                .stop(server_id)
                .await
                .expect("stop active real fixture");
        }
        let error = task
            .await
            .expect("join interrupted real call")
            .expect_err("interrupted dispatched call must not be replayed");
        assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::PossiblyDispatched)
        );
        assert_eq!(manager.active_call_count().unwrap(), 0);
        #[cfg(unix)]
        assert_child_was_reaped(fixture_pid);
        if !remove {
            manager.stop_all().await;
        }
    }
}

async fn crashed_server_can_be_reconfigured_and_restarted() {
    let registry = InMemoryMcpRegistry::shared();
    let crashing = fixture_config(EXIT_AFTER_NEGOTIATION_FIXTURE);
    let server_id = crashing.id;
    registry
        .add(crashing.clone())
        .expect("register crashing fixture");
    let manager = McpConnectionManager::without_events(
        Arc::clone(&registry) as Arc<dyn McpRegistry>,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct crashing fixture manager");
    manager
        .start(server_id)
        .await
        .expect("start crashing fixture");
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if manager
                .get_status(server_id)
                .unwrap()
                .is_some_and(|status| status.state == McpServerState::Error)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("manager must observe fixture crash");

    let mut recovered = crashing;
    let McpTransportConfig::Stdio(stdio) = &mut recovered.transport else {
        panic!("owned fixture must use stdio");
    };
    stdio.arguments = vec![MODERN_FIXTURE.to_string()];
    registry
        .upsert(recovered)
        .expect("replace crashed fixture launch spec");
    let status = manager
        .restart(server_id)
        .await
        .expect("restart reconfigured owned fixture");
    assert_eq!(status.state, McpServerState::Ready);
    manager.stop_all().await;
}

async fn large_real_catalog_honors_the_1024_tool_boundary() {
    let registry = InMemoryMcpRegistry::shared();
    let accepted = fixture_config(LARGE_CATALOG_FIXTURE);
    let accepted_id = accepted.id;
    registry.add(accepted).expect("register 1024-tool fixture");
    let rejected = fixture_config(OVER_LIMIT_CATALOG_FIXTURE);
    let rejected_id = rejected.id;
    registry.add(rejected).expect("register 1025-tool fixture");
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct large-catalog fixture manager");
    manager.start_enabled().await;

    let accepted = manager.catalog(accepted_id).unwrap().unwrap();
    assert_eq!(accepted.completeness, McpCatalogCompleteness::Complete);
    assert_eq!(accepted.tools.len(), 1024);
    let rejected = manager.catalog(rejected_id).unwrap().unwrap();
    assert_eq!(
        rejected.completeness,
        McpCatalogCompleteness::Failed(McpCatalogIssue::ToolLimitExceeded)
    );
    assert!(rejected.resolve_model_name("mcp__anything").is_none());
    manager.stop_all().await;
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
        "return_protocol_error",
        "oversized_text_result",
        "progress_until_timeout",
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

async fn json_rpc_tool_error_is_an_authoritative_response() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let dispatch = McpDispatchTracker::new();
    let error = client
        .call_tool_tracked(
            McpToolCall::new("return_protocol_error", json!({})),
            McpCancellationToken::new(),
            dispatch.clone(),
        )
        .await
        .expect_err("fixture must return a JSON-RPC error response");
    assert_eq!(error.kind, McpErrorKind::Protocol);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::ResponseReceived)
    );
    assert_eq!(error.outcome_unknown_reason, None);
    assert_eq!(dispatch.phase(), McpDispatchPhase::ResponseReceived);
    assert!(
        !error.message.contains("SERVER_PRIVATE_ERROR_CANARY"),
        "untrusted JSON-RPC error text must not cross the safe error boundary"
    );
    client.close().await.expect("close protocol-error fixture");
}

async fn manager_preserves_authoritative_json_rpc_error_certainty() {
    let registry = InMemoryMcpRegistry::shared();
    let server = fixture_config(MODERN_FIXTURE);
    let server_id = server.id;
    registry
        .add(server)
        .expect("register protocol-error fixture");
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct protocol-error fixture manager");
    manager
        .start(server_id)
        .await
        .expect("start protocol-error fixture");
    let error = manager
        .call_catalog_tool(
            catalog_call_for_manager(
                &manager,
                server_id,
                "return_protocol_error",
                json!({}),
                None,
            ),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("manager must surface the authoritative JSON-RPC error");
    assert_eq!(error.kind, McpErrorKind::Protocol);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::ResponseReceived)
    );
    assert_eq!(error.outcome_unknown_reason, None);
    assert_eq!(manager.active_call_count().unwrap(), 0);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Ready
    );
    manager.stop_all().await;
}

async fn direct_stdio_result_limit_is_enforced() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let error = call(&client, "oversized_text_result", json!({}))
        .await
        .expect_err("low-level stdio client must enforce the Host result budget");
    assert_eq!(error.kind, McpErrorKind::OutputTooLarge);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::ResponseReceived)
    );
    client
        .close()
        .await
        .expect("close oversized direct fixture");
}

async fn direct_stdio_structured_media_and_block_limits_are_enforced() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    for tool_name in [
        "oversized_structured_result",
        "oversized_image_result",
        "too_many_content_blocks",
    ] {
        let error = match call(&client, tool_name, json!({})).await {
            Ok(_) => panic!("owned {tool_name} must exceed its Host result budget"),
            Err(error) => error,
        };
        assert_eq!(error.kind, McpErrorKind::OutputTooLarge);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::ResponseReceived)
        );
    }
    client
        .close()
        .await
        .expect("close structured/media/block-limit fixture");
}

async fn manager_rejects_a_real_stdio_result_beyond_the_raw_byte_limit() {
    let registry = InMemoryMcpRegistry::shared();
    let server = fixture_config(MODERN_FIXTURE);
    let server_id = server.id;
    registry.add(server).expect("register oversized fixture");
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct oversized-result manager");
    manager
        .start(server_id)
        .await
        .expect("start oversized-result fixture");
    let error = manager
        .call_catalog_tool(
            catalog_call_for_manager(
                &manager,
                server_id,
                "oversized_text_result",
                json!({}),
                None,
            ),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("Host must reject a real result beyond the raw-result budget");
    assert_eq!(error.kind, McpErrorKind::OutputTooLarge);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::ResponseReceived)
    );
    assert_eq!(manager.active_call_count().unwrap(), 0);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Ready
    );
    manager.stop_all().await;
}

async fn progress_never_extends_the_host_deadline() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let mut progress = McpToolCall::new("progress_until_timeout", json!({"delay_ms": 1_000}));
    progress.timeout_ms = Some(75);
    let started = tokio::time::Instant::now();
    let error = client
        .call_tool(progress, McpCancellationToken::new())
        .await
        .expect_err("progress must not keep a tool call alive past its Host deadline");
    let elapsed = started.elapsed();
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.outcome_unknown_reason,
        Some(McpOutcomeUnknownReason::TimedOut)
    );
    assert!(
        elapsed < Duration::from_millis(500),
        "progress unexpectedly extended the hard deadline: {elapsed:?}"
    );
    client.close().await.expect("close progress fixture");
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

async fn python_sdk_invalid_params_discover_falls_back_once() {
    let client = connect_fixture(PYTHON_LEGACY_FIXTURE).await;
    let snapshot = client.protocol_snapshot();
    assert_eq!(snapshot.negotiated_version, "2025-11-25");
    assert_eq!(snapshot.lifecycle, McpLifecycleKind::InitializeFallback);

    let page = client
        .list_tools(None)
        .await
        .expect("list tools after Python SDK compatibility fallback");
    assert!(page.tools.iter().any(|tool| tool.name == "echo_text"));
    let echo = call(&client, "echo_text", json!({"text": "python-legacy"}))
        .await
        .expect("call tool after Python SDK compatibility fallback");
    assert_eq!(
        echo.content,
        vec![McpContentBlock::Text {
            text: "python-legacy".to_string()
        }]
    );
    client.close().await.expect("close Python-legacy fixture");
}

async fn unrelated_invalid_params_does_not_downgrade_lifecycle() {
    let error = fixture_connector()
        .connect(&fixture_config(NON_LEGACY_INVALID_PARAMS_FIXTURE))
        .await
        .expect_err("an unrelated INVALID_PARAMS response must not trigger legacy fallback");
    assert!(
        matches!(
            error.kind,
            McpErrorKind::Negotiation | McpErrorKind::ServerExited
        ),
        "unexpected safe failure kind: {:?}",
        error.kind
    );
}

async fn initialize_invalid_params_after_compatibility_fallback_is_terminal() {
    let error = fixture_connector()
        .connect(&fixture_config(PYTHON_LEGACY_BAD_INITIALIZE_FIXTURE))
        .await
        .expect_err("initialize INVALID_PARAMS must remain a terminal negotiation failure");
    assert!(
        matches!(
            error.kind,
            McpErrorKind::Negotiation | McpErrorKind::ServerExited
        ),
        "unexpected safe failure kind: {:?}",
        error.kind
    );
}

async fn real_stdio_tool_pagination_and_repeated_cursor_protection() {
    let client = connect_fixture(PAGED_FIXTURE).await;
    let first = client
        .list_tools(None)
        .await
        .expect("read first real stdio tool page");
    assert_eq!(first.tools[0].name, "page_one_tool");
    assert_eq!(first.next_cursor.as_deref(), Some("owned-page-two"));
    let second = client
        .list_tools(first.next_cursor)
        .await
        .expect("read second real stdio tool page");
    assert_eq!(second.tools[0].name, "page_two_tool");
    assert_eq!(second.next_cursor, None);
    client.close().await.expect("close paged fixture client");

    let registry = InMemoryMcpRegistry::shared();
    let paged = fixture_config(PAGED_FIXTURE);
    let paged_id = paged.id;
    registry.add(paged).expect("register paged fixture");
    let repeated = fixture_config(REPEATED_CURSOR_FIXTURE);
    let repeated_id = repeated.id;
    registry
        .add(repeated)
        .expect("register repeated-cursor fixture");
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct real stdio pagination manager");
    let results = manager.start_enabled().await;
    assert_eq!(results.len(), 2);
    let complete = manager.catalog(paged_id).unwrap().unwrap();
    assert_eq!(complete.completeness, McpCatalogCompleteness::Complete);
    assert_eq!(complete.page_count, 2);
    assert_eq!(complete.tools.len(), 2);
    let repeated = manager.catalog(repeated_id).unwrap().unwrap();
    assert_eq!(
        repeated.completeness,
        McpCatalogCompleteness::Partial(McpCatalogIssue::RepeatedCursor)
    );
    assert!(
        repeated
            .tools
            .iter()
            .all(|tool| repeated.resolve_model_name(&tool.model_name).is_none()),
        "partial wire catalogs must fail closed at the routing boundary"
    );
    manager.stop_all().await;
}

async fn timeout_sends_protocol_cancellation() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let mut slow = McpToolCall::new("slow_tool", json!({"delay_ms": 2_000}));
    slow.timeout_ms = Some(40);
    let error = client
        .call_tool(slow, McpCancellationToken::new())
        .await
        .expect_err("slow tool should time out");
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert_eq!(
        error.outcome_unknown_reason,
        Some(McpOutcomeUnknownReason::TimedOut)
    );
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
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert_eq!(
        error.outcome_unknown_reason,
        Some(McpOutcomeUnknownReason::Cancelled)
    );
    wait_for_slow_status(&client, true).await;
    client.close().await.expect("close cancellation fixture");
}

async fn cancellation_before_dispatch_is_definitely_not_dispatched() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    let cancellation = McpCancellationToken::new();
    cancellation.cancel();
    let dispatch = McpDispatchTracker::new();
    let error = client
        .call_tool_tracked(
            McpToolCall::new("slow_tool", json!({"delay_ms": 2_000})),
            cancellation,
            dispatch.clone(),
        )
        .await
        .expect_err("a pre-cancelled call must never be dispatched");
    assert_eq!(error.kind, McpErrorKind::Cancelled);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(dispatch.phase(), McpDispatchPhase::NotStarted);
    let status = call(&client, "slow_status", json!({}))
        .await
        .expect("read fixture status after pre-dispatch cancellation");
    let status = status
        .structured_content
        .expect("fixture status is structured");
    assert_eq!(status["started"], false);
    assert_eq!(status["cancelled"], false);
    client.close().await.expect("close pre-cancelled fixture");
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
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert_eq!(
        error.outcome_unknown_reason,
        Some(McpOutcomeUnknownReason::Cancelled)
    );
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
    #[cfg(unix)]
    let fixture_pid = fixture_process_id(&client).await;
    assert_eq!(client.connection_state(), McpConnectionState::Ready);
    client.close().await.expect("close and reap fixture");
    assert_eq!(client.connection_state(), McpConnectionState::Closed);
    client.close().await.expect("close is idempotent");
    #[cfg(unix)]
    assert_child_was_reaped(fixture_pid);
}

async fn concurrent_close_callers_share_one_cleanup_result() {
    let client = connect_fixture(MODERN_FIXTURE).await;
    #[cfg(unix)]
    let fixture_pid = fixture_process_id(&client).await;
    let first = Arc::clone(&client);
    let second = Arc::clone(&client);
    let (first_result, second_result) = tokio::join!(first.close(), second.close());
    assert_eq!(first_result, second_result);
    first_result.expect("concurrent close callers share successful cleanup");
    assert_eq!(client.connection_state(), McpConnectionState::Closed);
    #[cfg(unix)]
    assert_child_was_reaped(fixture_pid);
}

async fn forced_close_terminates_uncooperative_owned_fixture() {
    let mut config = fixture_config(UNCOOPERATIVE_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect uncooperative owned fixture");
    #[cfg(unix)]
    let fixture_pid = fixture_process_id(&client).await;
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
    #[cfg(unix)]
    assert_child_was_reaped(fixture_pid);
}

#[cfg(unix)]
async fn dropped_handle_force_kills_and_reaps_uncooperative_owned_fixture() {
    let mut config = fixture_config(UNCOOPERATIVE_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect dropped uncooperative owned fixture");
    let fixture_pid = fixture_process_id(&client).await;

    drop(client);

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // SAFETY: signal 0 performs existence/permission checking without delivering a signal.
            let result = unsafe { libc::kill(fixture_pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("detached stdio supervisor must force-kill and reap its owned child");
    assert_child_was_reaped(fixture_pid);
}

#[cfg(unix)]
async fn close_settles_notification_and_reader_tasks_before_reporting_closed() {
    let mut config = fixture_config(UNCOOPERATIVE_DYNAMIC_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect uncooperative dynamic owned fixture");
    assert_eq!(
        client.subscribe_signals().snapshot().notification_state,
        McpPeerNotificationState::Active
    );
    let fixture_pid = client
        .protocol_snapshot()
        .server
        .as_ref()
        .and_then(|server| server.version.strip_prefix("fixture-pid:"))
        .and_then(|pid| pid.parse::<libc::pid_t>().ok())
        .expect("dynamic fixture version must carry its owned child pid");

    client
        .close()
        .await
        .expect("close must settle auxiliary tasks and reap the uncooperative child");
    assert_eq!(client.connection_state(), McpConnectionState::Closed);
    assert_child_was_reaped(fixture_pid);
}

#[cfg(unix)]
async fn graceful_close_uses_term_before_kill() {
    let mut config = fixture_config(TERM_AWARE_FIXTURE);
    config.shutdown_timeout_ms = 100;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect TERM-aware owned fixture");
    client
        .close()
        .await
        .expect("TERM-aware fixture should be reaped cleanly");
    let snapshot = client.stderr_snapshot().await;
    assert!(snapshot.retained.contains("[mcp stderr omitted]"));
    assert!(!snapshot.retained.contains("owned fixture received SIGTERM"));
}

#[cfg(unix)]
async fn normal_shutdown_terminates_forked_descendant() {
    let mut config = fixture_config(FORKED_DESCENDANT_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect repository-owned forked-descendant fixture");
    let leader_pid = fixture_process_id(&client).await;
    let descendant_pid = fixture_descendant_pid(&client).await;
    assert_fixture_process_group(leader_pid, descendant_pid);

    client
        .close()
        .await
        .expect("normal shutdown must terminate descendant and reap leader");

    assert_child_was_reaped(leader_pid);
    wait_for_process_to_disappear(descendant_pid).await;
}

#[cfg(unix)]
async fn exited_leader_still_terminates_forked_descendant() {
    let mut config = fixture_config(FORKED_DESCENDANT_FIXTURE);
    config.shutdown_timeout_ms = 150;
    let client = fixture_connector()
        .connect(&config)
        .await
        .expect("connect leader-exit forked-descendant fixture");
    let leader_pid = fixture_process_id(&client).await;
    let descendant_pid = fixture_descendant_pid(&client).await;
    assert_fixture_process_group(leader_pid, descendant_pid);

    // Kill only the leader. The TERM-ignoring descendant must subsequently be found and killed via
    // the immutable spawn-time process-group identity.
    // SAFETY: leader_pid belongs to this repository-owned fixture.
    assert_eq!(unsafe { libc::kill(leader_pid, libc::SIGTERM) }, 0);
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if client.connection_state() == McpConnectionState::Failed {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("leader exit must be observed");

    wait_for_process_to_disappear(descendant_pid).await;
    let _ = client.close().await;
    assert_child_was_reaped(leader_pid);
}

async fn manager_shutdown_force_reaps_uncooperative_owned_fixture() {
    let registry = InMemoryMcpRegistry::shared();
    let mut config = fixture_config(UNCOOPERATIVE_FIXTURE);
    config.shutdown_timeout_ms = 2_000;
    let server_id = config.id;
    registry
        .add(config)
        .expect("register uncooperative owned fixture");
    let manager = McpConnectionManager::without_events(
        registry,
        Arc::new(fixture_connector()),
        McpManagerPolicy::default(),
    )
    .expect("construct uncooperative fixture manager");
    manager
        .start(server_id)
        .await
        .expect("start uncooperative fixture through manager");

    let report = tokio::time::timeout(
        Duration::from_secs(2),
        manager.shutdown(Duration::from_millis(300)),
    )
    .await
    .expect("manager shutdown must remain Host-deadline bounded");
    assert!(report.forced);
    assert!(
        report.cleanup_complete,
        "force token must let the stdio supervisor terminate and reap its live child"
    );
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        mycopilot_mcp_client::McpServerState::Disabled
    );
    assert_eq!(
        manager
            .start(server_id)
            .await
            .expect_err("permanently shut down manager cannot restart a fixture")
            .kind,
        McpErrorKind::Shutdown
    );
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

async fn malformed_and_non_utf8_protocol_frames_are_rejected() {
    for mode in [MALFORMED_JSON_FIXTURE, INVALID_UTF8_FIXTURE] {
        let mut config = fixture_config(mode);
        config.connect_timeout_ms = 500;
        config.shutdown_timeout_ms = 150;
        let error =
            tokio::time::timeout(Duration::from_secs(2), fixture_connector().connect(&config))
                .await
                .expect("malformed owned fixture rejection must remain bounded")
                .expect_err("malformed protocol frame must fail negotiation");
        assert!(
            matches!(
                error.kind,
                McpErrorKind::Negotiation | McpErrorKind::ServerExited | McpErrorKind::Timeout
            ),
            "unexpected error for {mode}: {:?}",
            error.kind
        );
    }
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

async fn shell_looking_arguments_remain_literal_argv() {
    let sentinel = std::env::temp_dir().join(format!(
        "mycopilot-mcp-owned-shell-sentinel-{}",
        uuid::Uuid::new_v4()
    ));
    let mut config = fixture_config(EARLY_EXIT_FIXTURE);
    config.transport = McpTransportConfig::Stdio(McpStdioConfig {
        program: std::env::current_exe().expect("resolve repository-owned test executable"),
        arguments: vec![
            EARLY_EXIT_FIXTURE.to_string(),
            String::new(),
            format!("; touch {}", sentinel.display()),
            format!("$(touch {})", sentinel.display()),
        ],
        cwd: std::env::current_dir().expect("resolve repository-owned test cwd"),
        environment: Vec::new(),
    });
    fixture_connector()
        .connect(&config)
        .await
        .expect_err("owned early-exit fixture does not negotiate");
    let created = sentinel.exists();
    if created {
        let _ = std::fs::remove_file(&sentinel);
    }
    assert!(
        !created,
        "stdio argv must never be interpreted by a command shell"
    );
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
        approval_mode: mycopilot_mcp_client::McpApprovalMode::Prompt,
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

fn catalog_call_for_manager(
    manager: &McpConnectionManager,
    server_id: McpServerId,
    raw_name: &str,
    arguments: Value,
    timeout_ms: Option<u64>,
) -> McpCatalogToolCall {
    let status = manager
        .get_status(server_id)
        .expect("read real fixture status")
        .expect("real fixture status");
    let catalog = manager
        .catalog(server_id)
        .expect("read real fixture catalog")
        .expect("real fixture catalog");
    let tool = catalog
        .tools
        .iter()
        .find(|tool| tool.raw_name == raw_name)
        .unwrap_or_else(|| panic!("real fixture catalog is missing {raw_name}"));
    McpCatalogToolCall {
        tool_id: tool.id.clone(),
        expected_config_epoch: status.config_epoch,
        expected_registry_revision: status.registry_revision,
        expected_config_digest: status.config_digest,
        expected_catalog_generation: catalog.generation,
        expected_catalog_digest: catalog
            .content_digest
            .expect("complete real fixture catalog digest"),
        expected_schema_digest: tool.schema_digest.clone(),
        expected_model_name: tool.model_name.clone(),
        arguments,
        timeout_ms,
    }
}

#[cfg(unix)]
async fn fixture_process_id(client: &Arc<mycopilot_mcp_client::McpClientHandle>) -> libc::pid_t {
    let result = call(client, "fixture_process_id", json!({}))
        .await
        .expect("read repository-owned fixture process identifier");
    let pid = result
        .structured_content
        .as_ref()
        .and_then(|content| content.get("pid"))
        .and_then(Value::as_u64)
        .and_then(|pid| libc::pid_t::try_from(pid).ok())
        .expect("fixture process identifier must fit pid_t");
    assert!(pid > 0);
    pid
}

#[cfg(unix)]
async fn fixture_descendant_pid(
    client: &Arc<mycopilot_mcp_client::McpClientHandle>,
) -> libc::pid_t {
    let result = call(client, "fixture_descendant_pid", json!({}))
        .await
        .expect("read repository-owned fixture descendant identifier");
    let pid = result
        .structured_content
        .as_ref()
        .and_then(|content| content.get("pid"))
        .and_then(Value::as_i64)
        .and_then(|pid| libc::pid_t::try_from(pid).ok())
        .expect("fixture descendant identifier must fit pid_t");
    assert!(pid > 0);
    pid
}

#[cfg(unix)]
fn assert_fixture_process_group(leader_pid: libc::pid_t, descendant_pid: libc::pid_t) {
    // SAFETY: both identifiers came from the repository-owned fixture.
    let descendant_group = unsafe { libc::getpgid(descendant_pid) };
    assert_eq!(
        descendant_group, leader_pid,
        "fixture descendant must inherit the isolated leader process group"
    );
}

#[cfg(unix)]
async fn wait_for_process_to_disappear(pid: libc::pid_t) {
    let disappeared = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            // SAFETY: signal 0 only probes the repository-owned fixture process.
            let result = unsafe { libc::kill(pid, 0) };
            if result == -1 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    if disappeared.is_err() {
        // Best-effort test cleanup before reporting the containment failure.
        // SAFETY: pid belongs to the repository-owned descendant.
        unsafe {
            libc::kill(pid, libc::SIGKILL);
        }
        panic!("repository-owned fixture descendant remained after MCP cleanup");
    }
}

#[cfg(unix)]
fn assert_child_was_reaped(pid: libc::pid_t) {
    let mut status = 0;
    // SAFETY: `pid` came from the repository-owned direct child. WNOHANG never blocks and the
    // status pointer is valid for the duration of this call.
    let result = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    assert_eq!(
        result, -1,
        "the stdio supervisor must reap the child before close returns"
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ECHILD),
        "a reaped direct child must no longer be waitable by the host"
    );
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
