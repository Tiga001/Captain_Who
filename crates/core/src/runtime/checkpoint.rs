//! Durable agent-loop checkpoints used at approval boundaries.
//!
//! A checkpoint owns every piece of in-memory state needed to resume the same logical run. The
//! pending tool call already appears as the final, unresolved exchange in `context`; queued calls
//! belong to the same model response but have not started yet. On resume, the approved/rejected
//! result closes the pending exchange before queued calls continue.

use super::tool_failure_guard::semantic_tool_call_fingerprint;
#[cfg(test)]
use super::tool_flow::build_tool_observation_message;
#[cfg(test)]
use crate::context::ContextCapacityDetector;
use crate::context::{
    ContextFrame, ContextGroup, ContextOrigin, ContextSource, ModelToolResultGate,
};
use crate::conversation_trace::{
    canonical_tool_result_for_context, ConversationHistoryArchiveTraceMetadata,
    ConversationTraceRecorder, ConversationTurnTraceItem,
};
use crate::llm::{
    validate_model_tool_call_id, validate_provider_tool_call_id, LlmAssistantTurn,
    LlmRuntimeToolCallBinding, LlmToolCall,
};
use crate::protocol::{
    AgentAssistantTurnCheckpointIdentity, AgentContextCheckpointItem,
    AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentProviderToolCallIdentity, AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint,
    AgentRunContext, AgentRunToolSetCheckpoint, AgentToolContinuation, ModelCapabilities,
    AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::tools::{
    validate_tool_set_checkpoint_shape, AgentToolCallCheckpointPersistence, EffectiveToolSet,
};
use crate::world_state::{
    WorldStateLifetime, WorldStateSectionId, WorldStateSnapshot, WorldStateVisibility,
};
use std::collections::{BTreeSet, VecDeque};

#[derive(Debug, Clone)]
pub(super) struct QueuedToolCall {
    /// Original provider identity and arguments. The complete provider turn is the replay
    /// authority; this copy exists only to keep the ordered identity binding attached while the
    /// runtime executes the queue.
    pub(super) provider_call: LlmToolCall,
    pub(super) provider_tool_index: usize,
    pub(super) call: LlmToolCall,
    /// Security projection that may enter a durable approval checkpoint. It is never executed.
    pub(super) checkpoint_call: LlmToolCall,
    /// Whether this queued call may cross a durable approval boundary.
    ///
    /// MCP calls are denied as a class until resumable external-tool authorization and a Secret
    /// Store exist. Field-name redaction is intentionally not treated as a persistence boundary.
    pub(super) checkpoint_persistence: AgentToolCallCheckpointPersistence,
    pub(super) assistant_content: String,
    pub(super) group_id: String,
}

impl QueuedToolCall {
    pub(super) fn context_group(&self) -> ContextGroup {
        ContextGroup::tool_exchange(self.group_id.clone())
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct ToolCallBatch {
    queue: VecDeque<QueuedToolCall>,
    /// The one authoritative assistant turn that produced this execution batch. Runtime policy
    /// and results refer to its bindings; no per-call assistant copies are retained.
    assistant_turn: Option<LlmAssistantTurn>,
    assistant_turn_identity: Option<AgentAssistantTurnCheckpointIdentity>,
    deferred_external_tool_call_count: u32,
    suppressed_narration: bool,
    /// Semantic calls already accepted from this one model response.
    ///
    /// This is not a global result cache. It only prevents duplicate side effects inside one
    /// provider response. Approval restore reconstructs it from the checkpoint's durable tool
    /// exchange groups, so pausing cannot make a queued duplicate executable again.
    seen_semantic_fingerprints: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolCallBatchClaim {
    Execute,
    Duplicate { semantic_fingerprint: String },
}

impl ToolCallBatch {
    #[cfg(test)]
    pub(super) fn from_model_response(
        run_id: &str,
        model_request_index: usize,
        assistant_content: String,
        calls: Vec<LlmToolCall>,
        suppressed_narration: bool,
        checkpoint_projection: impl FnMut(
            &LlmToolCall,
        ) -> (LlmToolCall, AgentToolCallCheckpointPersistence),
    ) -> Self {
        let bindings = calls
            .iter()
            .cloned()
            .enumerate()
            .map(|(provider_tool_index, call)| {
                LlmRuntimeToolCallBinding::new(provider_tool_index, &call, call.clone())
            })
            .collect::<Vec<_>>();
        let assistant_turn = LlmAssistantTurn::from_legacy(assistant_content, calls)
            .with_runtime_tool_bindings(bindings)
            .expect("identity bindings built from the same provider calls");
        Self::from_provider_response(
            run_id,
            model_request_index,
            assistant_turn,
            Vec::new(),
            suppressed_narration,
            checkpoint_projection,
        )
        .expect("legacy test batch is assembled from matching provider/runtime calls")
    }

    pub(super) fn from_provider_response(
        run_id: &str,
        model_request_index: usize,
        mut assistant_turn: LlmAssistantTurn,
        runtime_bindings: Vec<LlmRuntimeToolCallBinding>,
        suppressed_narration: bool,
        mut checkpoint_projection: impl FnMut(
            &LlmToolCall,
        )
            -> (LlmToolCall, AgentToolCallCheckpointPersistence),
    ) -> AgentResult<Self> {
        let assistant_content = assistant_turn.visible_text().to_string();
        let runtime_bindings = if runtime_bindings.is_empty() {
            assistant_turn
                .runtime_tool_bindings()
                .unwrap_or_default()
                .to_vec()
        } else {
            runtime_bindings
        };
        let queue = runtime_bindings
            .iter()
            .cloned()
            .enumerate()
            .map(|(batch_index, binding)| -> AgentResult<QueuedToolCall> {
                let call = binding.runtime_call;
                let provider_call = assistant_turn
                    .provider_tool_calls()
                    .get(binding.provider_tool_index)
                    .ok_or_else(|| {
                        AgentError::new("Tool Call 批次引用了不存在的 Provider Tool Call。")
                    })?
                    .clone();
                let (checkpoint_call, checkpoint_persistence) = checkpoint_projection(&call);
                Ok(QueuedToolCall {
                    provider_call,
                    provider_tool_index: binding.provider_tool_index,
                    call,
                    checkpoint_call,
                    checkpoint_persistence,
                    assistant_content: if batch_index == 0 {
                        assistant_content.clone()
                    } else {
                        String::new()
                    },
                    group_id: format!("run:{run_id}:tool-exchange:{}", model_request_index + 1),
                })
            })
            .collect::<AgentResult<VecDeque<_>>>()?;
        if assistant_turn.runtime_tool_bindings().is_none() {
            let context_bindings = queue
                .iter()
                .map(|queued| {
                    LlmRuntimeToolCallBinding::new(
                        queued.provider_tool_index,
                        &queued.provider_call,
                        queued.call.clone(),
                    )
                })
                .collect();
            assistant_turn.set_runtime_tool_bindings(context_bindings)?;
        } else {
            let context_ids = assistant_turn
                .runtime_tool_bindings()
                .unwrap_or_default()
                .iter()
                .map(|binding| binding.runtime_call.id.as_str())
                .collect::<Vec<_>>();
            let execution_ids = queue
                .iter()
                .map(|queued| queued.call.id.as_str())
                .collect::<Vec<_>>();
            if context_ids != execution_ids {
                return Err(AgentError::new(
                    "Tool Call 批次的 Context 与执行身份顺序不一致。",
                ));
            }
        }
        let assistant_turn_identity = assistant_turn.checkpoint_identity()?;
        Ok(Self {
            queue,
            assistant_turn: Some(assistant_turn),
            assistant_turn_identity: Some(assistant_turn_identity),
            deferred_external_tool_call_count: 0,
            suppressed_narration,
            seen_semantic_fingerprints: BTreeSet::new(),
        })
    }

    pub(super) fn claim(&mut self, call: &LlmToolCall) -> ToolCallBatchClaim {
        let semantic_fingerprint = semantic_tool_call_fingerprint(&call.name, &call.args);
        if self
            .seen_semantic_fingerprints
            .insert(semantic_fingerprint.clone())
        {
            ToolCallBatchClaim::Execute
        } else {
            ToolCallBatchClaim::Duplicate {
                semantic_fingerprint,
            }
        }
    }

    pub(super) fn pop_front(&mut self) -> Option<QueuedToolCall> {
        self.queue.pop_front()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Drops queued external calls that do not yet have a one-time Host preparation.
    ///
    /// Only a count survives the approval checkpoint. In particular, model-authored arguments,
    /// Server names and Server-authored Tool names never enter this diagnostic channel.
    pub(super) fn defer_external_calls(
        &mut self,
        mut is_external: impl FnMut(&QueuedToolCall) -> bool,
    ) -> Vec<QueuedToolCall> {
        let mut retained = VecDeque::with_capacity(self.queue.len());
        let mut dropped = Vec::new();
        while let Some(call) = self.queue.pop_front() {
            if is_external(&call) {
                dropped.push(call);
            } else {
                retained.push_back(call);
            }
        }
        self.queue = retained;
        let dropped_count = u32::try_from(dropped.len()).unwrap_or(u32::MAX);
        self.deferred_external_tool_call_count = self
            .deferred_external_tool_call_count
            .saturating_add(dropped_count);
        dropped
    }

    pub(super) fn take_deferred_external_tool_call_count(&mut self) -> Option<u32> {
        if self.queue.is_empty() && self.deferred_external_tool_call_count > 0 {
            Some(std::mem::take(&mut self.deferred_external_tool_call_count))
        } else {
            None
        }
    }

    pub(super) fn take_suppressed_narration(&mut self) -> bool {
        if self.queue.is_empty() && self.suppressed_narration {
            self.suppressed_narration = false;
            true
        } else {
            false
        }
    }

    fn assistant_turn_identity(&self) -> AgentResult<&AgentAssistantTurnCheckpointIdentity> {
        self.assistant_turn_identity.as_ref().ok_or_else(|| {
            AgentError::new("无法创建运行检查点：工具批次缺少 Provider Assistant Turn 身份。")
        })
    }

    pub(super) fn take_assistant_turn(&mut self) -> Option<LlmAssistantTurn> {
        self.assistant_turn.take()
    }

    pub(super) fn context_group(&self) -> Option<ContextGroup> {
        self.queue.front().map(QueuedToolCall::context_group)
    }

    pub(super) fn checkpoint_assistant_message(
        &self,
    ) -> AgentResult<Option<crate::llm::LlmMessage>> {
        let Some(turn) = self.assistant_turn.as_ref() else {
            return Ok(None);
        };
        let checkpoint_calls: Vec<LlmToolCall> = self
            .queue
            .iter()
            .map(|queued| queued.checkpoint_call.clone())
            .collect();
        let mut checkpoint_turn = turn.without_raw_continuation_for_checkpoint();
        let bindings = checkpoint_turn
            .runtime_tool_bindings()
            .unwrap_or_default()
            .iter()
            .zip(checkpoint_calls)
            .map(|(binding, runtime_call)| -> AgentResult<_> {
                let provider_call = checkpoint_turn
                    .provider_tool_calls()
                    .get(binding.provider_tool_index)
                    .ok_or_else(|| {
                        AgentError::new(
                            "Checkpoint Tool Call 映射引用了不存在的 Provider Tool Call。",
                        )
                    })?;
                Ok(LlmRuntimeToolCallBinding::new(
                    binding.provider_tool_index,
                    provider_call,
                    runtime_call,
                ))
            })
            .collect::<AgentResult<Vec<_>>>()?;
        checkpoint_turn.set_runtime_tool_bindings(bindings)?;
        Ok(Some(crate::llm::LlmMessage::from_assistant_turn(
            checkpoint_turn,
        )))
    }
}

pub(super) struct RestoredRunCheckpoint {
    pub(super) context: ContextFrame,
    pub(super) next_model_request_index: usize,
    pub(super) tool_batch: ToolCallBatch,
    pub(super) extension_snapshots: Vec<AgentExtensionSnapshot>,
    pub(super) conversation_trace: ConversationTraceRecorder,
    pub(super) tool_set: AgentRunToolSetCheckpoint,
    pub(super) run_context: Option<AgentRunContext>,
    pub(super) model_capabilities: ModelCapabilities,
    pub(super) run_world_state: WorldStateSnapshot,
    pub(super) provider_profile_config: ProviderProfileConfig,
    pub(super) provider_protocol_key: ProviderProtocolKey,
}

pub(super) struct RunCheckpointState<'a> {
    pub(super) context: &'a ContextFrame,
    pub(super) next_model_request_index: usize,
    pub(super) tool_batch: &'a ToolCallBatch,
    pub(super) extension_snapshots: Vec<AgentExtensionSnapshot>,
    pub(super) pending_tool_call_id: &'a str,
    pub(super) conversation_trace: &'a ConversationTraceRecorder,
    pub(super) tool_set: &'a EffectiveToolSet,
    pub(super) run_context: Option<&'a AgentRunContext>,
    pub(super) model_capabilities: ModelCapabilities,
    pub(super) run_world_state: &'a WorldStateSnapshot,
    pub(super) provider_profile_config: &'a ProviderProfileConfig,
    pub(super) provider_protocol_key: &'a ProviderProtocolKey,
}

pub(super) fn create_run_checkpoint(
    run_id: &str,
    state: RunCheckpointState<'_>,
) -> AgentResult<AgentRunCheckpoint> {
    let RunCheckpointState {
        context,
        next_model_request_index,
        tool_batch,
        extension_snapshots,
        pending_tool_call_id,
        conversation_trace,
        tool_set,
        run_context,
        model_capabilities,
        run_world_state,
        provider_profile_config,
        provider_protocol_key,
    } = state;
    validate_model_tool_call_id(pending_tool_call_id)?;
    for queued in &tool_batch.queue {
        validate_model_tool_call_id(&queued.call.id)?;
    }
    let queued_tool_call_ids = tool_batch
        .queue
        .iter()
        .map(|queued| queued.call.id.clone())
        .collect::<Vec<_>>();
    context.validate_pending_tool_batch(pending_tool_call_id, &queued_tool_call_ids)?;
    provider_profile_config
        .validate()
        .map_err(|error| AgentError::new(format!("运行检查点的 Provider profile 无效：{error}")))?;
    provider_protocol_key
        .validate_against_config(provider_profile_config)
        .map_err(|error| AgentError::new(format!("运行检查点的 Provider key 无效：{error}")))?;
    let assistant_turn_identity = tool_batch.assistant_turn_identity()?.clone();
    validate_assistant_turn_identity(&assistant_turn_identity, pending_tool_call_id, tool_batch)?;
    context.validate_assistant_turn_checkpoint_identity(
        &assistant_turn_identity,
        pending_tool_call_id,
        &queued_tool_call_ids,
        true,
    )?;
    let (
        conversation_trace_items,
        conversation_model_context_items,
        next_conversation_trace_sequence,
        conversation_trace_truncated,
    ) = conversation_trace.checkpoint();
    validate_conversation_trace_tool_call_ids(&conversation_trace_items)?;
    let mut context_items = context.checkpoint_items()?;
    project_mcp_result_context_for_checkpoint(&mut context_items);
    validate_context_checkpoint_tool_call_ids(&context_items)?;
    validate_checkpoint_world_state(run_world_state, model_capabilities)?;
    Ok(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items,
        next_model_request_index,
        queued_tool_calls: tool_batch
            .queue
            .iter()
            .map(|queued| {
                queued_tool_call_checkpoint(queued, &assistant_turn_identity.assistant_turn_id)
            })
            .collect::<AgentResult<Vec<_>>>()?,
        deferred_external_tool_call_count: tool_batch.deferred_external_tool_call_count,
        suppressed_narration: tool_batch.suppressed_narration,
        extension_snapshots,
        tool_set: tool_set.checkpoint(),
        run_context: run_context.cloned(),
        model_capabilities,
        provider_profile_config: provider_profile_config.clone(),
        provider_protocol_key: provider_protocol_key.clone(),
        assistant_turn_identity,
        run_world_state: run_world_state.clone(),
        pending_action_id: None,
        pending_tool_call_id: pending_tool_call_id.to_string(),
        conversation_trace_items,
        conversation_model_context_items,
        next_conversation_trace_sequence,
        conversation_trace_truncated,
    })
}

fn validate_assistant_turn_identity(
    identity: &AgentAssistantTurnCheckpointIdentity,
    pending_tool_call_id: &str,
    tool_batch: &ToolCallBatch,
) -> AgentResult<()> {
    if identity.assistant_turn_id.trim().is_empty()
        || identity.assistant_turn_digest.trim().is_empty()
    {
        return Err(AgentError::new(
            "运行检查点的 Provider Assistant Turn 身份为空。",
        ));
    }
    let mut provider_indexes = BTreeSet::new();
    let mut runtime_call_ids = BTreeSet::new();
    let mut previous_index = None;
    for mapping in &identity.tool_call_identities {
        validate_provider_tool_call_id(&mapping.provider_call_id)?;
        if mapping.runtime_call_id.trim().is_empty()
            || !provider_indexes.insert(mapping.provider_tool_index)
            || !runtime_call_ids.insert(mapping.runtime_call_id.clone())
            || previous_index.is_some_and(|previous| mapping.provider_tool_index <= previous)
        {
            return Err(AgentError::new(
                "运行检查点的 Provider Tool Call 身份映射无效或无序。",
            ));
        }
        validate_model_tool_call_id(&mapping.runtime_call_id)?;
        previous_index = Some(mapping.provider_tool_index);
    }

    let pending_mapping_index = identity
        .tool_call_identities
        .iter()
        .position(|mapping| mapping.runtime_call_id == pending_tool_call_id)
        .ok_or_else(|| AgentError::new("运行检查点的 Provider Tool Call 映射缺少待审批调用。"))?;
    let mut unresolved_suffix = Vec::with_capacity(tool_batch.queue.len().saturating_add(1));
    unresolved_suffix.push(pending_tool_call_id);
    unresolved_suffix.extend(
        tool_batch
            .queue
            .iter()
            .map(|queued| queued.call.id.as_str()),
    );
    let unresolved_positions = unresolved_suffix
        .iter()
        .map(|runtime_call_id| {
            identity
                .tool_call_identities
                .iter()
                .position(|mapping| mapping.runtime_call_id == *runtime_call_id)
                .ok_or_else(|| {
                    AgentError::new("运行检查点的 Provider Tool Call 映射缺少未结算调用。")
                })
        })
        .collect::<AgentResult<Vec<_>>>()?;
    if unresolved_positions.first().copied() != Some(pending_mapping_index)
        || unresolved_positions
            .windows(2)
            .any(|positions| positions[0] >= positions[1])
    {
        return Err(AgentError::new(
            "运行检查点中的待审批及 queued Tool Calls 未保持完整 Provider Turn 的原序。",
        ));
    }
    let deferred_external_tool_call_count =
        usize::try_from(tool_batch.deferred_external_tool_call_count)
            .map_err(|_| AgentError::new("运行检查点中的外部 Tool Call 延后计数超出平台范围。"))?;
    let expected_remaining = tool_batch
        .queue
        .len()
        .saturating_add(1)
        .saturating_add(deferred_external_tool_call_count);
    if identity
        .tool_call_identities
        .len()
        .saturating_sub(pending_mapping_index)
        != expected_remaining
    {
        return Err(AgentError::new(
            "运行检查点中的未结算 Tool Calls 与已延后的外部调用数量不一致。",
        ));
    }

    for queued in &tool_batch.queue {
        let provider_tool_index = u32::try_from(queued.provider_tool_index).unwrap_or(u32::MAX);
        let matches = identity.tool_call_identities.iter().any(|mapping| {
            mapping.provider_tool_index == provider_tool_index
                && mapping.provider_call_id == queued.provider_call.id
                && mapping.runtime_call_id == queued.call.id
        });
        if !matches {
            return Err(AgentError::new(
                "运行检查点的 queued Tool Call 与 Provider 身份映射不一致。",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn restore_run_checkpoint(
    checkpoint: AgentRunCheckpoint,
    run_id: &str,
    continuation: &AgentToolContinuation,
) -> AgentResult<RestoredRunCheckpoint> {
    restore_run_checkpoint_with_history_ref(checkpoint, run_id, continuation, None)
}

#[cfg(test)]
pub(super) fn restore_run_checkpoint_with_history_ref(
    checkpoint: AgentRunCheckpoint,
    run_id: &str,
    continuation: &AgentToolContinuation,
    assistant_message_id: Option<&str>,
) -> AgentResult<RestoredRunCheckpoint> {
    let gate = ContextCapacityDetector::for_model(
        "checkpoint-compatibility",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &[],
    )
    .model_tool_result_gate();
    restore_run_checkpoint_with_model_projection(
        checkpoint,
        run_id,
        continuation,
        assistant_message_id,
        &gate,
        &ConversationHistoryArchiveTraceMetadata::default(),
    )
}

pub(super) fn restore_run_checkpoint_with_model_projection(
    checkpoint: AgentRunCheckpoint,
    run_id: &str,
    continuation: &AgentToolContinuation,
    assistant_message_id: Option<&str>,
    model_tool_result_gate: &ModelToolResultGate,
    archive_metadata: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<RestoredRunCheckpoint> {
    if checkpoint.version != AGENT_RUN_CHECKPOINT_SCHEMA_VERSION {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：不支持版本 {}，当前版本为 {AGENT_RUN_CHECKPOINT_SCHEMA_VERSION}。",
            checkpoint.version,
        )));
    }
    if checkpoint.run_id != run_id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：检查点属于 `{}`，当前运行是 `{run_id}`。",
            checkpoint.run_id
        )));
    }
    checkpoint
        .provider_profile_config
        .validate()
        .map_err(|error| {
            AgentError::new(format!(
                "无法恢复运行检查点：Provider profile 无效：{error}"
            ))
        })?;
    checkpoint
        .provider_protocol_key
        .validate_against_config(&checkpoint.provider_profile_config)
        .map_err(|error| {
            AgentError::new(format!(
                "无法恢复运行检查点：Provider protocol key 无效：{error}"
            ))
        })?;
    validate_model_tool_call_id(&checkpoint.pending_tool_call_id)?;
    if checkpoint
        .pending_action_id
        .as_ref()
        .is_some_and(|action_id| {
            action_id.trim().is_empty()
                || action_id.trim() != action_id
                || action_id.len() > 2_048
                || action_id.chars().any(char::is_control)
        })
    {
        return Err(AgentError::new("无法恢复运行检查点：待审批动作标识无效。"));
    }
    validate_tool_set_checkpoint_shape(&checkpoint.tool_set)?;
    validate_checkpoint_world_state(&checkpoint.run_world_state, checkpoint.model_capabilities)?;
    validate_model_tool_call_id(&continuation.call.id)?;
    validate_model_tool_call_id(&continuation.result.call_id)?;
    validate_context_checkpoint_tool_call_ids(&checkpoint.context_items)?;
    validate_queued_checkpoint_tool_call_ids(&checkpoint.queued_tool_calls)?;
    validate_conversation_trace_tool_call_ids(&checkpoint.conversation_trace_items)?;
    if checkpoint.pending_tool_call_id != continuation.call.id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：待审批调用 `{}` 与续跑结果 `{}` 不一致。",
            checkpoint.pending_tool_call_id, continuation.call.id
        )));
    }
    if continuation.call.id != continuation.result.call_id {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：续跑调用 `{}` 与工具结果 `{}` 不一致。",
            continuation.call.id, continuation.result.call_id
        )));
    }
    if continuation.call.tool != continuation.result.tool {
        return Err(AgentError::new(format!(
            "无法恢复运行检查点：续跑调用工具 `{}` 与工具结果 `{}` 不一致。",
            continuation.call.tool, continuation.result.tool
        )));
    }
    if checkpoint.next_model_request_index == 0 {
        return Err(AgentError::new(
            "无法恢复运行检查点：下一次模型请求序号无效。",
        ));
    }

    let continuation_result_sequence =
        continuation_result_sequence(&checkpoint, &continuation.call.id);
    let provider_profile_config = checkpoint.provider_profile_config.clone();
    let provider_protocol_key = checkpoint.provider_protocol_key.clone();
    let assistant_turn_identity = checkpoint.assistant_turn_identity.clone();
    let tool_set = checkpoint.tool_set;
    let restored_batch_fingerprints =
        restore_batch_fingerprints(&checkpoint.context_items, &checkpoint.pending_tool_call_id)?;
    let mut conversation_trace = ConversationTraceRecorder::from_checkpoint_with_model_context(
        checkpoint.conversation_trace_items,
        checkpoint.conversation_model_context_items,
        checkpoint.next_conversation_trace_sequence,
        checkpoint.conversation_trace_truncated,
    );
    let mut context = ContextFrame::from_checkpoint_items(checkpoint.context_items)?;
    let queue = restore_queued_tool_calls(
        checkpoint.queued_tool_calls,
        &checkpoint.pending_tool_call_id,
        &assistant_turn_identity,
        &context,
    )?;
    let queued_tool_call_ids = queue
        .iter()
        .map(|queued| queued.call.id.clone())
        .collect::<Vec<_>>();
    let checkpoint_call = context
        .validate_pending_tool_batch(&checkpoint.pending_tool_call_id, &queued_tool_call_ids)?;
    let restored_batch = ToolCallBatch {
        queue,
        // The checkpoint Context already contains the continuation-free Assistant Turn. Restore
        // must not append a second authoritative assistant message.
        assistant_turn: None,
        assistant_turn_identity: Some(assistant_turn_identity),
        deferred_external_tool_call_count: checkpoint.deferred_external_tool_call_count,
        suppressed_narration: checkpoint.suppressed_narration,
        seen_semantic_fingerprints: restored_batch_fingerprints,
    };
    validate_assistant_turn_identity(
        restored_batch.assistant_turn_identity()?,
        &checkpoint.pending_tool_call_id,
        &restored_batch,
    )?;
    context.validate_assistant_turn_checkpoint_identity(
        restored_batch.assistant_turn_identity()?,
        &checkpoint.pending_tool_call_id,
        &queued_tool_call_ids,
        false,
    )?;
    if checkpoint_call.name != continuation.call.tool
        || checkpoint_call.args != continuation.call.args
    {
        return Err(AgentError::new(
            "无法恢复运行检查点：续跑调用的工具或参数与冻结的待审批调用不一致。",
        ));
    }
    let continuation_call = LlmToolCall {
        id: continuation.call.id.clone(),
        name: continuation.call.tool.clone(),
        args: continuation.call.args.clone(),
    };
    let is_mcp = checkpoint.pending_action_id.is_some();
    let durable_result = if is_mcp {
        crate::tools::mcp_tool_result_persistence_projection(&continuation.result)
    } else {
        canonical_tool_result_for_context(&continuation.result)
    };
    let llm_result = if is_mcp {
        crate::tools::mcp_tool_result_model_projection(&continuation.result)
    } else {
        crate::tools::model_projection_for_persisted_continuation(&continuation.result)
    };
    let model_observation = super::finalize_model_tool_observation(
        model_tool_result_gate,
        &continuation.call.id,
        !continuation.result.ok,
        &llm_result,
        archive_metadata,
    )?;
    let persisted_model_observation = if is_mcp {
        super::finalize_model_tool_observation(
            model_tool_result_gate,
            &continuation.call.id,
            !continuation.result.ok,
            &durable_result,
            archive_metadata,
        )?
    } else {
        model_observation.clone()
    };
    context.append_tool_continuation_in_batch(
        &continuation_call,
        model_observation.clone(),
        is_mcp.then_some(persisted_model_observation.clone()),
        !continuation.result.ok,
        is_mcp,
        assistant_message_id.map(|assistant_message_id| {
            ContextOrigin::conversation_trace_item(
                assistant_message_id,
                continuation_result_sequence,
            )
        }),
        &queued_tool_call_ids,
    )?;
    let call_sequence = conversation_trace.record_tool_call(&continuation.call);
    if let Some(sequence) = call_sequence {
        conversation_trace.record_model_message(
            sequence,
            0,
            &crate::llm::LlmMessage::assistant("", vec![continuation_call.clone()]),
        );
    }
    if let Some(sequence) = conversation_trace.record_tool_result_with_archive(
        &continuation.call,
        &durable_result,
        archive_metadata.clone(),
    ) {
        conversation_trace.record_model_message(
            sequence,
            0,
            &crate::llm::LlmMessage::tool_result(
                continuation.call.id.clone(),
                persisted_model_observation,
                !continuation.result.ok,
            ),
        );
    }

    Ok(RestoredRunCheckpoint {
        context,
        next_model_request_index: checkpoint.next_model_request_index,
        tool_batch: restored_batch,
        extension_snapshots: checkpoint.extension_snapshots,
        conversation_trace,
        tool_set,
        run_context: checkpoint.run_context,
        model_capabilities: checkpoint.model_capabilities,
        run_world_state: checkpoint.run_world_state,
        provider_profile_config,
        provider_protocol_key,
    })
}

pub(super) const MCP_DURABLE_RESULT_PLACEHOLDER: &str =
    "MCP result content omitted from durable state; consult the live invocation lifecycle.";

fn project_mcp_result_context_for_checkpoint(items: &mut [AgentContextCheckpointItem]) {
    for item in items {
        if item
            .sources
            .iter()
            .any(|source| source == ContextSource::McpToolResult.as_str())
        {
            if !is_safe_mcp_durable_observation(&item.content) {
                item.content = MCP_DURABLE_RESULT_PLACEHOLDER.to_string();
            }
            item.images.clear();
        }
    }
}

fn is_safe_mcp_durable_observation(content: &str) -> bool {
    let Ok(serde_json::Value::Object(value)) = serde_json::from_str(content) else {
        return content == MCP_DURABLE_RESULT_PLACEHOLDER;
    };
    const ALLOWED_FIELDS: &[&str] = &[
        "schemaVersion",
        "type",
        "external",
        "status",
        "outcome",
        "dispatchCertainty",
        "contentOmitted",
        "isError",
        "feedbackProvided",
        "code",
        "retryable",
        "error",
    ];
    if value
        .keys()
        .any(|key| !ALLOWED_FIELDS.contains(&key.as_str()))
    {
        return false;
    }
    if value
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || value.get("type").and_then(serde_json::Value::as_str) != Some("mcp_tool")
        || value.get("external").and_then(serde_json::Value::as_bool) != Some(true)
        || value
            .get("contentOmitted")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
    {
        return false;
    }
    let status_is_valid = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|status| {
            matches!(
                status,
                "completed"
                    | "rejected"
                    | "cancelled"
                    | "expired"
                    | "payload_unavailable"
                    | "policy_denied"
                    | "outcome_unknown"
                    | "failed"
            )
        });
    let outcome_is_valid = value
        .get("outcome")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|outcome| {
            matches!(
                outcome,
                "succeeded"
                    | "tool_error"
                    | "output_too_large"
                    | "transport_error"
                    | "timed_out"
                    | "cancelled"
                    | "rejected"
                    | "expired"
                    | "payload_unavailable"
                    | "policy_denied"
                    | "outcome_unknown"
            )
        });
    let dispatch_is_valid = value
        .get("dispatchCertainty")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|certainty| {
            matches!(
                certainty,
                "definitely_not_dispatched" | "possibly_dispatched" | "response_received"
            )
        });
    let code_is_safe = value.get("code").is_none_or(|code| {
        code.as_str().is_some_and(|code| {
            code.starts_with("mcp.")
                && code.len() <= 128
                && code
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
    });
    status_is_valid
        && outcome_is_valid
        && dispatch_is_valid
        && value
            .get("isError")
            .is_some_and(serde_json::Value::is_boolean)
        && value
            .get("feedbackProvided")
            .is_none_or(|feedback| feedback.as_bool() == Some(true))
        && value
            .get("retryable")
            .is_none_or(serde_json::Value::is_boolean)
        && code_is_safe
        && value
            .get("error")
            .is_none_or(|error| error.as_str() == Some("The external MCP tool reported an error."))
}

fn validate_checkpoint_world_state(
    snapshot: &WorldStateSnapshot,
    model_capabilities: ModelCapabilities,
) -> AgentResult<()> {
    snapshot
        .validate()
        .map_err(|error| AgentError::new(format!("运行检查点的 World State 无效：{error}")))?;
    if snapshot
        .sections
        .iter()
        .any(|section| section.lifetime != WorldStateLifetime::Run)
    {
        return Err(AgentError::new(
            "运行检查点的 World State 只能包含 Run-lifetime section。",
        ));
    }
    let capability = snapshot
        .section(&WorldStateSectionId::ModelCapabilities)
        .ok_or_else(|| AgentError::new("运行检查点缺少模型能力 World State。"))?;
    if capability.visibility != WorldStateVisibility::HostOnly
        || capability.model_projection.is_some()
        || capability
            .state
            .get("imageInput")
            .and_then(serde_json::Value::as_bool)
            != Some(model_capabilities.image_input)
    {
        return Err(AgentError::new(
            "运行检查点的模型能力与冻结的 World State 不一致。",
        ));
    }
    Ok(())
}

fn restore_batch_fingerprints(
    context_items: &[AgentContextCheckpointItem],
    pending_tool_call_id: &str,
) -> AgentResult<BTreeSet<String>> {
    let pending_item = context_items
        .iter()
        .find(|item| {
            item.tool_calls
                .iter()
                .any(|call| call.id == pending_tool_call_id)
        })
        .ok_or_else(|| AgentError::new("运行检查点缺少冻结的待审批工具调用。"))?;
    let mut fingerprints = BTreeSet::new();
    // A complete Assistant Turn contains both the settled prefix and unresolved suffix. Seed only
    // calls through the pending action: later queued calls have not executed and must remain
    // eligible after approval recovery.
    for call in &pending_item.tool_calls {
        fingerprints.insert(semantic_tool_call_fingerprint(&call.name, &call.args));
        if call.id == pending_tool_call_id {
            return Ok(fingerprints);
        }
    }
    Err(AgentError::new(
        "运行检查点的完整 Assistant Turn 缺少待审批 Tool Call。",
    ))
}

fn queued_tool_call_checkpoint(
    call: &QueuedToolCall,
    assistant_turn_id: &str,
) -> AgentResult<AgentQueuedToolCallCheckpoint> {
    if call.checkpoint_persistence != AgentToolCallCheckpointPersistence::Allowed {
        let code = match call.checkpoint_persistence {
            AgentToolCallCheckpointPersistence::DeniedMcp => "mcpToolCallPersistenceDenied",
            AgentToolCallCheckpointPersistence::DeniedUnknown => "unknownToolCallPersistenceDenied",
            AgentToolCallCheckpointPersistence::Allowed => unreachable!("checked above"),
        };
        return Err(AgentError::structured(
            "agent.checkpoint_private_tool_arguments",
            "无法创建运行检查点：同批待执行工具调用不能在当前版本中安全持久化。",
            serde_json::json!({
                "type": "checkpoint",
                "code": code,
                "recovery": "restartRun",
                "tool": call.call.name,
            }),
        ));
    }
    if call.checkpoint_call.id != call.call.id || call.checkpoint_call.name != call.call.name {
        return Err(AgentError::new(
            "无法创建运行检查点：工具调用安全投影改变了调用身份。",
        ));
    }
    if call.checkpoint_call.args != call.call.args {
        return Err(AgentError::structured(
            "agent.checkpoint_private_tool_arguments",
            "无法创建运行检查点：同批待执行工具调用包含不能安全持久化的参数。",
            serde_json::json!({
                "type": "checkpoint",
                "code": "privateToolArguments",
                "recovery": "restartRun",
                "tool": call.call.name,
            }),
        ));
    }
    Ok(AgentQueuedToolCallCheckpoint {
        call: AgentContextCheckpointToolCall {
            id: call.checkpoint_call.id.clone(),
            name: call.checkpoint_call.name.clone(),
            args: call.checkpoint_call.args.clone(),
            provider_identity: Some(AgentProviderToolCallIdentity {
                provider_tool_index: u32::try_from(call.provider_tool_index).unwrap_or(u32::MAX),
                provider_call_id: call.provider_call.id.clone(),
                runtime_call_id: call.call.id.clone(),
            }),
        },
        assistant_content: call.assistant_content.clone(),
        group_id: call.group_id.clone(),
        assistant_turn_id: assistant_turn_id.to_string(),
        provider_tool_index: u32::try_from(call.provider_tool_index).unwrap_or(u32::MAX),
    })
}

fn validate_context_checkpoint_tool_call_ids(
    items: &[AgentContextCheckpointItem],
) -> AgentResult<()> {
    for item in items {
        if let Some(call_id) = &item.tool_call_id {
            validate_model_tool_call_id(call_id)?;
        }
        for call in &item.tool_calls {
            validate_model_tool_call_id(&call.id)?;
        }
    }
    Ok(())
}

fn validate_queued_checkpoint_tool_call_ids(
    calls: &[AgentQueuedToolCallCheckpoint],
) -> AgentResult<()> {
    for queued in calls {
        validate_model_tool_call_id(&queued.call.id)?;
    }
    Ok(())
}

fn validate_conversation_trace_tool_call_ids(
    items: &[ConversationTurnTraceItem],
) -> AgentResult<()> {
    for item in items {
        match item {
            ConversationTurnTraceItem::ToolCall { call_id, .. }
            | ConversationTurnTraceItem::ToolResult { call_id, .. } => {
                validate_model_tool_call_id(call_id)?;
            }
            ConversationTurnTraceItem::AssistantNarration { .. }
            | ConversationTurnTraceItem::UserGuidance { .. }
            | ConversationTurnTraceItem::CommandSessionLifecycle { .. } => {}
        }
    }
    Ok(())
}

pub(super) fn continuation_result_sequence(checkpoint: &AgentRunCheckpoint, call_id: &str) -> u64 {
    checkpoint
        .next_conversation_trace_sequence
        .saturating_add(u64::from(!checkpoint.conversation_trace_items.iter().any(
            |item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall {
                        call_id: recorded_call_id,
                        ..
                    } if recorded_call_id == call_id
                )
            },
        )))
}

fn restore_queued_tool_calls(
    calls: Vec<AgentQueuedToolCallCheckpoint>,
    pending_tool_call_id: &str,
    assistant_turn_identity: &AgentAssistantTurnCheckpointIdentity,
    context: &ContextFrame,
) -> AgentResult<VecDeque<QueuedToolCall>> {
    let mut ids = BTreeSet::new();
    ids.insert(pending_tool_call_id.to_string());
    calls
        .into_iter()
        .map(|queued| {
            validate_model_tool_call_id(&queued.call.id)?;
            if queued.call.name.trim().is_empty() {
                return Err(AgentError::new("运行检查点中的待执行工具调用缺少名称。"));
            }
            if !ids.insert(queued.call.id.clone()) {
                return Err(AgentError::new(format!(
                    "运行检查点中的工具调用 id `{}` 重复。",
                    queued.call.id
                )));
            }
            if !context.contains_tool_call_id(&queued.call.id) {
                return Err(AgentError::new(format!(
                    "运行检查点中的待执行工具调用 id `{}` 未出现在完整 Assistant Turn 中。",
                    queued.call.id
                )));
            }
            if queued.group_id.trim().is_empty() || !context.contains_group_id(&queued.group_id) {
                return Err(AgentError::new(
                    "运行检查点中的 queued Tool Call 未绑定完整 Assistant Turn 的交换分组。",
                ));
            }
            if queued.assistant_turn_id != assistant_turn_identity.assistant_turn_id {
                return Err(AgentError::new(
                    "运行检查点中的 queued Tool Call 引用了其他 Assistant Turn。",
                ));
            }
            let mapping = assistant_turn_identity
                .tool_call_identities
                .iter()
                .find(|mapping| mapping.provider_tool_index == queued.provider_tool_index)
                .ok_or_else(|| {
                    AgentError::new("运行检查点中的 queued Tool Call 缺少 Provider 身份映射。")
                })?;
            if mapping.runtime_call_id != queued.call.id
                || queued.call.provider_identity.as_ref() != Some(mapping)
            {
                return Err(AgentError::new(
                    "运行检查点中的 queued Tool Call 与 Provider 身份映射不一致。",
                ));
            }
            let call = LlmToolCall {
                id: queued.call.id,
                name: queued.call.name,
                args: queued.call.args,
            };
            Ok(QueuedToolCall {
                provider_call: LlmToolCall {
                    id: mapping.provider_call_id.clone(),
                    name: call.name.clone(),
                    // Raw Provider arguments are intentionally not durable in Round 1. The
                    // checkpoint projection is sufficient for identity validation and execution;
                    // a later private continuation store restores raw replay state.
                    args: call.args.clone(),
                },
                provider_tool_index: usize::try_from(mapping.provider_tool_index)
                    .unwrap_or(usize::MAX),
                checkpoint_call: call.clone(),
                call,
                checkpoint_persistence: AgentToolCallCheckpointPersistence::Allowed,
                assistant_content: queued.assistant_content,
                group_id: queued.group_id,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{
        ContextCapacityDetector, ContextItem, ContextMetadata, ContextRetention, ContextScope,
        ContextSource,
    };
    use crate::llm::{model_response_tool_call_id, LlmMessage, LlmMessageRole};
    use crate::protocol::{AgentApiStyle, AgentApprovalStatus, AgentToolCall, AgentToolResult};
    use crate::tools::ToolRegistry;
    use serde_json::json;

    fn test_tool_set() -> EffectiveToolSet {
        let registry = ToolRegistry::defaults_with_search(None);
        registry
            .effective_tool_set(registry.definitions(), &std::collections::BTreeSet::new())
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
            None,
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

    #[test]
    fn approval_checkpoint_rejects_private_queued_tool_arguments_without_leaking_them() {
        let secret = "fixture-token-that-must-not-persist";
        let call_id = canonical_test_call_id(1, "provider-private-mcp");
        let provider_call = LlmToolCall {
            id: "provider-private-mcp".to_string(),
            name: "mcp__fixture__credential_tool".to_string(),
            args: json!({"token": secret, "query": "safe"}),
        };
        let queued = QueuedToolCall {
            provider_call,
            provider_tool_index: 1,
            call: LlmToolCall {
                id: call_id.clone(),
                name: "mcp__fixture__credential_tool".to_string(),
                args: json!({"token": secret, "query": "safe"}),
            },
            checkpoint_call: LlmToolCall {
                id: call_id,
                name: "mcp__fixture__credential_tool".to_string(),
                args: json!({"token": "[redacted MCP argument]", "query": "safe"}),
            },
            checkpoint_persistence: AgentToolCallCheckpointPersistence::DeniedMcp,
            assistant_content: String::new(),
            group_id: "run:checkpoint-validation-run:tool-exchange:1:2".to_string(),
        };

        let error = queued_tool_call_checkpoint(&queued, "assistant-turn-test").unwrap_err();
        assert_eq!(
            error.code(),
            Some("agent.checkpoint_private_tool_arguments")
        );
        assert!(!error.to_string().contains(secret));
        assert!(!error
            .details()
            .is_some_and(|details| details.to_string().contains(secret)));
    }

    #[test]
    fn approval_checkpoint_rejects_all_mcp_calls_even_when_projection_cannot_detect_secret() {
        let secret = "Bearer fixture-neutral-field-secret";
        let call_id = canonical_test_call_id(1, "provider-neutral-mcp");
        let call = LlmToolCall {
            id: call_id,
            name: "provider_visible_name_without_routing_semantics".to_string(),
            args: json!({"text": secret}),
        };
        let queued = QueuedToolCall {
            provider_call: LlmToolCall {
                id: "provider-neutral-mcp".to_string(),
                name: call.name.clone(),
                args: call.args.clone(),
            },
            provider_tool_index: 1,
            checkpoint_call: call.clone(),
            call,
            checkpoint_persistence: AgentToolCallCheckpointPersistence::DeniedMcp,
            assistant_content: String::new(),
            group_id: "run:checkpoint-validation-run:tool-exchange:1:2".to_string(),
        };

        let error = queued_tool_call_checkpoint(&queued, "assistant-turn-test").unwrap_err();
        assert_eq!(
            error.code(),
            Some("agent.checkpoint_private_tool_arguments")
        );
        assert!(!error.to_string().contains(secret));
        assert!(!error
            .details()
            .is_some_and(|details| details.to_string().contains(secret)));
    }

    #[test]
    fn mcp_approval_barrier_drops_only_external_calls_and_persists_a_safe_reprepare_count() {
        let secret = "queued-mcp-secret-must-not-persist";
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "provider-pending"),
            name: "write_file".to_string(),
            args: json!({"path": "report.txt"}),
        };
        let mut batch = ToolCallBatch::from_model_response(
            "checkpoint-validation-run",
            0,
            String::new(),
            vec![
                pending.clone(),
                LlmToolCall {
                    id: canonical_test_call_id(1, "provider-mcp-one"),
                    name: "mcp__fixture__first".to_string(),
                    args: json!({"value": secret}),
                },
                LlmToolCall {
                    id: canonical_test_call_id(2, "provider-builtin"),
                    name: "read_file".to_string(),
                    args: json!({"path": "report.txt"}),
                },
                LlmToolCall {
                    id: canonical_test_call_id(3, "provider-mcp-two"),
                    name: "mcp__fixture__second".to_string(),
                    args: json!({"other": secret}),
                },
            ],
            false,
            |call| {
                (
                    LlmToolCall {
                        id: call.id.clone(),
                        name: call.name.clone(),
                        args: if call.name.starts_with("mcp__") {
                            json!({})
                        } else {
                            call.args.clone()
                        },
                    },
                    if call.name.starts_with("mcp__") {
                        AgentToolCallCheckpointPersistence::DeniedMcp
                    } else {
                        AgentToolCallCheckpointPersistence::Allowed
                    },
                )
            },
        );
        let checkpoint_message = batch.checkpoint_assistant_message().unwrap().unwrap();
        let group = batch.context_group().unwrap();
        let complete_turn = batch.take_assistant_turn().unwrap();
        pop_test_call(&mut batch, &pending.id);
        let mut context = ContextFrame::new(vec![ContextItem::new(
            LlmMessage::from_assistant_turn(complete_turn),
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        )
        .with_checkpoint_message(checkpoint_message)]);

        let deferred = batch.defer_external_calls(|queued| queued.call.name.starts_with("mcp__"));
        assert_eq!(deferred.len(), 2);
        let deferred_ids = deferred
            .into_iter()
            .map(|queued| queued.call.id)
            .collect::<BTreeSet<_>>();
        context
            .omit_runtime_tool_calls_from_group(&group, &deferred_ids)
            .unwrap();
        assert_eq!(batch.queue.len(), 1);
        assert_eq!(batch.queue[0].call.name, "read_file");
        assert_eq!(batch.take_deferred_external_tool_call_count(), None);

        let trace = ConversationTraceRecorder::default();
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
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap();

        assert_eq!(checkpoint.queued_tool_calls.len(), 1);
        assert_eq!(checkpoint.deferred_external_tool_call_count, 2);
        let rendered = serde_json::to_string(&checkpoint).unwrap();
        assert!(!rendered.contains(secret));
        assert!(!rendered.contains("mcp__fixture__first"));
        assert!(!rendered.contains("mcp__fixture__second"));

        batch.pop_front();
        assert_eq!(batch.take_deferred_external_tool_call_count(), Some(2));
        assert_eq!(batch.take_deferred_external_tool_call_count(), None);
    }

    fn restorable_checkpoint_fixture_for_pending_tool(
        pending_tool_name: &str,
    ) -> (AgentRunCheckpoint, AgentToolContinuation) {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "provider-pending"),
            name: pending_tool_name.to_string(),
            args: json!({ "path": "report.txt" }),
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
        let trace = ConversationTraceRecorder::default();
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
        restorable_checkpoint_fixture_for_pending_tool("write_file")
    }

    #[test]
    fn mcp_continuation_keeps_live_model_result_but_redacts_durable_trace_and_checkpoint() {
        const RESULT_CANARY: &str = "MCP_RESULT_CANARY_MUST_NOT_PERSIST";
        let model_tool_name = "mcp__fixture__secret_result".to_string();
        let (mut checkpoint, mut continuation) =
            restorable_checkpoint_fixture_for_pending_tool(&model_tool_name);
        checkpoint.pending_action_id = Some(uuid::Uuid::new_v4().to_string());
        checkpoint
            .tool_set
            .exposed_tool_names
            .push(model_tool_name.clone());
        checkpoint.tool_set.exposed_tool_names.sort();
        checkpoint.tool_set.exposed_tool_names.dedup();
        continuation.result.result = Some(json!({
            "value": RESULT_CANARY,
            "neutral": {"data": RESULT_CANARY},
        }));

        let restored =
            restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
        let live_messages = restored.context.to_messages();
        assert!(
            live_messages
                .iter()
                .any(|message| message.content().contains(RESULT_CANARY)),
            "the current process must still supply the bounded authoritative result to the model"
        );
        let mut live_context_items = restored.context.checkpoint_items().unwrap();

        let (trace_items, model_items, _, _) = restored.conversation_trace.checkpoint();
        assert!(!serde_json::to_string(&trace_items)
            .unwrap()
            .contains(RESULT_CANARY));
        assert!(!serde_json::to_string(&model_items)
            .unwrap()
            .contains(RESULT_CANARY));

        project_mcp_result_context_for_checkpoint(&mut live_context_items);
        let durable_context = serde_json::to_string(&live_context_items).unwrap();
        assert!(!durable_context.contains(RESULT_CANARY));
        assert!(!durable_context.contains(MCP_DURABLE_RESULT_PLACEHOLDER));
        assert!(durable_context.contains(r#"\"status\":\"completed\""#));
        assert!(durable_context.contains(r#"\"outcome\":\"succeeded\""#));
        assert!(durable_context.contains(r#"\"contentOmitted\":true"#));
    }

    fn assert_invalid_tool_call_id(error: AgentError) {
        assert_eq!(error.code(), Some("agent.invalid_model_tool_call_id"));
    }

    fn restore_error(result: AgentResult<RestoredRunCheckpoint>) -> AgentError {
        result.err().expect("checkpoint restore should fail")
    }

    #[test]
    fn checkpoint_v2_is_rejected_without_attempting_identity_migration() {
        let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
        checkpoint.version = 2;
        checkpoint.pending_tool_call_id = "legacy/provider/call".to_string();

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert!(error.to_string().contains("不支持版本 2"));
        assert!(error
            .to_string()
            .contains(&format!("当前版本为 {AGENT_RUN_CHECKPOINT_SCHEMA_VERSION}")));
    }

    #[test]
    fn checkpoint_restore_rejects_oversized_raw_provider_call_identity() {
        let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
        checkpoint.assistant_turn_identity.tool_call_identities[0].provider_call_id =
            "x".repeat(crate::llm::MAX_PROVIDER_TOOL_CALL_ID_BYTES + 1);

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert_eq!(error.code(), Some("agent.invalid_provider_tool_call_id"));
    }

    #[test]
    fn approval_restore_preserves_frozen_run_authority_and_capabilities() {
        let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
        let frozen_context = AgentRunContext {
            conversation_id: Some("conversation-frozen".to_string()),
            project_id: Some("project-frozen".to_string()),
            workspace: Some(crate::protocol::AgentWorkspaceContext {
                project_id: Some("project-frozen".to_string()),
                display_name: Some("Frozen workspace".to_string()),
                root_path: Some("/frozen/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: crate::protocol::AgentReadPermission::All,
                write: crate::protocol::AgentWritePermission::All,
                ..crate::protocol::AgentPermissions::default()
            },
        };
        checkpoint.run_context = Some(frozen_context.clone());
        checkpoint.model_capabilities = ModelCapabilities { image_input: true };
        checkpoint.run_world_state = test_run_world_state_for(true);

        let restored =
            restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();

        assert_eq!(restored.run_context, Some(frozen_context));
        assert_eq!(
            restored.model_capabilities,
            ModelCapabilities { image_input: true }
        );
        assert_eq!(
            restored
                .run_world_state
                .section(&WorldStateSectionId::ModelCapabilities)
                .unwrap()
                .state["imageInput"],
            true
        );
    }

    #[test]
    fn approval_restore_keeps_backend_history_metadata_out_of_the_model_result() {
        let (checkpoint, continuation) = restorable_checkpoint_fixture();
        let restored = restore_run_checkpoint_with_history_ref(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
            Some("assistant-approval"),
        )
        .unwrap();
        let messages = restored.context.to_messages();
        let observation = messages.last().unwrap().content();

        assert!(!observation.contains("historyRef"));
        assert!(!observation.contains("assistantMessageId"));
        assert!(!observation.contains("\"callId\""));
    }

    #[test]
    fn approval_restore_rejects_capability_and_world_state_mismatch() {
        let (mut checkpoint, continuation) = restorable_checkpoint_fixture();
        checkpoint.model_capabilities = ModelCapabilities { image_input: true };

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert!(error.to_string().contains("模型能力"));
        assert!(error.to_string().contains("不一致"));
    }

    #[test]
    fn one_model_response_claims_reason_only_duplicates_once() {
        let mut batch = ToolCallBatch::from_model_response(
            "claim-run",
            0,
            String::new(),
            vec![
                LlmToolCall {
                    id: canonical_test_call_id(0, "claim-first"),
                    name: "write_file".to_string(),
                    args: json!({
                        "filePath": "report.txt",
                        "content": "same",
                        "reason": "Create the report"
                    }),
                },
                LlmToolCall {
                    id: canonical_test_call_id(1, "claim-duplicate"),
                    name: "write_file".to_string(),
                    args: json!({
                        "reason": "Write the requested file",
                        "content": "same",
                        "filePath": "report.txt"
                    }),
                },
            ],
            false,
            |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
        );
        let first = batch.pop_front().unwrap();
        let duplicate = batch.pop_front().unwrap();

        assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
        assert!(matches!(
            batch.claim(&duplicate.call),
            ToolCallBatchClaim::Duplicate { .. }
        ));
    }

    #[test]
    fn approval_restore_reconstructs_all_seen_calls_from_the_same_model_response() {
        let first = LlmToolCall {
            id: canonical_test_call_id(0, "restore-first"),
            name: "read_file".to_string(),
            args: json!({ "path": "source.txt", "reason": "Read source" }),
        };
        let pending = LlmToolCall {
            id: canonical_test_call_id(1, "restore-pending"),
            name: "write_file".to_string(),
            args: json!({
                "filePath": "report.txt",
                "content": "draft",
                "reason": "Write report"
            }),
        };
        let duplicate_first = LlmToolCall {
            id: canonical_test_call_id(2, "restore-duplicate"),
            name: "read_file".to_string(),
            args: json!({ "reason": "Read it again", "path": "source.txt" }),
        };
        let mut batch = ToolCallBatch::from_model_response(
            "checkpoint-validation-run",
            0,
            String::new(),
            vec![first.clone(), pending.clone(), duplicate_first],
            false,
            |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
        );
        let first = batch.pop_front().unwrap();
        assert_eq!(batch.claim(&first.call), ToolCallBatchClaim::Execute);
        let pending_queued = batch.pop_front().unwrap();
        assert_eq!(
            batch.claim(&pending_queued.call),
            ToolCallBatchClaim::Execute
        );

        let batch_group = first.context_group();
        assert_eq!(batch_group, pending_queued.context_group());
        let complete_turn = batch
            .take_assistant_turn()
            .expect("new model response owns one complete assistant turn");
        let context = ContextFrame::new(vec![
            ContextItem::new(
                LlmMessage::from_assistant_turn(complete_turn),
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(batch_group.clone()),
            ),
            ContextItem::tool_result(
                first.call.id.clone(),
                "{}",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(batch_group),
            ),
        ]);
        let checkpoint = create_run_checkpoint(
            "checkpoint-validation-run",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.id,
                conversation_trace: &ConversationTraceRecorder::default(),
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap();
        assert_eq!(
            checkpoint
                .assistant_turn_identity
                .tool_call_identities
                .len(),
            3
        );
        assert_eq!(checkpoint.context_items[0].tool_calls.len(), 3);
        assert_eq!(
            checkpoint.context_items[1].tool_call_id.as_deref(),
            Some(first.call.id.as_str())
        );
        assert_eq!(checkpoint.queued_tool_calls.len(), 1);
        assert_eq!(
            checkpoint.queued_tool_calls[0].call.id,
            checkpoint.assistant_turn_identity.tool_call_identities[2].runtime_call_id
        );
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
                result: Some(json!({ "status": "written" })),
                error: None,
            },
        };

        let mut restored =
            restore_run_checkpoint(checkpoint, "checkpoint-validation-run", &continuation).unwrap();
        let queued_duplicate = restored.tool_batch.pop_front().unwrap();
        assert!(matches!(
            restored.tool_batch.claim(&queued_duplicate.call),
            ToolCallBatchClaim::Duplicate { .. }
        ));
    }

    #[test]
    fn approval_checkpoint_uses_provider_index_when_provider_call_ids_repeat() {
        let provider_calls = vec![
            LlmToolCall {
                id: "provider-reused-id".to_string(),
                name: "write_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
            LlmToolCall {
                id: "provider-reused-id".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
        ];
        let runtime_calls = [
            LlmToolCall {
                id: canonical_test_call_id(0, "provider-reused-id"),
                name: "write_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
            LlmToolCall {
                id: canonical_test_call_id(1, "provider-reused-id"),
                name: "read_file".to_string(),
                args: json!({ "path": "report.txt" }),
            },
        ];
        let bindings = provider_calls
            .iter()
            .zip(runtime_calls.iter().cloned())
            .enumerate()
            .map(|(index, (provider_call, runtime_call))| {
                LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
            })
            .collect::<Vec<_>>();
        let turn = LlmAssistantTurn::from_legacy("", provider_calls);
        let mut batch = ToolCallBatch::from_provider_response(
            "checkpoint-validation-run",
            0,
            turn,
            bindings,
            false,
            |call| (call.clone(), AgentToolCallCheckpointPersistence::Allowed),
        )
        .unwrap();
        let checkpoint_message = batch.checkpoint_assistant_message().unwrap().unwrap();
        let group = batch.context_group().unwrap();
        let complete_turn = batch.take_assistant_turn().unwrap();
        let pending = pop_test_call(&mut batch, &runtime_calls[0].id);
        let context = ContextFrame::new(vec![ContextItem::new(
            LlmMessage::from_assistant_turn(complete_turn),
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        )
        .with_checkpoint_message(checkpoint_message)]);
        let checkpoint = create_run_checkpoint(
            "checkpoint-validation-run",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.call.id,
                conversation_trace: &ConversationTraceRecorder::default(),
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap();
        assert_eq!(
            checkpoint
                .assistant_turn_identity
                .tool_call_identities
                .iter()
                .map(|identity| identity.provider_call_id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider-reused-id", "provider-reused-id"]
        );
        assert_ne!(
            checkpoint.assistant_turn_identity.tool_call_identities[0].runtime_call_id,
            checkpoint.assistant_turn_identity.tool_call_identities[1].runtime_call_id
        );
    }

    #[test]
    fn checkpoint_creation_rejects_invalid_pending_queued_context_and_trace_ids() {
        let invalid_id = "legacy/provider/call".to_string();
        let pending = LlmToolCall {
            id: invalid_id.clone(),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        };
        let (mut batch, assistant_item) =
            test_batch_and_context_item("create-invalid-pending", "", vec![pending.clone()], false);
        pop_test_call(&mut batch, &pending.id);
        let context = ContextFrame::new(vec![assistant_item]);
        let trace = ConversationTraceRecorder::default();
        let error = create_run_checkpoint(
            "create-invalid-pending",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let valid_pending = LlmToolCall {
            id: canonical_test_call_id(0, "create-valid-pending"),
            name: "write_file".to_string(),
            args: json!({ "path": "report.txt" }),
        };
        let (mut invalid_queue, valid_pending_item) = test_batch_and_context_item(
            "create-invalid-queue",
            "",
            vec![
                valid_pending.clone(),
                LlmToolCall {
                    id: invalid_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "report.txt" }),
                },
            ],
            false,
        );
        pop_test_call(&mut invalid_queue, &valid_pending.id);
        let valid_pending_context = ContextFrame::new(vec![valid_pending_item]);
        let error = create_run_checkpoint(
            "create-invalid-queue",
            RunCheckpointState {
                context: &valid_pending_context,
                next_model_request_index: 1,
                tool_batch: &invalid_queue,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let (mut valid_batch, valid_pending_item) = test_batch_and_context_item(
            "create-valid-pending",
            "",
            vec![valid_pending.clone()],
            false,
        );
        pop_test_call(&mut valid_batch, &valid_pending.id);
        let valid_pending_context = ContextFrame::new(vec![valid_pending_item.clone()]);

        let historical_group = ContextGroup::tool_exchange("invalid-history");
        let invalid_context = ContextFrame::new(vec![
            ContextItem::assistant(
                "",
                vec![LlmToolCall {
                    id: invalid_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "old.txt" }),
                }],
                ContextMetadata::new(
                    ContextSource::ModelResponse,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(historical_group.clone()),
            ),
            ContextItem::tool_result(
                invalid_id.clone(),
                "{}",
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(historical_group),
            ),
            valid_pending_item,
        ]);
        let error = create_run_checkpoint(
            "create-invalid-context",
            RunCheckpointState {
                context: &invalid_context,
                next_model_request_index: 1,
                tool_batch: &valid_batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);

        let mut invalid_trace = ConversationTraceRecorder::default();
        invalid_trace.record_tool_call(&AgentToolCall {
            id: invalid_id,
            tool: "read_file".to_string(),
            args: json!({ "path": "old.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        });
        let error = create_run_checkpoint(
            "create-invalid-trace",
            RunCheckpointState {
                context: &valid_pending_context,
                next_model_request_index: 1,
                tool_batch: &valid_batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &valid_pending.id,
                conversation_trace: &invalid_trace,
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap_err();
        assert_invalid_tool_call_id(error);
    }

    #[test]
    fn checkpoint_restore_validates_every_persisted_and_continuation_id() {
        let (checkpoint, continuation) = restorable_checkpoint_fixture();

        let mut invalid_pending = checkpoint.clone();
        invalid_pending.pending_tool_call_id = "legacy/provider/pending".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_pending,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_context = checkpoint.clone();
        invalid_context.context_items[0].tool_calls[0].id = "legacy/provider/context".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_context,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_queue = checkpoint.clone();
        invalid_queue.queued_tool_calls[0].call.id = "legacy/provider/queued".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_queue,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_trace = checkpoint.clone();
        invalid_trace
            .conversation_trace_items
            .push(ConversationTurnTraceItem::ToolCall {
                sequence: 0,
                call_id: "legacy/provider/trace".to_string(),
                tool: "read_file".to_string(),
                provenance: None,
                operation: json!({ "path": "old.txt" }),
                approval_status: AgentApprovalStatus::NotRequired,
                truncated: false,
            });
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            invalid_trace,
            "checkpoint-validation-run",
            &continuation,
        )));

        let mut invalid_continuation_call = continuation.clone();
        invalid_continuation_call.call.id = "legacy/provider/continuation".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &invalid_continuation_call,
        )));

        let mut invalid_continuation_result = continuation.clone();
        invalid_continuation_result.result.call_id = "legacy/provider/result".to_string();
        assert_invalid_tool_call_id(restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &invalid_continuation_result,
        )));
    }

    #[test]
    fn checkpoint_restore_rejects_a_valid_but_mismatched_continuation_result_id() {
        let (checkpoint, mut continuation) = restorable_checkpoint_fixture();
        continuation.result.call_id = canonical_test_call_id(9, "different-result");

        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &continuation,
        ));

        assert!(error.to_string().contains("续跑调用"));
        assert!(error.to_string().contains("工具结果"));
        assert!(error.to_string().contains("不一致"));
    }

    #[test]
    fn checkpoint_restore_rejects_changed_call_tool_args_and_result_tool() {
        let (checkpoint, continuation) = restorable_checkpoint_fixture();

        let mut changed_tool = continuation.clone();
        changed_tool.call.tool = "run_command".to_string();
        changed_tool.result.tool = "run_command".to_string();
        let error = restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &changed_tool,
        ));
        assert!(error.to_string().contains("工具或参数"));

        let mut changed_args = continuation.clone();
        changed_args.call.args = json!({ "path": "different.txt" });
        let error = restore_error(restore_run_checkpoint(
            checkpoint.clone(),
            "checkpoint-validation-run",
            &changed_args,
        ));
        assert!(error.to_string().contains("工具或参数"));

        let mut changed_result_tool = continuation;
        changed_result_tool.result.tool = "run_command".to_string();
        let error = restore_error(restore_run_checkpoint(
            checkpoint,
            "checkpoint-validation-run",
            &changed_result_tool,
        ));
        assert!(error.to_string().contains("续跑调用工具"));
        assert!(error.to_string().contains("工具结果"));
    }

    #[test]
    fn checkpoint_round_trip_closes_pending_exchange_and_restores_queue() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "round-trip-pending"),
            name: "write_file".to_string(),
            args: json!({ "phase": "finish" }),
        };
        let queued_call_id = canonical_test_call_id(1, "round-trip-queued");
        let (mut batch, assistant_item) = test_batch_and_context_item(
            "run-1",
            "",
            vec![
                pending.clone(),
                LlmToolCall {
                    id: queued_call_id.clone(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "report.txt" }),
                },
            ],
            true,
        );
        pop_test_call(&mut batch, &pending.id);
        let context = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            assistant_item,
        ]);
        let trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-1",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
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
                tool: "write_file".to_string(),
                args: json!({ "phase": "finish" }),
                approval_status: AgentApprovalStatus::Approved,
                reason: None,
            },
            result: AgentToolResult {
                exact_archive_file: None,
                call_id: pending.id,
                tool: "write_file".to_string(),
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };

        let mut restored = restore_run_checkpoint(checkpoint, "run-1", &continuation).unwrap();

        restored
            .context
            .validate_pending_tool_batch(&queued_call_id, &[])
            .unwrap();
        assert_eq!(restored.next_model_request_index, 1);
        assert!(!restored.tool_batch.take_suppressed_narration());
        let queued = restored.tool_batch.pop_front().unwrap();
        assert_eq!(queued.call.id, queued_call_id);
        assert!(restored.tool_batch.take_suppressed_narration());
    }

    #[test]
    fn approval_resume_preserves_failed_command_observation_for_the_model() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "failed-command-pending"),
            name: "run_command".to_string(),
            args: json!({ "command": "python3 -c 'import openpyxl'" }),
        };
        let (mut batch, assistant_item) =
            test_batch_and_context_item("run-command", "", vec![pending.clone()], false);
        pop_test_call(&mut batch, &pending.id);
        let context = ContextFrame::new(vec![assistant_item]);
        let trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-command",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
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
                ok: false,
                result: Some(json!({
                    "exitCode": 1,
                    "stdout": "dependency check started",
                    "stderr": "ModuleNotFoundError: No module named 'openpyxl'",
                    "timedOut": false,
                    "cancelled": false,
                    "stdoutTruncated": false,
                    "stderrTruncated": false,
                })),
                error: Some("命令执行失败。".to_string()),
            },
        };

        let restored = restore_run_checkpoint(checkpoint, "run-command", &continuation).unwrap();

        restored.context.validate_complete_tool_protocol().unwrap();
        let messages = restored.context.to_messages();
        let observation = messages.last().expect("restored tool observation");
        assert_eq!(observation.role(), LlmMessageRole::Tool);
        assert!(observation.is_error());
        assert!(observation.content().contains("\"exitCode\":1"));
        assert!(observation.content().contains("dependency check started"));
        assert!(observation.content().contains("ModuleNotFoundError"));
        assert!(!observation.content().contains("\"stdoutTruncated\""));
        assert!(!observation.content().contains("\"stderrTruncated\""));
    }

    #[test]
    fn approval_resume_preserves_compacted_context_without_restoring_raw_history() {
        let pending = LlmToolCall {
            id: canonical_test_call_id(0, "compaction-pending"),
            name: "write_file".to_string(),
            args: json!({ "phase": "finish", "path": "report.txt" }),
        };
        let (mut tool_batch, assistant_item) = test_batch_and_context_item(
            "run-compacted",
            "I will write the report.",
            vec![pending.clone()],
            false,
        );
        pop_test_call(&mut tool_batch, &pending.id);
        let active = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "RAW_HISTORY_MUST_NOT_RETURN",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "old answer",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "current request",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            assistant_item,
        ]);
        let mut compacted = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "system rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::Assistant,
                "COMPACTED_HISTORY_SURVIVES_RESUME",
                ContextSource::ConversationSummary,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
            ContextItem::text(
                LlmMessageRole::User,
                "current request",
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        ]);
        let detector =
            ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
        detector.prepare_frame(&mut compacted);
        let compacted_baseline = compacted.share_measured_persistent_baseline().unwrap();
        let active = active.replace_persistent_baseline(compacted_baseline);
        let conversation_trace = ConversationTraceRecorder::default();
        let checkpoint = create_run_checkpoint(
            "run-compacted",
            RunCheckpointState {
                context: &active,
                next_model_request_index: 2,
                tool_batch: &tool_batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending.id,
                conversation_trace: &conversation_trace,
                tool_set: &test_tool_set(),
                run_context: None,
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

        let restored = restore_run_checkpoint(checkpoint, "run-compacted", &continuation).unwrap();
        restored.context.validate_complete_tool_protocol().unwrap();
        let combined_content = restored
            .context
            .to_messages()
            .into_iter()
            .map(|message| message.content().to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(combined_content.contains("COMPACTED_HISTORY_SURVIVES_RESUME"));
        assert!(combined_content.contains("current request"));
        assert!(combined_content.contains("applied"));
        assert!(!combined_content.contains("RAW_HISTORY_MUST_NOT_RETURN"));
    }

    #[test]
    fn approval_resume_preserves_the_complete_committed_trace() {
        let completed_call = AgentToolCall {
            id: canonical_test_call_id(0, "completed-trace"),
            tool: "read_file".to_string(),
            args: json!({ "path": "notes.txt" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        };
        let completed_result = AgentToolResult {
            exact_archive_file: None,
            call_id: completed_call.id.clone(),
            tool: completed_call.tool.clone(),
            ok: true,
            result: Some(json!({ "content": "completed result" })),
            error: None,
        };
        let pending_call = AgentToolCall {
            id: canonical_test_call_id(1, "pending-trace"),
            tool: "write_file".to_string(),
            args: json!({ "phase": "finish" }),
            approval_status: AgentApprovalStatus::Required,
            reason: None,
        };
        let completed_llm_call = LlmToolCall {
            id: completed_call.id.clone(),
            name: completed_call.tool.clone(),
            args: completed_call.args.clone(),
        };
        let pending_llm_call = LlmToolCall {
            id: pending_call.id.clone(),
            name: pending_call.tool.clone(),
            args: pending_call.args.clone(),
        };
        let (mut tool_batch, assistant_item) = test_batch_and_context_item(
            "run-multi-tool",
            "",
            vec![completed_llm_call, pending_llm_call],
            false,
        );
        let completed_queued = pop_test_call(&mut tool_batch, &completed_call.id);
        let pending_queued = pop_test_call(&mut tool_batch, &pending_call.id);
        assert_eq!(
            completed_queued.context_group(),
            pending_queued.context_group()
        );
        let group = completed_queued.context_group();
        let context = ContextFrame::new(vec![
            ContextItem::text(
                LlmMessageRole::System,
                "rules",
                ContextSource::BackendSystemPrompt,
                ContextScope::Run,
                ContextRetention::Retained,
            ),
            assistant_item,
            ContextItem::tool_result(
                completed_call.id.clone(),
                build_tool_observation_message(&completed_result),
                false,
                ContextMetadata::new(
                    ContextSource::ToolResult,
                    ContextScope::Run,
                    ContextRetention::Retained,
                )
                .with_group(group),
            ),
        ]);
        let mut trace = ConversationTraceRecorder::default();
        trace.record_tool_call(&completed_call);
        trace.record_tool_result(&completed_call, &completed_result);
        trace.record_tool_call(&pending_call);
        let checkpoint = create_run_checkpoint(
            "run-multi-tool",
            RunCheckpointState {
                context: &context,
                next_model_request_index: 1,
                tool_batch: &tool_batch,
                extension_snapshots: Vec::new(),
                pending_tool_call_id: &pending_call.id,
                conversation_trace: &trace,
                tool_set: &test_tool_set(),
                run_context: None,
                model_capabilities: ModelCapabilities::default(),
                run_world_state: &test_run_world_state(),
                provider_profile_config: &test_provider_profile(),
                provider_protocol_key: &test_provider_key(),
            },
        )
        .unwrap();
        let pending_call_id = pending_call.id.clone();
        let continuation = AgentToolContinuation {
            call: AgentToolCall {
                approval_status: AgentApprovalStatus::Approved,
                ..pending_call
            },
            result: AgentToolResult {
                exact_archive_file: None,
                call_id: pending_call_id,
                tool: "write_file".to_string(),
                ok: true,
                result: Some(json!({ "status": "applied" })),
                error: None,
            },
        };

        let restored = restore_run_checkpoint(checkpoint, "run-multi-tool", &continuation).unwrap();

        assert_eq!(restored.conversation_trace.committed_item_count(), 4);
    }
}
