use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use mycopilot_core::{
    AgentMcpApprovalMode, AgentMcpArgumentSummary, AgentMcpServerScope, AgentMcpToolApproval,
    AgentMcpToolApprovalSummary, AgentMcpToolInvocationIdentity, AgentMcpToolProvenance,
    AgentMcpToolRisk, McpAgentToolDescriptor, McpApprovedToolInvocation, McpToolApprovalRequest,
    McpToolCatalogContext, McpToolInvocationFuture, McpToolInvoker,
    MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
};
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Default)]
struct ExpiryInvoker {
    invalidated: Mutex<Vec<String>>,
}

impl ExpiryInvoker {
    fn invalidated(&self) -> Vec<String> {
        self.invalidated
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}

impl McpToolInvoker for ExpiryInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn prepare_approval(
        &self,
        _request: McpToolApprovalRequest,
    ) -> AgentResult<AgentMcpToolApproval> {
        Err(AgentError::new("expiry fixture does not prepare payloads"))
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidated
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(identity.invocation_id.clone());
        Ok(())
    }

    fn revalidate_approved(&self, _approval: &AgentMcpToolApproval) -> AgentResult<()> {
        Err(AgentError::new("expiry fixture cannot dispatch"))
    }

    fn invoke_approved<'a>(
        &'a self,
        _invocation: McpApprovedToolInvocation,
        _cancellation: AgentCancellationToken,
    ) -> McpToolInvocationFuture<'a> {
        Box::pin(async { Err(AgentError::new("expiry fixture cannot dispatch")) })
    }
}

fn call_id(seed: &str) -> String {
    format!("tc1_{}", URL_SAFE_NO_PAD.encode(Sha256::digest(seed)))
}

fn approval_action(
    run_id: &str,
    action_id: &str,
    invocation_id: &str,
    created_at: i64,
    expires_at: i64,
) -> AgentProposedAction {
    let call_id = call_id(action_id);
    let server_id = uuid::Uuid::new_v4().to_string();
    let scope = AgentMcpServerScope::User;
    let raw_tool_name = "expiry_fixture".to_string();
    let model_tool_name = "mcp__expiry_fixture__expire".to_string();
    AgentProposedAction::McpToolCall {
        approval: Box::new(AgentMcpToolApproval {
            identity: AgentMcpToolInvocationIdentity {
                action_id: action_id.to_string(),
                invocation_id: invocation_id.to_string(),
                run_id: run_id.to_string(),
                call_id: call_id.clone(),
                provenance: AgentMcpToolProvenance {
                    server_id: server_id.clone(),
                    scope: scope.clone(),
                    raw_tool_name: raw_tool_name.clone(),
                    model_tool_name: model_tool_name.clone(),
                    config_epoch: uuid::Uuid::new_v4().to_string(),
                    registry_revision: 1,
                    config_digest: "1".repeat(64),
                    catalog_generation: 1,
                    catalog_digest: "2".repeat(64),
                    catalog_schema_digest: "3".repeat(64),
                    schema_digest: "4".repeat(64),
                    schema_normalizer_version: MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
                },
                arguments_digest: mycopilot_core::mcp_tool_arguments_digest(&json!({})).unwrap(),
            },
            call: AgentToolCall {
                id: call_id,
                tool: model_tool_name.clone(),
                args: json!({}),
                approval_status: AgentApprovalStatus::Required,
                reason: None,
            },
            summary: AgentMcpToolApprovalSummary {
                server_id,
                server_display_name: "Expiry fixture".to_string(),
                scope,
                raw_tool_name,
                model_tool_name,
                arguments: AgentMcpArgumentSummary {
                    encoded_bytes: 2,
                    top_level_property_count: 0,
                    string_value_count: 0,
                    number_value_count: 0,
                    boolean_value_count: 0,
                    null_value_count: 0,
                    object_value_count: 1,
                    array_value_count: 0,
                    max_depth: 1,
                    truncated: false,
                },
                risk: AgentMcpToolRisk::Unknown,
                external: true,
            },
            approval_mode: AgentMcpApprovalMode::Prompt,
            payload_persistence: mycopilot_core::AgentMcpApprovalPayloadPersistence::ProcessOnly,
            created_at,
            expires_at,
        }),
    }
}

fn resume_checkpoint(run_id: &str, action_id: &str) -> AgentRunCheckpoint {
    serde_json::from_value(json!({
        "version": AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        "runId": run_id,
        "contextItems": [],
        "nextModelRequestIndex": 1,
        "queuedToolCalls": [],
        "suppressedNarration": false,
        "extensionSnapshots": [],
        "toolSet": crate::test_tool_set_checkpoint(),
        "runContext": null,
        "modelCapabilities": { "imageInput": false },
        "runWorldState": crate::test_run_world_state(),
        "pendingActionId": action_id,
        "pendingToolCallId": call_id(action_id),
        "conversationTraceItems": [],
        "nextConversationTraceSequence": 0,
        "conversationTraceTruncated": false
    }))
    .unwrap()
}

fn store_action(
    service: &AgentService,
    storage: &StorageService,
    run_id: &str,
    status: PendingActionStatus,
    created_at: i64,
    expires_at: i64,
) -> (String, String) {
    let action_id = uuid::Uuid::new_v4().to_string();
    let invocation_id = uuid::Uuid::new_v4().to_string();
    let action = approval_action(run_id, &action_id, &invocation_id, created_at, expires_at);
    let mut input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "fixed-expiry-test-token",
        "model": "expiry-test-model",
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(resume_checkpoint(run_id, &action_id));
    save_test_pending_provider_for_input(storage, &input);
    service
        .store_pending_action(
            run_id,
            &format!("conversation-{run_id}"),
            &format!("assistant-{run_id}"),
            action,
            input,
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &action_id);
    if status != PendingActionStatus::Pending {
        let record = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())[&storage_id]
            .clone();
        service
            .transition_pending_status(&record, PendingActionStatus::Approved)
            .unwrap();
    }
    if status == PendingActionStatus::Executing {
        let record = service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())[&storage_id]
            .clone();
        service
            .transition_pending_status(&record, PendingActionStatus::Executing)
            .unwrap();
    } else if status == PendingActionStatus::Approved {
        service
            .startup_recoverable_mcp_approvals
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(storage_id.clone());
    }
    (storage_id, invocation_id)
}

#[test]
fn expiry_tick_terminalizes_only_expired_predispatch_actions_and_invalidates_payloads() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let now = Arc::new(AtomicI64::new(2_000_000));
    let invoker = Arc::new(ExpiryInvoker::default());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>)
        .with_mcp_approval_clock({
            let now = Arc::clone(&now);
            move || now.load(Ordering::SeqCst)
        });

    let (pending_id, pending_invocation) = store_action(
        &service,
        &storage,
        "run-expired-pending",
        PendingActionStatus::Pending,
        1_939_999,
        1_999_999,
    );
    let (approved_id, approved_invocation) = store_action(
        &service,
        &storage,
        "run-expired-approved",
        PendingActionStatus::Approved,
        1_940_000,
        2_000_000,
    );
    let (executing_id, executing_invocation) = store_action(
        &service,
        &storage,
        "run-expired-executing",
        PendingActionStatus::Executing,
        1_939_999,
        1_999_999,
    );
    let (future_id, future_invocation) = store_action(
        &service,
        &storage,
        "run-future-pending",
        PendingActionStatus::Pending,
        1_940_001,
        2_000_001,
    );

    let summary = service.reconcile_expired_mcp_approvals().unwrap();
    assert_eq!(summary.cutoff_ms, 2_000_000);
    assert_eq!(summary.candidates, 2);
    assert_eq!(summary.terminalized, 2);
    assert_eq!(summary.status_cas_conflicts, 0);
    assert_eq!(summary.payload_invalidation_attempts, 2);
    assert_eq!(summary.payload_invalidation_failures, 0);

    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert!(!pending.contains_key(&pending_id));
    assert!(!pending.contains_key(&approved_id));
    assert_eq!(
        pending[&executing_id].snapshot.status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        pending[&future_id].snapshot.status,
        PendingActionStatus::Pending
    );
    drop(pending);
    assert!(!service
        .startup_recoverable_mcp_approvals
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&approved_id));
    let mut invalidated = invoker.invalidated();
    invalidated.sort();
    let mut expected = vec![pending_invocation, approved_invocation];
    expected.sort();
    assert_eq!(invalidated, expected);
    assert!(!invoker.invalidated().contains(&executing_invocation));
    assert!(!invoker.invalidated().contains(&future_invocation));
}

#[test]
fn expiry_tick_loses_a_durable_status_cas_without_invalidating_or_removing_memory() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let now = 2_000_000;
    let invoker = Arc::new(ExpiryInvoker::default());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>)
        .with_mcp_approval_clock(move || now);
    let (storage_id, _) = store_action(
        &service,
        &storage,
        "run-expiry-cas",
        PendingActionStatus::Pending,
        1_939_999,
        1_999_999,
    );
    let durable = storage
        .list_active_agent_actions_for_startup()
        .unwrap()
        .into_iter()
        .find(|record| record.action_id == storage_id)
        .unwrap();
    storage
        .transition_pending_agent_action(
            &storage_id,
            "pending",
            "approved",
            &durable.agent_input_json,
            now - 1,
        )
        .unwrap();

    let summary = service.reconcile_expired_mcp_approvals().unwrap();
    assert_eq!(summary.candidates, 1);
    assert_eq!(summary.terminalized, 0);
    assert_eq!(summary.status_cas_conflicts, 1);
    assert_eq!(summary.payload_invalidation_attempts, 0);
    assert!(invoker.invalidated().is_empty());
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())[&storage_id]
            .snapshot
            .status,
        PendingActionStatus::Pending
    );
    assert_eq!(
        storage
            .list_active_agent_actions_for_startup()
            .unwrap()
            .into_iter()
            .find(|record| record.action_id == storage_id)
            .unwrap()
            .status,
        "approved"
    );
}
