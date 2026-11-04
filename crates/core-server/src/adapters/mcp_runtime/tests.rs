use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mycopilot_core::image_generation::InMemoryCredentialStore;
use mycopilot_core::{
    mcp_tool_arguments_digest, send_chat_with_host_services, AgentApiStyle, AgentApprovalStatus,
    AgentChatInput, AgentChatMessage, AgentEventEmitter, AgentMcpArgumentSummary,
    AgentMcpToolApprovalSummary, AgentMcpToolRisk, AgentProposedAction, AgentRunContext,
    AgentRuntimeHostServices, AgentToolCall, ConversationTraceSnapshot, McpToolRuntime,
    ModelCapabilities,
};
use mycopilot_mcp_client::{
    BoxMcpFuture, InMemoryMcpRegistry, McpCapabilitySnapshot, McpConnectionState, McpConnector,
    McpImplementationInfo, McpLifecycleKind, McpManagerPolicy, McpPeer, McpProtocolSnapshot,
    McpRegistry, McpServerConfig, McpStdioConfig, McpToolCall, McpToolDescriptor, McpToolPage,
    McpTransportConfig,
};
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;
use crate::application::mcp::approval_payload_store::{
    DurableMcpApprovalPayloadStore, InMemoryMcpApprovalEnvelopeRepository,
    UnavailableMcpApprovalPayloadStore,
};

async fn read_json_request(stream: &mut TcpStream) -> serde_json::Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "fixture model connection closed early");
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
                    .unwrap_or_default();
                body_start = Some(header_end + 4);
                expected_length = Some(header_end + 4 + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_json_response(stream: &mut TcpStream, body: serde_json::Value) {
    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

struct OwnedFixturePeer {
    server_id: McpServerId,
    protocol: McpProtocolSnapshot,
    tool: McpToolDescriptor,
    result: Mutex<McpToolResult>,
    calls: Mutex<Vec<McpToolCall>>,
    wait_for_cancellation: AtomicBool,
    call_started: AtomicBool,
    cancellation_seen: AtomicBool,
}

impl OwnedFixturePeer {
    fn new(server_id: McpServerId) -> Self {
        Self {
            server_id,
            protocol: McpProtocolSnapshot {
                negotiated_version: "2026-07-28".to_string(),
                lifecycle: McpLifecycleKind::Discover,
                server: Some(McpImplementationInfo {
                    name: "core-server-owned-fixture".to_string(),
                    version: "1.0.0".to_string(),
                }),
                capabilities: McpCapabilitySnapshot {
                    tools: true,
                    ..McpCapabilitySnapshot::default()
                },
            },
            tool: McpToolDescriptor {
                name: "raw/echo".to_string(),
                title: None,
                description: Some("Owned read-only fixture.".to_string()),
                input_schema: json!({
                    "type": "object",
                    "properties": {"text": {"type": "string"}},
                    "required": ["text"]
                }),
                output_schema: Some(json!({"type": "object"})),
                annotations: Some(McpToolAnnotations {
                    read_only_hint: Some(true),
                    ..McpToolAnnotations::default()
                }),
            },
            result: Mutex::new(McpToolResult {
                content: vec![
                    McpContentBlock::Text {
                        text: "fixture response".to_string(),
                    },
                    McpContentBlock::Image {
                        data: "encoded-image-secret".to_string(),
                        mime_type: "image/png".to_string(),
                    },
                ],
                structured_content: Some(json!({"sum": 3})),
                is_error: false,
            }),
            calls: Mutex::new(Vec::new()),
            wait_for_cancellation: AtomicBool::new(false),
            call_started: AtomicBool::new(false),
            cancellation_seen: AtomicBool::new(false),
        }
    }
}

impl McpPeer for OwnedFixturePeer {
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
                return Err(McpError::protocol("unexpected owned fixture cursor"));
            }
            Ok(McpToolPage {
                tools: vec![self.tool.clone()],
                next_cursor: None,
                ttl_ms: None,
                cache_scope: None,
            })
        })
    }

    fn call_tool<'a>(
        &'a self,
        call: McpToolCall,
        cancellation: McpCancellationToken,
    ) -> BoxMcpFuture<'a, McpToolResult> {
        Box::pin(async move {
            self.call_started.store(true, Ordering::SeqCst);
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(call);
            if self.wait_for_cancellation.load(Ordering::SeqCst) {
                cancellation.cancelled().await;
                self.cancellation_seen.store(true, Ordering::SeqCst);
                return Err(McpError::cancelled("owned fixture tools/call"));
            }
            Ok(self
                .result
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone())
        })
    }

    fn close(&self) -> BoxMcpFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }
}

struct OwnedFixtureConnector {
    peer: Arc<OwnedFixturePeer>,
}

impl McpConnector for OwnedFixtureConnector {
    fn connect<'a>(&'a self, config: &'a McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async move {
            if config.id != self.peer.server_id {
                return Err(McpError::config("owned fixture identity mismatch"));
            }
            Ok(Arc::clone(&self.peer) as Arc<dyn McpPeer>)
        })
    }
}

fn config_with_mode(server_id: McpServerId, approval_mode: McpApprovalMode) -> McpServerConfig {
    McpServerConfig {
        id: server_id,
        display_name: "owned fixture".to_string(),
        scope: McpServerScope::Project {
            project_id: "project-fixture".to_string(),
        },
        trust: McpTrustLevel::Managed,
        approval_mode,
        enabled: true,
        transport: McpTransportConfig::Stdio(McpStdioConfig {
            program: PathBuf::from("/owned/fixture"),
            arguments: Vec::new(),
            cwd: PathBuf::from("/owned"),
            environment: Vec::new(),
        }),
        connect_timeout_ms: 1_000,
        request_timeout_ms: 1_000,
        shutdown_timeout_ms: 1_000,
    }
}

fn project_context() -> McpToolCatalogContext {
    McpToolCatalogContext {
        project_id: Some("project-fixture".to_string()),
    }
}

async fn ready_bridge() -> (
    Arc<McpRuntimeBridge>,
    Arc<McpConnectionManager>,
    Arc<OwnedFixturePeer>,
) {
    ready_bridge_with_mode(McpApprovalMode::Prompt).await
}

async fn ready_bridge_with_mode(
    approval_mode: McpApprovalMode,
) -> (
    Arc<McpRuntimeBridge>,
    Arc<McpConnectionManager>,
    Arc<OwnedFixturePeer>,
) {
    let server_id = McpServerId::new();
    let registry = InMemoryMcpRegistry::shared();
    registry
        .add(config_with_mode(server_id, approval_mode))
        .unwrap();
    let peer = Arc::new(OwnedFixturePeer::new(server_id));
    let connector: Arc<dyn McpConnector> = Arc::new(OwnedFixtureConnector {
        peer: Arc::clone(&peer),
    });
    let manager = Arc::new(
        McpConnectionManager::without_events(registry, connector, McpManagerPolicy::default())
            .unwrap(),
    );
    manager.start(server_id).await.unwrap();
    (
        Arc::new(McpRuntimeBridge::new(Arc::clone(&manager))),
        manager,
        peer,
    )
}

fn seal_test_approval(
    bridge: &McpRuntimeBridge,
    provenance: AgentMcpToolProvenance,
    arguments: serde_json::Value,
    call_id: &str,
) -> AgentMcpToolApproval {
    seal_test_invocation(
        bridge,
        provenance,
        arguments,
        call_id,
        AgentMcpApprovalMode::Prompt,
        AgentApprovalStatus::Required,
    )
}

fn seal_test_invocation(
    bridge: &McpRuntimeBridge,
    provenance: AgentMcpToolProvenance,
    arguments: serde_json::Value,
    call_id: &str,
    approval_mode: AgentMcpApprovalMode,
    approval_status: AgentApprovalStatus,
) -> AgentMcpToolApproval {
    let call_id = format!("tc1_{}", URL_SAFE_NO_PAD.encode(Sha256::digest(call_id)));
    let created_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(1);
    let approval = AgentMcpToolApproval {
        identity: AgentMcpToolInvocationIdentity {
            action_id: uuid::Uuid::new_v4().to_string(),
            invocation_id: uuid::Uuid::new_v4().to_string(),
            run_id: "owned-adapter-test-run".to_string(),
            call_id: call_id.clone(),
            provenance: provenance.clone(),
            arguments_digest: mcp_tool_arguments_digest(&arguments).unwrap(),
        },
        call: AgentToolCall {
            id: call_id,
            tool: provenance.model_tool_name.clone(),
            args: json!({}),
            approval_status,
            reason: None,
        },
        summary: AgentMcpToolApprovalSummary {
            server_id: provenance.server_id.clone(),
            server_display_name: "owned fixture".to_string(),
            scope: provenance.scope.clone(),
            raw_tool_name: provenance.raw_tool_name.clone(),
            model_tool_name: provenance.model_tool_name.clone(),
            display_reason: None,
            arguments: AgentMcpArgumentSummary {
                encoded_bytes: serde_json::to_vec(&arguments).unwrap().len() as u64,
                top_level_property_count: arguments
                    .as_object()
                    .map_or(0, |object| object.len() as u64),
                string_value_count: 0,
                number_value_count: 0,
                boolean_value_count: 0,
                null_value_count: 0,
                object_value_count: 1,
                array_value_count: 0,
                max_depth: 1,
                truncated: false,
            },
            risk: AgentMcpToolRisk::ReadOnlyClaimed,
            external: true,
        },
        approval_mode,
        payload_persistence: AgentMcpApprovalPayloadPersistence::ProcessOnly,
        created_at,
        expires_at: created_at.saturating_add(60_000),
    };
    validate_mcp_approval_arguments(&approval, &arguments).unwrap();
    let invocation_id =
        McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
    let payload = McpApprovalPayload::from_json(&arguments).unwrap();
    bridge
        .payloads
        .seal(&invocation_id, approval_aad(&approval), payload)
        .unwrap();
    approval
}

struct ClosedRegistrySecurityGate;

impl McpRegistrySecurityGate for ClosedRegistrySecurityGate {
    fn ensure_reconciled(&self) -> Result<(), String> {
        Err("fixed test gate is closed".to_string())
    }
}

#[tokio::test]
async fn frozen_payload_persistence_must_match_the_current_backend() {
    let (process_bridge, manager, _) = ready_bridge().await;
    let descriptor = process_bridge
        .catalog(&project_context())
        .unwrap()
        .remove(0);
    let process_approval = seal_test_approval(
        &process_bridge,
        descriptor.provenance,
        json!({"text": "backend-binding"}),
        "payload-backend-binding",
    );
    assert_eq!(
        process_bridge.current_payload_persistence().unwrap(),
        AgentMcpApprovalPayloadPersistence::ProcessOnly
    );
    process_bridge
        .revalidate_approved(&process_approval)
        .expect("a process-only approval remains valid in its originating process");
    assert_eq!(
        process_bridge.inspect_startup_payload(&process_approval),
        McpApprovalStartupPayloadState::Unavailable,
        "process-only payloads are never restart-recoverable"
    );

    let mut forged_durable = process_approval.clone();
    forged_durable.payload_persistence =
        AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope;
    assert_eq!(
        process_bridge
            .revalidate_approved(&forged_durable)
            .unwrap_err()
            .code(),
        Some("mcp.approval_payload_binding_changed")
    );

    let repository = Arc::new(InMemoryMcpApprovalEnvelopeRepository::default());
    let credentials = Arc::new(InMemoryCredentialStore::default());
    let (durable_store, _) =
        DurableMcpApprovalPayloadStore::provision(repository, credentials).unwrap();
    let durable_bridge =
        McpRuntimeBridge::with_payload_store(Arc::clone(&manager), Arc::new(durable_store));
    assert_eq!(
        durable_bridge.current_payload_persistence().unwrap(),
        AgentMcpApprovalPayloadPersistence::DurableAuthenticatedEnvelope
    );
    assert_eq!(
        durable_bridge
            .revalidate_approved(&process_approval)
            .unwrap_err()
            .code(),
        Some("mcp.approval_payload_binding_changed")
    );
    assert_eq!(
        durable_bridge.inspect_startup_payload(&process_approval),
        McpApprovalStartupPayloadState::Unavailable
    );
    assert_eq!(
        durable_bridge.inspect_startup_payload(&forged_durable),
        McpApprovalStartupPayloadState::Unavailable,
        "a durable marker alone cannot recover a payload from another backend"
    );

    let unavailable_bridge = McpRuntimeBridge::with_payload_store(
        Arc::clone(&manager),
        Arc::new(UnavailableMcpApprovalPayloadStore),
    );
    assert_eq!(
        unavailable_bridge
            .current_payload_persistence()
            .unwrap_err()
            .code(),
        Some("mcp.approval_payload_unavailable")
    );

    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn closed_registry_security_gate_blocks_catalog_and_dispatch_without_consuming_payload() {
    let (healthy_bridge, manager, peer) = ready_bridge().await;
    let descriptor = healthy_bridge
        .catalog(&project_context())
        .unwrap()
        .remove(0);
    let payloads: Arc<dyn McpApprovalPayloadStore> =
        Arc::new(InMemoryMcpApprovalPayloadStore::default());
    let blocked_bridge = McpRuntimeBridge::with_payload_store_projection_limits_and_security_gate(
        Arc::clone(&manager),
        Arc::clone(&payloads),
        McpRuntimeProjectionLimits::default(),
        Arc::new(ClosedRegistrySecurityGate),
    )
    .unwrap();
    let approval = seal_test_approval(
        &blocked_bridge,
        descriptor.provenance,
        json!({"text": "must-not-dispatch"}),
        "closed-registry-gate",
    );
    let invocation_id =
        McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
    let aad = approval_aad(&approval);

    let catalog_error = blocked_bridge
        .catalog(&project_context())
        .expect_err("a closed Registry security gate must hide the external catalog");
    assert_eq!(
        catalog_error.code(),
        Some("mcp.registry_reconciliation_required")
    );
    let dispatch_error = blocked_bridge
        .invoke_approved(
            McpApprovedToolInvocation {
                approval: approval.clone(),
            },
            AgentCancellationToken::new(),
        )
        .await
        .expect_err("a closed Registry security gate must reject dispatch");
    assert_eq!(
        dispatch_error.code(),
        Some("mcp.registry_reconciliation_required")
    );
    assert!(
        payloads.load(&invocation_id, &aad).is_ok(),
        "fail-closed denial must occur before one-time payload consumption"
    );
    assert!(peer
        .calls
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .is_empty());
    manager.stop_all().await;
}

#[tokio::test]
async fn bridge_catalog_budget_fails_before_unbounded_cross_server_aggregation() {
    let (_bridge, manager, _peer) = ready_bridge().await;
    let server_id = manager.list_statuses().unwrap()[0].server_id;
    let tool = manager.catalog(server_id).unwrap().unwrap().tools[0].clone();
    let mut tool_count = 0;
    let mut bytes = 0;
    let limits = McpRuntimeProjectionLimits::default();
    for _ in 0..limits.max_tool_definitions {
        reserve_catalog_budget(&tool, &mut tool_count, &mut bytes, &limits).unwrap();
    }
    let error = reserve_catalog_budget(&tool, &mut tool_count, &mut bytes, &limits).unwrap_err();
    assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));

    let mut oversized = tool;
    oversized.descriptor.input_schema = json!({
        "type": "object",
        "description": "x".repeat(limits.max_catalog_bytes + 1),
    });
    let error = reserve_catalog_budget(&oversized, &mut 0, &mut 0, &limits).unwrap_err();
    assert_eq!(error.code(), Some("mcp.catalog_budget_exceeded"));
    manager.stop_all().await;
}

#[tokio::test]
async fn ready_catalog_routes_by_typed_raw_identity_and_omits_binary_payload() {
    let (bridge, manager, peer) = ready_bridge().await;
    let catalog = bridge.catalog(&project_context()).unwrap();
    assert_eq!(catalog.len(), 1);
    let descriptor = catalog.into_iter().next().unwrap();
    assert_eq!(descriptor.approval_mode, AgentMcpApprovalMode::Prompt);
    assert_eq!(descriptor.provenance.raw_tool_name, "raw/echo");
    assert_eq!(
        descriptor.provenance.model_tool_name,
        "mcp__owned_fixture__raw_echo"
    );
    let status = manager.list_statuses().unwrap().remove(0);
    assert_eq!(
        descriptor.provenance.config_epoch,
        status.config_epoch.to_string()
    );
    assert_eq!(
        descriptor.provenance.registry_revision,
        status.registry_revision
    );
    assert_eq!(
        descriptor.provenance.scope,
        AgentMcpServerScope::Project {
            project_id: "project-fixture".to_string()
        }
    );
    assert_eq!(descriptor.annotations.read_only_hint, Some(true));
    assert!(descriptor.output_schema.is_some());

    let approval = seal_test_approval(
        &bridge,
        descriptor.provenance,
        json!({"text": "hello"}),
        "provider-adapter-call-1",
    );
    let result = bridge
        .invoke_approved(
            McpApprovedToolInvocation { approval },
            AgentCancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result.structured_content, Some(json!({"sum": 3})));
    assert_eq!(
        result.content,
        vec![
            McpToolContentBlock::Text {
                text: "fixture response".to_string()
            },
            McpToolContentBlock::Omitted {
                kind: McpOmittedContentKind::Image,
                mime_type: Some("image/png".to_string()),
                encoded_bytes: Some("encoded-image-secret".len() as u64)
            }
        ]
    );
    {
        let calls = peer.calls.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].name, "raw/echo");
        assert_eq!(calls[0].arguments, json!({"text": "hello"}));
    }
    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn auto_catalog_is_visible_and_revalidation_requires_the_exact_policy() {
    let (bridge, manager, peer) = ready_bridge_with_mode(McpApprovalMode::Auto).await;
    let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
    assert_eq!(descriptor.approval_mode, AgentMcpApprovalMode::Auto);

    let approval = seal_test_invocation(
        &bridge,
        descriptor.provenance,
        json!({"text": "automatic"}),
        "provider-adapter-auto-call",
        AgentMcpApprovalMode::Auto,
        AgentApprovalStatus::Approved,
    );
    bridge
        .revalidate_approved(&approval)
        .expect("an exact automatic policy snapshot must remain routable");
    let result = bridge
        .invoke_approved(
            McpApprovedToolInvocation {
                approval: approval.clone(),
            },
            AgentCancellationToken::new(),
        )
        .await
        .expect("automatic policy must use the same sealed one-time invocation path");
    assert!(!result.is_error);

    let mut stale_prompt = approval.clone();
    stale_prompt.approval_mode = AgentMcpApprovalMode::Prompt;
    stale_prompt.call.approval_status = AgentApprovalStatus::Required;
    assert_eq!(
        bridge
            .revalidate_approved(&stale_prompt)
            .unwrap_err()
            .code(),
        Some("mcp.approval_snapshot_stale")
    );
    assert_eq!(
        peer.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1
    );
    assert_eq!(
        bridge
            .invoke_approved(
                McpApprovedToolInvocation { approval },
                AgentCancellationToken::new(),
            )
            .await
            .expect_err("an automatically dispatched payload must not replay")
            .code(),
        Some("mcp.approval_payload_unavailable")
    );
    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn final_revalidation_checks_raw_and_normalized_schema_identities_independently() {
    let (bridge, manager, peer) = ready_bridge().await;
    let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
    assert_ne!(
        descriptor.provenance.catalog_schema_digest,
        descriptor.provenance.schema_digest
    );
    let approval = seal_test_approval(
        &bridge,
        descriptor.provenance,
        json!({"text": "identity"}),
        "provider-adapter-schema-identity",
    );
    bridge.revalidate_approved(&approval).unwrap();

    let mut raw_drift = approval.clone();
    raw_drift.identity.provenance.catalog_schema_digest = "f".repeat(64);
    assert_eq!(
        bridge.revalidate_approved(&raw_drift).unwrap_err().code(),
        Some("mcp.approval_tool_drift")
    );

    let mut normalized_drift = approval.clone();
    normalized_drift.identity.provenance.schema_digest = "e".repeat(64);
    assert_eq!(
        bridge
            .revalidate_approved(&normalized_drift)
            .unwrap_err()
            .code(),
        Some("mcp.approval_tool_schema_drift")
    );

    let mut epoch_drift = approval.clone();
    epoch_drift.identity.provenance.config_epoch = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        bridge.revalidate_approved(&epoch_drift).unwrap_err().code(),
        Some("mcp.approval_snapshot_stale")
    );

    let mut revision_drift = approval.clone();
    revision_drift.identity.provenance.registry_revision += 1;
    assert_eq!(
        bridge
            .revalidate_approved(&revision_drift)
            .unwrap_err()
            .code(),
        Some("mcp.approval_snapshot_stale")
    );

    let mut normalizer_drift = approval;
    normalizer_drift
        .identity
        .provenance
        .schema_normalizer_version += 1;
    assert_eq!(
        bridge
            .revalidate_approved(&normalizer_drift)
            .unwrap_err()
            .code(),
        Some("mcp.approval_tool_schema_drift")
    );
    assert!(
        peer.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty(),
        "schema identity revalidation must never dispatch"
    );
    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn server_scope_is_revalidated_independently_of_connection_trust() {
    let (bridge, manager, _) = ready_bridge().await;
    assert!(bridge
        .catalog(&McpToolCatalogContext::default())
        .unwrap()
        .is_empty());
    assert!(bridge
        .catalog(&McpToolCatalogContext {
            project_id: Some("different-project".to_string()),
        })
        .unwrap()
        .is_empty());
    assert_eq!(bridge.catalog(&project_context()).unwrap().len(), 1);
    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn agent_cancellation_is_forwarded_to_mcp_call() {
    let (bridge, manager, peer) = ready_bridge().await;
    peer.wait_for_cancellation.store(true, Ordering::SeqCst);
    let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
    let approval = seal_test_approval(
        &bridge,
        descriptor.provenance,
        json!({"text": "cancel"}),
        "provider-adapter-cancel-call",
    );
    let cancellation = AgentCancellationToken::new();
    let trigger = cancellation.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        trigger.cancel();
    });
    let error = bridge
        .invoke_approved(McpApprovedToolInvocation { approval }, cancellation)
        .await
        .unwrap_err();
    assert_eq!(error.code(), Some("mcp.tool_outcome_unknown"));
    assert!(peer.cancellation_seen.load(Ordering::SeqCst));
    let _ = manager.stop_all().await;
}

#[test]
fn authoritative_oversized_response_is_non_retryable_not_outcome_unknown() {
    let error = map_invocation_error(McpError::output_too_large(
        "MCP tools/call",
        "fixture response exceeded limit",
    ));
    assert_eq!(error.code(), Some("mcp.tool_output_too_large"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[test]
fn host_capacity_is_not_misreported_as_a_stale_tool_snapshot() {
    let error = map_invocation_error(McpError::capacity(
        "untrusted internal active-call capacity detail",
    ));
    assert_eq!(error.code(), Some("mcp.tool_capacity_exceeded"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details.get("dispatchCertainty"))
            .and_then(serde_json::Value::as_str),
        Some("definitely_not_dispatched")
    );
}

#[test]
fn invocation_error_projection_preserves_safe_dispatch_certainty() {
    let possibly_dispatched = map_invocation_error(
        McpError::protocol("untrusted fixture diagnostic")
            .with_dispatch_certainty(McpDispatchCertainty::PossiblyDispatched),
    );
    assert_eq!(possibly_dispatched.code(), Some("mcp.tool_outcome_unknown"));
    assert_eq!(
        possibly_dispatched
            .details()
            .and_then(|details| details.get("dispatchCertainty"))
            .and_then(serde_json::Value::as_str),
        Some("possibly_dispatched")
    );
    assert_eq!(
        possibly_dispatched
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );

    let response_received = map_invocation_error(
        McpError::protocol("another untrusted fixture diagnostic")
            .with_dispatch_certainty(McpDispatchCertainty::ResponseReceived),
    );
    assert_eq!(response_received.code(), Some("mcp.tool_unavailable"));
    assert_eq!(
        response_received
            .details()
            .and_then(|details| details.get("dispatchCertainty"))
            .and_then(serde_json::Value::as_str),
        Some("response_received")
    );
    assert_eq!(
        response_received
            .details()
            .and_then(|details| details.get("retryable"))
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
}

#[test]
fn bridge_rejects_deep_structured_content_before_recursive_encoding() {
    let limits = McpRuntimeProjectionLimits::default();
    let mut value = serde_json::Value::Null;
    for _ in 0..limits.max_model_structured_depth {
        value = serde_json::json!({"nested": value});
    }
    assert!(!structured_content_within_limits(
        &serde_json::json!({"root": value}),
        &limits,
    ));
}

#[test]
fn bridge_result_projection_uses_the_host_owned_limits() {
    let limits = McpRuntimeProjectionLimits {
        max_model_text_bytes: 4,
        max_content_blocks: 1,
        ..McpRuntimeProjectionLimits::default()
    };
    limits.validate().unwrap();

    let mapped = map_tool_result(
        McpToolResult {
            content: vec![
                McpContentBlock::Text {
                    text: "abcdef".to_string(),
                },
                McpContentBlock::Text {
                    text: "not retained".to_string(),
                },
            ],
            structured_content: None,
            is_error: false,
        },
        &limits,
    )
    .unwrap();
    assert_eq!(
        mapped.content,
        vec![McpToolContentBlock::Text {
            text: "abcd".to_string(),
        }]
    );
    assert!(mapped.truncated_at_source);
}

#[tokio::test]
async fn sealed_approval_payload_is_consumed_at_most_once() {
    let (bridge, manager, peer) = ready_bridge().await;
    let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
    let approval = seal_test_approval(
        &bridge,
        descriptor.provenance,
        json!({"text": "once"}),
        "provider-adapter-once-call",
    );
    bridge
        .invoke_approved(
            McpApprovedToolInvocation {
                approval: approval.clone(),
            },
            AgentCancellationToken::new(),
        )
        .await
        .unwrap();

    let error = bridge
        .invoke_approved(
            McpApprovedToolInvocation { approval },
            AgentCancellationToken::new(),
        )
        .await
        .expect_err("a consumed approval payload must not replay");
    assert_eq!(error.code(), Some("mcp.approval_payload_unavailable"));
    assert_eq!(
        peer.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .len(),
        1
    );
    let _ = manager.stop_all().await;
}

#[tokio::test]
async fn agent_runtime_calls_owned_fixture_through_catalog_bridge_end_to_end() {
    const MODEL_PRIVATE_ARGUMENT: &str = "fixture-private-token-never-persist";
    const MCP_PRIVATE_RESULT: &str = "fixture-private-result-never-persist";
    let (bridge, manager, peer) = ready_bridge().await;
    peer.result
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .content
        .push(McpContentBlock::Text {
            text: MCP_PRIVATE_RESULT.to_string(),
        });
    let descriptor = bridge.catalog(&project_context()).unwrap().remove(0);
    let model_name = descriptor.provenance.model_tool_name.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let response_model_name = model_name.clone();
    let server = tokio::spawn(async move {
        for _ in 0..1 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            requests_for_server
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(request);
            let response = json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "provider-fixture-call",
                            "type": "function",
                            "function": {
                                "name": response_model_name,
                                "arguments": format!(
                                    "{{\"text\":\"runtime request\",\"api_token\":\"{}\"}}",
                                    MODEL_PRIVATE_ARGUMENT
                                )
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            });
            write_json_response(&mut stream, response).await;
        }
    });

    let provider_profile_config = mycopilot_core::ProviderProfileConfig::generic_for_dialect(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
    );
    let provider_configuration_revision =
        Some(format!("provider-protocol-v1:{}", uuid::Uuid::new_v4()));
    let provider_protocol_key = mycopilot_core::ProviderProtocolKey::new(
        mycopilot_core::ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile_config,
        "owned-fixture-model",
        provider_configuration_revision.clone(),
    )
    .unwrap();

    let input = AgentChatInput {
        initial_conversation_trace: None,
        context_image_attachments: Vec::new(),
        folder_references: Vec::new(),
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "owned-fixture-token".to_string(),
        provider_configuration_revision,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: Some(provider_profile_config),
        provider_protocol_key: Some(provider_protocol_key),
        model_config_id: None,
        model: "owned-fixture-model".to_string(),
        model_capabilities: ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(4_096),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-mcp-e2e".to_string()),
            project_id: Some("project-fixture".to_string()),
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
        assistant_message_id: Some("assistant-mcp-e2e".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: None,
        skill_discovery: None,
        messages: vec![AgentChatMessage {
            conversation_completion_covered: false,
            message_id: Some("user-mcp-e2e".to_string()),
            role: "user".to_string(),
            content: "Call the owned MCP fixture.".to_string(),
            created_at: Some(1),
            conversation_turn_trace: None,
            conversation_model_context_items: Vec::new(),
        }],
    };
    let invoker: Arc<dyn McpToolInvoker> = bridge.clone();
    let trace_snapshots = Arc::new(Mutex::new(Vec::<ConversationTraceSnapshot>::new()));
    let trace_snapshots_for_observer = Arc::clone(&trace_snapshots);
    let host_services = AgentRuntimeHostServices::new()
        .with_mcp_tools(McpToolRuntime::capture_for_context(
            invoker,
            project_context(),
        ))
        .with_trace_observer(Arc::new(move |snapshot| {
            trace_snapshots_for_observer
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(snapshot.into_snapshot());
            Ok(None)
        }));
    let emitter: AgentEventEmitter = Arc::new(|_| {});
    let output = send_chat_with_host_services(
        input,
        "run-mcp-e2e".to_string(),
        emitter,
        AgentCancellationToken::new(),
        host_services,
    )
    .await
    .unwrap();
    server.await.unwrap();

    assert_eq!(
        output.status,
        mycopilot_core::AgentRunStatus::WaitingForApproval
    );
    {
        let requests = requests.lock().unwrap_or_else(|error| error.into_inner());
        assert_eq!(requests.len(), 1);
        let projected_tool = requests[0]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == model_name)
            .expect("friendly MCP tool must enter the Provider payload");
        let projected_description = projected_tool["function"]["description"]
            .as_str()
            .expect("MCP tool description");
        assert!(projected_description.contains("MCP server: \"owned fixture\""));
        assert!(projected_description.contains("MCP tool: \"raw/echo\""));
        assert!(!projected_description.contains("Treat the following"));
        let system_prompt = requests[0]["messages"][0]["content"]
            .as_str()
            .expect("stable system prompt");
        assert_eq!(
            system_prompt.matches("动态 MCP 工具的服务器标签").count(),
            1
        );
    }
    assert!(
        peer.calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty(),
        "Prompt mode must stop at a prepared approval before dispatch"
    );
    let approval = match output.proposed_actions.as_slice() {
        [AgentProposedAction::McpToolCall { approval }] => approval.as_ref().clone(),
        actions => panic!("expected one typed MCP approval, got {actions:?}"),
    };
    assert!(approval
        .call
        .args
        .as_object()
        .is_some_and(|args| args.is_empty()));
    let invocation_id =
        McpApprovalInvocationId::parse(approval.identity.invocation_id.clone()).unwrap();
    let sealed = bridge
        .payloads
        .load(&invocation_id, &approval_aad(&approval))
        .expect("prepare_approval seals the transient argument payload");
    assert!(!format!("{sealed:?}").contains(MODEL_PRIVATE_ARGUMENT));
    sealed
        .with_json(|arguments| {
            assert_eq!(arguments["api_token"], MODEL_PRIVATE_ARGUMENT);
        })
        .unwrap();

    let durable_output = serde_json::to_string(&output).unwrap();
    assert!(!durable_output.contains(MODEL_PRIVATE_ARGUMENT));
    assert!(
        !durable_output.contains(MCP_PRIVATE_RESULT),
        "no MCP output exists before the approved invocation"
    );
    {
        let trace_snapshots = trace_snapshots
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let serialized_snapshots = serde_json::to_string(
            &trace_snapshots
                .iter()
                .map(|snapshot| &snapshot.model_context_items)
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(!serialized_snapshots.contains(MODEL_PRIVATE_ARGUMENT));
        assert!(!serialized_snapshots.contains(MCP_PRIVATE_RESULT));
    }
    let _ = manager.stop_all().await;
}

#[test]
fn resource_uri_and_binary_content_are_not_retained() {
    let mapped = map_content_block(
        McpContentBlock::EmbeddedResource {
            resource: McpEmbeddedResource::Blob {
                uri: "https://secret.invalid/private".to_string(),
                mime_type: Some("application/octet-stream".to_string()),
                data: "private-base64".to_string(),
            },
        },
        &McpRuntimeProjectionLimits::default(),
    );
    assert_eq!(
        mapped,
        McpToolContentBlock::Omitted {
            kind: McpOmittedContentKind::EmbeddedResource,
            mime_type: Some("application/octet-stream".to_string()),
            encoded_bytes: Some("private-base64".len() as u64),
        }
    );
    let rendered = format!("{mapped:?}");
    assert!(!rendered.contains("secret.invalid"));
    assert!(!rendered.contains("private-base64"));
}

#[test]
fn diagnostics_are_bounded_and_replace_the_previous_snapshot() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let bridge = runtime.block_on(async {
        let registry = InMemoryMcpRegistry::shared();
        let connector: Arc<dyn McpConnector> = Arc::new(OwnedFixtureConnector {
            peer: Arc::new(OwnedFixturePeer::new(McpServerId::new())),
        });
        McpRuntimeBridge::new(Arc::new(
            McpConnectionManager::without_events(registry, connector, McpManagerPolicy::default())
                .unwrap(),
        ))
    });
    let diagnostic = McpToolRegistrationDiagnostic {
        server_id: None,
        model_tool_name: None,
        code: mycopilot_core::McpToolDiagnosticCode::CatalogUnavailable,
        message: "safe diagnostic".to_string(),
    };
    bridge.report_diagnostics(&vec![diagnostic; MAX_RETAINED_DIAGNOSTICS + 10]);
    assert_eq!(bridge.diagnostics().len(), MAX_RETAINED_DIAGNOSTICS);
}
