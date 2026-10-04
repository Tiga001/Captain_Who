//! Pure preparation of a provider turn and its sanitized runtime tool queue.
use super::checkpoint::ToolCallBatch;
use super::provider_continuation_runtime_error;
use crate::llm::{LlmAssistantTurn, LlmRuntimeToolCallBinding, LlmToolCall};
use crate::protocol::{AgentApprovalStatus, AgentError, AgentResult, AgentToolCall};
use crate::provider_profile::ReasoningMode;
use crate::tools::ToolRegistry;
use crate::{
    ProviderContinuationRequirement, ProviderRuntimeCapabilities, ProviderTurnRuntimePolicy,
};
use serde_json::json;

pub(super) struct ProviderToolBatchContext<'a> {
    pub(super) run_id: &'a str,
    pub(super) model_request_index: usize,
    pub(super) retained_assistant_content: &'a str,
    pub(super) suppressed_narration: bool,
    pub(super) provider_runtime_capabilities: ProviderRuntimeCapabilities,
    pub(super) reasoning_mode: ReasoningMode,
    pub(super) tool_registry: &'a ToolRegistry,
}

pub(super) struct PreparedProviderToolBatch {
    pub(super) batch: ToolCallBatch,
    pub(super) policy: ProviderTurnRuntimePolicy,
}

/// Validate provider bindings and freeze model/checkpoint projections without persisting a turn.
/// The driver retains ownership of continuation staging, terminal settlement, and tool dispatch.
pub(super) fn prepare_provider_tool_batch(
    mut assistant_turn: LlmAssistantTurn,
    tool_bindings: Vec<LlmRuntimeToolCallBinding>,
    context: ProviderToolBatchContext<'_>,
) -> AgentResult<PreparedProviderToolBatch> {
    let ProviderToolBatchContext {
        run_id,
        model_request_index,
        retained_assistant_content,
        suppressed_narration,
        provider_runtime_capabilities,
        reasoning_mode,
        tool_registry,
    } = context;

    assistant_turn.set_runtime_visible_text(retained_assistant_content);
    if assistant_turn.provider_tool_calls().is_empty() && !tool_bindings.is_empty() {
        if provider_runtime_capabilities.requires_provider_native_tool_calls() {
            return Err(AgentError::structured(
                "provider_context_boundary_required",
                "当前 Provider Profile 只能执行 Provider 原生 Tool Call，不能把文本猜测为工具协议。",
                json!({
                    "type": "providerContextBoundary",
                    "recovery": "requestNativeProviderToolCalls"
                }),
            ));
        }
        let provider_protocol = assistant_turn.provider_protocol().cloned().ok_or_else(|| {
            AgentError::new(
                "Split-projection assistant turn 不能产生新的 text-fallback Tool Call。",
            )
        })?;
        let fallback_provider_calls = tool_bindings
            .iter()
            .map(|binding| LlmToolCall {
                id: binding.provider_call_id.clone(),
                name: binding.runtime_call.name.clone(),
                args: binding.runtime_call.args.clone(),
            })
            .collect();
        assistant_turn = LlmAssistantTurn::from_provider(
            provider_protocol,
            assistant_turn.provider_visible_text(),
            fallback_provider_calls,
        )?;
        assistant_turn.set_runtime_visible_text(retained_assistant_content);
    }
    let context_bindings = tool_bindings
        .iter()
        .map(|binding| -> AgentResult<_> {
            let model_call = tool_registry.model_call_projection(&AgentToolCall {
                id: binding.runtime_call.id.clone(),
                tool: binding.runtime_call.name.clone(),
                args: binding.runtime_call.args.clone(),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            });
            let provider_call = assistant_turn
                .provider_tool_calls()
                .get(binding.provider_tool_index)
                .ok_or_else(|| {
                    AgentError::new("Runtime Tool Call 映射引用了不存在的 Provider Tool Call。")
                })?;
            Ok(LlmRuntimeToolCallBinding::new(
                binding.provider_tool_index,
                provider_call,
                LlmToolCall {
                    id: model_call.id,
                    name: model_call.tool,
                    args: model_call.args,
                },
            ))
        })
        .collect::<AgentResult<Vec<_>>>()?;
    assistant_turn.set_runtime_tool_bindings(context_bindings)?;
    let has_provider_continuation = assistant_turn.provider_continuation().is_some();
    let policy = provider_runtime_capabilities.classify_turn(
        !assistant_turn.provider_tool_calls().is_empty(),
        has_provider_continuation,
        reasoning_mode,
    );
    let continuation_requirement = policy.continuation_requirement();
    if matches!(
        (continuation_requirement, has_provider_continuation),
        (ProviderContinuationRequirement::Required, false)
            | (ProviderContinuationRequirement::Forbidden, true)
    ) {
        return Err(provider_continuation_runtime_error(
            crate::ProviderContinuationStoreError::InvalidTurn,
        ));
    }
    let batch = ToolCallBatch::from_provider_response(
        run_id,
        model_request_index,
        assistant_turn,
        tool_bindings,
        suppressed_narration,
        |call| {
            let projected = tool_registry.checkpoint_call_projection(&AgentToolCall {
                id: call.id.clone(),
                tool: call.name.clone(),
                args: call.args.clone(),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            });
            (
                LlmToolCall {
                    id: projected.id,
                    name: projected.tool,
                    args: projected.args,
                },
                tool_registry.checkpoint_persistence(&call.name),
            )
        },
    )?;
    Ok(PreparedProviderToolBatch { batch, policy })
}
