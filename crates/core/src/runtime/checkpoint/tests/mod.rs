use super::*;
use crate::context::{
    ContextCapacityDetector, ContextItem, ContextMetadata, ContextRetention, ContextScope,
    ContextSource,
};
use crate::llm::{model_response_tool_call_id, LlmMessage, LlmMessageRole};
use crate::protocol::{
    AgentApiStyle, AgentApprovalStatus, AgentMcpServerScope, AgentMcpToolProvenance, AgentToolCall,
    AgentToolIdentity, AgentToolResult,
};
use crate::tools::ToolRegistry;
use serde_json::json;

fn test_tool_set() -> EffectiveToolSet {
    let registry = ToolRegistry::defaults_with_search(None);
    registry
        .effective_tool_set(registry.definitions(), &std::collections::BTreeSet::new())
        .unwrap()
}

fn collaboration_tool_set() -> EffectiveToolSet {
    let mut registry = ToolRegistry::defaults_with_search(None);
    registry.register_agent_collaboration_tools();
    registry
        .effective_tool_set(
            registry.definitions(),
            &std::collections::BTreeSet::from([crate::tools::ToolCapabilityId::application_owned(
                crate::tools::AGENT_COLLABORATION_CAPABILITY,
            )]),
        )
        .unwrap()
}

fn test_run_world_state() -> WorldStateSnapshot {
    test_run_world_state_for(false)
}

fn test_provider_profile() -> ProviderProfileConfig {
    ProviderProfileConfig::generic_for_dialect(
        crate::provider_profile::ProviderProtocolDialect::OpenAiChatCompletions,
    )
}

fn test_provider_key() -> ProviderProtocolKey {
    ProviderProtocolKey::new(
        crate::provider_profile::ProviderProtocolDialect::OpenAiChatCompletions,
        &test_provider_profile(),
        "checkpoint-test-model",
        Some("provider-protocol-v1:checkpoint-test".to_string()),
    )
    .unwrap()
}

fn test_run_world_state_for(image_input: bool) -> WorldStateSnapshot {
    WorldStateSnapshot::new(
        "checkpoint-test-world-state",
        0,
        vec![crate::world_state::WorldStateSectionEnvelope::host_only(
            WorldStateSectionId::ModelCapabilities,
            WorldStateLifetime::Run,
            json!({ "imageInput": image_input }),
        )
        .unwrap()],
    )
    .unwrap()
}

fn canonical_test_call_id(tool_index: usize, provider_call_id: &str) -> String {
    model_response_tool_call_id("checkpoint-validation-run", 0, tool_index, provider_call_id)
}

fn apply_patch_args(request: serde_json::Value) -> serde_json::Value {
    json!({ "request": request })
}

fn test_batch_and_context_item(
    run_id: &str,
    assistant_content: &str,
    calls: Vec<LlmToolCall>,
    suppressed_narration: bool,
) -> (ToolCallBatch, ContextItem) {
    let mut batch = ToolCallBatch::from_model_response(
        run_id,
        0,
        assistant_content.to_string(),
        calls,
        suppressed_narration,
        |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
    );
    let checkpoint_message = batch
        .checkpoint_assistant_message()
        .unwrap()
        .expect("test batch has one complete Assistant Turn");
    let group = batch.context_group().expect("test batch is not empty");
    let turn = batch
        .take_assistant_turn()
        .expect("test batch has one complete Assistant Turn");
    let item = ContextItem::new(
        LlmMessage::from_assistant_turn(turn),
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group),
    )
    .with_checkpoint_message(checkpoint_message);
    (batch, item)
}

fn pop_test_call(batch: &mut ToolCallBatch, expected_call_id: &str) -> QueuedToolCall {
    let queued = batch.pop_front().expect("test batch call");
    assert_eq!(queued.call.id, expected_call_id);
    queued
}

fn current_test_tool_identity(tool_name: &str) -> AgentToolIdentity {
    let registry = ToolRegistry::defaults_with_search(None);
    if let Some(identity) = registry.identity(tool_name) {
        return identity.clone();
    }
    if tool_name.starts_with("mcp__") {
        return AgentToolIdentity::Mcp {
            provenance: AgentMcpToolProvenance {
                server_id: "7f4a2d91-24ab-4d24-9eed-63daf26a6c15".to_string(),
                scope: AgentMcpServerScope::User,
                raw_tool_name: tool_name
                    .strip_prefix("mcp__fixture__")
                    .unwrap_or(tool_name)
                    .to_string(),
                model_tool_name: tool_name.to_string(),
                config_epoch: "66dcbb6b-92a3-4d4e-9591-f0707e4ca3e3".to_string(),
                registry_revision: 3,
                config_digest: "a".repeat(64),
                catalog_generation: 4,
                catalog_digest: "b".repeat(64),
                catalog_schema_digest: "c".repeat(64),
                schema_digest: "d".repeat(64),
                schema_normalizer_version: crate::MCP_INPUT_SCHEMA_NORMALIZER_VERSION,
            },
        };
    }
    AgentToolIdentity::Unregistered {
        tool_name: tool_name.to_string(),
    }
}

fn record_current_test_tool_call(
    trace: &mut ConversationTraceRecorder,
    batch: &ToolCallBatch,
    call: &AgentToolCall,
) -> u64 {
    let provider_identity = batch
        .assistant_turn_identity()
        .unwrap()
        .tool_call_identities
        .iter()
        .find(|identity| identity.runtime_call_id == call.id)
        .cloned()
        .expect("current test call has an exact Provider/Runtime identity");
    let sequence = trace
        .record_tool_call_with_identity(call, current_test_tool_identity(&call.tool))
        .expect("current test Tool Call is recorded once");
    trace
        .record_model_tool_call_message(
            sequence,
            0,
            &LlmMessage::assistant(
                "",
                vec![LlmToolCall {
                    id: call.id.clone(),
                    name: call.tool.clone(),
                    args: call.args.clone(),
                }],
            ),
            provider_identity,
        )
        .expect("current test Tool Call has immutable model context");
    sequence
}

fn current_test_pending_trace(
    batch: &ToolCallBatch,
    pending: &LlmToolCall,
) -> ConversationTraceRecorder {
    let mut trace = ConversationTraceRecorder::default();
    record_current_test_tool_call(
        &mut trace,
        batch,
        &AgentToolCall {
            id: pending.id.clone(),
            tool: pending.name.clone(),
            args: pending.args.clone(),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        },
    );
    trace
}

fn restorable_checkpoint_fixture_for_pending_tool(
    pending_tool_name: &str,
) -> (AgentRunCheckpoint, AgentToolContinuation) {
    let pending = LlmToolCall {
        id: canonical_test_call_id(0, "provider-pending"),
        name: pending_tool_name.to_string(),
        args: if pending_tool_name == "apply_patch" {
            apply_patch_args(json!({
                "action": "apply",
                "operation": "create",
                "filePath": "report.txt",
                "content": "report\n"
            }))
        } else {
            json!({ "path": "report.txt" })
        },
    };
    let queued = LlmToolCall {
        id: canonical_test_call_id(1, "provider-queued"),
        name: "read_file".to_string(),
        args: json!({ "path": "report.txt" }),
    };
    let mut batch = ToolCallBatch::from_model_response(
        "checkpoint-validation-run",
        0,
        String::new(),
        vec![pending.clone(), queued],
        false,
        |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
    );
    let complete_turn = batch.take_assistant_turn().unwrap();
    let pending_queued = batch.pop_front().unwrap();
    assert_eq!(pending_queued.call.id, pending.id);
    let context = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::from_assistant_turn(complete_turn),
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(pending_queued.context_group()),
    )]);
    let trace = current_test_pending_trace(&batch, &pending);
    let checkpoint = create_run_checkpoint(
        "checkpoint-validation-run",
        RunCheckpointState {
            context: &context,
            next_model_request_index: 1,
            tool_batch: &batch,
            extension_snapshots: Vec::new(),
            pending_tool_call_id: &pending.id,
            conversation_trace: &trace,
            tool_set: &test_tool_set(),
            run_context: None,
            collaboration_run_snapshot: None,
            model_capabilities: ModelCapabilities::default(),
            run_world_state: &test_run_world_state(),
            provider_profile_config: &test_provider_profile(),
            provider_protocol_key: &test_provider_key(),
        },
    )
    .unwrap();
    let continuation = AgentToolContinuation {
        call: AgentToolCall {
            id: pending.id.clone(),
            tool: pending.name.clone(),
            args: pending.args.clone(),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending.id,
            tool: pending.name,
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    };
    (checkpoint, continuation)
}

fn restorable_checkpoint_fixture() -> (AgentRunCheckpoint, AgentToolContinuation) {
    restorable_checkpoint_fixture_for_pending_tool("apply_patch")
}

struct NeverCollaborationExecutor;

impl crate::AgentCollaborationExecutor for NeverCollaborationExecutor {
    fn execute(
        &self,
        _invocation: crate::AgentCollaborationInvocation,
        _control: crate::AgentCollaborationExecutionControl,
    ) -> crate::AgentCollaborationExecutionFuture {
        Box::pin(async { panic!("checkpoint test never executes collaboration Host work") })
    }
}

fn test_collaboration_caller() -> crate::AgentCollaborationCaller {
    crate::AgentCollaborationCaller {
        agent_id: "agent-root".to_string(),
        root_agent_id: "agent-root".to_string(),
        root_conversation_id: "conversation-root".to_string(),
        parent_agent_id: None,
        conversation_id: "conversation-root".to_string(),
        project_id: Some("project-root".to_string()),
        task_name: "Root".to_string(),
        task_path: "/root".to_string(),
    }
}

fn assert_invalid_tool_call_id(error: AgentError) {
    assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
}

fn restore_error(result: AgentResult<RestoredRunCheckpoint>) -> AgentError {
    result.err().expect("checkpoint restore should fail")
}

mod creation_and_mcp;
mod restore_validation;
mod resume_projection;
