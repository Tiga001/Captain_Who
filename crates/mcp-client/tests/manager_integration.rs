use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use mycopilot_mcp_client::{
    BoxMcpFuture, InMemoryMcpRegistry, McpBatchOperationResult, McpCancellationToken,
    McpCapabilitySnapshot, McpCatalogCompleteness, McpCatalogDiagnosticKind, McpCatalogDigest,
    McpCatalogIssue, McpCatalogPolicy, McpCatalogToolCall, McpConnectionManager,
    McpConnectionState, McpConnector, McpContentBlock, McpEnvBinding, McpError, McpErrorKind,
    McpEvent, McpEventSink, McpImplementationInfo, McpLifecycleKind, McpManagerPolicy, McpPeer,
    McpProtocolSnapshot, McpRegistry, McpServerConfig, McpServerId, McpServerScope, McpServerState,
    McpStdioConfig, McpToolCall, McpToolDescriptor, McpToolPage, McpToolResult, McpTransportConfig,
    McpTrustLevel,
};
use serde_json::json;

const SECRET_SENTINEL: &str = "ROUND2_SECRET_MUST_NOT_ESCAPE";

type PageScript = BTreeMap<Option<String>, Result<McpToolPage, McpError>>;

struct MockServer {
    pages: RwLock<PageScript>,
    connect_count: AtomicUsize,
    close_count: AtomicUsize,
    list_count: AtomicUsize,
    connect_delay_ms: AtomicU64,
    list_delay_ms: AtomicU64,
    close_delay_ms: AtomicU64,
    connect_error: Mutex<Option<McpError>>,
    calls: Mutex<Vec<McpToolCall>>,
    wait_for_call_cancellation: AtomicBool,
    registry_at_close: Mutex<Option<Arc<InMemoryMcpRegistry>>>,
    close_saw_registered: AtomicBool,
}

impl MockServer {
    fn with_tools(tools: Vec<McpToolDescriptor>) -> Arc<Self> {
        Arc::new(Self {
            pages: RwLock::new(single_page(tools)),
            connect_count: AtomicUsize::new(0),
            close_count: AtomicUsize::new(0),
            list_count: AtomicUsize::new(0),
            connect_delay_ms: AtomicU64::new(0),
            list_delay_ms: AtomicU64::new(0),
            close_delay_ms: AtomicU64::new(0),
            connect_error: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            wait_for_call_cancellation: AtomicBool::new(false),
            registry_at_close: Mutex::new(None),
            close_saw_registered: AtomicBool::new(false),
        })
    }

    fn set_pages(&self, pages: PageScript) {
        *self.pages.write().expect("mock pages write lock") = pages;
    }

    fn set_connect_error(&self, error: McpError) {
        *self.connect_error.lock().expect("mock connect error lock") = Some(error);
    }

    fn calls(&self) -> Vec<McpToolCall> {
        self.calls.lock().expect("mock calls lock").clone()
    }
}

#[derive(Default)]
struct MockConnector {
    servers: Mutex<HashMap<McpServerId, Arc<MockServer>>>,
}

impl MockConnector {
    fn add(&self, server_id: McpServerId, server: Arc<MockServer>) {
        self.servers
            .lock()
            .expect("mock connector lock")
            .insert(server_id, server);
    }
}

impl McpConnector for MockConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        let server_id = config.id;
        Box::pin(async move {
            let server = self
                .servers
                .lock()
                .map_err(|_| McpError::protocol("mock connector lock unavailable"))?
                .get(&server_id)
                .cloned()
                .ok_or_else(|| McpError::spawn("owned mock server is not configured"))?;
            server.connect_count.fetch_add(1, Ordering::SeqCst);
            let delay = server.connect_delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if let Some(error) = server
                .connect_error
                .lock()
                .map_err(|_| McpError::protocol("mock connect error lock unavailable"))?
                .clone()
            {
                return Err(error);
            }
            Ok(Arc::new(MockPeer::new(server_id, server)) as Arc<dyn McpPeer>)
        })
    }
}

struct MockPeer {
    server_id: McpServerId,
    server: Arc<MockServer>,
    protocol: McpProtocolSnapshot,
    closed: AtomicBool,
}

impl MockPeer {
    fn new(server_id: McpServerId, server: Arc<MockServer>) -> Self {
        Self {
            server_id,
            server,
            protocol: McpProtocolSnapshot {
                negotiated_version: "2026-07-28".to_string(),
                lifecycle: McpLifecycleKind::Discover,
                server: Some(McpImplementationInfo {
                    name: "mycopilot-owned-mock".to_string(),
                    version: "1.0.0".to_string(),
                }),
                capabilities: McpCapabilitySnapshot {
                    tools: true,
                    ..McpCapabilitySnapshot::default()
                },
            },
            closed: AtomicBool::new(false),
        }
    }
}

impl McpPeer for MockPeer {
    fn server_id(&self) -> McpServerId {
        self.server_id
    }

    fn connection_state(&self) -> McpConnectionState {
        if self.closed.load(Ordering::SeqCst) {
            McpConnectionState::Closed
        } else {
            McpConnectionState::Ready
        }
    }

    fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        &self.protocol
    }

    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
        Box::pin(async move {
            self.server.list_count.fetch_add(1, Ordering::SeqCst);
            let delay = self.server.list_delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            self.server
                .pages
                .read()
                .map_err(|_| McpError::protocol("mock pages read lock unavailable"))?
                .get(&cursor)
                .cloned()
                .unwrap_or_else(|| Err(McpError::protocol("unexpected mock cursor")))
        })
    }

    fn call_tool<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        Box::pin(async move {
            self.server
                .calls
                .lock()
                .map_err(|_| McpError::protocol("mock calls lock unavailable"))?
                .push(call);
            if self
                .server
                .wait_for_call_cancellation
                .load(Ordering::SeqCst)
            {
                cancellation.cancelled().await;
                return Err(McpError::cancelled("mock tools/call"));
            }
            Ok(McpToolResult {
                content: vec![McpContentBlock::Text {
                    text: "owned mock result".to_string(),
                }],
                structured_content: None,
                is_error: false,
            })
        })
    }

    fn force_close(&self) -> bool {
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.server.close_count.fetch_add(1, Ordering::SeqCst);
        }
        true
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async move {
            let delay = self.server.close_delay_ms.load(Ordering::SeqCst);
            if delay > 0 {
                tokio::time::sleep(Duration::from_millis(delay)).await;
            }
            if !self.closed.swap(true, Ordering::SeqCst) {
                self.server.close_count.fetch_add(1, Ordering::SeqCst);
                if let Some(registry) = self
                    .server
                    .registry_at_close
                    .lock()
                    .map_err(|_| McpError::protocol("mock registry tracker lock unavailable"))?
                    .as_ref()
                {
                    self.server
                        .close_saw_registered
                        .store(registry.get(self.server_id)?.is_some(), Ordering::SeqCst);
                }
            }
            Ok(())
        })
    }
}

#[derive(Default)]
struct RecordingEventSink {
    events: Mutex<Vec<McpEvent>>,
}

impl RecordingEventSink {
    fn snapshot(&self) -> Vec<McpEvent> {
        self.events.lock().expect("event sink lock").clone()
    }
}

impl McpEventSink for RecordingEventSink {
    fn emit(&self, event: McpEvent) {
        self.events.lock().expect("event sink lock").push(event);
    }
}

fn config(server_id: McpServerId, display_name: &str, enabled: bool) -> McpServerConfig {
    McpServerConfig {
        id: server_id,
        display_name: display_name.to_string(),
        scope: McpServerScope::User,
        trust: McpTrustLevel::UserApproved,
        enabled,
        transport: McpTransportConfig::Stdio(McpStdioConfig {
            program: PathBuf::from("/owned/mock/server"),
            arguments: Vec::new(),
            cwd: PathBuf::from("/owned/mock"),
            environment: vec![McpEnvBinding::SecretRef {
                name: "MOCK_TOKEN".to_string(),
                secret_id: SECRET_SENTINEL.to_string(),
            }],
        }),
        connect_timeout_ms: 1_000,
        request_timeout_ms: 1_000,
        shutdown_timeout_ms: 1_000,
    }
}

fn descriptor(name: &str, description: &str) -> McpToolDescriptor {
    McpToolDescriptor {
        name: name.to_string(),
        title: None,
        description: Some(description.to_string()),
        input_schema: json!({
            "type": "object",
            "properties": {
                "value": { "type": "string" }
            }
        }),
        output_schema: None,
        annotations: None,
    }
}

fn page(tools: Vec<McpToolDescriptor>, next_cursor: Option<&str>) -> McpToolPage {
    McpToolPage {
        tools,
        next_cursor: next_cursor.map(str::to_string),
        ttl_ms: None,
        cache_scope: None,
    }
}

fn single_page(tools: Vec<McpToolDescriptor>) -> PageScript {
    BTreeMap::from([(None, Ok(page(tools, None)))])
}

fn manager(
    registry: Arc<InMemoryMcpRegistry>,
    connector: Arc<MockConnector>,
    sink: Arc<RecordingEventSink>,
    policy: McpManagerPolicy,
) -> McpConnectionManager {
    McpConnectionManager::new(registry, connector, sink, policy)
        .expect("construct manager with valid owned-mock policy")
}

fn catalog_call(
    manager: &McpConnectionManager,
    server_id: McpServerId,
    raw_name: &str,
    arguments: serde_json::Value,
) -> McpCatalogToolCall {
    let status = manager
        .get_status(server_id)
        .expect("read manager status")
        .expect("managed server status");
    let catalog = manager
        .catalog(server_id)
        .expect("read manager catalog")
        .expect("managed server catalog");
    let tool = catalog
        .tools
        .iter()
        .find(|tool| tool.raw_name == raw_name)
        .expect("catalog tool");
    McpCatalogToolCall {
        tool_id: tool.id.clone(),
        expected_config_digest: status.config_digest,
        expected_catalog_generation: catalog.generation,
        expected_catalog_digest: catalog.content_digest.unwrap_or_else(|| {
            McpCatalogDigest::from_str(&"0".repeat(64)).expect("test fallback catalog digest")
        }),
        expected_model_name: tool.model_name.clone(),
        arguments,
        timeout_ms: None,
    }
}

fn result_for(
    results: &[McpBatchOperationResult],
    server_id: McpServerId,
) -> &McpBatchOperationResult {
    results
        .iter()
        .find(|result| result.server_id == Some(server_id))
        .expect("batch result for server")
}

#[tokio::test]
async fn untrusted_server_is_rejected_by_manager_before_connector_dispatch() {
    let server_id = McpServerId::new();
    let registry = InMemoryMcpRegistry::shared();
    let mut untrusted = config(server_id, "untrusted", true);
    untrusted.trust = McpTrustLevel::Untrusted;
    registry.add(untrusted).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "echo")]);
    let connector = Arc::new(MockConnector::default());
    connector.add(server_id, Arc::clone(&server));
    let sink = Arc::new(RecordingEventSink::default());
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let error = manager.start(server_id).await.unwrap_err();
    assert_eq!(error.kind, McpErrorKind::Config);
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 0);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Error
    );
}

async fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn event_sequence(event: &McpEvent) -> (McpServerId, u64) {
    match event {
        McpEvent::ServerStateChanged {
            server_id,
            sequence,
            ..
        }
        | McpEvent::CatalogChanged {
            server_id,
            sequence,
            ..
        }
        | McpEvent::ServerError {
            server_id,
            sequence,
            ..
        }
        | McpEvent::ServerExited {
            server_id,
            sequence,
            ..
        } => (*server_id, *sequence),
        _ => panic!("unexpected future MCP event variant in round-2 ordering test"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_enabled_isolates_failures_and_emits_ordered_safe_events() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let ready_id = McpServerId::new();
    let failed_id = McpServerId::new();
    registry
        .add(config(ready_id, "duplicate display name", true))
        .unwrap();
    registry
        .add(config(failed_id, "duplicate display name", true))
        .unwrap();

    let ready = MockServer::with_tools(vec![descriptor("same_tool", "ready")]);
    let failed = MockServer::with_tools(vec![descriptor("never_listed", "failed")]);
    failed.set_connect_error(McpError::spawn(format!(
        "owned mock failed: {SECRET_SENTINEL}"
    )));
    connector.add(ready_id, Arc::clone(&ready));
    connector.add(failed_id, Arc::clone(&failed));
    let manager = manager(
        Arc::clone(&registry),
        Arc::clone(&connector),
        Arc::clone(&sink),
        McpManagerPolicy::default(),
    );

    let results = manager.start_enabled().await;
    assert_eq!(results.len(), 2);
    assert_eq!(
        result_for(&results, ready_id)
            .status
            .as_ref()
            .unwrap()
            .state,
        McpServerState::Ready
    );
    let failure = result_for(&results, failed_id).error.as_ref().unwrap();
    assert_eq!(failure.kind, McpErrorKind::Spawn);
    assert!(!failure.message.contains(SECRET_SENTINEL));
    assert_eq!(
        manager.get_status(failed_id).unwrap().unwrap().state,
        McpServerState::Error
    );

    let ready_catalog = manager.catalog(ready_id).unwrap().unwrap();
    assert_eq!(ready_catalog.server_id, ready_id);
    assert_eq!(ready_catalog.tools[0].id.server_id, ready_id);
    assert_eq!(ready_catalog.tools[0].raw_name, "same_tool");
    wait_until(Duration::from_secs(1), || sink.snapshot().len() >= 7).await;
    let events = sink.snapshot();
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(SECRET_SENTINEL));
    for server_id in [ready_id, failed_id] {
        let sequences = events
            .iter()
            .map(event_sequence)
            .filter_map(|(event_server, sequence)| (event_server == server_id).then_some(sequence))
            .collect::<Vec<_>>();
        assert!(
            sequences.windows(2).all(|pair| pair[0] < pair[1]),
            "event sequence must be strictly increasing for {server_id}: {sequences:?}"
        );
    }
    let ready_states = events
        .iter()
        .filter_map(|event| match event {
            McpEvent::ServerStateChanged {
                server_id, current, ..
            } if *server_id == ready_id => Some(*current),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        ready_states,
        vec![
            McpServerState::Starting,
            McpServerState::Discovering,
            McpServerState::Ready
        ]
    );
    let failed_events = events
        .iter()
        .filter(|event| event_sequence(event).0 == failed_id)
        .collect::<Vec<_>>();
    assert!(matches!(
        failed_events.as_slice(),
        [
            McpEvent::ServerStateChanged {
                current: McpServerState::Starting,
                ..
            },
            McpEvent::ServerStateChanged {
                current: McpServerState::Error,
                ..
            },
            McpEvent::ServerError { .. }
        ]
    ));

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_duplicate_start_is_single_flight() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "single-flight", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.connect_delay_ms.store(75, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let first_manager = manager.clone();
    let second_manager = manager.clone();
    let first = tokio::spawn(async move { first_manager.start(server_id).await });
    let second = tokio::spawn(async move { second_manager.start(server_id).await });
    let first = tokio::time::timeout(Duration::from_secs(2), first)
        .await
        .expect("first start timeout")
        .expect("first start task")
        .expect("first start result");
    let second = tokio::time::timeout(Duration::from_secs(2), second)
        .await
        .expect("second start timeout")
        .expect("second start task")
        .expect("second start result");
    assert_eq!(first.state, McpServerState::Ready);
    assert_eq!(second.state, McpServerState::Ready);
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 1);

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_wins_a_race_with_an_inflight_start_and_closes_the_stale_peer() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry.add(config(server_id, "race", true)).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.connect_delay_ms.store(100, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let start_manager = manager.clone();
    let start_task = tokio::spawn(async move { start_manager.start(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;
    let stopped = tokio::time::timeout(Duration::from_secs(2), manager.stop(server_id))
        .await
        .expect("stop timeout")
        .expect("stop result");
    let start_error = tokio::time::timeout(Duration::from_secs(2), start_task)
        .await
        .expect("start task timeout")
        .expect("start task")
        .expect_err("inflight start must lose to stop");
    assert_eq!(start_error.kind, McpErrorKind::Cancelled);
    assert_eq!(stopped.state, McpServerState::Disabled);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Disabled
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_cancellation_does_not_abandon_start_refresh_or_stop_cleanup() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "cancellation-safe", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("alpha", "version one")]);
    server.connect_delay_ms.store(100, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let start_manager = manager.clone();
    let start_waiter = tokio::spawn(async move { start_manager.start(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;
    start_waiter.abort();
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Ready)
    })
    .await;

    server.set_pages(single_page(vec![descriptor("beta", "version two")]));
    server.list_delay_ms.store(100, Ordering::SeqCst);
    let list_count = server.list_count.load(Ordering::SeqCst);
    let refresh_manager = manager.clone();
    let refresh_waiter = tokio::spawn(async move { refresh_manager.refresh(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.list_count.load(Ordering::SeqCst) > list_count
    })
    .await;
    refresh_waiter.abort();
    wait_until(Duration::from_secs(2), || {
        manager
            .catalog(server_id)
            .ok()
            .flatten()
            .is_some_and(|catalog| {
                catalog.generation == 2 && catalog.tools.iter().any(|tool| tool.raw_name == "beta")
            })
    })
    .await;

    server.close_delay_ms.store(100, Ordering::SeqCst);
    let stop_manager = manager.clone();
    let stop_waiter = tokio::spawn(async move { stop_manager.stop(server_id).await });
    wait_until(Duration::from_secs(1), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Stopping)
    })
    .await;
    stop_waiter.abort();
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Disabled)
    })
    .await;
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_reconnects_and_remove_closes_before_registry_deletion() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "restart-remove", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    *server
        .registry_at_close
        .lock()
        .expect("mock registry tracker lock") = Some(Arc::clone(&registry));
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );

    assert_eq!(
        manager.start(server_id).await.unwrap().state,
        McpServerState::Ready
    );
    assert_eq!(
        manager.restart(server_id).await.unwrap().state,
        McpServerState::Ready
    );
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 2);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);

    server.close_saw_registered.store(false, Ordering::SeqCst);
    let removed = manager.remove_server(server_id).await.unwrap();
    assert!(removed.is_some());
    assert!(server.close_saw_registered.load(Ordering::SeqCst));
    assert_eq!(server.close_count.load(Ordering::SeqCst), 2);
    assert!(registry.get(server_id).unwrap().is_none());
    assert!(manager.get_status(server_id).unwrap().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_generation_changes_only_with_effective_content() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry.add(config(server_id, "generation", true)).unwrap();
    let server = MockServer::with_tools(vec![descriptor("alpha", "version one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    manager.start(server_id).await.unwrap();
    let first = manager.catalog(server_id).unwrap().unwrap();
    assert_eq!(first.generation, 1);
    let first_model_name = first.tools[0].model_name.clone();
    assert_eq!(manager.refresh(server_id).await.unwrap().generation, 1);

    server.set_pages(single_page(vec![
        descriptor("alpha", "version two"),
        descriptor("beta", "new tool"),
    ]));
    let changed = manager.refresh(server_id).await.unwrap();
    assert_eq!(changed.generation, 2);
    assert_eq!(
        changed
            .tools
            .iter()
            .find(|tool| tool.raw_name == "alpha")
            .unwrap()
            .model_name,
        first_model_name
    );
    assert!(first_model_name.starts_with("mcp__"));
    assert!(first_model_name.len() <= 64);
    assert_eq!(
        manager.resolve_model_name(&first_model_name).unwrap(),
        Some(mycopilot_mcp_client::McpToolId {
            server_id,
            raw_name: "alpha".to_string()
        })
    );
    assert_eq!(manager.refresh(server_id).await.unwrap().generation, 2);

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_change_invalidates_old_routes_when_new_discovery_fails() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let initial_config = config(server_id, "old-config", true);
    registry.add(initial_config.clone()).unwrap();
    let server = MockServer::with_tools(vec![descriptor("old_tool", "old")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );
    manager.start(server_id).await.unwrap();
    let old_model_name = manager.catalog(server_id).unwrap().unwrap().tools[0]
        .model_name
        .clone();
    assert!(manager
        .resolve_model_name(&old_model_name)
        .unwrap()
        .is_some());

    server.set_pages(BTreeMap::from([(
        None,
        Err(McpError::protocol("owned new-config discovery failure")),
    )]));
    let mut changed_config = initial_config;
    changed_config.display_name = "new-config".to_string();
    let changed = registry.upsert(changed_config).unwrap();
    let expected_digest = match changed {
        mycopilot_mcp_client::McpRegistryMutation::Updated(entry) => entry.config_digest,
        other => panic!("expected updated registry entry, got {other:?}"),
    };
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| {
                status.config_digest == expected_digest && status.state == McpServerState::Degraded
            })
    })
    .await;
    let catalog = manager.catalog(server_id).unwrap().unwrap();
    assert_eq!(
        catalog.completeness,
        McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed)
    );
    assert!(catalog.tools.is_empty());
    assert_eq!(manager.resolve_model_name(&old_model_name).unwrap(), None);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disable_then_enable_reconciles_latest_revision_without_implicit_relaunch() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let enabled = config(server_id, "revision-order", true);
    registry.add(enabled.clone()).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );
    manager.start(server_id).await.unwrap();

    let mut disabled = enabled.clone();
    disabled.enabled = false;
    registry.upsert(disabled).unwrap();
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| !status.enabled && status.state == McpServerState::Disabled)
    })
    .await;
    let latest = match registry.upsert(enabled).unwrap() {
        mycopilot_mcp_client::McpRegistryMutation::Updated(entry) => entry,
        other => panic!("expected latest enabled update, got {other:?}"),
    };
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| {
                status.enabled
                    && status.config_digest == latest.config_digest
                    && status.state == McpServerState::Disabled
            })
    })
    .await;
    assert_eq!(
        manager.start(server_id).await.unwrap().state,
        McpServerState::Ready
    );
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_paginates_and_reports_cursor_and_name_collisions() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let paged_id = McpServerId::new();
    let repeated_id = McpServerId::new();
    let collision_id = McpServerId::new();
    for (id, name) in [
        (paged_id, "paged"),
        (repeated_id, "repeated"),
        (collision_id, "collision"),
    ] {
        registry.add(config(id, name, true)).unwrap();
    }

    let paged = MockServer::with_tools(Vec::new());
    paged.set_pages(BTreeMap::from([
        (
            None,
            Ok(page(vec![descriptor("first", "page one")], Some("two"))),
        ),
        (
            Some("two".to_string()),
            Ok(page(vec![descriptor("second", "page two")], None)),
        ),
    ]));
    let repeated = MockServer::with_tools(Vec::new());
    repeated.set_pages(BTreeMap::from([
        (
            None,
            Ok(page(vec![descriptor("first", "page one")], Some("again"))),
        ),
        (
            Some("again".to_string()),
            Ok(page(vec![descriptor("second", "page two")], Some("again"))),
        ),
    ]));
    let collision = MockServer::with_tools(vec![
        descriptor("alpha.beta", "dot"),
        descriptor("alpha/beta", "slash"),
        descriptor("duplicate", "first"),
        descriptor("duplicate", "second"),
    ]);
    connector.add(paged_id, paged);
    connector.add(repeated_id, repeated);
    connector.add(collision_id, collision);
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let results = manager.start_enabled().await;
    assert_eq!(results.len(), 3);
    let paged_catalog = manager.catalog(paged_id).unwrap().unwrap();
    assert_eq!(paged_catalog.completeness, McpCatalogCompleteness::Complete);
    assert_eq!(paged_catalog.page_count, 2);
    assert_eq!(paged_catalog.tools.len(), 2);

    let repeated_catalog = manager.catalog(repeated_id).unwrap().unwrap();
    assert_eq!(
        repeated_catalog.completeness,
        McpCatalogCompleteness::Partial(McpCatalogIssue::RepeatedCursor)
    );
    assert_eq!(
        manager.get_status(repeated_id).unwrap().unwrap().state,
        McpServerState::Degraded
    );
    assert_eq!(
        repeated_catalog.resolve_model_name(&repeated_catalog.tools[0].model_name),
        None,
        "partial catalogs must never route tools"
    );

    let collision_catalog = manager.catalog(collision_id).unwrap().unwrap();
    assert!(collision_catalog
        .diagnostics
        .iter()
        .any(|diagnostic| { diagnostic.kind == McpCatalogDiagnosticKind::NormalizationCollision }));
    assert!(collision_catalog.diagnostics.iter().any(|diagnostic| {
        diagnostic.kind == McpCatalogDiagnosticKind::DuplicateRawName
            && diagnostic.occurrence_count == 2
    }));
    assert!(
        !collision_catalog
            .tools
            .iter()
            .find(|tool| tool.raw_name == "duplicate")
            .unwrap()
            .routable
    );
    let normalized = collision_catalog
        .tools
        .iter()
        .filter(|tool| tool.raw_name.starts_with("alpha"))
        .map(|tool| tool.model_name.clone())
        .collect::<Vec<_>>();
    assert_eq!(normalized.len(), 2);
    assert_ne!(normalized[0], normalized[1]);

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_limits_fail_closed_and_stop_all_closes_every_active_peer() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let limited_id = McpServerId::new();
    let healthy_id = McpServerId::new();
    registry.add(config(limited_id, "limited", true)).unwrap();
    registry.add(config(healthy_id, "healthy", true)).unwrap();
    let limited = MockServer::with_tools(vec![
        descriptor("first", "one"),
        descriptor("second", "two"),
    ]);
    let healthy = MockServer::with_tools(vec![descriptor("healthy", "one")]);
    connector.add(limited_id, Arc::clone(&limited));
    connector.add(healthy_id, Arc::clone(&healthy));
    let mut catalog_policy = McpCatalogPolicy::default();
    catalog_policy.limits.max_tools = 1;
    let manager = manager(
        registry,
        connector,
        sink,
        McpManagerPolicy {
            catalog: catalog_policy,
            notification_debounce: Duration::from_millis(1),
        },
    );

    manager.start_enabled().await;
    assert_eq!(
        manager.catalog(limited_id).unwrap().unwrap().completeness,
        McpCatalogCompleteness::Failed(McpCatalogIssue::ToolLimitExceeded)
    );
    assert_eq!(
        manager.get_status(limited_id).unwrap().unwrap().state,
        McpServerState::Degraded
    );
    assert_eq!(
        manager.get_status(healthy_id).unwrap().unwrap().state,
        McpServerState::Ready
    );

    let stopped = manager.stop_all().await;
    assert_eq!(stopped.len(), 2);
    assert!(stopped.iter().all(|result| {
        result.error.is_none()
            && result
                .status
                .as_ref()
                .is_some_and(|status| status.state == McpServerState::Disabled)
    }));
    assert_eq!(limited.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(healthy.close_count.load(Ordering::SeqCst), 1);
    assert!(manager
        .list_statuses()
        .unwrap()
        .iter()
        .all(|status| status.state == McpServerState::Disabled));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_all_waits_for_an_already_admitted_start() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry.add(config(server_id, "drain-gate", true)).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.connect_delay_ms.store(100, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let start_manager = manager.clone();
    let start_waiter = tokio::spawn(async move { start_manager.start(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;
    let stopped = manager.stop_all().await;
    let _ = start_waiter.await;
    assert_eq!(stopped.len(), 1);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Disabled
    );
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_shutdown_reclaims_an_aborted_start_enabled_coordinator() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "shutdown-start", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.connect_delay_ms.store(10_000, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let startup_manager = manager.clone();
    let startup = tokio::spawn(async move { startup_manager.start_enabled().await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;
    startup.abort();
    let _ = startup.await;

    let shutdown = tokio::time::timeout(
        Duration::from_secs(1),
        manager.shutdown(Duration::from_millis(200)),
    )
    .await
    .expect("permanent shutdown must remain bounded");
    assert!(shutdown.results.iter().all(|result| {
        result
            .status
            .as_ref()
            .is_some_and(|status| status.state == McpServerState::Disabled)
    }));
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Disabled
    );
    let restart_error = manager
        .start(server_id)
        .await
        .expect_err("permanent shutdown must reject future starts");
    assert_eq!(restart_error.kind, McpErrorKind::Shutdown);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn caller_cancellation_does_not_abandon_start_enabled_children() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "start-enabled-cancellation", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.connect_delay_ms.store(100, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());

    let startup_manager = manager.clone();
    let startup = tokio::spawn(async move { startup_manager.start_enabled().await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;
    startup.abort();
    let _ = startup.await;
    wait_until(Duration::from_secs(2), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Ready)
    })
    .await;
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 1);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_shutdown_cancels_a_detached_hung_catalog_refresh() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "shutdown-refresh", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();

    server.list_delay_ms.store(10_000, Ordering::SeqCst);
    let initial_lists = server.list_count.load(Ordering::SeqCst);
    let refresh_manager = manager.clone();
    let refresh = tokio::spawn(async move { refresh_manager.refresh(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.list_count.load(Ordering::SeqCst) > initial_lists
    })
    .await;

    let shutdown = tokio::time::timeout(
        Duration::from_secs(1),
        manager.shutdown(Duration::from_millis(200)),
    )
    .await
    .expect("shutdown must cancel detached refresh");
    assert!(!shutdown.results.is_empty());
    let refresh_error = refresh
        .await
        .expect("refresh waiter")
        .expect_err("refresh must be cancelled by permanent shutdown");
    assert_eq!(refresh_error.kind, McpErrorKind::Shutdown);
    let status = manager.get_status(server_id).unwrap().unwrap();
    assert_eq!(status.state, McpServerState::Disabled);
    assert_eq!(status.tool_count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forced_shutdown_invalidates_catalog_and_reports_disabled_status() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "forced-shutdown", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    server.close_delay_ms.store(10_000, Ordering::SeqCst);

    let report = manager.shutdown(Duration::from_millis(100)).await;
    assert!(report.forced);
    assert!(!report.cleanup_complete);
    assert_eq!(report.results.len(), 1);
    let status = report.results[0].status.as_ref().unwrap();
    assert_eq!(status.state, McpServerState::Disabled);
    assert_eq!(status.tool_count, 0);
    assert_eq!(status.catalog_generation, 1);
    assert_eq!(
        status.catalog_completeness,
        McpCatalogCompleteness::Failed(McpCatalogIssue::RequestFailed)
    );
    assert!(manager
        .catalog(server_id)
        .unwrap()
        .unwrap()
        .tools
        .is_empty());
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn permanent_shutdown_cancels_a_detached_slow_stop() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "shutdown-stop", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    server.close_delay_ms.store(10_000, Ordering::SeqCst);

    let stop_manager = manager.clone();
    let stop = tokio::spawn(async move { stop_manager.stop(server_id).await });
    wait_until(Duration::from_secs(1), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Stopping)
    })
    .await;
    let report = tokio::time::timeout(
        Duration::from_secs(1),
        manager.shutdown(Duration::from_millis(300)),
    )
    .await
    .expect("shutdown must not inherit a detached stop delay");
    assert!(report.forced);
    assert!(!report.cleanup_complete);
    let stop_error = stop
        .await
        .expect("stop waiter")
        .expect_err("permanent shutdown must cancel the detached stop");
    assert_eq!(stop_error.kind, McpErrorKind::Shutdown);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Disabled
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_routes_by_server_id_and_raw_name() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry.add(config(server_id, "typed-call", true)).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo/raw", "typed route")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();

    let mut request = catalog_call(&manager, server_id, "echo/raw", json!({"value": "hello"}));
    let model_name = request.expected_model_name.clone();
    request.timeout_ms = Some(321);
    let result = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .unwrap();
    assert!(!result.is_error);
    let calls = server.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "echo/raw");
    assert_ne!(calls[0].name, model_name);
    assert_eq!(calls[0].arguments, json!({"value": "hello"}));
    assert_eq!(calls[0].timeout_ms, Some(321));

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_server_that_is_not_ready() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "not-ready-call", true))
        .unwrap();
    let server = MockServer::with_tools(Vec::new());
    server.set_pages(BTreeMap::from([
        (
            None,
            Ok(page(
                vec![descriptor("echo", "partial catalog")],
                Some("again"),
            )),
        ),
        (
            Some("again".to_string()),
            Ok(page(Vec::new(), Some("again"))),
        ),
    ]));
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    assert_eq!(
        manager.start(server_id).await.unwrap().state,
        McpServerState::Degraded
    );
    let request = catalog_call(&manager, server_id, "echo", json!({}));

    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("degraded connection must not invoke");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(error.message.contains("not ready"));
    assert!(server.calls().is_empty());

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_stale_config_digest() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let initial = config(server_id, "old-config-call", true);
    registry.add(initial.clone()).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "old config")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "echo", json!({}));

    let mut changed = initial;
    changed.display_name = "new-config-call".to_string();
    registry.upsert(changed).unwrap();
    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("old config digest must fail closed");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(server.calls().is_empty());

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_stale_catalog_generation() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "old-generation-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "generation one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "echo", json!({}));

    server.set_pages(single_page(vec![descriptor("echo", "generation two")]));
    assert_eq!(manager.refresh(server_id).await.unwrap().generation, 2);
    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("old generation must fail closed");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(error.message.contains("generation"));
    assert!(server.calls().is_empty());

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_stale_catalog_content_digest() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "old-catalog-digest-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "catalog one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let mut request = catalog_call(&manager, server_id, "echo", json!({}));

    server.set_pages(single_page(vec![descriptor("echo", "catalog two")]));
    let refreshed = manager.refresh(server_id).await.unwrap();
    request.expected_catalog_generation = refreshed.generation;
    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("old catalog digest must fail closed even when generation is forged current");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(error.message.contains("content digest"));
    assert!(server.calls().is_empty());

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_model_name_mismatch_without_parsing_it() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "model-name-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "model mismatch")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let mut request = catalog_call(&manager, server_id, "echo", json!({}));
    request.expected_model_name = "mcp__spoofed__tool".to_string();

    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("model name mismatch must fail closed");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(error.message.contains("route"));
    assert!(server.calls().is_empty());

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_propagates_cancellation_to_the_peer() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "cancel-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("slow", "waits for cancellation")]);
    server
        .wait_for_call_cancellation
        .store(true, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "slow", json!({}));
    let cancellation = McpCancellationToken::new();
    let call_cancellation = cancellation.clone();
    let call_manager = manager.clone();
    let call = tokio::spawn(async move {
        call_manager
            .call_catalog_tool(request, call_cancellation)
            .await
    });
    wait_until(Duration::from_secs(1), || server.calls().len() == 1).await;
    cancellation.cancel();

    let error = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .expect("cancelled call timeout")
        .expect("cancelled call task")
        .expect_err("cancelled peer call");
    assert_eq!(error.kind, McpErrorKind::Cancelled);

    manager.stop_all().await;
}
