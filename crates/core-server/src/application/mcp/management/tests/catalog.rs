use std::sync::Arc;

use base64::Engine as _;
use mycopilot_mcp_client::{
    BoxMcpFuture, McpCancellationToken, McpCapabilitySnapshot, McpConnectionState, McpConnector,
    McpPeer, McpProtocolSnapshot, McpToolCall, McpToolDescriptor, McpToolPage, McpToolResult,
};
use serde_json::{json, Value};

use super::*;

struct CatalogFixturePeer {
    server_id: McpServerId,
    protocol: McpProtocolSnapshot,
}

impl CatalogFixturePeer {
    fn new(server_id: McpServerId) -> Self {
        Self {
            server_id,
            protocol: McpProtocolSnapshot {
                negotiated_version: "2026-07-28".to_string(),
                lifecycle: McpLifecycleKind::Discover,
                server: None,
                capabilities: McpCapabilitySnapshot {
                    tools: true,
                    ..McpCapabilitySnapshot::default()
                },
            },
        }
    }
}

impl McpPeer for CatalogFixturePeer {
    fn server_id(&self) -> McpServerId {
        self.server_id
    }

    fn connection_state(&self) -> McpConnectionState {
        McpConnectionState::Ready
    }

    fn protocol_snapshot(&self) -> &McpProtocolSnapshot {
        &self.protocol
    }

    fn list_tools<'a>(&'a self, cursor: Option<String>) -> BoxMcpFuture<'a, McpToolPage> {
        Box::pin(async move {
            if cursor.is_some() {
                return Err(McpError::protocol(
                    "the management Catalog fixture has one page",
                ));
            }
            Ok(McpToolPage {
                tools: vec![McpToolDescriptor {
                    name: "stale_fixture_tool".to_string(),
                    title: None,
                    description: Some("A bounded management Catalog fixture.".to_string()),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    annotations: None,
                }],
                next_cursor: None,
                ttl_ms: None,
                cache_scope: None,
            })
        })
    }

    fn call_tool<'a>(
        &'a self,
        _call: McpToolCall,
        _cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        Box::pin(async {
            Err(McpError::protocol(
                "the management Catalog fixture does not execute tools",
            ))
        })
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct CatalogFixtureConnector;

impl McpConnector for CatalogFixtureConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        let server_id = config.id;
        Box::pin(
            async move { Ok(Arc::new(CatalogFixturePeer::new(server_id)) as Arc<dyn McpPeer>) },
        )
    }
}

#[tokio::test(flavor = "current_thread")]
async fn catalog_projection_rejects_snapshot_from_previous_registry_identity() {
    let harness = TestHarness::with_connector(Arc::new(CatalogFixtureConnector));
    let added = harness
        .service
        .add_server(create_input("Catalog identity fixture"))
        .expect("add server");
    let authorized = authorize(&harness.service, &added.server);
    let enabled = harness
        .service
        .enable_server(mutation_input(&authorized.server))
        .expect("enable server");
    let started = harness
        .service
        .start_server(
            mutation_input(&enabled.server),
            McpManagementOperationDto::Start,
        )
        .await
        .expect("start bounded Catalog fixture");
    let server_id = server_id(&started.server);
    let page_input = McpCatalogToolsPageInput {
        schema_version: MCP_MANAGEMENT_SCHEMA_VERSION,
        server_id: server_id.to_string(),
        cursor: None,
        limit: 25,
    };
    let current = harness
        .service
        .list_tools(page_input.clone())
        .expect("current Catalog identity is projectable");
    assert_eq!(current.tools.len(), 1);
    assert_eq!(current.tools[0].raw_name, "stale_fixture_tool");

    let persisted = harness
        .registry
        .get_persisted(server_id)
        .expect("read current Registry identity")
        .expect("server exists");
    let mut changed_config = persisted.entry.config.clone();
    changed_config.display_name = "Catalog identity changed".to_string();
    let mutation = harness
        .registry
        .update_with_precondition(
            &McpRegistryMutationPrecondition::from_entry(&persisted.entry),
            changed_config,
        )
        .expect("commit a newer Registry identity");
    let McpRegistryMutation::Updated(changed) = mutation else {
        panic!("display name change must update Registry identity");
    };
    assert_ne!(changed.config_epoch, persisted.entry.config_epoch);
    assert_ne!(changed.revision, persisted.entry.revision);
    assert_ne!(changed.config_digest, persisted.entry.config_digest);

    // This synchronous read intentionally occurs before the Manager watcher can
    // invalidate its old snapshot. The Host boundary must reject that snapshot
    // rather than returning descriptors produced by the prior Registry identity.
    let failure = harness
        .service
        .list_tools(page_input)
        .expect_err("stale Catalog descriptors must fail closed")
        .into_data();
    assert_eq!(failure.code, McpManagementErrorCodeDto::Conflict);
    assert_eq!(failure.recovery, McpManagementRecoveryDto::Refresh);

    harness.shutdown().await;
}

#[test]
fn host_catalog_cursor_rejects_generation_digest_and_server_drift() {
    let server_id = McpServerId::new();
    let mut catalog = McpCatalogSnapshot::empty(server_id);
    catalog.generation = 7;
    let cursor = encode_catalog_cursor(&catalog, 11).expect("encode Host cursor");
    assert_eq!(
        decode_catalog_cursor(&cursor, &catalog, McpManagementOperationDto::ListTools)
            .expect("decode unchanged Host cursor"),
        11
    );

    let mut newer_generation = catalog.clone();
    newer_generation.generation += 1;
    let generation_error = decode_catalog_cursor(
        &cursor,
        &newer_generation,
        McpManagementOperationDto::ListTools,
    )
    .expect_err("Catalog generation drift must stale the Host cursor");
    assert_eq!(
        failure_code(generation_error),
        McpManagementErrorCodeDto::Conflict
    );

    let other_server = McpCatalogSnapshot::empty(McpServerId::new());
    let server_error =
        decode_catalog_cursor(&cursor, &other_server, McpManagementOperationDto::ListTools)
            .expect_err("a Host cursor is bound to one Server ID");
    assert_eq!(
        failure_code(server_error),
        McpManagementErrorCodeDto::Conflict
    );

    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&cursor)
        .expect("decode cursor envelope");
    let mut envelope: Value = serde_json::from_slice(&raw).expect("parse cursor envelope");
    envelope["catalogDigest"] = Value::String("f".repeat(64));
    let digest_cursor = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&envelope).expect("encode changed cursor envelope"));
    let digest_error = decode_catalog_cursor(
        &digest_cursor,
        &catalog,
        McpManagementOperationDto::ListTools,
    )
    .expect_err("Catalog digest drift must stale the Host cursor");
    assert_eq!(
        failure_code(digest_error),
        McpManagementErrorCodeDto::Conflict
    );
}
