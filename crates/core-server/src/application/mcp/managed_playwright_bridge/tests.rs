use super::*;
use mycopilot_mcp_client::{
    InMemoryMcpRegistry, McpApprovalMode, McpContentBlock, McpHostBridgeConfig, McpManagerPolicy,
    McpRegistry, McpServerScope, McpTrustLevel,
};
use std::collections::{BTreeSet, VecDeque};

#[cfg(target_os = "macos")]
use crate::application::mcp::browser_risk::BrowserRiskCoordinator;
#[cfg(target_os = "macos")]
use crate::application::mcp::builtin_capability_policy::{
    BuiltinCapabilityId as StoredCapabilityId, SqliteBuiltinCapabilityPolicyStore,
};
#[cfg(target_os = "macos")]
use crate::application::mcp::builtin_capability_runtime::HostBuiltinCapabilityProvider;
#[cfg(target_os = "macos")]
use crate::application::mcp::playwright_manifest::BROWSER_AUTOMATION_CAPABILITY_ID;
#[cfg(target_os = "macos")]
use mycopilot_core::{
    AgentApprovalStatus, AgentBuiltinCapabilityActivationApproval, AgentEvent, AgentProposedAction,
    AgentRunStatus, BuiltinCapabilityRuntime, BuiltinMcpToolRiskKind, CapabilityGrant,
    BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS,
};
#[cfg(target_os = "macos")]
use mycopilot_protocol_rs::{
    BrowserRiskAuthorizeInput, BrowserRiskCancelInput, BuiltinMcpToolRiskKindDto,
    ManagedPlaywrightBuiltinToolGrantContext, ManagedPlaywrightSensitiveBindingScopeDto,
    ManagedPlaywrightSensitiveFilePreparation, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION,
};
#[cfg(target_os = "macos")]
use std::process::Stdio;
#[cfg(target_os = "macos")]
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
#[cfg(target_os = "macos")]
use tokio::process::Command;

#[cfg(target_os = "macos")]
const FIXTURE_AGENT_EVENT_METHOD: &str = "fixture.agentEvent";

fn test_authorization_context() -> ManagedPlaywrightAuthorizationContext {
    ManagedPlaywrightAuthorizationContext {
        conversation_id: Some("conversation-test".to_string()),
        run_id: "run-test".to_string(),
        capability_id: "browser_automation".to_string(),
        activation_id: Uuid::new_v4().to_string(),
        manifest_digest: format!("sha256:{}", "a".repeat(64)),
        policy_revision: 1,
        grant_expires_at_ms: 2_000_000_000_000,
        invocation_id: Uuid::new_v4().to_string(),
        call_id: "call-test".to_string(),
        trigger_tool_name: "browser_snapshot".to_string(),
        call_reason: "Exercise the reverse bridge dispatch boundary.".to_string(),
        builtin_tool_grant: None,
    }
}

fn test_call_command() -> ManagedPlaywrightCommand {
    ManagedPlaywrightCommand::CallTool {
        name: "browser_snapshot".to_string(),
        arguments: json!({
            "call_reason": "Exercise the reverse bridge dispatch boundary."
        }),
        timeout_ms: 1_000,
        authorization_context: Box::new(test_authorization_context()),
    }
}

fn test_protocol_snapshot() -> McpProtocolSnapshot {
    serde_json::from_value(json!({
        "negotiatedVersion": "2025-11-25",
        "lifecycle": "initialize_fallback",
        "server": {"name": "@playwright/mcp", "version": "0.0.79"},
        "capabilities": {
            "tools": true,
            "toolsListChanged": false,
            "resources": false,
            "resourcesListChanged": false,
            "resourcesSubscribe": false,
            "prompts": false,
            "promptsListChanged": false,
            "logging": false,
            "completions": false,
            "tasks": false,
            "extensions": []
        }
    }))
    .unwrap()
}

#[tokio::test]
async fn reverse_peer_pre_dispatch_rejections_never_cross_the_queue_boundary() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let error = bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            None,
            Some(dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(bridge.pending_request_count(), 0);

    let cancelled_bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    cancelled_bridge.attach_outbound(outbound).unwrap();
    let cancellation = McpCancellationToken::new();
    cancellation.cancel();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let error = cancelled_bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            Some(cancellation),
            Some(dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Cancelled);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert!(matches!(
        commands.try_recv(),
        Err(mpsc::error::TryRecvError::Empty)
    ));
    assert_eq!(cancelled_bridge.pending_request_count(), 0);

    let (outbound, commands) = mpsc::unbounded_channel();
    drop(commands);
    bridge.attach_outbound(outbound).unwrap();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let error = bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            None,
            Some(dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(bridge.pending_request_count(), 0);

    let poisoned_bridge = ManagedPlaywrightHostBridge::new(server_id);
    assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe({
        let poisoned_bridge = Arc::clone(&poisoned_bridge);
        move || {
            let _outbound = poisoned_bridge.outbound.lock().unwrap();
            panic!("poison the outbound lock");
        }
    }))
    .is_err());
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let error = poisoned_bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            None,
            Some(dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(poisoned_bridge.pending_request_count(), 0);

    bridge.close_now();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let error = bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            None,
            Some(dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn reverse_call_tool_wait_has_no_transport_deadline_but_still_cancels() {
    let bridge = ManagedPlaywrightHostBridge::new(McpServerId::new());
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let cancellation = McpCancellationToken::new();
    let request_cancellation = cancellation.clone();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let task = {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_millis(20),
                    Some(request_cancellation),
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.expect("call command");
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();

    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(
        !task.is_finished(),
        "CallTool bridge wait must outlive its execution budget while Main owns that budget"
    );

    cancellation.cancel();
    let _cancel = commands.recv().await.expect("explicit cancel notification");
    assert!(bridge
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::Cancelled,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            },
        })
        .unwrap());
    assert!(matches!(
        task.await.unwrap().unwrap(),
        ManagedPlaywrightCompletionOutcome::Error {
            code: ManagedPlaywrightBridgeErrorCode::Cancelled,
            ..
        }
    ));
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn reverse_bridge_capacity_rejection_does_not_queue_or_leak_the_rejected_request() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let mut tasks = Vec::new();
    for _ in 0..MAX_PENDING_REQUESTS {
        let bridge = Arc::clone(&bridge);
        tasks.push(tokio::spawn(async move {
            let dispatch = McpDispatchTracker::new();
            dispatch.mark_dispatching();
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(10),
                    None,
                    Some(dispatch),
                )
                .await
        }));
        let command = commands.recv().await.unwrap();
        assert_eq!(
            command["method"],
            json!(MANAGED_PLAYWRIGHT_COMMAND_NOTIFICATION_METHOD)
        );
    }
    assert_eq!(bridge.pending_request_count(), MAX_PENDING_REQUESTS);

    let rejected_dispatch = McpDispatchTracker::new();
    rejected_dispatch.mark_dispatching();
    let error = bridge
        .request(
            test_call_command(),
            PendingOperation::CallTool,
            Duration::from_secs(1),
            None,
            Some(rejected_dispatch.clone()),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Capacity);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(
        rejected_dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(bridge.pending_request_count(), MAX_PENDING_REQUESTS);

    bridge.abort_pending(ManagedPlaywrightCancelReason::Shutdown);
    for task in tasks {
        assert!(matches!(
            task.await.unwrap().unwrap(),
            ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::Closed,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            }
        ));
    }
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn dropping_a_reverse_request_future_removes_pending_and_notifies_main() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let task = {
        let bridge = Arc::clone(&bridge);
        tokio::spawn(async move {
            let dispatch = McpDispatchTracker::new();
            dispatch.mark_dispatching();
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(10),
                    None,
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.unwrap();
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert_eq!(bridge.pending_request_count(), 1);

    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(bridge.pending_request_count(), 0);
    let cancel = commands.recv().await.unwrap();
    let cancel_params: ManagedPlaywrightCancelNotification =
        serde_json::from_value(cancel["params"].clone()).unwrap();
    assert_eq!(cancel_params.request_id, params.request_id);
    assert_eq!(
        cancel_params.reason,
        ManagedPlaywrightCancelReason::Shutdown
    );
}

#[tokio::test]
async fn successful_reverse_send_is_the_only_manager_queue_boundary() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let task = {
        let bridge = Arc::clone(&bridge);
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(1),
                    None,
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.unwrap();
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::PossiblyDispatched
    );
    assert_eq!(bridge.pending_request_count(), 1);
    assert!(bridge
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::ToolCalled {
                result: json!({"content": [], "structuredContent": null, "isError": false}),
                host_artifact_publish_path: None,
            },
        })
        .unwrap());
    assert!(matches!(
        task.await.unwrap().unwrap(),
        ManagedPlaywrightCompletionOutcome::ToolCalled { .. }
    ));
    assert_eq!(dispatch.certainty(), McpDispatchCertainty::ResponseReceived);
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn cancellation_grace_accepts_exact_pre_dispatch_completion_without_leaking_pending() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let cancellation = McpCancellationToken::new();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let task = {
        let bridge = Arc::clone(&bridge);
        let cancellation = cancellation.clone();
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(10),
                    Some(cancellation),
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.unwrap();
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert!(bridge
        .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id.clone(),
            phase: ManagedPlaywrightDispatchPhase::PreDispatch,
        })
        .unwrap());
    cancellation.cancel();
    let cancel = commands.recv().await.unwrap();
    assert_eq!(
        cancel["method"],
        json!(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD)
    );
    assert_eq!(bridge.pending_request_count(), 1);
    assert!(bridge
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::Cancelled,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            },
        })
        .unwrap());
    let outcome = task.await.unwrap().unwrap();
    let error = error_from_outcome(outcome, PendingOperation::CallTool);
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Cancelled);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert_eq!(bridge.pending_request_count(), 0);
}

#[test]
fn queue_timeout_is_undispatched_capacity_with_actionable_retry_guidance() {
    let outcome: ManagedPlaywrightCompletionOutcome = serde_json::from_value(json!({
        "type": "error",
        "code": "queue_timeout",
        "dispatchCertainty": "definitely_not_dispatched"
    }))
    .unwrap();
    let error = error_from_outcome(outcome, PendingOperation::CallTool);
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Capacity);
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
    assert!(error.message.contains("before this action started"));
    assert!(error.message.contains("then retry"));
}

#[tokio::test]
async fn cancellation_after_main_dispatch_is_unknown_and_cleans_pending_after_grace() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let cancellation = McpCancellationToken::new();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let task = {
        let bridge = Arc::clone(&bridge);
        let cancellation = cancellation.clone();
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(10),
                    Some(cancellation),
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.unwrap();
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert!(bridge
        .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id.clone(),
            phase: ManagedPlaywrightDispatchPhase::PossiblyDispatched,
        })
        .unwrap());
    assert!(bridge
        .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id,
            phase: ManagedPlaywrightDispatchPhase::PreDispatch,
        })
        .unwrap());
    cancellation.cancel();
    let _cancel = commands.recv().await.unwrap();
    assert_eq!(bridge.pending_request_count(), 1);
    let error = tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.kind,
        mycopilot_mcp_client::McpErrorKind::OutcomeUnknown
    );
    assert_eq!(
        error.dispatch_certainty,
        Some(McpDispatchCertainty::PossiblyDispatched)
    );
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn main_dispatch_completion_cannot_regress_acknowledged_certainty() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let dispatch = McpDispatchTracker::new();
    dispatch.mark_dispatching();
    let task = {
        let bridge = Arc::clone(&bridge);
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            bridge
                .request(
                    test_call_command(),
                    PendingOperation::CallTool,
                    Duration::from_secs(1),
                    None,
                    Some(dispatch),
                )
                .await
        })
    };
    let command = commands.recv().await.unwrap();
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert!(bridge
        .acknowledge_dispatch_phase(ManagedPlaywrightDispatchPhaseInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id.clone(),
            phase: ManagedPlaywrightDispatchPhase::PossiblyDispatched,
        })
        .unwrap());
    assert!(!bridge
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: params.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::Error {
                code: ManagedPlaywrightBridgeErrorCode::InvalidArguments,
                dispatch_certainty: ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
            },
        })
        .unwrap());
    let outcome = task.await.unwrap().unwrap();
    assert!(matches!(
        outcome,
        ManagedPlaywrightCompletionOutcome::Error {
            code: ManagedPlaywrightBridgeErrorCode::ProtocolError,
            dispatch_certainty: ManagedPlaywrightDispatchCertainty::PossiblyDispatched,
        }
    ));
    assert_eq!(bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn managed_peer_rejects_not_ready_and_missing_invocation_or_authorization_pre_dispatch() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let peer = ManagedPlaywrightHostBridgePeer {
        server_id,
        protocol: test_protocol_snapshot(),
        bridge,
        request_timeout: Duration::from_secs(1),
        shutdown_timeout: Duration::from_secs(1),
        state: AtomicU8::new(STATE_CLOSED),
    };
    let dispatch = McpDispatchTracker::new();
    let error = peer
        .call_tool_tracked(
            McpToolCall {
                name: "browser_snapshot".to_string(),
                arguments: json!({}),
                timeout_ms: None,
                invocation_id: None,
            },
            McpCancellationToken::new(),
            dispatch.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Shutdown);
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );

    peer.state.store(STATE_READY, Ordering::Release);
    let dispatch = McpDispatchTracker::new();
    let error = peer
        .call_tool_tracked(
            McpToolCall {
                name: "browser_snapshot".to_string(),
                arguments: json!({}),
                timeout_ms: None,
                invocation_id: None,
            },
            McpCancellationToken::new(),
            dispatch.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol);
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );

    let dispatch = McpDispatchTracker::new();
    let error = peer
        .call_tool_tracked(
            McpToolCall {
                name: "browser_snapshot".to_string(),
                arguments: json!({}),
                timeout_ms: None,
                invocation_id: Some(McpInvocationId::new()),
            },
            McpCancellationToken::new(),
            dispatch.clone(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol);
    assert_eq!(
        dispatch.certainty(),
        McpDispatchCertainty::DefinitelyNotDispatched
    );
    assert_eq!(peer.bridge.pending_request_count(), 0);
}

#[tokio::test]
async fn connector_flows_through_manager_catalog_without_surface_identity() {
    let server_id = McpServerId::new();
    let bridge = ManagedPlaywrightHostBridge::new(server_id);
    let (outbound, mut commands) = mpsc::unbounded_channel();
    bridge.attach_outbound(outbound).unwrap();
    let registry = InMemoryMcpRegistry::shared();
    registry
        .add(McpServerConfig {
            id: server_id,
            display_name: "Managed browser automation".to_string(),
            scope: McpServerScope::Managed,
            trust: McpTrustLevel::Managed,
            approval_mode: McpApprovalMode::Auto,
            enabled: true,
            transport: McpTransportConfig::HostBridge(
                McpHostBridgeConfig::new(MANAGED_PLAYWRIGHT_BRIDGE_CHANNEL).unwrap(),
            ),
            connect_timeout_ms: 1_000,
            request_timeout_ms: 1_000,
            shutdown_timeout_ms: 1_000,
        })
        .unwrap();
    let manager = mycopilot_mcp_client::McpConnectionManager::without_events(
        registry,
        Arc::new(ManagedPlaywrightHostBridgeConnector::new(Arc::clone(
            &bridge,
        ))),
        McpManagerPolicy::default(),
    )
    .unwrap();
    let responder = tokio::spawn(async move {
        for _ in 0..2 {
            let command = commands.recv().await.unwrap();
            let params: ManagedPlaywrightCommandNotification =
                serde_json::from_value(command["params"].clone()).unwrap();
            let outcome = match params.command {
                ManagedPlaywrightCommand::Connect => {
                    ManagedPlaywrightCompletionOutcome::Connected {
                        protocol: json!({
                            "negotiatedVersion": "2025-11-25",
                            "lifecycle": "initialize_fallback",
                            "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                            "capabilities": {"tools": true, "toolsListChanged": false,
                              "resources": false, "resourcesListChanged": false,
                              "resourcesSubscribe": false, "prompts": false,
                              "promptsListChanged": false, "logging": false,
                              "completions": false, "tasks": false, "extensions": []}
                        }),
                    }
                }
                ManagedPlaywrightCommand::ListTools { cursor: None } => {
                    ManagedPlaywrightCompletionOutcome::ToolsListed {
                        page: json!({
                            "tools": [{"name":"browser_snapshot","title":null,
                              "description":"Snapshot","inputSchema":{"type":"object",
                              "properties":{"call_reason":{"type":"string"}},
                              "required":["call_reason"],"additionalProperties":false},
                              "outputSchema":null,"annotations":{"readOnlyHint":true,
                              "destructiveHint":false,"idempotentHint":null,"openWorldHint":null,
                              "title":null}}],
                            "nextCursor":null,"ttlMs":null,"cacheScope":null
                        }),
                    }
                }
                _ => panic!("unexpected command"),
            };
            bridge
                .complete(ManagedPlaywrightCompletionInput {
                    schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                    request_id: params.request_id,
                    outcome,
                })
                .unwrap();
        }
    });

    let status = manager.start(server_id).await.unwrap();
    assert_eq!(status.state, mycopilot_mcp_client::McpServerState::Ready);
    let catalog = manager.catalog(server_id).unwrap().unwrap();
    assert_eq!(catalog.tools.len(), 1);
    assert!(catalog.tools[0].descriptor.input_schema["required"]
        .as_array()
        .unwrap()
        .contains(&json!("call_reason")));
    responder.await.unwrap();
}

fn attach_reviewed_runtime_responder(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
) -> tokio::task::JoinHandle<()> {
    attach_reviewed_runtime_responder_with_call_error(runtime, None)
}

fn attach_reviewed_runtime_responder_with_call_error(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    call_error: Option<(
        ManagedPlaywrightBridgeErrorCode,
        ManagedPlaywrightDispatchCertainty,
    )>,
) -> tokio::task::JoinHandle<()> {
    let (outbound, mut commands) = mpsc::unbounded_channel();
    runtime.bridge.attach_outbound(outbound).unwrap();
    let bridge = Arc::clone(&runtime.bridge);
    tokio::spawn(async move {
        while let Some(command) = commands.recv().await {
            let params: ManagedPlaywrightCommandNotification =
                serde_json::from_value(command["params"].clone()).unwrap();
            let outcome = match params.command {
                ManagedPlaywrightCommand::Connect => {
                    ManagedPlaywrightCompletionOutcome::Connected {
                        protocol: json!({
                            "negotiatedVersion": "2025-11-25",
                            "lifecycle": "initialize_fallback",
                            "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                            "capabilities": {"tools": true, "toolsListChanged": false,
                              "resources": false, "resourcesListChanged": false,
                              "resourcesSubscribe": false, "prompts": false,
                              "promptsListChanged": false, "logging": false,
                              "completions": false, "tasks": false, "extensions": []}
                        }),
                    }
                }
                ManagedPlaywrightCommand::ListTools { cursor: None } => {
                    let manifest = load_playwright_browser_manifest().unwrap();
                    ManagedPlaywrightCompletionOutcome::ToolsListed {
                        page: json!({
                            "tools": manifest.tools.into_iter().map(|tool| json!({
                                "name": tool.tool_id,
                                "title": null,
                                "description": tool.description,
                                "inputSchema": tool.input_schema,
                                "outputSchema": null,
                                "annotations": {"readOnlyHint": false,
                                  "destructiveHint": false,
                                  "idempotentHint": null, "openWorldHint": null, "title": null}
                            })).collect::<Vec<_>>(),
                            "nextCursor": null,
                            "ttlMs": null,
                            "cacheScope": null
                        }),
                    }
                }
                ManagedPlaywrightCommand::CallTool { .. } => {
                    if let Some((code, dispatch_certainty)) = call_error {
                        ManagedPlaywrightCompletionOutcome::Error {
                            code,
                            dispatch_certainty,
                        }
                    } else {
                        ManagedPlaywrightCompletionOutcome::ToolCalled {
                            result: json!({"content":[{"type":"text","text":"ok"}],
                              "structuredContent":null,"isError":false}),
                            host_artifact_publish_path: None,
                        }
                    }
                }
                ManagedPlaywrightCommand::PrepareSensitiveTool { input } => {
                    ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
                        binding_id: Uuid::new_v4().to_string(),
                        target_binding_digest: format!("sha256:{}", "7".repeat(64)),
                        origin: Some("https://mail.example.test".to_string()),
                        created_at_ms: input.created_at_ms,
                        expires_at_ms: input.expires_at_ms,
                        file_basenames: Vec::new(),
                        file_revision_digest: None,
                    }
                }
                ManagedPlaywrightCommand::ReleaseSensitiveToolBinding { .. } => {
                    ManagedPlaywrightCompletionOutcome::SensitiveToolBindingReleased {
                        released: true,
                    }
                }
                ManagedPlaywrightCommand::Close => ManagedPlaywrightCompletionOutcome::Closed,
                ManagedPlaywrightCommand::ListTools { cursor: Some(_) } => {
                    panic!("reviewed catalog is not paginated")
                }
            };
            bridge
                .complete(ManagedPlaywrightCompletionInput {
                    schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
                    request_id: params.request_id,
                    outcome,
                })
                .unwrap();
        }
    })
}

fn deterministic_idle_sleeper() -> (
    ManagedPlaywrightIdleSleeper,
    Arc<StdMutex<VecDeque<oneshot::Sender<()>>>>,
) {
    let pending = Arc::new(StdMutex::new(VecDeque::new()));
    let pending_for_sleep = Arc::clone(&pending);
    let sleeper: ManagedPlaywrightIdleSleeper = Arc::new(move |_| {
        let (release, wait) = oneshot::channel();
        pending_for_sleep.lock().unwrap().push_back(release);
        Box::pin(async move {
            let _ = wait.await;
        })
    });
    (sleeper, pending)
}

async fn release_next_idle_timer(pending: &Arc<StdMutex<VecDeque<oneshot::Sender<()>>>>) {
    for _ in 0..100 {
        if let Some(release) = pending.lock().unwrap().pop_front() {
            let _ = release.send(());
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("managed Playwright idle timer was not registered");
}

async fn join_owned_lifecycle_tasks(runtime: &ManagedPlaywrightMcpRuntime) {
    let mut tasks = runtime
        .lifecycle_tasks
        .lock()
        .map(|mut tasks| std::mem::replace(&mut *tasks, JoinSet::new()))
        .unwrap();
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result.unwrap() {
            assert!(
                matches!(
                    error.kind,
                    mycopilot_mcp_client::McpErrorKind::Cancelled
                        | mycopilot_mcp_client::McpErrorKind::Shutdown
                ),
                "unexpected lifecycle error: {error:?}"
            );
            assert_eq!(
                error.dispatch_certainty,
                Some(mycopilot_mcp_client::McpDispatchCertainty::DefinitelyNotDispatched),
                "lifecycle cancellation before Tool dispatch must remain authoritative"
            );
        }
    }
}

#[tokio::test]
async fn overlapping_start_generations_leave_reviewed_runtime_ready() {
    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let responder = attach_reviewed_runtime_responder(&runtime);
    runtime.request_start().unwrap();
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;
    assert_eq!(
        runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap()
            .state,
        mycopilot_mcp_client::McpServerState::Ready
    );
    runtime.shutdown().await;
    responder.await.unwrap();
}

#[tokio::test]
async fn stale_stop_generation_cannot_close_newer_start() {
    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let responder = attach_reviewed_runtime_responder(&runtime);
    runtime.request_start().unwrap();
    runtime.request_stop().unwrap();
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;
    assert_eq!(
        runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap()
            .state,
        mycopilot_mcp_client::McpServerState::Ready
    );
    runtime.shutdown().await;
    responder.await.unwrap();
}

#[tokio::test]
async fn trusted_main_pre_dispatch_rejection_stays_definite_through_runtime_manager() {
    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let responder = attach_reviewed_runtime_responder_with_call_error(
        &runtime,
        Some((
            ManagedPlaywrightBridgeErrorCode::InvalidArguments,
            ManagedPlaywrightDispatchCertainty::DefinitelyNotDispatched,
        )),
    );
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;

    let error = invoke_browser_tool_result(
        &runtime,
        "browser_snapshot",
        json!({"call_reason": "Exercise an authoritative pre-dispatch rejection."}),
    )
    .await
    .expect_err("trusted Main rejection must remain an error");
    assert!(
        matches!(error.kind, mycopilot_mcp_client::McpErrorKind::Protocol),
        "unexpected trusted Main rejection: {error:?}"
    );
    assert_eq!(
        error.dispatch_certainty,
        Some(mycopilot_mcp_client::McpDispatchCertainty::DefinitelyNotDispatched)
    );

    runtime.shutdown().await;
    responder.await.unwrap();
}

#[cfg(target_os = "macos")]
#[test]
fn managed_tab_count_accepts_the_fixed_official_text_result() {
    let result = McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "### Open tabs\n- 0: Fixture (current)\n- 1: Secondary Fixture".to_string(),
        }],
        structured_content: None,
        is_error: false,
    };
    assert_eq!(managed_tab_count(&result), 2);
}

#[tokio::test]
async fn idle_policy_reuses_one_timer_and_reactivates_for_one_hundred_cycles() {
    let (sleeper, pending_idle) = deterministic_idle_sleeper();
    let runtime =
        ManagedPlaywrightMcpRuntime::with_idle_policy(Duration::from_secs(10 * 60), sleeper)
            .unwrap();
    let responder = attach_reviewed_runtime_responder(&runtime);
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;
    assert!(runtime.is_ready());

    for cycle in 0..100 {
        release_next_idle_timer(&pending_idle).await;
        for _ in 0..100 {
            if !runtime.is_ready() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!runtime.is_ready(), "idle stop failed at cycle {cycle}");
        let result = invoke_browser_tool(
            &runtime,
            "browser_snapshot",
            json!({"call_reason": format!("idle cycle {cycle}")}),
        )
        .await;
        assert!(!result.is_error, "reactivation failed at cycle {cycle}");
        assert!(runtime.is_ready());
        assert_eq!(runtime.lifecycle_task_count(), 0);
        assert!(pending_idle.lock().unwrap().len() <= 1);
    }

    runtime.shutdown().await;
    responder.await.unwrap();
    assert!(pending_idle.lock().unwrap().len() <= 1);
}

#[tokio::test]
async fn stop_aborts_a_hung_connect_within_the_shutdown_budget() {
    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let (outbound, mut commands) = mpsc::unbounded_channel();
    runtime.bridge.attach_outbound(outbound).unwrap();
    runtime.request_start().unwrap();
    let command = tokio::time::timeout(Duration::from_secs(1), commands.recv())
        .await
        .expect("connect notification timeout")
        .expect("connect notification");
    let params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(command["params"].clone()).unwrap();
    assert!(matches!(params.command, ManagedPlaywrightCommand::Connect));

    tokio::time::timeout(Duration::from_secs(2), runtime.stop())
        .await
        .expect("managed runtime stop exceeded its bounded budget")
        .unwrap();
    let cancel = tokio::time::timeout(Duration::from_secs(1), commands.recv())
        .await
        .expect("cancel notification timeout")
        .expect("cancel notification");
    assert_eq!(
        cancel["method"],
        json!(MANAGED_PLAYWRIGHT_CANCEL_NOTIFICATION_METHOD)
    );
    let cancel_params: ManagedPlaywrightCancelNotification =
        serde_json::from_value(cancel["params"].clone()).unwrap();
    assert_eq!(cancel_params.request_id, params.request_id);
    assert_eq!(
        cancel_params.reason,
        ManagedPlaywrightCancelReason::Shutdown
    );
    join_owned_lifecycle_tasks(&runtime).await;
    assert_ne!(
        runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap()
            .state,
        mycopilot_mcp_client::McpServerState::Ready
    );
    runtime.shutdown().await;
}

#[tokio::test]
async fn shutdown_aborts_a_hung_catalog_discovery_within_the_shutdown_budget() {
    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let (outbound, mut commands) = mpsc::unbounded_channel();
    runtime.bridge.attach_outbound(outbound).unwrap();
    runtime.request_start().unwrap();
    let connect = tokio::time::timeout(Duration::from_secs(1), commands.recv())
        .await
        .expect("connect notification timeout")
        .expect("connect notification");
    let connect_params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(connect["params"].clone()).unwrap();
    runtime
        .bridge
        .complete(ManagedPlaywrightCompletionInput {
            schema_version: MANAGED_PLAYWRIGHT_BRIDGE_SCHEMA_VERSION,
            request_id: connect_params.request_id,
            outcome: ManagedPlaywrightCompletionOutcome::Connected {
                protocol: json!({
                    "negotiatedVersion": "2025-11-25",
                    "lifecycle": "initialize_fallback",
                    "server": {"name": "@playwright/mcp", "version": "0.0.79"},
                    "capabilities": {"tools": true, "toolsListChanged": false,
                      "resources": false, "resourcesListChanged": false,
                      "resourcesSubscribe": false, "prompts": false,
                      "promptsListChanged": false, "logging": false,
                      "completions": false, "tasks": false, "extensions": []}
                }),
            },
        })
        .unwrap();
    let list = tokio::time::timeout(Duration::from_secs(1), commands.recv())
        .await
        .expect("list notification timeout")
        .expect("list notification");
    let list_params: ManagedPlaywrightCommandNotification =
        serde_json::from_value(list["params"].clone()).unwrap();
    assert!(matches!(
        list_params.command,
        ManagedPlaywrightCommand::ListTools { .. }
    ));

    tokio::time::timeout(Duration::from_secs(3), runtime.shutdown())
        .await
        .expect("managed runtime shutdown exceeded its bounded budget");
    let cancel = tokio::time::timeout(Duration::from_secs(1), commands.recv())
        .await
        .expect("cancel notification timeout")
        .expect("cancel notification");
    let cancel_params: ManagedPlaywrightCancelNotification =
        serde_json::from_value(cancel["params"].clone()).unwrap();
    assert_eq!(cancel_params.request_id, list_params.request_id);
    assert_eq!(
        cancel_params.reason,
        ManagedPlaywrightCancelReason::Shutdown
    );
}

/// Cross-language release gate for the production managed-browser stack.
///
/// The normal Vitest wrapper builds the repository-owned Electron helper and supplies these
/// two paths. Keeping this test ignored avoids starting Electron from an ordinary Rust-only
/// test run while still making the full gate explicit and reproducible.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "run through the managed Playwright Electron fixture wrapper"]
async fn managed_playwright_official_electron_e2e() {
    const READY_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_READY=";
    const COMPLETION_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_COMPLETION=";
    const DISPATCH_PHASE_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_DISPATCH_PHASE=";
    const RESULT_MARKER: &str = "MYCOPILOT_MANAGED_PLAYWRIGHT_RESULT=";
    const RISK_AUTHORIZE_MARKER: &str = "MYCOPILOT_BROWSER_RISK_AUTHORIZE=";
    const RISK_CANCEL_MARKER: &str = "MYCOPILOT_BROWSER_RISK_CANCEL=";

    let electron = std::env::var("MYCOPILOT_MANAGED_PLAYWRIGHT_ELECTRON")
        .expect("Electron fixture executable must be supplied by the Vitest wrapper");
    let fixture = std::env::var("MYCOPILOT_MANAGED_PLAYWRIGHT_FIXTURE")
        .expect("managed Playwright fixture bundle must be supplied by the Vitest wrapper");
    let workspace = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("resolve workspace root");

    let mut child = Command::new(electron)
        .arg(fixture)
        .current_dir(workspace)
        .env_remove("ELECTRON_RUN_AS_NODE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("start repository-owned managed Playwright Electron fixture");
    let mut child_stdin = child.stdin.take().expect("fixture stdin");
    let child_stdout = child.stdout.take().expect("fixture stdout");
    let child_stderr = child.stderr.take().expect("fixture stderr");

    let runtime = ManagedPlaywrightMcpRuntime::new().unwrap();
    let bridge = runtime.bridge();
    let (outbound, mut outbound_commands) = mpsc::unbounded_channel::<Value>();
    let fixture_events = outbound.clone();
    // The reader must be able to answer Main's reverse Browser-risk request on the same
    // newline-framed stdin without retaining a Sender forever (which would deadlock fixture
    // shutdown). The slot is explicitly cleared after managed runtime shutdown.
    let fixture_input = Arc::new(StdMutex::new(Some(outbound.clone())));
    bridge.attach_outbound(outbound).unwrap();

    let (_risk_directory, _risk_storage, capability_runtime, risk_coordinator, capability_grant) =
        managed_playwright_e2e_risk_authority();
    let (risk_notifications, mut risk_approval_events) = mpsc::unbounded_channel::<Value>();
    let approval_coordinator = Arc::clone(&risk_coordinator);
    let approval_count = Arc::new(AtomicUsize::new(0));
    let observed_approval_count = Arc::clone(&approval_count);
    let risk_approver = tokio::spawn(async move {
        while let Some(notification) = risk_approval_events.recv().await {
            let action: AgentProposedAction =
                serde_json::from_value(notification["params"]["action"].clone())
                    .expect("parse typed Browser-risk approval notification");
            let AgentProposedAction::BrowserRiskApproval { approval } = action else {
                panic!("managed fixture emitted a non-Browser risk approval");
            };
            assert!(approval.trigger_tool_name.starts_with("browser_"));
            match observed_approval_count.fetch_add(1, Ordering::AcqRel) {
                0 => assert!(approval_coordinator
                    .approve(&approval.run_id, &approval.action_id)
                    .expect("approve fixture Browser risk")
                    .is_some()),
                1 => assert!(approval_coordinator
                    .reject(
                        &approval.run_id,
                        &approval.action_id,
                        Some("The fixture user declined this destination.".to_string()),
                    )
                    .expect("reject fixture Browser risk")
                    .is_some()),
                2 => {
                    assert_eq!(
                        approval.trigger,
                        mycopilot_core::BrowserRiskTrigger::Redirect
                    );
                    assert!(approval_coordinator
                        .reject(
                            &approval.run_id,
                            &approval.action_id,
                            Some("The fixture user declined this redirect.".to_string()),
                        )
                        .expect("reject fixture Browser redirect risk")
                        .is_some());
                }
                unexpected => panic!("unexpected Browser-risk approval #{unexpected}"),
            }
        }
    });

    let writer = tokio::spawn(async move {
        while let Some(command) = outbound_commands.recv().await {
            eprintln!(
                "managed Playwright bridge outbound: {} {}",
                command["method"].as_str().unwrap_or("invalid"),
                command["params"]["command"]["type"]
                    .as_str()
                    .unwrap_or("cancel")
            );
            let encoded = serde_json::to_vec(&command).expect("serialize bridge notification");
            child_stdin
                .write_all(&encoded)
                .await
                .expect("write fixture command");
            child_stdin
                .write_all(b"\n")
                .await
                .expect("frame fixture command");
            child_stdin.flush().await.expect("flush fixture command");
        }
    });

    let (ready_sender, ready_receiver) = oneshot::channel::<Value>();
    let (result_sender, result_receiver) = oneshot::channel::<Value>();
    let reader_bridge = Arc::clone(&bridge);
    let reader_risk_coordinator = Arc::clone(&risk_coordinator);
    let reader_fixture_input = Arc::clone(&fixture_input);
    let risk_authorize_count = Arc::new(AtomicUsize::new(0));
    let observed_authorize_count = Arc::clone(&risk_authorize_count);
    let reader = tokio::spawn(async move {
        let mut ready_sender = Some(ready_sender);
        let mut result_sender = Some(result_sender);
        let mut lines = BufReader::new(child_stdout).lines();
        while let Some(line) = lines.next_line().await.expect("read fixture protocol line") {
            if let Some(payload) = line.strip_prefix(READY_MARKER) {
                let value: Value = serde_json::from_str(payload).expect("parse fixture ready");
                if let Some(sender) = ready_sender.take() {
                    let _ = sender.send(value);
                }
                continue;
            }
            if let Some(payload) = line.strip_prefix(COMPLETION_MARKER) {
                let completion: ManagedPlaywrightCompletionInput =
                    serde_json::from_str(payload).expect("parse fixture completion");
                assert!(reader_bridge
                    .complete(completion)
                    .expect("complete bridge request"));
                continue;
            }
            if let Some(payload) = line.strip_prefix(DISPATCH_PHASE_MARKER) {
                let phase: ManagedPlaywrightDispatchPhaseInput =
                    serde_json::from_str(payload).expect("parse fixture dispatch phase");
                let request_id = phase.request_id.clone();
                let phase_name = phase.phase;
                let accepted = reader_bridge
                    .acknowledge_dispatch_phase(phase)
                    .expect("acknowledge fixture dispatch phase");
                let response = json!({
                    "jsonrpc": "2.0",
                    "method": "fixture.managedPlaywright.dispatchPhaseAck",
                    "params": {
                        "requestId": request_id,
                        "phase": phase_name,
                        "accepted": accepted,
                    },
                });
                reader_fixture_input
                    .lock()
                    .expect("fixture input lock")
                    .as_ref()
                    .expect("fixture input remains attached while acknowledging dispatch")
                    .send(response)
                    .expect("write fixture dispatch phase acknowledgement");
                continue;
            }
            if let Some(payload) = line.strip_prefix(RISK_AUTHORIZE_MARKER) {
                observed_authorize_count.fetch_add(1, Ordering::AcqRel);
                let input: BrowserRiskAuthorizeInput =
                    serde_json::from_str(payload).expect("parse fixture Browser-risk request");
                let request_id = input.request_id.clone();
                let output = reader_risk_coordinator
                    .authorize(
                        input,
                        "managed-playwright-e2e-conversation".to_string(),
                        "managed-playwright-e2e-assistant".to_string(),
                        risk_notifications.clone(),
                    )
                    .await;
                let response = json!({
                    "jsonrpc": "2.0",
                    "method": "fixture.browserRisk.decision",
                    "params": {"requestId": request_id, "output": output},
                });
                reader_fixture_input
                    .lock()
                    .expect("fixture input lock")
                    .as_ref()
                    .expect("fixture input remains attached while authorizing")
                    .send(response)
                    .expect("write fixture Browser-risk decision");
                continue;
            }
            if let Some(payload) = line.strip_prefix(RISK_CANCEL_MARKER) {
                let input: BrowserRiskCancelInput =
                    serde_json::from_str(payload).expect("parse fixture Browser-risk cancel");
                assert_eq!(input.schema_version, BROWSER_RISK_PROTOCOL_SCHEMA_VERSION);
                reader_risk_coordinator.cancel_request(input);
                continue;
            }
            if let Some(payload) = line.strip_prefix(RESULT_MARKER) {
                let value: Value = serde_json::from_str(payload).expect("parse fixture result");
                if let Some(sender) = result_sender.take() {
                    let _ = sender.send(value);
                }
            }
        }
        reader_bridge.close_now();
    });
    let stderr_reader = tokio::spawn(async move {
        let mut stderr = String::new();
        let mut lines = BufReader::new(child_stderr.take(1_000_000)).lines();
        while let Some(line) = lines.next_line().await.expect("read fixture stderr") {
            if stderr.len() < 64 * 1024 {
                stderr.push_str(&line);
                stderr.push('\n');
            }
            eprintln!("managed Playwright fixture: {line}");
        }
        stderr
    });

    let ready = tokio::time::timeout(Duration::from_secs(15), ready_receiver)
        .await
        .expect("fixture ready timeout")
        .expect("fixture ready channel");
    let fixture_url = ready["fixtureUrl"]
        .as_str()
        .expect("fixture URL")
        .to_string();
    let secondary_url = ready["secondaryUrl"]
        .as_str()
        .expect("secondary fixture URL")
        .to_string();
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;
    let status = runtime
        .manager
        .get_status(runtime.server_id)
        .unwrap()
        .unwrap();
    assert_eq!(status.state, mycopilot_mcp_client::McpServerState::Ready);
    let catalog = runtime.manager.catalog(runtime.server_id).unwrap().unwrap();
    let reviewed_manifest = load_playwright_browser_manifest().unwrap();
    assert_eq!(catalog.tools.len(), reviewed_manifest.tools.len());
    let catalog_names = catalog
        .tools
        .iter()
        .map(|tool| tool.raw_name.as_str())
        .collect::<BTreeSet<_>>();
    for expected in [
        "browser_drag",
        "browser_handle_dialog",
        "browser_hover",
        "browser_navigate_back",
        "browser_select_option",
    ] {
        assert!(catalog_names.contains(expected), "{expected}");
    }
    let forbidden = "browser_run_code_unsafe";
    assert!(!catalog_names.contains(forbidden), "{forbidden}");

    let fixture_origin = fixture_url
        .strip_suffix("/interactive")
        .expect("fixture origin");
    let zero_tab_cookies = invoke_approved_browser_sensitive_tool_after_waiting_event(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_list",
        json!({
            "path": "/",
            "call_reason": "Verify the managed browser profile before any visible tab exists."
        }),
        vec![BuiltinMcpToolRiskKind::CookieRead],
        vec![BuiltinMcpToolRiskKindDto::CookieRead],
        &fixture_events,
    )
    .await;
    assert!(
        !zero_tab_cookies.is_error,
        "zero-tab managed cookie list failed: {}",
        tool_result_text(&zero_tab_cookies)
    );
    let first_tab = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_navigate",
        json!({
            "url": fixture_url.clone(),
            "call_reason": "Navigate the empty managed context directly to the first local fixture."
        }),
    )
    .await;
    assert!(
        !first_tab.is_error,
        "managed first browser_navigate failed: {}",
        tool_result_text(&first_tab)
    );
    let first_tab_list = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({
            "action": "list",
            "call_reason": "Verify the empty group became exactly one managed tab."
        }),
    )
    .await;
    let first_tab_list_text = tool_result_text(&first_tab_list);
    assert!(
        !first_tab_list.is_error && first_tab_list_text.contains("0:"),
        "first managed tab was not listed: {first_tab_list_text}"
    );
    assert!(
        !first_tab_list_text.lines().any(|line| {
            let line = line.trim_start();
            line.starts_with("1:") || line.starts_with("- 1:")
        }),
        "zero-group browser_tabs new created more than one tab: {first_tab_list_text}"
    );
    let approval_sequence_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Locate the local approval-lifecycle smoke button."}),
    )
    .await;
    let approval_sequence_snapshot_text = tool_result_text(&approval_sequence_snapshot);
    let approval_sequence_button_ref = snapshot_ref(&approval_sequence_snapshot_text, "Apply");
    let approval_sequence_click = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": approval_sequence_button_ref,
            "call_reason": "Click before the approved local script lifecycle check."
        }),
    )
    .await;
    assert!(
        !approval_sequence_click.is_error,
        "managed pre-approval browser_click failed: {}",
        tool_result_text(&approval_sequence_click)
    );
    let workspace_fixture_directory = tempfile::Builder::new()
        .prefix(".mycopilot-managed-playwright-files-")
        .tempdir_in(workspace)
        .expect("create repository-local managed file fixture directory");
    let admissions_fixture = workspace_fixture_directory
        .path()
        .join("浙江大学2026年招生资料汇编.pptx");
    let admissions_fixture_content = b"repository-owned synthetic admissions presentation fixture";
    std::fs::write(&admissions_fixture, admissions_fixture_content)
        .expect("write synthetic admissions presentation fixture");
    let admissions_fixture = admissions_fixture
        .canonicalize()
        .expect("canonicalize admissions presentation fixture")
        .to_string_lossy()
        .into_owned();
    let large_drop_fixture = workspace_fixture_directory
        .path()
        .join("fixture-large-drop.bin");
    std::fs::write(&large_drop_fixture, vec![b'L'; 1_100_000])
        .expect("write large managed drop fixture");
    let large_drop_fixture = large_drop_fixture
        .canonicalize()
        .expect("canonicalize large managed drop fixture")
        .to_string_lossy()
        .into_owned();
    let storage_state_fixture = workspace_fixture_directory
        .path()
        .join("fixture-storage-state.json");
    std::fs::write(
        &storage_state_fixture,
        serde_json::to_vec(&json!({
            "cookies": [{
                "name": "restored-cookie",
                "value": "repository-owned-storage-cookie",
                "domain": "127.0.0.1",
                "path": "/",
                "expires": -1,
                "httpOnly": false,
                "secure": false,
                "sameSite": "Lax"
            }],
            "origins": [{
                "origin": fixture_origin,
                "localStorage": [{
                    "name": "restored-local",
                    "value": "repository-owned-storage-local-value"
                }]
            }]
        }))
        .expect("serialize storage-state fixture"),
    )
    .expect("write storage-state fixture");
    let storage_state_fixture = storage_state_fixture
        .canonicalize()
        .expect("canonicalize storage-state fixture")
        .to_string_lossy()
        .into_owned();
    let evaluate_started = tokio::time::Instant::now();
    let evaluated = invoke_approved_browser_evaluate(
        &runtime,
        &capability_grant,
        fixture_origin,
        &fixture_events,
    )
    .await;
    assert!(
        evaluate_started.elapsed() < Duration::from_secs(5),
        "synchronous fixed-official browser_evaluate did not complete promptly"
    );
    assert!(
        !evaluated.is_error,
        "managed browser_evaluate failed: {}",
        tool_result_text(&evaluated)
    );
    assert!(tool_result_text(&evaluated).contains("builtin-evaluate-ok"));

    let snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the local fixture."}),
    )
    .await;
    let snapshot_text = tool_result_text(&snapshot);
    assert!(snapshot_text.contains("Managed Playwright Bridge Fixture"));
    assert!(snapshot_text.contains("evaluated:你好"));
    let evaluated_wait = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({
            "text": "evaluated:你好",
            "call_reason": "Verify ordinary page tools remain usable after approved evaluation."
        }),
    )
    .await;
    assert!(
        !evaluated_wait.is_error && tool_result_text(&evaluated_wait).contains("evaluated:你好"),
        "managed post-approval browser_wait_for failed: {}",
        tool_result_text(&evaluated_wait)
    );
    let cross_frame_subject_ref = snapshot_ref(&snapshot_text, "Cross Frame Subject");
    let cross_frame_evaluate = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_evaluate",
        json!({
            "element": "Cross-origin local fixture subject",
            "target": cross_frame_subject_ref,
            "function": r#"(element) => {
                element.value = 'cross-frame-evaluated';
                element.dispatchEvent(new Event('input', { bubbles: true }));
                element.ownerDocument.querySelector('#evaluate-value').textContent =
                    'evaluate:' + element.value;
                return element.value;
            }"#,
            "call_reason": "Evaluate the exact OOPIF fixture element in the managed surface."
        }),
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
        vec![BuiltinMcpToolRiskKindDto::PageScriptExecution],
    )
    .await;
    assert!(
        !cross_frame_evaluate.is_error,
        "managed OOPIF browser_evaluate failed: {}",
        tool_result_text(&cross_frame_evaluate)
    );
    assert!(tool_result_text(&cross_frame_evaluate).contains("cross-frame-evaluated"));

    let upload_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Locate the repository-owned file chooser fixture."}),
    )
    .await;
    let upload_snapshot_text = tool_result_text(&upload_snapshot);
    assert!(upload_snapshot_text.contains("evaluate:cross-frame-evaluated"));
    let upload_ref = snapshot_ref(&upload_snapshot_text, "Fixture upload");
    let chooser = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": upload_ref.clone(),
            "call_reason": "Open the repository-owned fixture file chooser."
        }),
    )
    .await;
    assert!(
        !chooser.is_error,
        "managed fixture chooser click failed: {}",
        tool_result_text(&chooser)
    );
    let cancelled_chooser = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_file_upload",
        json!({
            "call_reason": "Cancel the pending fixture chooser using fixed official semantics."
        }),
    )
    .await;
    assert!(
        !cancelled_chooser.is_error,
        "fixed official chooser cancel failed: {}",
        tool_result_text(&cancelled_chooser)
    );
    assert!(!tool_result_text(&cancelled_chooser).contains("browser-file:"));
    let reopened_chooser = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": upload_ref,
            "call_reason": "Reopen the chooser for the one-call workspace attachment."
        }),
    )
    .await;
    assert!(
        !reopened_chooser.is_error,
        "managed fixture chooser reopen failed: {}",
        tool_result_text(&reopened_chooser)
    );
    let uploaded = invoke_approved_browser_sensitive_tool_with_resolved_files(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_file_upload",
        json!({
            "paths": [admissions_fixture.clone()],
            "call_reason": "Upload the exact workspace admissions presentation in one original call."
        }),
        vec![
            BuiltinMcpToolRiskKind::FileRead,
            BuiltinMcpToolRiskKind::FileUpload,
        ],
        vec![
            BuiltinMcpToolRiskKindDto::FileRead,
            BuiltinMcpToolRiskKindDto::FileUpload,
        ],
        vec![admissions_fixture.clone()],
    )
    .await;
    assert!(
        !uploaded.is_error,
        "managed browser_file_upload failed: {}",
        tool_result_text(&uploaded)
    );
    assert!(!tool_result_text(&uploaded).contains(&admissions_fixture));
    let admissions_basename = "浙江大学2026年招生资料汇编.pptx";
    let upload_selection = format!(
        "upload-selection:{admissions_basename}:{}",
        admissions_fixture_content.len()
    );
    let upload_effect = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({
            "text": upload_selection,
            "call_reason": "Wait for the exact fixture file chooser selection effect."
        }),
    )
    .await;
    assert!(
        !upload_effect.is_error,
        "managed fixture upload effect missing: {}",
        tool_result_text(&upload_effect)
    );
    let upload_text = format!(
        "upload:{admissions_basename}:{}",
        std::str::from_utf8(admissions_fixture_content).expect("fixture UTF-8")
    );
    let upload_content = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({
            "text": upload_text,
            "call_reason": "Wait for delayed File.text() after the upload tool returned."
        }),
    )
    .await;
    assert!(
        !upload_content.is_error,
        "managed fixture delayed file read failed: {}",
        tool_result_text(&upload_content)
    );
    let submit_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Locate the delayed fixture upload submit action."}),
    )
    .await;
    let submit_ref = snapshot_ref(&tool_result_text(&submit_snapshot), "Submit fixture upload");
    let submitted = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": submit_ref,
            "call_reason": "Submit the selected fixture after browser_file_upload returned."
        }),
    )
    .await;
    assert!(
        !submitted.is_error,
        "managed fixture delayed upload submit failed: {}",
        tool_result_text(&submitted)
    );
    let submit_effect = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({
            "text": format!("upload-submit:{admissions_basename}:content-ok"),
            "call_reason": "Verify the local fixture received the delayed file submission."
        }),
    )
    .await;
    assert!(
        !submit_effect.is_error,
        "managed fixture delayed upload receipt missing: {}",
        tool_result_text(&submit_effect)
    );

    let drop_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Locate the repository-owned cross-frame drop fixture."}),
    )
    .await;
    let drop_snapshot_text = tool_result_text(&drop_snapshot);
    assert!(drop_snapshot_text.contains(&upload_selection));
    let cross_frame_drop_ref = snapshot_ref(&drop_snapshot_text, "Cross file drop target");
    let dropped = invoke_approved_browser_sensitive_tool_with_resolved_files(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_drop",
        json!({
            "element": "Cross-origin local fixture file drop target",
            "target": cross_frame_drop_ref,
            "paths": [admissions_fixture.clone()],
            "call_reason": "Drop the exact workspace admissions presentation in the managed OOPIF."
        }),
        vec![
            BuiltinMcpToolRiskKind::FileRead,
            BuiltinMcpToolRiskKind::FileUpload,
        ],
        vec![
            BuiltinMcpToolRiskKindDto::FileRead,
            BuiltinMcpToolRiskKindDto::FileUpload,
        ],
        vec![admissions_fixture.clone()],
    )
    .await;
    assert!(
        !dropped.is_error,
        "managed browser_drop(paths) failed: {}",
        tool_result_text(&dropped)
    );
    let drop_diagnostic = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the exact OOPIF drop event effect."}),
    )
    .await;
    let dropped_text = tool_result_text(&drop_diagnostic);
    assert!(
        dropped_text.contains(&format!(
            "drop-selection:{admissions_basename}:{}",
            admissions_fixture_content.len()
        )),
        "managed browser_drop returned success without its exact drop effect: {dropped_text}"
    );
    assert!(
        dropped_text.contains("drop-events:dragenter,dragover,drop"),
        "managed browser_drop event order drifted: {dropped_text}"
    );
    assert!(
        dropped_text.contains(&format!(
            "drop:{admissions_basename}:{}",
            std::str::from_utf8(admissions_fixture_content).expect("fixture UTF-8")
        )),
        "managed browser_drop file bytes were not readable: {dropped_text}"
    );

    let large_drop_ref = snapshot_ref(&dropped_text, "Cross file drop target");
    let large_dropped = invoke_approved_browser_sensitive_tool_with_resolved_files(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_drop",
        json!({
            "element": "Cross-origin local large-file drop target",
            "target": large_drop_ref,
            "paths": [large_drop_fixture.clone()],
            "call_reason": "Drop a brokered file larger than the managed CDP string limit."
        }),
        vec![
            BuiltinMcpToolRiskKind::FileRead,
            BuiltinMcpToolRiskKind::FileUpload,
        ],
        vec![
            BuiltinMcpToolRiskKindDto::FileRead,
            BuiltinMcpToolRiskKindDto::FileUpload,
        ],
        vec![large_drop_fixture.clone()],
    )
    .await;
    assert!(
        !large_dropped.is_error,
        "managed large browser_drop(paths) failed: {}",
        tool_result_text(&large_dropped)
    );
    let large_drop_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the exact large OOPIF drop effect."}),
    )
    .await;
    let large_drop_text = tool_result_text(&large_drop_snapshot);
    assert!(large_drop_text.contains("drop-selection:fixture-large-drop.bin:1100000"));
    assert!(large_drop_text.contains("drop-events:dragenter,dragover,drop,dragenter,dragover,drop"));
    assert!(large_drop_text.contains("drop:fixture-large-drop.bin:1100000:L:L"));

    let network_ready = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({
            "text": "network-ready",
            "call_reason": "Wait for the local fixture request ledger."
        }),
    )
    .await;
    assert!(!network_ready.is_error);
    let request_list = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_network_requests",
        json!({
            "static": false,
            "call_reason": "Locate the repository-owned ping request."
        }),
    )
    .await;
    let ping_index = network_request_index(&request_list, "/api/ping");
    let request_detail = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_network_request",
        json!({
            "index": ping_index,
            "part": "response-body",
            "call_reason": "Read the exact repository-owned ping response body."
        }),
        vec![BuiltinMcpToolRiskKind::NetworkSensitiveRead],
        vec![BuiltinMcpToolRiskKindDto::NetworkSensitiveRead],
    )
    .await;
    assert!(
        !request_detail.is_error,
        "managed browser_network_request failed: {}",
        tool_result_text(&request_detail)
    );
    assert!(tool_result_text(&request_detail).contains(r#"{"ok":true}"#));

    let cookie_set = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_set",
        json!({
            "name": "managed-cookie",
            "value": "repository-owned-cookie-value",
            "call_reason": "Set a repository-owned cookie in the managed BrowserContext."
        }),
        vec![BuiltinMcpToolRiskKind::CookieWrite],
        vec![BuiltinMcpToolRiskKindDto::CookieWrite],
    )
    .await;
    assert!(
        !cookie_set.is_error,
        "managed browser_cookie_set failed: {}",
        tool_result_text(&cookie_set)
    );
    let cookie_get = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_get",
        json!({
            "name": "managed-cookie",
            "call_reason": "Read the repository-owned managed cookie."
        }),
        vec![BuiltinMcpToolRiskKind::CookieRead],
        vec![BuiltinMcpToolRiskKindDto::CookieRead],
    )
    .await;
    assert!(
        !cookie_get.is_error,
        "managed browser_cookie_get failed: {}",
        tool_result_text(&cookie_get)
    );
    assert!(tool_result_text(&cookie_get).contains("repository-owned-cookie-value"));
    let cookie_list = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_list",
        json!({
            "domain": "127.0.0.1",
            "path": "/",
            "call_reason": "List repository-owned cookies in the managed BrowserContext."
        }),
        vec![BuiltinMcpToolRiskKind::CookieRead],
        vec![BuiltinMcpToolRiskKindDto::CookieRead],
    )
    .await;
    assert!(
        !cookie_list.is_error,
        "managed browser_cookie_list failed: {}",
        tool_result_text(&cookie_list)
    );
    assert!(tool_result_text(&cookie_list).contains("repository-owned-cookie-value"));

    let storage_export = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_storage_state",
        json!({
            "filename": "fixture-storage-export.json",
            "call_reason": "Export repository-owned managed storage state."
        }),
        vec![
            BuiltinMcpToolRiskKind::FileWrite,
            BuiltinMcpToolRiskKind::CookieRead,
            BuiltinMcpToolRiskKind::LocalStorageRead,
            BuiltinMcpToolRiskKind::StorageStateExport,
        ],
        vec![
            BuiltinMcpToolRiskKindDto::FileWrite,
            BuiltinMcpToolRiskKindDto::CookieRead,
            BuiltinMcpToolRiskKindDto::LocalStorageRead,
            BuiltinMcpToolRiskKindDto::StorageStateExport,
        ],
    )
    .await;
    assert!(
        !storage_export.is_error,
        "managed browser_storage_state failed: {}",
        tool_result_text(&storage_export)
    );
    assert_safe_browser_artifact(&storage_export, "json", "browser_storage_state");

    let storage_import = invoke_approved_browser_sensitive_tool_with_resolved_files(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_set_storage_state",
        json!({
            "filename": storage_state_fixture.clone(),
            "call_reason": "Restore the exact workspace storage state in one original call."
        }),
        vec![
            BuiltinMcpToolRiskKind::FileRead,
            BuiltinMcpToolRiskKind::CookieWrite,
            BuiltinMcpToolRiskKind::LocalStorageWrite,
            BuiltinMcpToolRiskKind::StorageStateImport,
        ],
        vec![
            BuiltinMcpToolRiskKindDto::FileRead,
            BuiltinMcpToolRiskKindDto::CookieWrite,
            BuiltinMcpToolRiskKindDto::LocalStorageWrite,
            BuiltinMcpToolRiskKindDto::StorageStateImport,
        ],
        vec![storage_state_fixture.clone()],
    )
    .await;
    assert!(
        !storage_import.is_error,
        "managed browser_set_storage_state failed: {}",
        tool_result_text(&storage_import)
    );
    let restored_cookie = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_get",
        json!({
            "name": "restored-cookie",
            "call_reason": "Verify the repository-owned restored cookie."
        }),
        vec![BuiltinMcpToolRiskKind::CookieRead],
        vec![BuiltinMcpToolRiskKindDto::CookieRead],
    )
    .await;
    assert!(tool_result_text(&restored_cookie).contains("repository-owned-storage-cookie"));
    let restored_local_storage = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_localstorage_get",
        json!({
            "key": "restored-local",
            "call_reason": "Verify repository-owned restored local storage."
        }),
        vec![BuiltinMcpToolRiskKind::LocalStorageRead],
        vec![BuiltinMcpToolRiskKindDto::LocalStorageRead],
    )
    .await;
    assert!(
        tool_result_text(&restored_local_storage).contains("repository-owned-storage-local-value")
    );
    let cookie_delete = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_delete",
        json!({
            "name": "restored-cookie",
            "call_reason": "Delete the repository-owned restored cookie."
        }),
        vec![BuiltinMcpToolRiskKind::CookieWrite],
        vec![BuiltinMcpToolRiskKindDto::CookieWrite],
    )
    .await;
    assert!(
        !cookie_delete.is_error,
        "managed browser_cookie_delete failed: {}",
        tool_result_text(&cookie_delete)
    );
    let cookie_clear = invoke_approved_browser_sensitive_tool(
        &runtime,
        &capability_grant,
        fixture_origin,
        "browser_cookie_clear",
        json!({
            "call_reason": "Clear repository-owned cookies from the managed BrowserContext."
        }),
        vec![BuiltinMcpToolRiskKind::CookieWrite],
        vec![BuiltinMcpToolRiskKindDto::CookieWrite],
    )
    .await;
    assert!(
        !cookie_clear.is_error,
        "managed browser_cookie_clear failed: {}",
        tool_result_text(&cookie_clear)
    );

    let parity_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Verify sensitive managed-surface parity effects."}),
    )
    .await;
    let parity_snapshot_text = tool_result_text(&parity_snapshot);
    assert!(parity_snapshot_text.contains("evaluate:cross-frame-evaluated"));
    assert!(parity_snapshot_text.contains(&upload_selection));
    assert!(parity_snapshot_text.contains(&upload_text));
    assert!(
        parity_snapshot_text.contains(&format!("upload-submit:{admissions_basename}:content-ok"))
    );
    assert!(parity_snapshot_text.contains("drop-selection:fixture-large-drop.bin:1100000"));
    assert!(parity_snapshot_text.contains("drop:fixture-large-drop.bin:1100000:L:L"));
    let input_ref = snapshot_ref(&parity_snapshot_text, "Message");
    let button_ref = snapshot_ref(&parity_snapshot_text, "Apply");
    let dialog_button_ref = snapshot_ref(&parity_snapshot_text, "Show dialog");
    let select_ref = snapshot_ref(&parity_snapshot_text, "Plan");
    let drag_source_ref = snapshot_ref(&parity_snapshot_text, "Drag source");
    let drop_target_ref = snapshot_ref(&parity_snapshot_text, "Drop target");
    let list_ref = snapshot_ref(&parity_snapshot_text, "Visible items");
    let download_ref = snapshot_ref(&parity_snapshot_text, "Download fixture");
    let frame_subject_ref = snapshot_ref(&parity_snapshot_text, "Frame Subject");
    let mail_body_target = frame_editor_target(&parity_snapshot_text, 2);

    let frame_form = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_fill_form",
        json!({
            "fields": [{
                "target": frame_subject_ref,
                "name": "Frame Subject",
                "type": "textbox",
                "value": "iframe-subject"
            }],
            "call_reason": "Fill a repository-owned iframe input."
        }),
    )
    .await;
    assert!(
        !frame_form.is_error,
        "managed iframe browser_fill_form failed: {}",
        tool_result_text(&frame_form)
    );
    let frame_type = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_type",
        json!({
            "target": mail_body_target,
            "text": "你好",
            "call_reason": "Fill the repository-owned iframe editor in Chinese."
        }),
    )
    .await;
    assert!(
        !frame_type.is_error,
        "managed iframe Chinese browser_type failed: {}",
        tool_result_text(&frame_type)
    );
    let frame_result_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Verify repository-owned iframe input parity."}),
    )
    .await;
    let frame_result_text = tool_result_text(&frame_result_snapshot);
    assert!(frame_result_text.contains("subject:iframe-subject"));
    assert!(frame_result_text.contains("body:你好"));
    assert!(
        frame_result_text.contains("focused:mail-body"),
        "iframe focus projection mismatch: {frame_result_text}"
    );
    assert!(
        frame_result_text.contains("body-events:beforeinput,input"),
        "iframe event projection mismatch: {frame_result_text}"
    );

    for (tool, arguments) in [
        (
            "browser_resize",
            json!({"width": 360, "height": 240, "call_reason": "Resize the visible fixture guest."}),
        ),
        (
            "browser_route",
            json!({
                "pattern": "**/api/host-adapted",
                "status": 204,
                "call_reason": "Install a run-scoped local fixture route."
            }),
        ),
        (
            "browser_route_list",
            json!({"call_reason": "Inspect the run-scoped local fixture route."}),
        ),
        (
            "browser_unroute",
            json!({
                "pattern": "**/api/host-adapted",
                "call_reason": "Remove the run-scoped local fixture route."
            }),
        ),
        (
            "browser_network_state_set",
            json!({"state": "offline", "call_reason": "Exercise local offline state."}),
        ),
        (
            "browser_network_state_set",
            json!({"state": "online", "call_reason": "Restore local online state."}),
        ),
    ] {
        let result =
            invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
        assert!(
            !result.is_error,
            "managed fixture {tool} Host adapter failed: {}",
            tool_result_text(&result)
        );
    }

    for (tool, arguments, kind) in [
        (
            "browser_take_screenshot",
            json!({
                "scale": "css",
                "filename": "fixture-page.png",
                "call_reason": "Capture the repository-owned fixture."
            }),
            "image",
        ),
        (
            "browser_snapshot",
            json!({
                "filename": "fixture-snapshot.yml",
                "call_reason": "Export the repository-owned fixture snapshot."
            }),
            "snapshot",
        ),
    ] {
        let result =
            invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
        assert!(
            !result.is_error,
            "managed fixture {tool} Artifact failed: {}",
            tool_result_text(&result)
        );
        assert_safe_browser_artifact(&result, kind, tool);
    }
    let pdf = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_pdf_save",
        json!({
            "filename": "fixture-page.pdf",
            "call_reason": "Verify the current managed Electron PDF capability gate."
        }),
    )
    .await;
    assert!(
        pdf.is_error,
        "managed guest PDF must reject this OOPIF page"
    );
    assert_eq!(
        pdf.structured_content
            .as_ref()
            .and_then(|value| value["status"].as_str()),
        Some("unavailable")
    );
    assert_eq!(
        pdf.structured_content
            .as_ref()
            .and_then(|value| value["code"].as_str()),
        Some("browser.pdf_unavailable")
    );
    assert_eq!(
        tool_result_text(&pdf),
        "PDF export is unavailable for this page because it contains a cross-process embedded frame. Try a page without embedded content."
    );
    assert_eq!(
        pdf.structured_content
            .as_ref()
            .and_then(|value| value["reason"].as_str()),
        Some("cross_process_frame")
    );
    let trace_start = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_start_tracing",
        json!({"call_reason": "Start a trace for the repository-owned fixture."}),
    )
    .await;
    assert!(!trace_start.is_error);
    let trace_stop = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_stop_tracing",
        json!({"call_reason": "Publish the repository-owned fixture trace."}),
    )
    .await;
    assert!(!trace_stop.is_error);
    assert_safe_browser_artifact(&trace_stop, "trace", "browser_stop_tracing");
    let download = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": download_ref,
            "call_reason": "Download the repository-owned text fixture."
        }),
    )
    .await;
    assert!(
        !download.is_error,
        "managed fixture download failed: {}",
        tool_result_text(&download)
    );
    let download_structured = download
        .structured_content
        .as_ref()
        .expect("managed download start structured content");
    assert_eq!(download_structured["status"], "download_started");
    let started_download = browser_download_progress(&download, "fixture-download.bin")
        .expect("download start must expose task-scoped progress");
    assert_eq!(started_download["state"], "progressing");
    assert_eq!(
        started_download["totalBytes"].as_u64(),
        Some(16 * 1024 * 1024)
    );
    let initial_received = started_download["receivedBytes"].as_u64().unwrap_or(0);
    let serialized_start = serde_json::to_string(download_structured).unwrap();
    assert!(!serialized_start.contains("/Users/"));
    assert!(!serialized_start.contains(fixture_origin));

    let mut previous_received = initial_received;
    let mut observed_growth = false;
    let mut completed_download = None;
    for _ in 0..80 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let progress = invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_get_config",
            json!({"call_reason": "Observe the active repository-owned download."}),
        )
        .await;
        assert!(
            !progress.is_error,
            "managed browser_get_config download observation failed: {}",
            tool_result_text(&progress)
        );
        let status = browser_download_progress(&progress, "fixture-download.bin")
            .expect("download progress must remain visible to the task");
        let received = status["receivedBytes"].as_u64().unwrap_or(0);
        observed_growth |= received > previous_received;
        previous_received = previous_received.max(received);
        if status["state"] == "completed" {
            completed_download = Some(progress);
            break;
        }
    }
    assert!(observed_growth, "managed download bytes never advanced");
    let completed_download = completed_download.expect("managed download did not complete");
    assert_safe_browser_download(&completed_download, "browser_get_config completed download");

    for (tool, arguments) in [
        (
            "browser_find",
            json!({
                "text": "Managed Playwright Bridge Fixture",
                "call_reason": "Find the local fixture heading."
            }),
        ),
        (
            "browser_generate_locator",
            json!({
                "target": button_ref.clone(),
                "call_reason": "Generate a locator for the local apply button."
            }),
        ),
        (
            "browser_highlight",
            json!({
                "target": button_ref.clone(),
                "call_reason": "Highlight the local apply button."
            }),
        ),
        (
            "browser_hide_highlight",
            json!({
                "target": button_ref.clone(),
                "call_reason": "Remove the local fixture highlight."
            }),
        ),
        (
            "browser_verify_element_visible",
            json!({
                "role": "heading",
                "accessibleName": "Managed Playwright Bridge Fixture",
                "call_reason": "Verify the local fixture heading."
            }),
        ),
        (
            "browser_verify_text_visible",
            json!({
                "text": "Managed Playwright Bridge Fixture",
                "call_reason": "Verify the local fixture text."
            }),
        ),
        (
            "browser_verify_list_visible",
            json!({
                "element": "fixture item list",
                "target": list_ref.clone(),
                "items": ["Alpha", "Beta"],
                "call_reason": "Verify the local fixture list."
            }),
        ),
        (
            "browser_route_list",
            json!({"call_reason": "List active routes for the local fixture."}),
        ),
    ] {
        let result =
            invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
        assert!(
            !result.is_error,
            "managed fixture {tool} failed: {}",
            tool_result_text(&result)
        );
    }

    let hovered = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_hover",
        json!({
            "target": button_ref.clone(),
            "call_reason": "Exercise the newly exposed hover path."
        }),
    )
    .await;
    assert!(
        !hovered.is_error,
        "managed fixture hover failed: {}",
        tool_result_text(&hovered)
    );

    let opened_dialog = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_click",
        json!({
            "target": dialog_button_ref,
            "call_reason": "Open the repository-owned fixture dialog."
        }),
    )
    .await;
    assert!(
        !opened_dialog.is_error,
        "managed fixture dialog trigger failed: {}",
        tool_result_text(&opened_dialog)
    );
    let handled_dialog = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_handle_dialog",
        json!({
            "accept": true,
            "call_reason": "Accept the repository-owned fixture dialog."
        }),
    )
    .await;
    assert!(
        !handled_dialog.is_error,
        "managed fixture dialog handling failed: {}",
        tool_result_text(&handled_dialog)
    );

    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_fill_form",
            json!({
                "fields": [{
                    "target": input_ref,
                    "name": "Message",
                    "type": "textbox",
                    "value": "from-form"
                }],
                "call_reason": "Fill the local test form."
            }),
        )
        .await
        .is_error
    );
    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_select_option",
            json!({
                "target": select_ref.clone(),
                "values": ["pro"],
                "call_reason": "Select the local fixture plan."
            }),
        )
        .await
        .is_error
    );
    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_verify_value",
            json!({
                "type": "combobox",
                "element": "Plan",
                "target": select_ref,
                "value": "pro",
                "call_reason": "Verify the selected local fixture plan."
            }),
        )
        .await
        .is_error
    );
    let dragged = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_drag",
        json!({
            "startTarget": drag_source_ref,
            "endTarget": drop_target_ref,
            "call_reason": "Drag between local fixture elements."
        }),
    )
    .await;
    assert!(
        !dragged.is_error,
        "managed fixture drag failed: {}",
        tool_result_text(&dragged)
    );
    let drag_wait = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({"text": "dragged", "call_reason": "Wait for the local drag result."}),
    )
    .await;
    assert!(tool_result_text(&drag_wait).contains("dragged"));

    for (tool, arguments) in [
        (
            "browser_mouse_move_xy",
            json!({"x": 4, "y": 4, "call_reason": "Move within the local fixture."}),
        ),
        (
            "browser_mouse_down",
            json!({"button": "left", "call_reason": "Press the mouse in the local fixture."}),
        ),
        (
            "browser_mouse_up",
            json!({"button": "left", "call_reason": "Release the mouse in the local fixture."}),
        ),
        (
            "browser_mouse_click_xy",
            json!({"x": 4, "y": 4, "call_reason": "Click a safe local fixture coordinate."}),
        ),
        (
            "browser_mouse_drag_xy",
            json!({
                "startX": 4,
                "startY": 4,
                "endX": 8,
                "endY": 8,
                "call_reason": "Drag across safe local fixture coordinates."
            }),
        ),
        (
            "browser_mouse_wheel",
            json!({
                "deltaX": 0,
                "deltaY": 8,
                "call_reason": "Scroll the local fixture."
            }),
        ),
    ] {
        let result =
            invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
        assert!(
            !result.is_error,
            "managed fixture {tool} failed: {}",
            tool_result_text(&result)
        );
    }
    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_type",
            json!({
                "target": input_ref,
                "text": "hello",
                "call_reason": "Replace the local test input."
            }),
        )
        .await
        .is_error
    );
    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_press_key",
            json!({"key": "End", "call_reason": "Exercise the local input keyboard path."}),
        )
        .await
        .is_error
    );
    assert!(
        !invoke_browser_tool_with_grant(
            &runtime,
            &capability_grant,
            "browser_click",
            json!({"target": button_ref, "call_reason": "Apply the local test value."}),
        )
        .await
        .is_error
    );
    let waited = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_wait_for",
        json!({"text": "applied:hello", "call_reason": "Wait for the local result."}),
    )
    .await;
    assert!(tool_result_text(&waited).contains("applied:hello"));
    let tabs = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({"action": "list", "call_reason": "List the managed browser page."}),
    )
    .await;
    assert!(!tabs.is_error);
    assert_eq!(managed_tab_count(&tabs), 1);

    let new_tab = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({
            "action": "new",
            "url": secondary_url.clone(),
            "call_reason": "Open a second repository-owned fixture tab."
        }),
    )
    .await;
    assert!(
        !new_tab.is_error,
        "managed fixture tabs new failed: {}",
        tool_result_text(&new_tab)
    );
    let second_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the second managed fixture tab."}),
    )
    .await;
    assert!(tool_result_text(&second_snapshot).contains("Secondary Fixture Page"));
    let two_tabs = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({"action": "list", "call_reason": "Verify both managed fixture tabs."}),
    )
    .await;
    assert_eq!(managed_tab_count(&two_tabs), 2);

    let select_first = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({"action": "select", "index": 0, "call_reason": "Return to the first tab."}),
    )
    .await;
    assert!(!select_first.is_error);
    let restored_first = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Verify the first tab retained its page."}),
    )
    .await;
    assert!(tool_result_text(&restored_first).contains("Managed Playwright Bridge Fixture"));
    let close_second = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({"action": "close", "index": 1, "call_reason": "Close the background fixture tab."}),
    )
    .await;
    assert!(!close_second.is_error);
    let one_tab = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_tabs",
        json!({"action": "list", "call_reason": "Verify one managed fixture tab remains."}),
    )
    .await;
    assert_eq!(managed_tab_count(&one_tab), 1);

    let console = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_console_messages",
        json!({
            "level": "warning",
            "call_reason": "Read bounded local fixture warnings."
        }),
    )
    .await;
    assert!(!console.is_error);
    let console_text = tool_result_text(&console);
    assert!(console_text.contains("### Result"));
    assert!(console_text.contains("managed fixture warning"));
    let requests = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_network_requests",
        json!({
            "static": false,
            "call_reason": "Read the bounded local fixture request list."
        }),
    )
    .await;
    assert!(!requests.is_error);
    let requests_text = tool_result_text(&requests);
    assert!(requests_text.contains("### Result"));
    assert!(requests_text.contains("fixture-query-canary"));
    for (tool, arguments, kind) in [
        (
            "browser_console_messages",
            json!({
                "level": "warning",
                "filename": "fixture-console.log",
                "call_reason": "Export bounded local fixture warnings."
            }),
            "console",
        ),
        (
            "browser_network_requests",
            json!({
                "static": false,
                "filename": "fixture-network.log",
                "call_reason": "Export the bounded local fixture request list."
            }),
            "network",
        ),
    ] {
        let result =
            invoke_browser_tool_with_grant(&runtime, &capability_grant, tool, arguments).await;
        assert!(!result.is_error, "{tool}: {}", tool_result_text(&result));
        assert_safe_browser_artifact(&result, kind, tool);
        assert_browser_artifact_preview(&result, "none");
    }

    let secondary = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_navigate",
        json!({
            "url": secondary_url.clone(),
            "call_reason": "Open the secondary local fixture page."
        }),
    )
    .await;
    assert!(
        !secondary.is_error,
        "managed fixture secondary navigate failed: {}",
        tool_result_text(&secondary)
    );
    let secondary_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the secondary local fixture page."}),
    )
    .await;
    assert!(tool_result_text(&secondary_snapshot).contains("Secondary Fixture Page"));
    let back = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_navigate_back",
        json!({"call_reason": "Return to the primary local fixture page."}),
    )
    .await;
    assert!(
        !back.is_error,
        "managed fixture navigate back failed: {}",
        tool_result_text(&back)
    );
    let back_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the restored primary local fixture page."}),
    )
    .await;
    assert!(tool_result_text(&back_snapshot).contains("Managed Playwright Bridge Fixture"));

    let retained_run_one = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_navigate",
        json!({
            "url": secondary_url.clone(),
            "call_reason": "Leave one local page alive for the fresh Host reconnect regression."
        }),
    )
    .await;
    assert!(
        !retained_run_one.is_error,
        "managed retained-page setup navigate failed: {}",
        tool_result_text(&retained_run_one)
    );

    let closed = invoke_browser_tool_with_grant(
        &runtime,
        &capability_grant,
        "browser_close",
        json!({"call_reason": "Close the managed fixture page."}),
    )
    .await;
    assert!(!closed.is_error);

    runtime
        .stop()
        .await
        .expect("stop the first managed Host generation");
    let reconnect_capability_grant = approve_managed_playwright_e2e_activation(
        &capability_runtime,
        "run_managed_playwright_e2e_reconnect",
        "managed-playwright-e2e-reconnect-activation",
    );
    runtime.request_start().unwrap();
    join_owned_lifecycle_tasks(&runtime).await;
    assert_eq!(
        runtime
            .manager
            .get_status(runtime.server_id)
            .unwrap()
            .unwrap()
            .state,
        mycopilot_mcp_client::McpServerState::Ready
    );

    // This is the fresh Host's first Tool call. Electron still owns the run-one Page, while
    // fixed official Playwright starts with an unhydrated Context._tabs array. The Host must
    // import retained pages before exact selection instead of failing with `Tab 0 not found`
    // or creating a replacement Surface.
    let fresh_host_navigate = invoke_browser_tool_with_grant(
        &runtime,
        &reconnect_capability_grant,
        "browser_navigate",
        json!({
            "url": fixture_url.clone(),
            "call_reason": "Navigate the retained page from a fresh managed Host generation."
        }),
    )
    .await;
    assert!(
        !fresh_host_navigate.is_error,
        "fresh Host retained-page navigate failed: {}",
        tool_result_text(&fresh_host_navigate)
    );
    let fresh_host_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &reconnect_capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Inspect the retained page after fresh Host navigation."}),
    )
    .await;
    assert!(
        !fresh_host_snapshot.is_error
            && tool_result_text(&fresh_host_snapshot).contains("Managed Playwright Bridge Fixture"),
        "fresh Host retained-page snapshot failed: {}",
        tool_result_text(&fresh_host_snapshot)
    );
    let fresh_host_evaluate = invoke_approved_browser_evaluate(
        &runtime,
        &reconnect_capability_grant,
        fixture_origin,
        &fixture_events,
    )
    .await;
    assert!(
        !fresh_host_evaluate.is_error
            && tool_result_text(&fresh_host_evaluate).contains("builtin-evaluate-ok"),
        "fresh Host retained-page evaluate failed: {}",
        tool_result_text(&fresh_host_evaluate)
    );
    let fresh_host_evaluated_snapshot = invoke_browser_tool_with_grant(
        &runtime,
        &reconnect_capability_grant,
        "browser_snapshot",
        json!({"call_reason": "Confirm the fresh Host script result on the retained page."}),
    )
    .await;
    assert!(
        tool_result_text(&fresh_host_evaluated_snapshot).contains("evaluated:你好"),
        "fresh Host evaluated snapshot missed the script result: {}",
        tool_result_text(&fresh_host_evaluated_snapshot)
    );

    prepare_unconsumed_profile_binding_for_terminal_event(&runtime, &reconnect_capability_grant)
        .await;
    send_fixture_agent_done(
        &fixture_events,
        &reconnect_capability_grant.run_id,
        AgentRunStatus::Completed,
    );
    drop(fixture_events);

    runtime.shutdown().await;
    fixture_input.lock().expect("fixture input lock").take();
    tokio::time::timeout(Duration::from_secs(10), writer)
        .await
        .expect("fixture writer shutdown timeout")
        .expect("fixture writer task");
    let result_outcome = tokio::time::timeout(Duration::from_secs(15), result_receiver).await;
    let (exit, child_timed_out) =
        match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
            Ok(exit) => (exit.expect("wait for fixture"), false),
            Err(_) => {
                let _ = child.start_kill();
                let exit = tokio::time::timeout(Duration::from_secs(5), child.wait())
                    .await
                    .expect("fixture kill timeout")
                    .expect("wait for killed fixture");
                (exit, true)
            }
        };
    let stderr = stderr_reader.await.expect("fixture stderr task");
    reader.await.expect("fixture reader task");
    risk_approver.abort();
    let _ = risk_approver.await;
    assert!(risk_coordinator.list_pending().is_empty());
    assert_eq!(approval_count.load(Ordering::Acquire), 0);
    assert_eq!(risk_authorize_count.load(Ordering::Acquire), 0);
    let result = match result_outcome {
        Ok(Ok(result)) if !child_timed_out => result,
        Ok(Ok(_)) => panic!("fixture timed out after RESULT: exit={exit}; stderr={stderr}"),
        Ok(Err(_)) => {
            panic!("fixture closed without RESULT: exit={exit}; stderr={stderr}")
        }
        Err(_) => panic!("fixture RESULT timed out: exit={exit}; stderr={stderr}"),
    };
    assert!(exit.success(), "fixture failed: {stderr}");
    let lifecycle_snapshots = result["agentLifecycleSnapshots"]
        .as_array()
        .expect("fixture Agent lifecycle snapshots");
    assert_eq!(lifecycle_snapshots.len(), 4);
    for snapshot in &lifecycle_snapshots[..3] {
        assert_eq!(snapshot["status"], "waiting_for_approval");
        assert_eq!(snapshot["sensitiveBindings"], 1);
        assert_eq!(snapshot["sensitiveRequests"], 1);
    }
    assert_eq!(lifecycle_snapshots[3]["status"], "completed");
    assert_eq!(lifecycle_snapshots[3]["sensitiveBindings"], 0);
    assert_eq!(lifecycle_snapshots[3]["sensitiveRequests"], 0);
    let ensure_commands = result["ensureCommands"]
        .as_u64()
        .expect("fixture ensure command count");
    let close_commands = result["closeCommands"]
        .as_u64()
        .expect("fixture close command count");
    assert!(ensure_commands >= 2);
    assert!(close_commands >= 1);
    assert!(close_commands <= ensure_commands);
    let detach_snapshots = result["automationDetachSnapshots"]
        .as_array()
        .expect("fixture automation detach snapshots");
    assert!(
        detach_snapshots.len() >= 2,
        "browser_close and final fresh-Host shutdown must both detach automation"
    );
    let first_detach = &detach_snapshots[0];
    assert_eq!(
        first_detach["surfaces"]
            .as_array()
            .expect("first detach retained surfaces")
            .len(),
        1
    );
    assert_eq!(
        first_detach["ensureCommands"], result["preShutdownEnsureCommands"],
        "fresh Host reconnect must not issue a new ensure/create Surface command"
    );
    assert_eq!(
        first_detach["surfaces"], result["preShutdownSurfaces"],
        "fresh Host reconnect must retain the exact Surface generation"
    );
    assert_eq!(result["preShutdownEnsureCommands"], ensure_commands);
    assert_eq!(
        result["preShutdownSurfaces"]
            .as_array()
            .expect("pre-shutdown retained surfaces")
            .len(),
        1
    );
    assert_eq!(result["surfaceCount"], 0);
    assert_eq!(result["guestCount"], 0);
    assert_eq!(result["targetClosed"], true);
    assert_eq!(result["mainWindowAlive"], true);
    assert_eq!(result["broker"]["activeConnections"], 0);
    assert_eq!(result["broker"]["claimedSurfaces"], 0);
    assert_eq!(result["broker"]["registeredGuests"], 0);
    assert_eq!(result["fileSelectionCount"], 0);
    assert_eq!(result["fileBroker"]["handles"], 0);
    assert_eq!(result["fileBroker"]["bytes"], 0);
    assert_eq!(result["riskGuard"]["activeOperations"], 0);
    assert_eq!(result["riskGuard"]["downloads"], 0);
    assert_eq!(result["riskGuard"]["guests"], 0);
    assert_eq!(result["riskGuard"]["redirectMarkers"], 0);
    assert_eq!(result["riskGuard"]["stickyContexts"], 0);
}

#[cfg(target_os = "macos")]
fn managed_playwright_e2e_risk_authority() -> (
    tempfile::TempDir,
    mycopilot_core::storage::service::StorageService,
    BuiltinCapabilityRuntime,
    Arc<BrowserRiskCoordinator>,
    CapabilityGrant,
) {
    let directory = tempfile::tempdir().expect("create managed Browser risk database");
    let database_path = directory.path().join("storage.sqlite");
    let storage = mycopilot_core::storage::service::StorageService::open(&database_path)
        .expect("migrate managed Browser risk database");
    let policies = Arc::new(
        SqliteBuiltinCapabilityPolicyStore::open(&database_path)
            .expect("open managed Browser capability policy"),
    );
    policies
        .set_allowed(StoredCapabilityId::BrowserAutomation, 0, true)
        .expect("allow managed Browser capability in fixture");
    let (capability_runtime, _provider) =
        HostBuiltinCapabilityProvider::runtime_and_provider(policies, None)
            .expect("create managed Browser capability runtime");
    let grant = approve_managed_playwright_e2e_activation(
        &capability_runtime,
        "run_managed_playwright_e2e",
        "managed-playwright-e2e-activation",
    );
    let coordinator = BrowserRiskCoordinator::new(capability_runtime.clone());
    (directory, storage, capability_runtime, coordinator, grant)
}

#[cfg(target_os = "macos")]
fn approve_managed_playwright_e2e_activation(
    capability_runtime: &BuiltinCapabilityRuntime,
    run_id: &str,
    call_id: &str,
) -> CapabilityGrant {
    let manifest = capability_runtime
        .manifests()
        .iter()
        .next()
        .expect("managed Browser manifest");
    let now = unix_millis() / 1_000;
    let mut approval = AgentBuiltinCapabilityActivationApproval {
        action_id: Uuid::new_v4().to_string(),
        activation_id: Uuid::new_v4().to_string(),
        run_id: run_id.to_string(),
        call_id: call_id.to_string(),
        capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
        display_name: manifest.descriptor.display_name.clone(),
        reason: "Exercise the repository-owned local Browser fixture.".to_string(),
        manifest_digest: manifest.manifest_digest.clone(),
        policy_revision: 1,
        created_at: now,
        expires_at: now.saturating_add(BUILTIN_CAPABILITY_ACTIVATION_TTL_SECONDS),
        approval_status: AgentApprovalStatus::Required,
    };
    approval.approval_status = AgentApprovalStatus::Approved;
    capability_runtime
        .approve_activation(&approval)
        .expect("approve managed Browser capability in fixture")
}

async fn invoke_browser_tool(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    raw_name: &str,
    arguments: Value,
) -> McpToolResult {
    invoke_browser_tool_result(runtime, raw_name, arguments)
        .await
        .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
}

async fn invoke_browser_tool_result(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    raw_name: &str,
    arguments: Value,
) -> Result<McpToolResult, McpError> {
    let invocation_id = Uuid::new_v4().to_string();
    runtime
        .invoke(
            raw_name,
            &Uuid::new_v4().to_string(),
            arguments,
            ManagedPlaywrightAuthorizationContext {
                conversation_id: Some("conversation-managed-playwright-unit".to_string()),
                run_id: "run_managed_playwright_unit".to_string(),
                capability_id: BROWSER_AUTOMATION_CAPABILITY_ID.to_string(),
                activation_id: Uuid::new_v4().to_string(),
                manifest_digest: format!("sha256:{}", "a".repeat(64)),
                policy_revision: 1,
                grant_expires_at_ms: u64::MAX,
                invocation_id,
                call_id: Uuid::new_v4().to_string(),
                trigger_tool_name: raw_name.to_string(),
                call_reason: "Exercise the managed browser unit fixture.".to_string(),
                builtin_tool_grant: None,
            },
            McpCancellationToken::new(),
        )
        .await
}

async fn invoke_browser_tool_with_grant(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    raw_name: &str,
    arguments: Value,
) -> McpToolResult {
    invoke_browser_tool_with_grant_result(runtime, capability_grant, raw_name, arguments)
        .await
        .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
}

async fn invoke_browser_tool_with_grant_result(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    raw_name: &str,
    arguments: Value,
) -> Result<McpToolResult, McpError> {
    let invocation_id = Uuid::new_v4().to_string();
    runtime
        .invoke(
            raw_name,
            &Uuid::new_v4().to_string(),
            arguments,
            ManagedPlaywrightAuthorizationContext {
                conversation_id: Some("conversation-managed-playwright-fixture".to_string()),
                run_id: capability_grant.run_id.clone(),
                capability_id: capability_grant.capability_id.as_str().to_string(),
                activation_id: capability_grant.activation_id.as_str().to_string(),
                manifest_digest: capability_grant.manifest_digest.clone(),
                policy_revision: capability_grant.policy_revision,
                grant_expires_at_ms: capability_grant.expires_at.saturating_mul(1_000),
                invocation_id,
                call_id: Uuid::new_v4().to_string(),
                trigger_tool_name: raw_name.to_string(),
                call_reason: "Exercise the local managed browser fixture.".to_string(),
                builtin_tool_grant: None,
            },
            McpCancellationToken::new(),
        )
        .await
}

#[cfg(target_os = "macos")]
async fn invoke_approved_browser_evaluate(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    origin: &str,
    fixture_events: &mpsc::UnboundedSender<Value>,
) -> McpToolResult {
    let raw_name = "browser_evaluate";
    let call_reason = "Run one synchronous script in the repository-owned fixture.";
    let arguments = json!({
        "function": r#"() => {
            document.querySelector('#output').textContent = 'evaluated:你好';
            return 'builtin-evaluate-ok';
        }"#,
        "call_reason": call_reason,
    });
    invoke_approved_browser_sensitive_tool_after_waiting_event(
        runtime,
        capability_grant,
        origin,
        raw_name,
        arguments,
        vec![BuiltinMcpToolRiskKind::PageScriptExecution],
        vec![BuiltinMcpToolRiskKindDto::PageScriptExecution],
        fixture_events,
    )
    .await
}

#[cfg(target_os = "macos")]
async fn prepare_unconsumed_profile_binding_for_terminal_event(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
) {
    let arguments = json!({
        "path": "/",
        "call_reason": "Freeze one profile-scoped binding for terminal lifecycle cleanup."
    });
    let now_ms = unix_millis();
    let expires_at_ms = now_ms
        .saturating_add(60_000)
        .min(capability_grant.expires_at.saturating_mul(1_000));
    let prepared = runtime
        .bridge()
        .prepare_sensitive_tool(ManagedPlaywrightPrepareSensitiveToolInput {
            binding_request_id: Uuid::new_v4().to_string(),
            binding_scope: ManagedPlaywrightSensitiveBindingScopeDto::ManagedBrowserProfile,
            run_id: capability_grant.run_id.clone(),
            capability_id: capability_grant.capability_id.as_str().to_string(),
            activation_id: capability_grant.activation_id.as_str().to_string(),
            manifest_digest: capability_grant.manifest_digest.clone(),
            policy_revision: capability_grant.policy_revision,
            grant_expires_at_ms: capability_grant.expires_at.saturating_mul(1_000),
            call_id: Uuid::new_v4().to_string(),
            tool_name: "browser_cookie_list".to_string(),
            arguments_digest: mycopilot_core::builtin_mcp_tool_arguments_digest(&arguments)
                .expect("digest terminal profile binding arguments"),
            created_at_ms: now_ms,
            expires_at_ms,
            file_preparation: None,
        })
        .await
        .expect("prepare terminal profile binding");
    assert!(matches!(
        prepared,
        ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared { origin: None, .. }
    ));
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
async fn invoke_approved_browser_sensitive_tool_after_waiting_event(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    origin: &str,
    raw_name: &str,
    arguments: Value,
    risks: Vec<BuiltinMcpToolRiskKind>,
    risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
    fixture_events: &mpsc::UnboundedSender<Value>,
) -> McpToolResult {
    invoke_approved_browser_sensitive_tool_with_file_preparation(
        runtime,
        capability_grant,
        origin,
        raw_name,
        arguments,
        risks,
        risk_dtos,
        None,
        Some(fixture_events),
    )
    .await
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
async fn invoke_approved_browser_sensitive_tool(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    origin: &str,
    raw_name: &str,
    arguments: Value,
    risks: Vec<BuiltinMcpToolRiskKind>,
    risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
) -> McpToolResult {
    invoke_approved_browser_sensitive_tool_with_file_preparation(
        runtime,
        capability_grant,
        origin,
        raw_name,
        arguments,
        risks,
        risk_dtos,
        None,
        None,
    )
    .await
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
async fn invoke_approved_browser_sensitive_tool_with_resolved_files(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    origin: &str,
    raw_name: &str,
    arguments: Value,
    risks: Vec<BuiltinMcpToolRiskKind>,
    risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
    paths: Vec<String>,
) -> McpToolResult {
    invoke_approved_browser_sensitive_tool_with_file_preparation(
        runtime,
        capability_grant,
        origin,
        raw_name,
        arguments,
        risks,
        risk_dtos,
        Some(ManagedPlaywrightSensitiveFilePreparation::ResolvedPaths { paths }),
        None,
    )
    .await
}

#[cfg(target_os = "macos")]
#[allow(clippy::too_many_arguments)]
async fn invoke_approved_browser_sensitive_tool_with_file_preparation(
    runtime: &Arc<ManagedPlaywrightMcpRuntime>,
    capability_grant: &CapabilityGrant,
    origin: &str,
    raw_name: &str,
    arguments: Value,
    risks: Vec<BuiltinMcpToolRiskKind>,
    risk_dtos: Vec<BuiltinMcpToolRiskKindDto>,
    file_preparation: Option<ManagedPlaywrightSensitiveFilePreparation>,
    waiting_event: Option<&mpsc::UnboundedSender<Value>>,
) -> McpToolResult {
    let call_reason = arguments["call_reason"]
        .as_str()
        .expect("sensitive fixture call reason")
        .to_string();
    let arguments_digest = mycopilot_core::builtin_mcp_tool_arguments_digest(&arguments).unwrap();
    let now_ms = unix_millis();
    let grant_expires_at_ms = capability_grant.expires_at.saturating_mul(1_000);
    let expires_at_ms = now_ms.saturating_add(60_000).min(grant_expires_at_ms);
    let call_id = Uuid::new_v4().to_string();
    let profile_scoped = matches!(
        raw_name,
        "browser_cookie_clear"
            | "browser_cookie_delete"
            | "browser_cookie_get"
            | "browser_cookie_list"
            | "browser_set_storage_state"
            | "browser_storage_state"
    ) || (raw_name == "browser_cookie_set"
        && arguments
            .get("domain")
            .and_then(Value::as_str)
            .is_some_and(|domain| !domain.trim().is_empty()));
    let (binding_scope, resource_scope, expected_origin) = if profile_scoped {
        (
            ManagedPlaywrightSensitiveBindingScopeDto::ManagedBrowserProfile,
            "managed_browser_profile",
            None,
        )
    } else {
        (
            ManagedPlaywrightSensitiveBindingScopeDto::ManagedSurface,
            "managed_surface",
            Some(origin),
        )
    };
    let expected_file_basenames = file_preparation
        .as_ref()
        .map(|preparation| match preparation {
            ManagedPlaywrightSensitiveFilePreparation::ResolvedPaths { paths } => paths
                .iter()
                .map(|path| {
                    std::path::Path::new(path)
                        .file_name()
                        .and_then(|name| name.to_str())
                        .expect("resolved fixture basename")
                        .to_string()
                })
                .collect::<Vec<_>>(),
        })
        .unwrap_or_default();
    let prepared = runtime
        .bridge()
        .prepare_sensitive_tool(ManagedPlaywrightPrepareSensitiveToolInput {
            binding_request_id: Uuid::new_v4().to_string(),
            binding_scope,
            run_id: capability_grant.run_id.clone(),
            capability_id: capability_grant.capability_id.as_str().to_string(),
            activation_id: capability_grant.activation_id.as_str().to_string(),
            manifest_digest: capability_grant.manifest_digest.clone(),
            policy_revision: capability_grant.policy_revision,
            grant_expires_at_ms,
            call_id: call_id.clone(),
            tool_name: raw_name.to_string(),
            arguments_digest: arguments_digest.clone(),
            created_at_ms: now_ms,
            expires_at_ms,
            file_preparation,
        })
        .await
        .unwrap_or_else(|error| panic!("prepare exact {raw_name} target binding: {error:?}"));
    let ManagedPlaywrightCompletionOutcome::SensitiveToolPrepared {
        binding_id,
        target_binding_digest,
        origin: prepared_origin,
        created_at_ms,
        expires_at_ms: prepared_expires_at_ms,
        file_basenames,
        file_revision_digest,
        ..
    } = prepared
    else {
        panic!("Main returned an invalid {raw_name} target binding");
    };
    assert_eq!(prepared_origin.as_deref(), expected_origin);
    assert_eq!(file_basenames, expected_file_basenames);
    assert_eq!(file_revision_digest.is_some(), !file_basenames.is_empty());
    assert!(file_basenames
        .iter()
        .all(|basename| !basename.contains('/') && !basename.contains('\\')));
    assert_eq!(created_at_ms, now_ms);
    assert_eq!(prepared_expires_at_ms, expires_at_ms);
    if let Some(fixture_events) = waiting_event {
        send_fixture_agent_done(
            fixture_events,
            &capability_grant.run_id,
            AgentRunStatus::WaitingForApproval,
        );
    }
    let resource_scope_digest = mycopilot_core::builtin_mcp_tool_resource_scope_digest_v2(
        raw_name,
        &arguments_digest,
        expected_origin,
        &risks,
        resource_scope,
        &target_binding_digest,
    )
    .unwrap();
    runtime
        .invoke(
            raw_name,
            &Uuid::new_v4().to_string(),
            arguments,
            ManagedPlaywrightAuthorizationContext {
                conversation_id: Some("conversation-managed-playwright-fixture".to_string()),
                run_id: capability_grant.run_id.clone(),
                capability_id: capability_grant.capability_id.as_str().to_string(),
                activation_id: capability_grant.activation_id.as_str().to_string(),
                manifest_digest: capability_grant.manifest_digest.clone(),
                policy_revision: capability_grant.policy_revision,
                grant_expires_at_ms,
                invocation_id: Uuid::new_v4().to_string(),
                call_id,
                trigger_tool_name: raw_name.to_string(),
                call_reason,
                builtin_tool_grant: Some(Box::new(ManagedPlaywrightBuiltinToolGrantContext {
                    grant_id: Uuid::new_v4().to_string(),
                    approval_id: Uuid::new_v4().to_string(),
                    arguments_digest,
                    resource_scope_digest,
                    target_binding_id: binding_id,
                    target_binding_digest,
                    origin: prepared_origin,
                    risk_kinds: risk_dtos,
                    expires_at_ms: prepared_expires_at_ms,
                })),
            },
            McpCancellationToken::new(),
        )
        .await
        .unwrap_or_else(|error| panic!("{raw_name} failed: {error:?}"))
}

#[cfg(target_os = "macos")]
fn send_fixture_agent_done(
    fixture_events: &mpsc::UnboundedSender<Value>,
    run_id: &str,
    status: AgentRunStatus,
) {
    let success = matches!(
        status,
        AgentRunStatus::WaitingForApproval | AgentRunStatus::Completed
    );
    fixture_events
        .send(json!({
            "jsonrpc": "2.0",
            "method": FIXTURE_AGENT_EVENT_METHOD,
            "params": AgentEvent::Done {
                run_id: run_id.to_string(),
                success,
                status: Some(status),
                content: None,
                usage: None,
                finish_reason: None,
                proposed_actions: Vec::new(),
            },
        }))
        .expect("send fixture Agent done event");
}

#[cfg(target_os = "macos")]
fn network_request_index(result: &McpToolResult, expected_path: &str) -> u64 {
    assert!(
        !result.is_error,
        "managed network list failed: {}",
        tool_result_text(result)
    );
    tool_result_text(result)
        .lines()
        .find(|line| line.contains(expected_path) && line.contains(" => ["))
        .and_then(|line| line.split_once(". ["))
        .and_then(|(index, _)| index.parse::<u64>().ok())
        .unwrap_or_else(|| {
            panic!(
                "managed network list omitted {expected_path}: {}",
                tool_result_text(result)
            )
        })
}

#[cfg(target_os = "macos")]
fn tool_result_text(result: &McpToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            McpContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(target_os = "macos")]
fn assert_safe_browser_artifact(result: &McpToolResult, expected_kind: &str, operation: &str) {
    let structured = result.structured_content.as_ref().unwrap_or_else(|| {
        panic!(
            "managed Artifact structured content missing for {operation}: {}",
            tool_result_text(result)
        )
    });
    let artifacts = structured["artifacts"]
        .as_array()
        .expect("managed Artifact reference array");
    assert_eq!(artifacts.len(), 1);
    let artifact = artifacts[0].as_object().expect("managed Artifact object");
    assert_eq!(artifact["schemaVersion"], 1);
    assert_eq!(artifact["kind"], expected_kind);
    assert_eq!(artifact["owner"], "browser_automation");
    assert_eq!(artifact["lifecycle"], "run");
    assert_eq!(artifact.len(), 11);
    for forbidden in ["path", "managedPath", "outputDir", "data", "body"] {
        assert!(!artifact.contains_key(forbidden));
    }
    let serialized = serde_json::to_string(artifact).unwrap();
    assert!(!serialized.contains("base64"));
}

#[cfg(target_os = "macos")]
fn assert_safe_browser_download(result: &McpToolResult, operation: &str) {
    let structured = result.structured_content.as_ref().unwrap_or_else(|| {
        panic!(
            "managed Download structured content missing for {operation}: {}",
            tool_result_text(result)
        )
    });
    let downloads = structured["downloads"]
        .as_array()
        .expect("managed Download reference array");
    assert_eq!(downloads.len(), 1);
    let download = downloads[0].as_object().expect("managed Download object");
    assert_eq!(download["schemaVersion"], 2);
    assert_eq!(download["source"], "agent");
    assert_eq!(download.len(), 8);
    assert!(download["downloadId"]
        .as_str()
        .is_some_and(|value| value.starts_with("browser-download:")));
    assert!(download["sizeBytes"]
        .as_u64()
        .is_some_and(|value| value > 0));
    assert_eq!(download["sha256"].as_str().map(str::len), Some(64));
    for forbidden in [
        "path",
        "absolutePath",
        "managedPath",
        "outputDir",
        "data",
        "body",
    ] {
        assert!(!download.contains_key(forbidden));
    }
    let serialized = serde_json::to_string(download).unwrap();
    assert!(!serialized.contains("base64"));
    assert!(!serialized.contains("/Users/"));
}

#[cfg(target_os = "macos")]
fn browser_download_progress<'a>(
    result: &'a McpToolResult,
    display_name: &str,
) -> Option<&'a serde_json::Map<String, Value>> {
    result
        .structured_content
        .as_ref()?
        .get("downloadProgress")?
        .as_array()?
        .iter()
        .filter_map(Value::as_object)
        .find(|download| download.get("displayName").and_then(Value::as_str) == Some(display_name))
}

#[cfg(target_os = "macos")]
fn assert_browser_artifact_preview(result: &McpToolResult, expected_preview: &str) {
    let artifact = result
        .structured_content
        .as_ref()
        .and_then(|structured| structured["artifacts"].as_array())
        .and_then(|artifacts| artifacts.first())
        .unwrap_or_else(|| {
            panic!(
                "managed Artifact reference missing: {}",
                tool_result_text(result)
            )
        });
    assert_eq!(artifact["preview"], expected_preview);
}

#[cfg(target_os = "macos")]
fn managed_tab_count(result: &McpToolResult) -> usize {
    if let Some(count) = result
        .structured_content
        .as_ref()
        .and_then(|structured| structured["tabs"].as_array())
        .map(Vec::len)
    {
        return count;
    }
    let count = tool_result_text(result)
        .lines()
        .filter(|line| {
            line.trim()
                .strip_prefix("- ")
                .and_then(|line| line.split_once(": "))
                .is_some_and(|(index, description)| {
                    !index.is_empty()
                        && index.bytes().all(|byte| byte.is_ascii_digit())
                        && !description.is_empty()
                })
        })
        .count();
    assert!(count > 0, "managed tabs result omitted official tab rows");
    count
}

#[cfg(target_os = "macos")]
fn snapshot_ref(snapshot: &str, accessible_name: &str) -> String {
    let line = snapshot
        .lines()
        .find(|line| line.contains(accessible_name) && line.contains("[ref="))
        .unwrap_or_else(|| panic!("snapshot omitted {accessible_name:?}: {snapshot}"));
    let start = line.find("[ref=").expect("snapshot ref start") + "[ref=".len();
    let end = line[start..].find(']').expect("snapshot ref end") + start;
    line[start..end].to_string()
}

#[cfg(target_os = "macos")]
fn frame_editor_target(snapshot: &str, editor_index: usize) -> String {
    let marker = format!("contenteditable {editor_index}: ");
    snapshot
        .lines()
        .find_map(|line| line.split_once(&marker).map(|(_, target)| target.trim()))
        .filter(|target| target.starts_with("managed-frame-editor:"))
        .unwrap_or_else(|| panic!("snapshot omitted safe iframe editor {editor_index}: {snapshot}"))
        .to_string()
}
