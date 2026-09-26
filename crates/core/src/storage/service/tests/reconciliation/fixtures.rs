use super::*;

pub(super) fn current_pending_storage_id(run_id: &str, source_call_id: &str) -> String {
    crate::canonical_pending_action_id(run_id, source_call_id)
}

pub(super) fn current_pending_action(
    run_id: &str,
    source_call_id: &str,
    conversation_id: &str,
) -> AgentPendingActionRecord {
    let storage_id = current_pending_storage_id(run_id, source_call_id);
    let mut pending = pending_action(&storage_id, conversation_id);
    pending.run_id = run_id.to_string();
    pending.tool_call_id = Some(source_call_id.to_string());
    pending
}

pub(super) fn current_model_context_for_trace(
    trace: &ConversationTurnTrace,
) -> Vec<crate::ConversationModelContextItem> {
    trace
        .items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ContextMaterial {
                sequence,
                content,
                images,
                ..
            } => Some(crate::ConversationModelContextItem {
                images: images.clone(),
                sequence: *sequence,
                ordinal: 0,
                role: "user".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::AssistantNarration {
                sequence, content, ..
            } => Some(crate::ConversationModelContextItem {
                images: Vec::new(),
                sequence: *sequence,
                ordinal: 0,
                role: "assistant".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::UserGuidance {
                sequence, content, ..
            } => Some(crate::ConversationModelContextItem {
                images: Vec::new(),
                sequence: *sequence,
                ordinal: 0,
                role: "user".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::AgentMailboxDelivery {
                sequence, content, ..
            }
            | ConversationTurnTraceItem::BackendState {
                sequence, content, ..
            } => Some(crate::ConversationModelContextItem {
                images: Vec::new(),
                sequence: *sequence,
                ordinal: 0,
                role: "user".to_string(),
                content: content.clone(),
                tool_call_id: None,
                tool_calls: Vec::new(),
                is_error: false,
            }),
            ConversationTurnTraceItem::ToolCall {
                sequence,
                call_id,
                tool,
                operation,
                ..
            } => Some(crate::ConversationModelContextItem {
                images: Vec::new(),
                sequence: *sequence,
                ordinal: 0,
                role: "assistant".to_string(),
                content: String::new(),
                tool_call_id: None,
                tool_calls: vec![crate::AgentContextCheckpointToolCall {
                    id: call_id.clone(),
                    name: tool.clone(),
                    args: operation.clone(),
                    provider_identity: crate::AgentProviderToolCallIdentity {
                        provider_tool_index: u32::try_from(*sequence).unwrap(),
                        provider_call_id: call_id.clone(),
                        runtime_call_id: call_id.clone(),
                    },
                }],
                is_error: false,
            }),
            ConversationTurnTraceItem::ToolResult {
                sequence,
                call_id,
                success,
                observation,
                ..
            } => Some(crate::ConversationModelContextItem {
                images: Vec::new(),
                sequence: *sequence,
                ordinal: 0,
                role: "tool".to_string(),
                content: serde_json::to_string(observation).unwrap(),
                tool_call_id: Some(call_id.clone()),
                tool_calls: Vec::new(),
                is_error: !success,
            }),
            ConversationTurnTraceItem::CommandSessionLifecycle { .. }
            | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
            | ConversationTurnTraceItem::RuntimeError { .. } => None,
        })
        .collect()
}

pub(super) trait CurrentManualSettlementTestExt {
    fn commit_current_manual_settlement(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        committed_at: i64,
    ) -> Result<AgentPendingActionResultCommitOutcome, String>;
}

impl CurrentManualSettlementTestExt for StorageService {
    fn commit_current_manual_settlement(
        &self,
        terminal_audit: &AgentActionAuditRecord,
        expected_pending_status: &str,
        target_status: &str,
        trace: &ConversationTurnTrace,
        committed_at: i64,
    ) -> Result<AgentPendingActionResultCommitOutcome, String> {
        self.commit_pending_agent_action_audited_result_trace_with_model_context(
            terminal_audit,
            expected_pending_status,
            target_status,
            trace,
            &current_model_context_for_trace(trace),
            committed_at,
        )
    }
}

pub(super) fn save_assistant_conversation(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
) {
    save_assistant_conversation_in_scope(
        service,
        conversation_id,
        assistant_message_id,
        Some("project-1"),
    );
}

pub(super) fn save_assistant_conversation_in_scope(
    service: &StorageService,
    conversation_id: &str,
    assistant_message_id: &str,
    project_id: Option<&str>,
) {
    let mut stored = conversation(conversation_id, project_id, assistant_message_id);
    stored.messages[0].role = "assistant".to_string();
    service.save_conversation(stored).unwrap();
}

/// Seeds the exact durable observer state that every current pending action has before it can
/// cross an approval/dispatch boundary. Startup reconciliation must consume this trace and model
/// projection; tests must not manufacture the removed pre-trace storage shape.
pub(super) fn seed_current_in_progress_tool_trace(
    service: &StorageService,
    run_id: &str,
    conversation_id: &str,
    assistant_message_id: &str,
    call_id: &str,
    tool_name: &str,
) -> ConversationTurnTrace {
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        conversation_id: conversation_id.to_string(),
        assistant_message_id: assistant_message_id.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::ToolCall {
            sequence: 0,
            call_id: call_id.to_string(),
            tool: tool_name.to_string(),
            provenance: crate::AgentToolIdentity::Builtin {
                tool_name: tool_name.to_string(),
            },
            operation: serde_json::json!({}),
            approval_status: crate::AgentApprovalStatus::Approved,
            truncated: false,
        }],
    };
    service
        .append_in_progress_conversation_turn_trace_and_apply_guidances(
            &trace,
            // The exact Provider/Runtime identity is durably staged for startup recovery. Context
            // rendering excludes this open exchange until terminalization appends its ToolResult.
            &current_model_context_for_trace(&trace),
            1,
            1,
        )
        .unwrap();
    trace
}

pub(super) fn attach_current_manual_file_effect_checkpoint(
    pending: &mut AgentPendingActionRecord,
    trace: &ConversationTurnTrace,
) {
    let (call_id, tool) = trace
        .items
        .iter()
        .rev()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall { call_id, tool, .. } => {
                Some((call_id.clone(), tool.clone()))
            }
            _ => None,
        })
        .expect("manual file-effect settlement trace has a ToolCall");
    let provider_identity = crate::AgentProviderToolCallIdentity {
        provider_tool_index: 0,
        provider_call_id: call_id.clone(),
        runtime_call_id: call_id.clone(),
    };
    let provider_profile_config = crate::ProviderProfileConfig::generic_for_dialect(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
    );
    let provider_protocol_key = crate::ProviderProtocolKey::new(
        crate::ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile_config,
        "reconciliation-test",
        Some("provider-protocol-v1:reconciliation-test".to_string()),
    )
    .unwrap();
    let checkpoint = crate::AgentRunCheckpoint {
        conversation_world_state_records: Vec::new(),
        pause_reason: crate::AgentRunCheckpointPauseReason::Approval,
        version: crate::AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: pending.run_id.clone(),
        context_items: Vec::new(),
        next_model_request_index: 1,
        queued_tool_calls: Vec::new(),
        deferred_external_tool_call_count: 0,
        suppressed_narration: false,
        extension_snapshots: Vec::new(),
        tool_set: crate::AgentRunToolSetCheckpoint {
            stable_revision: "stable-tool-set-test-v1".to_string(),
            dynamic_revision: "dynamic-tool-set-test-v1".to_string(),
            effective_revision: "effective-tool-set-test-v1".to_string(),
            active_capability_ids: Vec::new(),
            exposed_tool_names: vec![tool.clone()],
        },
        run_context: None,
        collaboration_run_snapshot: None,
        model_capabilities: crate::ModelCapabilities::default(),
        provider_profile_config,
        provider_protocol_key,
        assistant_turn_identity: crate::AgentAssistantTurnCheckpointIdentity {
            assistant_turn_id: format!("turn:{}", pending.run_id),
            assistant_turn_digest:
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                    .to_string(),
            tool_call_identities: vec![provider_identity.clone()],
        },
        provider_continuation_refs: Vec::new(),
        run_world_state: serde_json::from_value(test_checkpoint_run_world_state()).unwrap(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: None,
        pending_tool_call_id: call_id.clone(),
        conversation_trace_items: trace.items.clone(),
        conversation_model_context_items: current_model_context_for_trace(trace),
        next_conversation_trace_sequence: trace
            .items
            .last()
            .map(ConversationTurnTraceItem::sequence)
            .unwrap_or(0)
            .saturating_add(1),
        conversation_trace_truncated: trace.truncated,
    };
    pending.agent_input_json = serde_json::json!({
        "resumeInputSchemaVersion": 6,
        "resumeCheckpoint": checkpoint,
    })
    .to_string();
}

fn test_checkpoint_run_world_state() -> serde_json::Value {
    let snapshot = crate::WorldStateSnapshot::new(
        "reconciliation-test-run-world-state",
        0,
        vec![crate::WorldStateSectionEnvelope::host_only(
            crate::WorldStateSectionId::ModelCapabilities,
            crate::WorldStateLifetime::Run,
            serde_json::json!({ "imageInput": false }),
        )
        .unwrap()],
    )
    .unwrap();
    serde_json::to_value(snapshot).unwrap()
}
