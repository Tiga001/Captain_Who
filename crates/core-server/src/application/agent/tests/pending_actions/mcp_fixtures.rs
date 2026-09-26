use super::fixtures::append_durable_pending_trace;
use super::*;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};

#[derive(Clone, Copy)]
pub(super) struct StaticMcpStartupInspector(pub(super) McpApprovalStartupPayloadState);

impl McpApprovalStartupInspector for StaticMcpStartupInspector {
    fn inspect_startup_payload(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> McpApprovalStartupPayloadState {
        self.0
    }
}

#[derive(Default)]
pub(super) struct InvalidatingMcpInvoker {
    pub(super) invalidations: std::sync::atomic::AtomicUsize,
    pub(super) storage: Option<Arc<StorageService>>,
}

impl McpToolInvoker for InvalidatingMcpInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(storage) = self.storage.as_ref() {
            storage
                .delete_mcp_approval_envelope(&identity.invocation_id)
                .map_err(AgentError::from)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct RecoverableApprovalRaceInvoker {
    pub(super) invalidations: std::sync::atomic::AtomicUsize,
    pub(super) invocations: std::sync::atomic::AtomicUsize,
    pub(super) storage: Option<Arc<StorageService>>,
}

impl McpToolInvoker for RecoverableApprovalRaceInvoker {
    fn catalog(
        &self,
        _context: &McpToolCatalogContext,
    ) -> AgentResult<Vec<mycopilot_core::McpAgentToolDescriptor>> {
        Ok(Vec::new())
    }

    fn invalidate_prepared_approval(
        &self,
        identity: &mycopilot_core::AgentMcpToolInvocationIdentity,
    ) -> AgentResult<()> {
        self.invalidations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(storage) = self.storage.as_ref() {
            storage
                .delete_mcp_approval_envelope(&identity.invocation_id)
                .map_err(mycopilot_core::AgentError::from)?;
        }
        Ok(())
    }

    fn revalidate_approved(
        &self,
        _approval: &mycopilot_core::AgentMcpToolApproval,
    ) -> AgentResult<()> {
        Ok(())
    }

    fn invoke_approved<'a>(
        &'a self,
        _invocation: McpApprovedToolInvocation,
        _cancellation: AgentCancellationToken,
    ) -> mycopilot_core::McpToolInvocationFuture<'a> {
        Box::pin(async move {
            self.invocations
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(mycopilot_core::McpToolInvocationResult {
                content: Vec::new(),
                structured_content: None,
                is_error: false,
                truncated_at_source: false,
            })
        })
    }
}

pub(super) fn test_mcp_pending_action(
    run_id: &str,
    action_id: &str,
    invocation_id: &str,
    created_at: i64,
) -> AgentProposedAction {
    let server_id = uuid::Uuid::new_v4().to_string();
    let model_tool_name = "mcp__startup_fixture__echo".to_string();
    let raw_tool_name = "echo".to_string();
    let scope = mycopilot_core::AgentMcpServerScope::User;
    let call_id = test_mcp_call_id(action_id);
    AgentProposedAction::McpToolCall {
        approval: Box::new(mycopilot_core::AgentMcpToolApproval {
            identity: mycopilot_core::AgentMcpToolInvocationIdentity {
                action_id: action_id.to_string(),
                invocation_id: invocation_id.to_string(),
                run_id: run_id.to_string(),
                call_id: call_id.clone(),
                provenance: mycopilot_core::AgentMcpToolProvenance {
                    server_id: server_id.clone(),
                    scope: scope.clone(),
                    raw_tool_name: raw_tool_name.clone(),
                    model_tool_name: model_tool_name.clone(),
                    config_epoch: "bf616f04-d3ec-4bd7-825f-731a9f0892f4".to_string(),
                    registry_revision: 11,
                    config_digest: "1".repeat(64),
                    catalog_generation: 1,
                    catalog_digest: "2".repeat(64),
                    catalog_schema_digest: "4".repeat(64),
                    schema_digest: "3".repeat(64),
                    schema_normalizer_version: mycopilot_core::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
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
            summary: mycopilot_core::AgentMcpToolApprovalSummary {
                server_id,
                server_display_name: "Startup fixture".to_string(),
                scope,
                raw_tool_name,
                model_tool_name,
                display_reason: None,
                arguments: mycopilot_core::AgentMcpArgumentSummary {
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
                risk: mycopilot_core::AgentMcpToolRisk::Unknown,
                external: true,
            },
            approval_mode: mycopilot_core::AgentMcpApprovalMode::Prompt,
            payload_persistence: mycopilot_core::AgentMcpApprovalPayloadPersistence::ProcessOnly,
            created_at,
            expires_at: created_at + 60_000,
        }),
    }
}

pub(super) fn test_mcp_call_id(seed: &str) -> String {
    format!("tc1_{}", URL_SAFE_NO_PAD.encode(Sha256::digest(seed)))
}

pub(super) fn test_mcp_resume_checkpoint(
    storage: &StorageService,
    run_id: &str,
    action_id: &str,
) -> AgentRunCheckpoint {
    let pending_tool_call_id = test_mcp_call_id(action_id);
    let provider_model_id = storage.load_model_settings().unwrap().unwrap().models[0]
        .provider_model_id
        .clone();
    let (_, provider_profile_config, provider_protocol_key) =
        test_frozen_provider_protocol(storage, &provider_model_id, None);
    serde_json::from_value(json!({
        "version": AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        "pauseReason": "approval",
        "runId": run_id,
        "contextItems": [{
            "role": "assistant",
            "content": "",
            "images": [],
            "toolCalls": [{
                "id": pending_tool_call_id,
                "name": "mcp__startup_fixture__echo",
                "args": {},
                "providerIdentity": {
                    "providerToolIndex": 0,
                    "providerCallId": pending_tool_call_id,
                    "runtimeCallId": pending_tool_call_id
                }
            }],
            "isError": false,
            "sources": ["model_response"],
            "scope": "conversation",
            "retention": "retained",
            "group": {
                "id": format!("run:{run_id}:tool-exchange:1"),
                "kind": "tool_exchange"
            }
        }],
        "nextModelRequestIndex": 1,
        "queuedToolCalls": [],
        "deferredExternalToolCallCount": 0,
        "suppressedNarration": false,
        "extensionSnapshots": [],
        "toolSet": crate::test_tool_set_checkpoint(),
        "runContext": null,
        "collaborationRunSnapshot": mycopilot_core::AgentCollaborationRunSnapshot {
            selector_directory: Default::default(),
            admitted_wait_model_batches: Vec::new(),
        },
        "modelCapabilities": { "imageInput": false },
        "providerProfileConfig": provider_profile_config,
        "providerProtocolKey": provider_protocol_key,
        "assistantTurnIdentity": crate::test_assistant_turn_identity(&[
            pending_tool_call_id.as_str()
        ]),
        "providerContinuationRefs": [],
        "conversationWorldStateRecords": [],
        "runWorldState": crate::test_run_world_state(),
        "pendingActionId": action_id,
        "pendingToolCallId": pending_tool_call_id,
        "conversationTraceItems": [],
        "conversationModelContextItems": [],
        "nextConversationTraceSequence": 0,
        "conversationTraceTruncated": false,
        "fileChangeRunGrantRef": null,
        "pendingFileObservation": null
    }))
    .unwrap()
}

pub(super) fn seed_durable_mcp_pending_owner(
    storage: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    run_id: &str,
    action: &AgentProposedAction,
    created_at: i64,
) {
    let AgentProposedAction::McpToolCall { approval } = action else {
        panic!("test helper requires an MCP action");
    };
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: None,
            model_id: Some("test-model".to_string()),
            title: "MCP pending action recovery".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
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
    append_durable_pending_trace(
        storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &approval.call,
        AgentToolIdentity::Mcp {
            provenance: approval.identity.provenance.clone(),
        },
        created_at,
    );
}

pub(super) fn test_mcp_envelope(
    invocation_id: &str,
    action_id: &str,
    created_at: i64,
    expires_at: i64,
) -> mycopilot_core::storage::models::McpApprovalEnvelopeRecord {
    mycopilot_core::storage::models::McpApprovalEnvelopeRecord {
        invocation_id: invocation_id.to_string(),
        action_id: action_id.to_string(),
        envelope_version: 3,
        nonce_base64: "AAAAAAAAAAAAAAAA".to_string(),
        ciphertext_base64: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".to_string(),
        aad_digest: "a".repeat(64),
        created_at,
        expires_at,
    }
}
