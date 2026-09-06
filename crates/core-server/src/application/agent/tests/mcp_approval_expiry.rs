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
                display_reason: None,
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

fn resume_checkpoint(
    run_id: &str,
    action_id: &str,
    action: &AgentProposedAction,
) -> AgentRunCheckpoint {
    let pending_tool_call_id = call_id(action_id);
    let mut checkpoint: AgentRunCheckpoint = serde_json::from_value(json!({
        "version": AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
            "pauseReason": "approval",
        "runId": run_id,
        "contextItems": [{
            "role": "assistant",
            "content": "",
            "images": [],
            "toolCalls": [{
                "id": pending_tool_call_id,
                "name": "mcp__expiry_fixture__expire",
                "args": {},
                "providerIdentity": {
                    "providerToolIndex": 0,
                    "providerCallId": pending_tool_call_id,
                    "runtimeCallId": pending_tool_call_id
                }
            }],
            "isError": false,
            "sources": [],
            "scope": "conversation",
            "retention": "durable"
        }],
        "nextModelRequestIndex": 1,
        "queuedToolCalls": [],
        "deferredExternalToolCallCount": 0,
        "suppressedNarration": false,
        "extensionSnapshots": [],
        "toolSet": crate::test_tool_set_checkpoint(),
        "runContext": null,
        "collaborationRunSnapshot": null,
        "modelCapabilities": { "imageInput": false },
        "providerProfileConfig": crate::test_provider_profile_config(),
        "providerProtocolKey": crate::test_provider_protocol_key("expiry-test-model"),
        "assistantTurnIdentity": crate::test_assistant_turn_identity(&[
            pending_tool_call_id.as_str()
        ]),
        "providerContinuationRefs": [],
        "conversationWorldStateRecords": [],
        "runWorldState": crate::test_run_world_state(),
        "pendingActionId": action_id,
        "pendingToolCallId": pending_tool_call_id,
        "fileChangeRunGrantRef": null,
        "pendingFileObservation": null,
        "conversationTraceItems": [],
        "conversationModelContextItems": [],
        "nextConversationTraceSequence": 0,
        "conversationTraceTruncated": false
    }))
    .unwrap();
    let AgentProposedAction::McpToolCall { approval } = action else {
        unreachable!("expiry fixture is always an MCP approval")
    };
    let provider_identity = AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: approval.call.id.clone(),
        runtime_call_id: approval.call.id.clone(),
    };
    checkpoint.conversation_trace_items = vec![ConversationTurnTraceItem::ToolCall {
        sequence: 0,
        call_id: approval.call.id.clone(),
        tool: approval.call.tool.clone(),
        operation: approval.call.args.clone(),
        provenance: AgentToolIdentity::Mcp {
            provenance: approval.identity.provenance.clone(),
        },
        approval_status: approval.call.approval_status,
        truncated: false,
    }];
    checkpoint.conversation_model_context_items = vec![ConversationModelContextItem {
        sequence: 0,
        ordinal: 0,
        role: "assistant".to_string(),
        content: String::new(),
        tool_call_id: None,
        tool_calls: vec![AgentContextCheckpointToolCall {
            id: approval.call.id.clone(),
            name: approval.call.tool.clone(),
            args: approval.call.args.clone(),
            provider_identity,
        }],
        is_error: false,
    }];
    checkpoint.next_conversation_trace_sequence = 1;
    checkpoint
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
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    input.resume_checkpoint = Some(resume_checkpoint(run_id, &action_id, &action));
    save_test_pending_provider_for_input(storage, &mut input);
    let conversation_id = format!("conversation-{run_id}");
    let assistant_message_id = format!("assistant-{run_id}");
    let AgentProposedAction::McpToolCall { approval } = &action else {
        unreachable!("expiry fixture is always an MCP approval")
    };
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.clone(),
            project_id: None,
            model_id: Some("expiry-test-model".to_string()),
            title: "MCP approval expiry".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant_message_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                agent_run_json: Some(
                    json!({
                        "runId": run_id,
                        "status": "waiting_for_approval",
                        "startedAt": created_at,
                        "toolDefinitions": [],
                        "toolCalls": [],
                        "toolResults": [],
                        "approvals": [],
                        "fileChangeProposals": [],
                        "fileChanges": [],
                        "webSearchActivities": [],
                        "readActivities": [],
                        "mcpInvocations": [{
                            "actionId": approval.identity.action_id,
                            "invocationId": approval.identity.invocation_id,
                            "callId": approval.identity.call_id,
                            "serverId": approval.identity.provenance.server_id,
                            "serverDisplayName": approval.summary.server_display_name,
                            "scope": approval.identity.provenance.scope,
                            "rawToolName": approval.identity.provenance.raw_tool_name,
                            "modelToolName": approval.identity.provenance.model_tool_name,
                            "external": true,
                            "state": "pending_approval",
                            "dispatchCertainty": "definitely_not_dispatched",
                            "outputTruncated": false
                        }],
                        "timeline": [{
                            "id": format!("mcp-invocation-{}", approval.identity.invocation_id),
                            "type": "mcp_tool_call",
                            "invocationId": approval.identity.invocation_id
                        }],
                        "messageStreamCheckpoints": {},
                        "state": {
                            "status": "waiting_for_approval",
                            "activeRunId": run_id,
                            "lastError": null,
                            "updatedAt": created_at
                        }
                    })
                    .to_string(),
                ),
                ui_state_json: None,
            }],
            created_at,
            updated_at: created_at,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    let checkpoint = input.resume_checkpoint.as_ref().unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.clone(),
        assistant_message_id: assistant_message_id.clone(),
        terminal_status: ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: checkpoint.conversation_trace_items.clone(),
    };
    storage
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            &checkpoint.conversation_model_context_items,
            created_at,
            created_at,
        )
        .unwrap();
    service
        .store_pending_action(
            run_id,
            &conversation_id,
            &assistant_message_id,
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
fn expiry_tick_preserves_human_approval_tickets_and_does_not_touch_payloads() {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    let cutoff = now_ms().saturating_add(120_000);
    let now = Arc::new(AtomicI64::new(cutoff));
    let invoker = Arc::new(ExpiryInvoker::default());
    let service = AgentService::new(Arc::clone(&storage))
        .with_mcp_tool_invoker(Arc::clone(&invoker) as Arc<dyn McpToolInvoker>)
        .with_mcp_approval_clock({
            let now = Arc::clone(&now);
            move || now.load(Ordering::SeqCst)
        });

    let (pending_id, _pending_invocation) = store_action(
        &service,
        &storage,
        "run-expired-pending",
        PendingActionStatus::Pending,
        cutoff - 60_001,
        cutoff - 1,
    );
    let (approved_id, _approved_invocation) = store_action(
        &service,
        &storage,
        "run-expired-approved",
        PendingActionStatus::Approved,
        cutoff - 60_000,
        cutoff,
    );
    let (executing_id, _executing_invocation) = store_action(
        &service,
        &storage,
        "run-expired-executing",
        PendingActionStatus::Executing,
        cutoff - 60_001,
        cutoff - 1,
    );
    let (future_id, _future_invocation) = store_action(
        &service,
        &storage,
        "run-future-pending",
        PendingActionStatus::Pending,
        cutoff - 59_999,
        cutoff + 1,
    );

    let summary = service.reconcile_expired_mcp_approvals().unwrap();
    assert_eq!(summary.cutoff_ms, cutoff);
    assert_eq!(summary.candidates, 0);
    assert_eq!(summary.terminalized, 0);
    assert_eq!(summary.status_cas_conflicts, 0);
    assert_eq!(summary.payload_invalidation_attempts, 0);
    assert_eq!(summary.payload_invalidation_failures, 0);

    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    assert_eq!(
        pending[&pending_id].snapshot.status,
        PendingActionStatus::Pending
    );
    assert_eq!(
        pending[&approved_id].snapshot.status,
        PendingActionStatus::Approved
    );
    assert_eq!(
        pending[&executing_id].snapshot.status,
        PendingActionStatus::Executing
    );
    assert_eq!(
        pending[&future_id].snapshot.status,
        PendingActionStatus::Pending
    );
    drop(pending);
    assert!(service
        .startup_recoverable_mcp_approvals
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains(&approved_id));
    assert!(invoker.invalidated().is_empty());
}

#[test]
fn expiry_tick_does_not_arbitrate_or_rewrite_durable_action_status() {
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
        .list_recoverable_agent_actions_after_reconciliation()
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
    assert_eq!(summary.candidates, 0);
    assert_eq!(summary.terminalized, 0);
    assert_eq!(summary.status_cas_conflicts, 0);
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
            .list_recoverable_agent_actions_after_reconciliation()
            .unwrap()
            .into_iter()
            .find(|record| record.action_id == storage_id)
            .unwrap()
            .status,
        "approved"
    );
}
