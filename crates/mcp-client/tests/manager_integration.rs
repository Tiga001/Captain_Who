use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use mycopilot_mcp_client::{
    BoxMcpFuture, InMemoryMcpRegistry, McpActiveCallId, McpBatchOperationResult,
    McpCancellationToken, McpCapabilitySnapshot, McpCatalogCompleteness, McpCatalogDiagnosticKind,
    McpCatalogDigest, McpCatalogIssue, McpCatalogPolicy, McpCatalogToolCall, McpConnectionManager,
    McpConnectionState, McpConnector, McpContentBlock, McpDispatchCertainty, McpEnvBinding,
    McpError, McpErrorKind, McpEvent, McpEventSink, McpImplementationInfo, McpInvocationId,
    McpLifecycleKind, McpManagerPolicy, McpModelCallId, McpPeer, McpProtocolSnapshot, McpRegistry,
    McpRegistryChangeKind, McpSecurityLimits, McpServerConfig, McpServerId, McpServerScope,
    McpServerState, McpStdioConfig, McpToolCall, McpToolDescriptor, McpToolPage, McpToolResult,
    McpTransportConfig, McpTrustLevel,
};
use serde_json::json;

const SECRET_SENTINEL: &str = "ROUND2_SECRET_MUST_NOT_ESCAPE";

type PageScript = BTreeMap<Option<String>, Result<McpToolPage, McpError>>;

struct MockServer {
    pages: RwLock<PageScript>,
    protocol: Mutex<McpProtocolSnapshot>,
    connect_count: AtomicUsize,
    close_count: AtomicUsize,
    list_count: AtomicUsize,
    connect_delay_ms: AtomicU64,
    list_delay_ms: AtomicU64,
    close_delay_ms: AtomicU64,
    hang_connect: AtomicBool,
    hang_list: AtomicBool,
    hang_close: AtomicBool,
    connect_drop_count: AtomicUsize,
    list_drop_count: AtomicUsize,
    close_drop_count: AtomicUsize,
    force_close_count: AtomicUsize,
    connect_error: Mutex<Option<McpError>>,
    calls: Mutex<Vec<McpToolCall>>,
    tool_result: Mutex<McpToolResult>,
    call_error: Mutex<Option<McpError>>,
    wait_for_call_cancellation: AtomicBool,
    owns_tool_timeout: AtomicBool,
    registry_at_close: Mutex<Option<Arc<InMemoryMcpRegistry>>>,
    close_saw_registered: AtomicBool,
}

impl MockServer {
    fn with_tools(tools: Vec<McpToolDescriptor>) -> Arc<Self> {
        Arc::new(Self {
            pages: RwLock::new(single_page(tools)),
            protocol: Mutex::new(McpProtocolSnapshot {
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
            }),
            connect_count: AtomicUsize::new(0),
            close_count: AtomicUsize::new(0),
            list_count: AtomicUsize::new(0),
            connect_delay_ms: AtomicU64::new(0),
            list_delay_ms: AtomicU64::new(0),
            close_delay_ms: AtomicU64::new(0),
            hang_connect: AtomicBool::new(false),
            hang_list: AtomicBool::new(false),
            hang_close: AtomicBool::new(false),
            connect_drop_count: AtomicUsize::new(0),
            list_drop_count: AtomicUsize::new(0),
            close_drop_count: AtomicUsize::new(0),
            force_close_count: AtomicUsize::new(0),
            connect_error: Mutex::new(None),
            calls: Mutex::new(Vec::new()),
            tool_result: Mutex::new(McpToolResult {
                content: vec![McpContentBlock::Text {
                    text: "owned mock result".to_string(),
                }],
                structured_content: None,
                is_error: false,
            }),
            call_error: Mutex::new(None),
            wait_for_call_cancellation: AtomicBool::new(false),
            owns_tool_timeout: AtomicBool::new(false),
            registry_at_close: Mutex::new(None),
            close_saw_registered: AtomicBool::new(false),
        })
    }

    fn set_pages(&self, pages: PageScript) {
        *self.pages.write().expect("mock pages write lock") = pages;
    }

    fn set_protocol(&self, protocol: McpProtocolSnapshot) {
        *self.protocol.lock().expect("mock protocol lock") = protocol;
    }

    fn set_connect_error(&self, error: McpError) {
        *self.connect_error.lock().expect("mock connect error lock") = Some(error);
    }

    fn calls(&self) -> Vec<McpToolCall> {
        self.calls.lock().expect("mock calls lock").clone()
    }

    fn set_tool_result(&self, result: McpToolResult) {
        *self.tool_result.lock().expect("mock result lock") = result;
    }

    fn set_call_error(&self, error: McpError) {
        *self.call_error.lock().expect("mock call error lock") = Some(error);
    }
}

#[derive(Clone, Copy)]
enum MockPendingOperation {
    Connect,
    ListTools,
    Close,
}

/// Records that a deliberately pending mock future was dropped by the Manager.
///
/// The probe is only created on the pending branch, so its counter cannot be
/// confused with an operation that completed normally.
struct MockPendingDropProbe {
    server: Arc<MockServer>,
    operation: MockPendingOperation,
}

impl MockPendingDropProbe {
    fn new(server: Arc<MockServer>, operation: MockPendingOperation) -> Self {
        Self { server, operation }
    }
}

impl Drop for MockPendingDropProbe {
    fn drop(&mut self) {
        let counter = match self.operation {
            MockPendingOperation::Connect => &self.server.connect_drop_count,
            MockPendingOperation::ListTools => &self.server.list_drop_count,
            MockPendingOperation::Close => &self.server.close_drop_count,
        };
        counter.fetch_add(1, Ordering::SeqCst);
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
            if server.hang_connect.load(Ordering::SeqCst) {
                let _drop_probe =
                    MockPendingDropProbe::new(Arc::clone(&server), MockPendingOperation::Connect);
                std::future::pending::<()>().await;
            }
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
        let protocol = server.protocol.lock().expect("mock protocol lock").clone();
        Self {
            server_id,
            server,
            protocol,
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

    fn owns_tool_timeout(&self) -> bool {
        self.server.owns_tool_timeout.load(Ordering::SeqCst)
    }

    fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        &self.protocol
    }

    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
        Box::pin(async move {
            self.server.list_count.fetch_add(1, Ordering::SeqCst);
            if self.server.hang_list.load(Ordering::SeqCst) {
                let _drop_probe = MockPendingDropProbe::new(
                    Arc::clone(&self.server),
                    MockPendingOperation::ListTools,
                );
                std::future::pending::<()>().await;
            }
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
            if let Some(error) = self
                .server
                .call_error
                .lock()
                .map_err(|_| McpError::protocol("mock call error lock unavailable"))?
                .clone()
            {
                return Err(error);
            }
            self.server
                .tool_result
                .lock()
                .map_err(|_| McpError::protocol("mock result lock unavailable"))
                .map(|result| result.clone())
        })
    }

    fn force_close(&self) -> bool {
        self.server.force_close_count.fetch_add(1, Ordering::SeqCst);
        if !self.closed.swap(true, Ordering::SeqCst) {
            self.server.close_count.fetch_add(1, Ordering::SeqCst);
        }
        true
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async move {
            if self.server.hang_close.load(Ordering::SeqCst) {
                let _drop_probe = MockPendingDropProbe::new(
                    Arc::clone(&self.server),
                    MockPendingOperation::Close,
                );
                std::future::pending::<()>().await;
            }
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
        approval_mode: mycopilot_mcp_client::McpApprovalMode::Prompt,
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
        expected_config_epoch: status.config_epoch,
        expected_registry_revision: status.registry_revision,
        expected_config_digest: status.config_digest,
        expected_catalog_generation: catalog.generation,
        expected_catalog_digest: catalog.content_digest.unwrap_or_else(|| {
            McpCatalogDigest::from_str(&"0".repeat(64)).expect("test fallback catalog digest")
        }),
        expected_schema_digest: tool.schema_digest.clone(),
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

#[tokio::test]
async fn connector_protocol_snapshot_is_revalidated_before_status_or_peer_retention() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let unknown_version_id = McpServerId::new();
    let oversized_metadata_id = McpServerId::new();
    registry
        .add(config(unknown_version_id, "unknown-protocol", true))
        .unwrap();
    registry
        .add(config(
            oversized_metadata_id,
            "oversized-protocol-metadata",
            true,
        ))
        .unwrap();

    let unknown = MockServer::with_tools(vec![descriptor("echo", "echo")]);
    let mut unknown_snapshot = unknown.protocol.lock().unwrap().clone();
    unknown_snapshot.negotiated_version = "2099-01-01".to_string();
    unknown.set_protocol(unknown_snapshot);
    connector.add(unknown_version_id, Arc::clone(&unknown));

    let oversized = MockServer::with_tools(vec![descriptor("echo", "echo")]);
    let mut oversized_snapshot = oversized.protocol.lock().unwrap().clone();
    oversized_snapshot.server = Some(McpImplementationInfo {
        name: "x".repeat(McpSecurityLimits::default().max_server_implementation_name_bytes + 1),
        version: "1.0.0".to_string(),
    });
    oversized.set_protocol(oversized_snapshot);
    connector.add(oversized_metadata_id, Arc::clone(&oversized));

    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    for (server_id, server) in [
        (unknown_version_id, unknown),
        (oversized_metadata_id, oversized),
    ] {
        let error = manager.start(server_id).await.unwrap_err();
        assert_eq!(error.kind, McpErrorKind::Negotiation);
        assert_eq!(server.connect_count.load(Ordering::SeqCst), 1);
        assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
        assert_eq!(server.list_count.load(Ordering::SeqCst), 0);
        let status = manager.get_status(server_id).unwrap().unwrap();
        assert_eq!(status.state, McpServerState::Error);
        assert!(status.protocol.is_none());
        assert_eq!(status.tool_count, 0);
    }
}

async fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while !predicate() {
        assert!(Instant::now() < deadline, "condition timed out");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test]
async fn registry_update_and_remove_emit_safe_source_identity_events() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let manager = manager(
        Arc::clone(&registry),
        connector,
        Arc::clone(&sink),
        McpManagerPolicy::default(),
    );
    let server_id = McpServerId::new();
    let original = config(server_id, "safe-event-original", false);
    registry.add(original.clone()).unwrap();
    let mut updated = original;
    updated.display_name = "safe-event-updated".to_string();
    updated.scope = McpServerScope::Plugin {
        plugin_id: "safe-plugin-id".to_string(),
    };
    let expected_digest = match registry.upsert(updated).unwrap() {
        mycopilot_mcp_client::McpRegistryMutation::Updated(entry) => entry.config_digest,
        other => panic!("expected registry update, got {other:?}"),
    };

    manager.remove_server(server_id).await.unwrap().unwrap();
    wait_until(Duration::from_secs(1), || {
        let events = sink.snapshot();
        events.iter().any(|event| {
            matches!(
                event,
                McpEvent::RegistryChanged {
                    kind: McpRegistryChangeKind::Updated,
                    server_id: event_server,
                    ..
                } if *event_server == server_id
            )
        }) && events.iter().any(|event| {
            matches!(
                event,
                McpEvent::RegistryChanged {
                    kind: McpRegistryChangeKind::Removed,
                    server_id: event_server,
                    ..
                } if *event_server == server_id
            )
        })
    })
    .await;

    let events = sink.snapshot();
    let relevant = events
        .iter()
        .filter_map(|event| match event {
            McpEvent::RegistryChanged {
                kind,
                server_id: event_server,
                scope,
                config_digest,
                ..
            } if *event_server == server_id => Some((*kind, scope.clone(), config_digest.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(relevant.contains(&(
        McpRegistryChangeKind::Updated,
        McpServerScope::Plugin {
            plugin_id: "safe-plugin-id".to_string()
        },
        expected_digest.clone()
    )));
    assert!(relevant.contains(&(
        McpRegistryChangeKind::Removed,
        McpServerScope::Plugin {
            plugin_id: "safe-plugin-id".to_string()
        },
        expected_digest
    )));
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(SECRET_SENTINEL));
    assert!(!serialized.contains("MOCK_TOKEN"));
    manager.shutdown(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn registry_broadcast_lag_emits_fail_closed_reconciliation_event() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let manager = manager(
        Arc::clone(&registry),
        connector,
        Arc::clone(&sink),
        McpManagerPolicy::default(),
    );
    let server_id = McpServerId::new();
    for revision in 0..=160 {
        let mut record = config(server_id, &format!("rapid-mutation-{revision}"), false);
        record.scope = McpServerScope::User;
        registry.upsert(record).unwrap();
    }

    wait_until(Duration::from_secs(1), || {
        sink.snapshot().iter().any(|event| {
            matches!(
                event,
                McpEvent::RegistryReconciliationRequired {
                    skipped_changes
                } if *skipped_changes > 0
            )
        })
    })
    .await;
    let serialized = serde_json::to_string(&sink.snapshot()).unwrap();
    assert!(!serialized.contains(SECRET_SENTINEL));
    assert!(!serialized.contains("MOCK_TOKEN"));
    manager.shutdown(Duration::from_millis(100)).await;
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
async fn stop_wins_a_race_with_an_inflight_start_before_a_peer_is_materialized() {
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
    assert_eq!(
        server.close_count.load(Ordering::SeqCst),
        0,
        "cancelling the connector future must avoid materializing a stale peer"
    );
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

fn short_lifecycle_policy() -> McpManagerPolicy {
    McpManagerPolicy {
        active_call_settle_timeout: Duration::from_millis(25),
        lifecycle_cleanup_timeout: Duration::from_millis(200),
        peer_close_timeout: Duration::from_millis(50),
        catalog_refresh_timeout: Duration::from_secs(1),
        ..McpManagerPolicy::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_a_pending_connect_without_later_becoming_ready() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "pending-connect", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    server.hang_connect.store(true, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, short_lifecycle_policy());

    let start_manager = manager.clone();
    let start = tokio::spawn(async move { start_manager.start(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.connect_count.load(Ordering::SeqCst) == 1
    })
    .await;

    let stopped = tokio::time::timeout(Duration::from_secs(1), manager.stop(server_id))
        .await
        .expect("stop must cancel a pending connector future")
        .expect("pending connect cleanup must complete");
    let start_error = tokio::time::timeout(Duration::from_secs(1), start)
        .await
        .expect("cancelled start must settle")
        .expect("join cancelled start")
        .expect_err("a stop must win over its pending start");

    assert_eq!(start_error.kind, McpErrorKind::Cancelled);
    assert_eq!(stopped.state, McpServerState::Disabled);
    assert_eq!(server.connect_drop_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 0);
    tokio::task::yield_now().await;
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Disabled,
        "a dropped connector future must never publish a late Ready state"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_a_pending_manual_catalog_refresh() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "pending-refresh", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, short_lifecycle_policy());
    manager
        .start(server_id)
        .await
        .expect("start refresh fixture");

    server.hang_list.store(true, Ordering::SeqCst);
    let initial_list_count = server.list_count.load(Ordering::SeqCst);
    let refresh_manager = manager.clone();
    let refresh = tokio::spawn(async move { refresh_manager.refresh(server_id).await });
    wait_until(Duration::from_secs(1), || {
        server.list_count.load(Ordering::SeqCst) > initial_list_count
    })
    .await;

    let stopped = tokio::time::timeout(Duration::from_secs(1), manager.stop(server_id))
        .await
        .expect("stop must cancel a pending Catalog refresh")
        .expect("pending refresh cleanup must complete");
    let refresh_error = tokio::time::timeout(Duration::from_secs(1), refresh)
        .await
        .expect("cancelled refresh must settle")
        .expect("join cancelled refresh")
        .expect_err("stop must invalidate the pending refresh");

    assert_eq!(refresh_error.kind, McpErrorKind::Cancelled);
    assert_eq!(stopped.state, McpServerState::Disabled);
    assert_eq!(stopped.active_call_count, 0);
    assert_eq!(server.list_drop_count.load(Ordering::SeqCst), 1);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_refresh_has_a_total_deadline_and_preserves_the_last_snapshot_as_stale() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "refresh-deadline", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let mut policy = short_lifecycle_policy();
    policy.catalog_refresh_timeout = Duration::from_millis(50);
    let manager = manager(registry, connector, sink, policy);
    let ready = manager
        .start(server_id)
        .await
        .expect("start refresh fixture");
    assert_eq!(ready.state, McpServerState::Ready);

    server.hang_list.store(true, Ordering::SeqCst);
    let started_at = Instant::now();
    let snapshot = tokio::time::timeout(Duration::from_secs(1), manager.refresh(server_id))
        .await
        .expect("Catalog refresh must obey its total deadline")
        .expect("a timed-out refresh returns the retained safe snapshot");

    assert!(started_at.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        snapshot.completeness,
        McpCatalogCompleteness::Stale(_)
    ));
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Degraded
    );
    assert_eq!(server.list_drop_count.load(Ordering::SeqCst), 1);
    server.hang_list.store(false, Ordering::SeqCst);
    manager.stop(server_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pending_peer_close_is_bounded_fail_closed_and_retryable() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "pending-close", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, short_lifecycle_policy());
    manager.start(server_id).await.expect("start close fixture");
    server.hang_close.store(true, Ordering::SeqCst);

    let started_at = Instant::now();
    let error = tokio::time::timeout(Duration::from_secs(1), manager.stop(server_id))
        .await
        .expect("peer close must honor the Host lifecycle deadline")
        .expect_err("an unconfirmed close must fail closed");
    assert_eq!(error.kind, McpErrorKind::Shutdown);
    assert!(started_at.elapsed() < Duration::from_secs(1));
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Error
    );
    assert!(server.force_close_count.load(Ordering::SeqCst) >= 1);
    assert!(server.close_drop_count.load(Ordering::SeqCst) >= 2);

    // A failed close must retain enough typed state for a later explicit cleanup attempt.
    server.hang_close.store(false, Ordering::SeqCst);
    let stopped = tokio::time::timeout(Duration::from_secs(1), manager.stop(server_id))
        .await
        .expect("cleanup retry must remain bounded")
        .expect("cleanup retry must recover the retained closing peer");
    assert_eq!(stopped.state, McpServerState::Disabled);
    assert_eq!(stopped.active_call_count, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_never_connects_a_replacement_until_old_peer_cleanup_succeeds() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "restart-cleanup-gate", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, short_lifecycle_policy());
    manager
        .start(server_id)
        .await
        .expect("start restart fixture");
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 1);

    server.hang_close.store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(Duration::from_secs(1), manager.restart(server_id))
        .await
        .expect("restart cleanup must remain bounded")
        .expect_err("restart must fail closed when the old peer cannot settle");
    assert_eq!(error.kind, McpErrorKind::Shutdown);
    assert_eq!(
        server.connect_count.load(Ordering::SeqCst),
        1,
        "a failed cleanup must not create a second server process"
    );
    assert_eq!(
        manager.get_status(server_id).unwrap().unwrap().state,
        McpServerState::Error
    );

    server.hang_close.store(false, Ordering::SeqCst);
    let recovered = manager
        .restart(server_id)
        .await
        .expect("an explicit retry may recover after cleanup becomes available");
    assert_eq!(recovered.state, McpServerState::Ready);
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 2);
    manager.stop(server_id).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_restarts_share_one_stop_and_reconnect_result() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "restart-single-flight", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "one")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager
        .start(server_id)
        .await
        .expect("start restart fixture");
    server.close_delay_ms.store(50, Ordering::SeqCst);

    let first_manager = manager.clone();
    let first = tokio::spawn(async move { first_manager.restart(server_id).await });
    wait_until(Duration::from_secs(1), || {
        manager
            .get_status(server_id)
            .ok()
            .flatten()
            .is_some_and(|status| status.state == McpServerState::Stopping)
    })
    .await;
    let second_manager = manager.clone();
    let second = tokio::spawn(async move { second_manager.restart(server_id).await });

    assert_eq!(first.await.unwrap().unwrap().state, McpServerState::Ready);
    assert_eq!(second.await.unwrap().unwrap().state, McpServerState::Ready);
    assert_eq!(
        server.connect_count.load(Ordering::SeqCst),
        2,
        "concurrent restart callers must share one replacement connection"
    );
    manager.stop(server_id).await.unwrap();
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
    assert_eq!(first_model_name, "mcp__generation__alpha");
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
            ..McpManagerPolicy::default()
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
async fn stop_all_cancels_an_already_admitted_start_before_peer_creation() {
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
    assert_eq!(server.close_count.load(Ordering::SeqCst), 0);
    assert_eq!(server.connect_count.load(Ordering::SeqCst), 1);
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
async fn catalog_call_rejects_argument_shape_depth_and_nodes_before_dispatch() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "argument-budget", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "argument budget")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let limits = McpSecurityLimits::default();

    let mut too_deep = json!({});
    for _ in 0..limits.max_arguments_depth {
        too_deep = json!({"nested": too_deep});
    }
    let too_many_nodes = json!({
        "items": vec![serde_json::Value::Null; limits.max_arguments_nodes]
    });
    let too_many_properties = (0..=limits.max_argument_object_properties)
        .map(|index| (format!("p{index}"), serde_json::Value::Null))
        .collect::<serde_json::Map<_, _>>();
    for arguments in [
        json!(null),
        too_deep,
        too_many_nodes,
        json!({"nested": too_many_properties}),
    ] {
        let error = manager
            .call_catalog_tool(
                catalog_call(&manager, server_id, "echo", arguments),
                McpCancellationToken::new(),
            )
            .await
            .expect_err("invalid arguments must fail before dispatch");
        assert_eq!(error.kind, McpErrorKind::Config);
    }
    assert!(server.calls().is_empty());
    assert_eq!(manager.active_call_count().unwrap(), 0);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn structured_result_depth_and_nodes_are_authoritative_output_too_large_failures() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "result-budget", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "result budget")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let limits = McpSecurityLimits::default();

    let mut too_deep = json!(null);
    for _ in 0..limits.max_structured_content_depth {
        too_deep = json!({"nested": too_deep});
    }
    let too_many_nodes = serde_json::Value::Array(vec![
        serde_json::Value::Null;
        limits.max_structured_content_nodes
    ]);
    for structured_content in [too_deep, too_many_nodes] {
        server.set_tool_result(McpToolResult {
            content: Vec::new(),
            structured_content: Some(structured_content),
            is_error: false,
        });
        let error = manager
            .call_catalog_tool(
                catalog_call(&manager, server_id, "echo", json!({})),
                McpCancellationToken::new(),
            )
            .await
            .expect_err("oversized structured result must fail closed");
        assert_eq!(error.kind, McpErrorKind::OutputTooLarge);
        assert_eq!(
            error.dispatch_certainty,
            Some(McpDispatchCertainty::ResponseReceived)
        );
        assert_eq!(manager.active_call_count().unwrap(), 0);
    }
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn post_dispatch_protocol_failure_without_response_is_outcome_unknown() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "protocol-outcome", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "protocol outcome")]);
    server.set_call_error(McpError::protocol("owned mock protocol failure"));
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();

    let error = manager
        .call_catalog_tool(
            catalog_call(&manager, server_id, "echo", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("post-dispatch protocol failure has no authoritative outcome");
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert_eq!(
        error.outcome_unknown_reason,
        Some(mycopilot_mcp_client::McpOutcomeUnknownReason::ProtocolFailure)
    );
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

#[tokio::test(flavor = "current_thread")]
async fn catalog_call_rejects_an_a_to_b_to_a_configuration_epoch_before_watcher_runs() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let config_a = config(server_id, "epoch-a", true);
    let first = registry.add(config_a.clone()).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "epoch route")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "echo", json!({}));
    let catalog = manager.catalog(server_id).unwrap().unwrap();
    assert_eq!(request.expected_config_epoch, first.config_epoch);
    assert_eq!(request.expected_registry_revision, first.revision);
    assert_eq!(catalog.source_config_epoch, Some(first.config_epoch));
    assert_eq!(catalog.source_registry_revision, Some(first.revision));

    // There is deliberately no await between these mutations and the call.
    // On the current-thread runtime the Manager's asynchronous Registry watcher
    // cannot be the mechanism that makes this test pass.
    let mut config_b = config_a.clone();
    config_b.display_name = "epoch-b".to_string();
    let changed = match registry.upsert(config_b).unwrap() {
        mycopilot_mcp_client::McpRegistryMutation::Updated(entry) => entry,
        other => panic!("expected B update, got {other:?}"),
    };
    let restored = match registry.upsert(config_a).unwrap() {
        mycopilot_mcp_client::McpRegistryMutation::Updated(entry) => entry,
        other => panic!("expected A restore, got {other:?}"),
    };
    assert_eq!(restored.config_digest, first.config_digest);
    assert_ne!(restored.config_epoch, first.config_epoch);
    assert!(restored.revision > changed.revision);

    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("an intervening configuration incarnation must revoke the old route");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(server.calls().is_empty());
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_rejects_a_route_after_remove_and_same_config_readd() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let server_config = config(server_id, "remove-readd", true);
    let first = registry.add(server_config.clone()).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "remove readd route")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        Arc::clone(&registry),
        connector,
        sink,
        McpManagerPolicy::default(),
    );
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "echo", json!({}));

    let removed = manager.remove_server(server_id).await.unwrap().unwrap();
    assert_eq!(removed.config_epoch, first.config_epoch);
    let readded = registry.add(server_config).unwrap();
    assert_eq!(readded.config_digest, first.config_digest);
    assert_ne!(readded.config_epoch, first.config_epoch);
    assert!(readded.revision > removed.revision);
    manager.start(server_id).await.unwrap();

    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("remove and same-config re-add must not revive an old route");
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
    let expected_model_name = request.expected_model_name.clone();
    let active_id = McpActiveCallId::new(
        server_id,
        McpInvocationId::new(),
        McpModelCallId::new("owned-model-call").unwrap(),
    );
    let cancellation = McpCancellationToken::new();
    let call_cancellation = cancellation.clone();
    let call_manager = manager.clone();
    let task_active_id = active_id.clone();
    let call = tokio::spawn(async move {
        call_manager
            .call_catalog_tool_identified(task_active_id, request, call_cancellation)
            .await
    });
    wait_until(Duration::from_secs(1), || server.calls().len() == 1).await;
    let active = manager
        .active_call(&active_id)
        .unwrap()
        .expect("identified call must be observable while active");
    assert_eq!(
        active.dispatch_certainty,
        McpDispatchCertainty::PossiblyDispatched
    );
    assert_eq!(active.provenance.tool_id.server_id, server_id);
    assert_eq!(active.provenance.tool_id.raw_name, "slow");
    assert_eq!(active.provenance.model_name, expected_model_name);
    assert_eq!(active.provenance.catalog_generation, 1);
    assert!(active.timeout_ms > 0);
    assert!(active.deadline_remaining_ms <= active.timeout_ms);
    assert_eq!(
        manager
            .get_status(server_id)
            .unwrap()
            .unwrap()
            .active_call_count,
        1
    );
    cancellation.cancel();

    let error = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .expect("cancelled call timeout")
        .expect("cancelled call task")
        .expect_err("cancelled peer call");
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert!(manager.active_call(&active_id).unwrap().is_none());
    assert_eq!(manager.active_call_count().unwrap(), 0);

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_owned_tool_timeout_disables_only_the_manager_deadline() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let mut server_config = config(server_id, "host-owned-timeout", true);
    server_config.request_timeout_ms = 20;
    registry.add(server_config).unwrap();
    let server = MockServer::with_tools(vec![descriptor("slow", "host owns its timeout")]);
    server
        .wait_for_call_cancellation
        .store(true, Ordering::SeqCst);
    server.owns_tool_timeout.store(true, Ordering::SeqCst);
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

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !call.is_finished(),
        "the generic Manager deadline must not cancel a Host-budgeted Tool"
    );

    cancellation.cancel();
    let error = tokio::time::timeout(Duration::from_secs(1), call)
        .await
        .expect("explicit cancellation timeout")
        .expect("host-owned timeout call task")
        .expect_err("explicit cancellation must still reach the peer");
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);

    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn active_call_admission_is_bounded_and_releases_capacity() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "bounded-calls", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("slow", "waits for cancellation")]);
    server
        .wait_for_call_cancellation
        .store(true, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(
        registry,
        connector,
        sink,
        McpManagerPolicy {
            max_active_calls_per_server: 1,
            max_active_calls_total: 1,
            ..McpManagerPolicy::default()
        },
    );
    manager.start(server_id).await.unwrap();

    let first_request = catalog_call(&manager, server_id, "slow", json!({}));
    let first_cancellation = McpCancellationToken::new();
    let task_manager = manager.clone();
    let task_cancellation = first_cancellation.clone();
    let first = tokio::spawn(async move {
        task_manager
            .call_catalog_tool(first_request, task_cancellation)
            .await
    });
    wait_until(Duration::from_secs(1), || server.calls().len() == 1).await;

    let error = manager
        .call_catalog_tool(
            catalog_call(&manager, server_id, "slow", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("second concurrent call must be rejected before peer dispatch");
    assert_eq!(error.kind, McpErrorKind::Capacity);
    assert!(error.message.contains("active-call limit"));
    assert_eq!(server.calls().len(), 1);

    first_cancellation.cancel();
    first
        .await
        .expect("join first bounded call")
        .expect_err("first bounded call is conservatively outcome-unknown");
    assert_eq!(manager.active_call_count().unwrap(), 0);

    server
        .wait_for_call_cancellation
        .store(false, Ordering::SeqCst);
    manager
        .call_catalog_tool(
            catalog_call(&manager, server_id, "slow", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect("capacity must be released when the active-call guard settles");
    assert_eq!(manager.active_call_count().unwrap(), 0);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_active_call_admission_spans_servers_and_duplicate_id_spends_no_permit() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let first_server_id = McpServerId::new();
    let second_server_id = McpServerId::new();
    registry
        .add(config(first_server_id, "global-capacity-a", true))
        .unwrap();
    registry
        .add(config(second_server_id, "global-capacity-b", true))
        .unwrap();
    let first_server = MockServer::with_tools(vec![descriptor("slow", "waits")]);
    first_server
        .wait_for_call_cancellation
        .store(true, Ordering::SeqCst);
    let second_server = MockServer::with_tools(vec![descriptor("echo", "returns")]);
    connector.add(first_server_id, Arc::clone(&first_server));
    connector.add(second_server_id, Arc::clone(&second_server));
    let manager = manager(
        registry,
        connector,
        sink,
        McpManagerPolicy {
            max_active_calls_per_server: 1,
            max_active_calls_total: 1,
            ..McpManagerPolicy::default()
        },
    );
    manager.start(first_server_id).await.unwrap();
    manager.start(second_server_id).await.unwrap();

    let fixed_id = McpActiveCallId::new(
        first_server_id,
        McpInvocationId::new(),
        McpModelCallId::new("owned-global-capacity-call").unwrap(),
    );
    let first_cancellation = McpCancellationToken::new();
    let first_manager = manager.clone();
    let first_task_id = fixed_id.clone();
    let first_task_cancellation = first_cancellation.clone();
    let first = tokio::spawn(async move {
        first_manager
            .call_catalog_tool_identified(
                first_task_id,
                catalog_call(&first_manager, first_server_id, "slow", json!({})),
                first_task_cancellation,
            )
            .await
    });
    wait_until(Duration::from_secs(1), || first_server.calls().len() == 1).await;

    let duplicate = manager
        .call_catalog_tool_identified(
            fixed_id,
            catalog_call(&manager, first_server_id, "slow", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("duplicate active identity must fail before reserving capacity");
    assert_eq!(duplicate.kind, McpErrorKind::Config);
    assert_eq!(
        duplicate.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );

    let global = manager
        .call_catalog_tool(
            catalog_call(&manager, second_server_id, "echo", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("one Server must consume the global admission slot");
    assert_eq!(global.kind, McpErrorKind::Capacity);
    assert_eq!(
        global.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert!(second_server.calls().is_empty());

    first_cancellation.cancel();
    first
        .await
        .expect("join first global-capacity call")
        .expect_err("cancelled dispatched call must be outcome-unknown");
    assert_eq!(manager.active_call_count().unwrap(), 0);
    manager
        .call_catalog_tool(
            catalog_call(&manager, second_server_id, "echo", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect("global permit must be immediately reusable after settlement");
    assert_eq!(second_server.calls().len(), 1);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_cancels_and_settles_active_calls_before_closing_peer() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "stop-active-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("slow", "waits for cancellation")]);
    server
        .wait_for_call_cancellation
        .store(true, Ordering::SeqCst);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let request = catalog_call(&manager, server_id, "slow", json!({}));
    let call_manager = manager.clone();
    let call = tokio::spawn(async move {
        call_manager
            .call_catalog_tool(request, McpCancellationToken::new())
            .await
    });
    wait_until(Duration::from_secs(1), || server.calls().len() == 1).await;

    let stopped = manager.stop(server_id).await.unwrap();
    assert_eq!(stopped.state, McpServerState::Disabled);
    assert_eq!(stopped.active_call_count, 0);
    let error = call
        .await
        .expect("active call task")
        .expect_err("stopped post-dispatch call must be outcome-unknown");
    assert_eq!(error.kind, McpErrorKind::OutcomeUnknown);
    assert_eq!(manager.active_call_count().unwrap(), 0);
    assert_eq!(server.close_count.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_approval_mode_allows_manager_dispatch_after_normal_policy_checks() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let mut server_config = config(server_id, "auto-invocation", true);
    server_config.approval_mode = mycopilot_mcp_client::McpApprovalMode::Auto;
    registry.add(server_config).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "automatic")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    let status = manager.start(server_id).await.unwrap();
    assert_eq!(
        status.approval_mode,
        mycopilot_mcp_client::McpApprovalMode::Auto
    );

    let result = manager
        .call_catalog_tool(
            catalog_call(&manager, server_id, "echo", json!({"value": "automatic"})),
            McpCancellationToken::new(),
        )
        .await
        .expect("auto mode may dispatch after normal Manager checks");
    assert!(!result.is_error);
    assert_eq!(server.calls().len(), 1);
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deny_approval_mode_fails_closed_inside_manager() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    let mut server_config = config(server_id, "deny-invocation", true);
    server_config.approval_mode = mycopilot_mcp_client::McpApprovalMode::Deny;
    registry.add(server_config).unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "denied")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    let status = manager.start(server_id).await.unwrap();
    assert_eq!(
        status.approval_mode,
        mycopilot_mcp_client::McpApprovalMode::Deny
    );

    let error = manager
        .call_catalog_tool(
            catalog_call(&manager, server_id, "echo", json!({})),
            McpCancellationToken::new(),
        )
        .await
        .expect_err("deny mode must reject invocation");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(server.calls().is_empty());
    manager.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn catalog_call_revalidates_expected_schema_digest() {
    let registry = InMemoryMcpRegistry::shared();
    let connector = Arc::new(MockConnector::default());
    let sink = Arc::new(RecordingEventSink::default());
    let server_id = McpServerId::new();
    registry
        .add(config(server_id, "schema-bound-call", true))
        .unwrap();
    let server = MockServer::with_tools(vec![descriptor("echo", "schema-bound")]);
    connector.add(server_id, Arc::clone(&server));
    let manager = manager(registry, connector, sink, McpManagerPolicy::default());
    manager.start(server_id).await.unwrap();
    let mut request = catalog_call(&manager, server_id, "echo", json!({}));
    request.expected_schema_digest = "0".repeat(64).parse().expect("fixed test schema digest");

    let error = manager
        .call_catalog_tool(request, McpCancellationToken::new())
        .await
        .expect_err("stale schema digest must fail closed");
    assert_eq!(error.kind, McpErrorKind::Config);
    assert!(server.calls().is_empty());
    manager.stop_all().await;
}
