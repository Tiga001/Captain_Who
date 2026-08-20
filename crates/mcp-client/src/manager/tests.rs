//! Manager-internal invariants that require access to private coordination state.

use super::invocation::normalize_dispatched_result;
use super::*;
use crate::{InMemoryMcpRegistry, McpContentBlock};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

struct RejectingConnector;

impl McpConnector for RejectingConnector {
    fn connect<'a>(&'a self, _: &'a crate::McpServerConfig) -> BoxMcpFuture<'a, Arc<dyn McpPeer>> {
        Box::pin(async { Err(McpError::spawn("test connector does not start processes")) })
    }
}

fn test_manager(registry: Arc<dyn McpRegistry>) -> McpConnectionManager {
    McpConnectionManager::new(
        registry,
        Arc::new(RejectingConnector),
        Arc::new(NoopMcpEventSink),
        McpManagerPolicy::default(),
    )
    .unwrap()
}

fn test_config(server_id: McpServerId) -> crate::McpServerConfig {
    crate::McpServerConfig {
        id: server_id,
        display_name: "shutdown-test".to_string(),
        scope: McpServerScope::User,
        trust: McpTrustLevel::UserApproved,
        approval_mode: McpApprovalMode::Prompt,
        enabled: true,
        transport: crate::McpTransportConfig::Stdio(crate::McpStdioConfig {
            program: PathBuf::from("/not-executed"),
            arguments: Vec::new(),
            cwd: PathBuf::from("/"),
            environment: Vec::new(),
        }),
        connect_timeout_ms: 10_000,
        request_timeout_ms: 60_000,
        shutdown_timeout_ms: 2_000,
    }
}

#[tokio::test]
async fn added_reconciliation_does_not_restart_the_same_registry_incarnation() {
    let registry = InMemoryMcpRegistry::shared();
    let server_id = McpServerId::new();
    let registered = registry.add(test_config(server_id)).unwrap();
    let manager = test_manager(registry);
    let entry = Arc::new(ManagedEntry::new(&registered));
    {
        let mut state = entry.state.lock().unwrap();
        state.status.state = McpServerState::Error;
    }
    manager
        .inner
        .entries
        .lock()
        .unwrap()
        .insert(server_id, Arc::clone(&entry));

    reconcile_registry_record(&manager, registered).await;

    let state = entry.state.lock().unwrap();
    assert_eq!(state.epoch, 0);
    assert_eq!(state.status.state, McpServerState::Error);
    assert!(!state.connect_inflight);
    assert!(!state.refresh_inflight);
}

#[tokio::test]
async fn stale_removed_change_cannot_stop_a_readded_registry_incarnation() {
    let registry = InMemoryMcpRegistry::shared();
    let server_id = McpServerId::new();
    let config = test_config(server_id);
    let first = registry.add(config.clone()).unwrap();
    let first_removed = registry.remove(server_id).unwrap().unwrap();
    let readded = registry.add(config).unwrap();
    let readded_removed = registry.remove(server_id).unwrap().unwrap();
    assert_eq!(first_removed.config_epoch, first.config_epoch);
    assert_ne!(readded.config_epoch, first.config_epoch);

    let manager = test_manager(registry);
    let entry = Arc::new(ManagedEntry::new(&readded));
    manager
        .inner
        .entries
        .lock()
        .unwrap()
        .insert(server_id, Arc::clone(&entry));
    let stale_change = McpRegistryChange {
        revision: first_removed.revision,
        kind: McpRegistryChangeKind::Removed,
        server_id,
        scope: first_removed.config.scope,
        enabled: first_removed.config.enabled,
        config_digest: first_removed.config_digest,
        config_epoch: first_removed.config_epoch,
    };

    stop_and_forget_removed_change(&manager, &stale_change).await;

    assert!(manager
        .get_entry(server_id)
        .unwrap()
        .is_some_and(|current| Arc::ptr_eq(&current, &entry)));
    assert!(!entry.state.lock().unwrap().removed);

    let current_change = McpRegistryChange {
        revision: readded_removed.revision,
        kind: McpRegistryChangeKind::Removed,
        server_id,
        scope: readded_removed.config.scope,
        enabled: readded_removed.config.enabled,
        config_digest: readded_removed.config_digest,
        config_epoch: readded_removed.config_epoch,
    };
    stop_and_forget_removed_change(&manager, &current_change).await;
    assert!(manager.get_entry(server_id).unwrap().is_none());
    assert!(entry.state.lock().unwrap().removed);
}

#[tokio::test]
async fn active_call_settlement_reports_timeout_until_terminal_removal() {
    let server_id = McpServerId::new();
    let control = Arc::new(ActiveCallControl::new(
        McpActiveCallId::new(
            server_id,
            McpInvocationId::new(),
            McpModelCallId::new("settlement-boundary").unwrap(),
        ),
        McpActiveCallProvenance {
            tool_id: McpToolId {
                server_id,
                raw_name: "slow".to_string(),
            },
            model_name: "mcp__fixture__slow".to_string(),
            config_epoch: McpConfigEpoch::new(),
            registry_revision: 1,
            config_digest: "a".repeat(64).parse().unwrap(),
            catalog_generation: 1,
            catalog_digest: "b".repeat(64).parse().unwrap(),
            schema_digest: "c".repeat(64).parse().unwrap(),
        },
        60_000,
    ));
    assert!(!settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(1),).await);

    control.set_state(McpInvocationState::OutcomeUnknown);
    assert!(
        !settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(1),).await,
        "a terminal state alone is not cleanup until the active registry guard is removed"
    );
    control.removed.store(true, Ordering::Release);
    control.settled.notify_waiters();
    assert!(settle_active_calls(std::slice::from_ref(&control), Duration::from_millis(50),).await);
}

#[tokio::test]
async fn forced_shutdown_recovers_poisoned_entry_map_but_reports_incomplete_cleanup() {
    let registry = InMemoryMcpRegistry::shared();
    let server_id = McpServerId::new();
    let registered = registry.add(test_config(server_id)).unwrap();
    let manager = test_manager(registry);
    let entry = Arc::new(ManagedEntry::new(&registered));
    manager
        .inner
        .entries
        .lock()
        .unwrap()
        .insert(server_id, Arc::clone(&entry));

    let inner = Arc::clone(&manager.inner);
    assert!(catch_unwind(AssertUnwindSafe(move || {
        let _entries = inner.entries.lock().unwrap();
        panic!("poison manager entry map for deterministic cleanup test");
    }))
    .is_err());

    let (results, cleanup_complete) = manager
        .force_shutdown_entries(Duration::from_millis(50))
        .await;
    assert!(!cleanup_complete);
    assert_eq!(results.len(), 1);
    let state = entry.state.lock().unwrap();
    assert!(state.removed);
    assert_eq!(state.status.state, McpServerState::Disabled);
    assert!(state.active_calls.is_empty());
}

#[tokio::test]
async fn forced_shutdown_recovers_poisoned_entry_state_but_never_claims_completion() {
    let registry = InMemoryMcpRegistry::shared();
    let server_id = McpServerId::new();
    let registered = registry.add(test_config(server_id)).unwrap();
    let manager = test_manager(registry);
    let entry = Arc::new(ManagedEntry::new(&registered));
    manager
        .inner
        .entries
        .lock()
        .unwrap()
        .insert(server_id, Arc::clone(&entry));

    let poisoned_entry = Arc::clone(&entry);
    assert!(catch_unwind(AssertUnwindSafe(move || {
        let _state = poisoned_entry.state.lock().unwrap();
        panic!("poison managed entry state for deterministic cleanup test");
    }))
    .is_err());

    let (results, cleanup_complete) = manager
        .force_shutdown_entries(Duration::from_millis(50))
        .await;
    assert!(!cleanup_complete);
    assert_eq!(results.len(), 1);
    let state = match entry.state.lock() {
        Ok(_) => panic!("managed entry state should remain poisoned"),
        Err(poisoned) => poisoned.into_inner(),
    };
    assert!(state.removed);
    assert_eq!(state.status.state, McpServerState::Disabled);
    assert!(state.active_calls.is_empty());
}

#[tokio::test]
async fn graceful_shutdown_completion_requires_successful_results_and_clean_entry_state() {
    let registry = InMemoryMcpRegistry::shared();
    let server_id = McpServerId::new();
    let registered = registry.add(test_config(server_id)).unwrap();
    let manager = test_manager(registry);
    let entry = Arc::new(ManagedEntry::new(&registered));
    manager
        .inner
        .entries
        .lock()
        .unwrap()
        .insert(server_id, Arc::clone(&entry));
    let disabled = entry.state.lock().unwrap().status.clone();
    let success = McpBatchOperationResult {
        server_id: Some(server_id),
        status: Some(disabled.clone()),
        error: None,
    };
    assert!(manager.graceful_shutdown_cleanup_complete(std::slice::from_ref(&success)));

    let close_error = McpError::shutdown("fixture peer close failed");
    let failed = McpBatchOperationResult {
        server_id: Some(server_id),
        status: None,
        error: Some(McpSafeError::from(&close_error)),
    };
    assert!(!manager.graceful_shutdown_cleanup_complete(&[failed]));

    entry.state.lock().unwrap().status.state = McpServerState::Error;
    assert!(!manager.graceful_shutdown_cleanup_complete(&[success]));
}

#[test]
fn tool_result_limits_reject_raw_bytes_blocks_structured_content_and_media() {
    let limits = McpSecurityLimits::default();

    let raw = McpToolResult {
        content: vec![McpContentBlock::Text {
            text: "x".repeat(limits.max_raw_tool_result_bytes),
        }],
        structured_content: None,
        is_error: false,
    };
    assert_eq!(
        validate_tool_result(&raw, &limits).unwrap_err().kind,
        McpErrorKind::OutputTooLarge
    );

    let blocks = McpToolResult {
        content: (0..=limits.max_content_blocks)
            .map(|_| McpContentBlock::Text {
                text: String::new(),
            })
            .collect(),
        structured_content: None,
        is_error: false,
    };
    assert_eq!(
        validate_tool_result(&blocks, &limits).unwrap_err().kind,
        McpErrorKind::OutputTooLarge
    );

    let structured = McpToolResult {
        content: Vec::new(),
        structured_content: Some(serde_json::json!({
            "value": "x".repeat(limits.max_structured_content_bytes)
        })),
        is_error: false,
    };
    assert_eq!(
        validate_tool_result(&structured, &limits).unwrap_err().kind,
        McpErrorKind::OutputTooLarge
    );

    let oversized_block = McpToolResult {
        content: vec![McpContentBlock::Image {
            data: "a".repeat(limits.max_encoded_media_bytes + 1),
            mime_type: "image/png".to_string(),
        }],
        structured_content: None,
        is_error: false,
    };
    assert_eq!(
        validate_tool_result(&oversized_block, &limits)
            .unwrap_err()
            .kind,
        McpErrorKind::OutputTooLarge
    );

    let aggregate = McpToolResult {
        content: vec![
            McpContentBlock::Audio {
                data: "a".repeat(limits.max_encoded_media_bytes),
                mime_type: "audio/wav".to_string(),
            },
            McpContentBlock::Image {
                data: "b".repeat(limits.max_encoded_media_bytes),
                mime_type: "image/png".to_string(),
            },
            McpContentBlock::EmbeddedResource {
                resource: crate::McpEmbeddedResource::Blob {
                    uri: "mcp-owned://fixture/blob".to_string(),
                    mime_type: Some("application/octet-stream".to_string()),
                    data: "c".to_string(),
                },
            },
        ],
        structured_content: None,
        is_error: false,
    };
    assert_eq!(
        validate_tool_result(&aggregate, &limits).unwrap_err().kind,
        McpErrorKind::OutputTooLarge
    );
}

#[test]
fn every_error_after_request_queue_is_outcome_unknown_without_response_evidence() {
    let server_id = McpServerId::new();
    let control = ActiveCallControl::new(
        McpActiveCallId::new(
            server_id,
            McpInvocationId::new(),
            McpModelCallId::new("post-dispatch-errors").unwrap(),
        ),
        McpActiveCallProvenance {
            tool_id: McpToolId {
                server_id,
                raw_name: "mutating_tool".to_string(),
            },
            model_name: "mcp__fixture__mutating_tool".to_string(),
            config_epoch: McpConfigEpoch::new(),
            registry_revision: 1,
            config_digest: "a".repeat(64).parse().unwrap(),
            catalog_generation: 1,
            catalog_digest: "b".repeat(64).parse().unwrap(),
            schema_digest: "c".repeat(64).parse().unwrap(),
        },
        60_000,
    );
    control.dispatch.mark_request_queued();

    for error in [
        McpError::config("post-dispatch config failure"),
        McpError::spawn("post-dispatch spawn failure"),
        McpError::negotiation("post-dispatch negotiation failure"),
        McpError::protocol("post-dispatch protocol failure"),
        McpError::cancelled("MCP tools/call"),
        McpError::timeout("MCP tools/call", 1),
        McpError::server_exited(None),
        McpError::shutdown("post-dispatch shutdown"),
    ] {
        let normalized = normalize_dispatched_result(&control, Err(error)).unwrap_err();
        assert_eq!(normalized.kind, McpErrorKind::OutcomeUnknown);
        assert_eq!(
            normalized.dispatch_certainty,
            Some(McpDispatchCertainty::PossiblyDispatched)
        );
    }
}

#[test]
fn trusted_host_completion_preserves_authoritative_pre_dispatch_rejection() {
    let server_id = McpServerId::new();
    let control = ActiveCallControl::new(
        McpActiveCallId::new(
            server_id,
            McpInvocationId::new(),
            McpModelCallId::new("trusted-host-rejection").unwrap(),
        ),
        McpActiveCallProvenance {
            tool_id: McpToolId {
                server_id,
                raw_name: "managed_tool".to_string(),
            },
            model_name: "mcp__managed__tool".to_string(),
            config_epoch: McpConfigEpoch::new(),
            registry_revision: 1,
            config_digest: "a".repeat(64).parse().unwrap(),
            catalog_generation: 1,
            catalog_digest: "b".repeat(64).parse().unwrap(),
            schema_digest: "c".repeat(64).parse().unwrap(),
        },
        60_000,
    );
    control.dispatch.mark_request_queued();

    let normalized = normalize_dispatched_result(
        &control,
        Err(McpError::protocol("trusted Host rejected before dispatch")
            .with_authoritative_dispatch_certainty(McpDispatchCertainty::DefinitelyNotDispatched)),
    )
    .unwrap_err();
    assert_eq!(normalized.kind, McpErrorKind::Protocol);
    assert_eq!(
        normalized.dispatch_certainty,
        Some(McpDispatchCertainty::DefinitelyNotDispatched)
    );
}
