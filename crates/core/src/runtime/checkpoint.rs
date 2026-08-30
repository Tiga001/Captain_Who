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
use crate::file_change::{
    FileChangePathPolicy, FileObservationCheckpoint, FileObservationRegistry, FileObservationState,
};
use crate::llm::{
    validate_model_tool_call_id, validate_provider_tool_call_id, LlmAssistantTurn,
    LlmRuntimeToolCallBinding, LlmToolCall,
};
use crate::protocol::{
    AgentApprovalStatus, AgentAssistantTurnCheckpointIdentity, AgentContextCheckpointItem,
    AgentContextCheckpointToolCall, AgentError, AgentExtensionSnapshot,
    AgentProviderToolCallIdentity, AgentQueuedToolCallCheckpoint, AgentResult, AgentRunCheckpoint,
    AgentRunContext, AgentRunToolSetCheckpoint, AgentToolContinuation, AgentToolIdentity,
    AgentWritePermission, ModelCapabilities, AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
};
use crate::provider_profile::{ProviderProfileConfig, ProviderProtocolKey};
use crate::resolve_provider_runtime_capabilities;
use crate::tools::{
    validate_tool_set_checkpoint_shape, AgentToolCallCheckpointPersistence, EffectiveToolSet,
    ToolExecutionContext,
};
use crate::world_state::{
    WorldStateLifetime, WorldStateSectionId, WorldStateSnapshot, WorldStateVisibility,
};
use crate::AGENT_COLLABORATION_TOOL_NAMES;
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

    pub(super) fn provider_identity(&self) -> AgentResult<AgentProviderToolCallIdentity> {
        validate_provider_tool_call_id(&self.provider_call.id)?;
        validate_model_tool_call_id(&self.call.id)?;
        Ok(AgentProviderToolCallIdentity {
            provider_tool_index: u32::try_from(self.provider_tool_index)
                .map_err(|_| AgentError::new("Provider tool call index 超出 checkpoint 范围。"))?,
            provider_call_id: self.provider_call.id.clone(),
            runtime_call_id: self.call.id.clone(),
        })
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
    /// Observation ids already consumed by an earlier call in this exact Provider batch.
    /// A later sibling cannot use a renewal it could not yet have observed.
    seen_file_observation_ids: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ToolCallBatchClaim {
    Execute,
    Duplicate { semantic_fingerprint: String },
    FileObservationReused,
}

fn claimed_file_observation_id(call: &LlmToolCall) -> Option<&str> {
    claimed_file_observation_id_from_args(&call.name, &call.args)
}

fn claimed_file_observation_id_from_args<'a>(
    name: &str,
    args: &'a serde_json::Value,
) -> Option<&'a str> {
    if name != "apply_patch" || !crate::tools::apply_patch_wire_is_valid(args) {
        return None;
    }
    let request = crate::tools::apply_patch_request(args)?;
    matches!(
        request.get("action").and_then(serde_json::Value::as_str),
        Some("apply") | Some("begin")
    )
    .then_some(())?;
    request
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
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
        let assistant_turn = LlmAssistantTurn::from_split_projection(assistant_content, calls)
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
        .expect("split-projection test batch has matching provider/runtime calls")
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
            seen_file_observation_ids: BTreeSet::new(),
        })
    }

    pub(super) fn claim(&mut self, call: &LlmToolCall) -> ToolCallBatchClaim {
        let semantic_fingerprint = semantic_tool_call_fingerprint(&call.name, &call.args);
        if !self
            .seen_semantic_fingerprints
            .insert(semantic_fingerprint.clone())
        {
            return ToolCallBatchClaim::Duplicate {
                semantic_fingerprint,
            };
        }
        if claimed_file_observation_id(call).is_some_and(|observation_id| {
            !self
                .seen_file_observation_ids
                .insert(observation_id.to_string())
        }) {
            return ToolCallBatchClaim::FileObservationReused;
        }
        ToolCallBatchClaim::Execute
    }

    pub(super) fn pop_front(&mut self) -> Option<QueuedToolCall> {
        self.queue.pop_front()
    }

    pub(super) fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.queue.len()
    }

    /// Drops queued private calls that cannot be rehydrated without their process-only payload.
    ///
    /// Only a count survives the approval checkpoint. In particular, model-authored arguments,
    /// Tool names and model-authored arguments never enter this diagnostic channel. The field
    /// retaining the count keeps its historical name for checkpoint compatibility.
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

    pub(super) fn checkpoint_assistant_turn_id(&self) -> Option<&str> {
        self.assistant_turn_identity
            .as_ref()
            .map(|identity| identity.assistant_turn_id.as_str())
    }

    pub(super) fn take_assistant_turn(&mut self) -> Option<LlmAssistantTurn> {
        self.assistant_turn.take()
    }

    pub(super) fn assistant_turn(&self) -> Option<&LlmAssistantTurn> {
        self.assistant_turn.as_ref()
    }

    pub(super) fn attach_provider_continuation_ref(
        &mut self,
        continuation_ref: crate::protocol::ProviderContinuationRef,
    ) -> AgentResult<()> {
        let turn = self.assistant_turn.take().ok_or_else(|| {
            AgentError::new(
                "Tool Call 批次缺少可绑定 Provider continuation ref 的 Assistant Turn。",
            )
        })?;
        self.assistant_turn = Some(turn.with_provider_continuation_ref(continuation_ref)?);
        Ok(())
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

    /// Replaces approval-checkpoint placeholders with the authenticated runtime calls from the
    /// encrypted provider turn. No queued call may execute before this succeeds.
    pub(super) fn rehydrate_queued_calls_from_provider_turn(
        &mut self,
        turn: &LlmAssistantTurn,
        mut checkpoint_projection: impl FnMut(
            &LlmToolCall,
        )
            -> (LlmToolCall, AgentToolCallCheckpointPersistence),
    ) -> AgentResult<()> {
        if self.deferred_external_tool_call_count != 0 {
            return Err(AgentError::new(
                "Provider 审批检查点包含旧版延后调用，无法安全恢复完整 Provider Turn。",
            ));
        }
        let expected_identity = self.assistant_turn_identity()?.clone();
        let actual_identity = turn.checkpoint_identity()?;
        if actual_identity != expected_identity {
            return Err(AgentError::new(
                "加密 Provider Turn 与审批检查点的完整身份不一致。",
            ));
        }
        // `ContextFrame::restore_provider_assistant_turn` replaces the durable split projection
        // with this exact authenticated Provider Turn and unifies its existing Tool results under
        // the same semantic exchange group. Queued siblings must use that group too; retaining the
        // pre-hydration checkpoint group would make a correctly ordered resumed result fail the
        // complete Tool protocol at the next request boundary.
        let authenticated_group_id = format!("provider-turn:{}", turn.stable_id());
        let bindings = turn
            .runtime_tool_bindings()
            .ok_or_else(|| AgentError::new("加密 Provider Turn 缺少 Runtime Tool Call 映射。"))?;
        for queued in &mut self.queue {
            let binding = bindings
                .iter()
                .find(|binding| binding.runtime_call.id == queued.call.id)
                .ok_or_else(|| {
                    AgentError::new("加密 Provider Turn 缺少审批检查点中的 queued Tool Call。")
                })?;
            let provider_call = turn
                .provider_tool_calls()
                .get(binding.provider_tool_index)
                .ok_or_else(|| AgentError::new("加密 Provider Tool Call 映射越界。"))?;
            if provider_call.id != binding.provider_call_id
                || binding.provider_tool_index != queued.provider_tool_index
            {
                return Err(AgentError::new(
                    "加密 Provider Turn 的 queued Tool Call 身份不一致。",
                ));
            }
            let (checkpoint_call, checkpoint_persistence) =
                checkpoint_projection(&binding.runtime_call);
            if checkpoint_call.id != binding.runtime_call.id
                || checkpoint_call.name != binding.runtime_call.name
            {
                return Err(AgentError::new(
                    "Provider Tool Call 的安全 Checkpoint 投影改变了调用身份。",
                ));
            }
            queued.provider_call = provider_call.clone();
            queued.provider_tool_index = binding.provider_tool_index;
            queued.call = binding.runtime_call.clone();
            queued.checkpoint_call = checkpoint_call;
            queued.checkpoint_persistence = checkpoint_persistence;
            queued.group_id = authenticated_group_id.clone();
        }
        Ok(())
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
    pub(super) collaboration_run_snapshot: Option<crate::AgentCollaborationRunSnapshot>,
    pub(super) model_capabilities: ModelCapabilities,
    pub(super) run_world_state: WorldStateSnapshot,
    pub(super) provider_profile_config: ProviderProfileConfig,
    pub(super) provider_protocol_key: ProviderProtocolKey,
    pub(super) provider_continuation_refs: Vec<crate::protocol::ProviderContinuationRef>,
    pub(super) file_observations: Arc<FileObservationRegistry>,
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
    pub(super) collaboration_run_snapshot: Option<crate::AgentCollaborationRunSnapshot>,
    pub(super) model_capabilities: ModelCapabilities,
    pub(super) run_world_state: &'a WorldStateSnapshot,
    pub(super) provider_profile_config: &'a ProviderProfileConfig,
    pub(super) provider_protocol_key: &'a ProviderProtocolKey,
}

#[cfg(test)]
pub(super) fn create_run_checkpoint(
    run_id: &str,
    state: RunCheckpointState<'_>,
) -> AgentResult<AgentRunCheckpoint> {
    create_run_checkpoint_with_file_observations(
        run_id,
        state,
        &FileObservationRegistry::default(),
        None,
    )
}

pub(super) fn create_run_checkpoint_with_file_observations(
    run_id: &str,
    state: RunCheckpointState<'_>,
    file_observations: &FileObservationRegistry,
    pending_file_observation: Option<&FileObservationCheckpoint>,
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
        collaboration_run_snapshot,
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
    validate_checkpoint_provider_protocol_revision(provider_protocol_key)?;
    let provider_runtime_capabilities =
        resolve_provider_runtime_capabilities(provider_protocol_key).map_err(|error| {
            AgentError::new(format!("运行检查点的 Provider capabilities 无效：{error}"))
        })?;
    let assistant_turn_identity = tool_batch.assistant_turn_identity()?.clone();
    validate_assistant_turn_identity(&assistant_turn_identity, pending_tool_call_id, tool_batch)?;
    context.validate_assistant_turn_checkpoint_identity(
        &assistant_turn_identity,
        pending_tool_call_id,
        &queued_tool_call_ids,
        true,
    )?;
    // The checkpoint and the durable in-progress Trace are two views of the same frozen prefix.
    // Persist the canonical/redacted Trace projection here as well; retaining the private raw
    // ToolCall operation would make approval resume rewrite an already committed prefix.
    let trace_snapshot = conversation_trace.snapshot();
    let conversation_trace_items = trace_snapshot.items;
    let conversation_model_context_items = trace_snapshot.model_context_items;
    let next_conversation_trace_sequence = trace_snapshot.next_sequence;
    let conversation_trace_truncated = trace_snapshot.truncated;
    validate_conversation_trace_tool_call_ids(&conversation_trace_items)?;
    let mut context_items = context.checkpoint_items()?;
    project_mcp_result_context_for_checkpoint(&mut context_items);
    validate_context_checkpoint_tool_call_ids(&context_items)?;
    let pending_tool_call = context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_tool_call_id)
        .ok_or_else(|| AgentError::new("无法创建运行检查点：缺少待审批 Tool Call。"))?;
    validate_pending_file_observation(
        pending_tool_call,
        pending_file_observation,
        run_id,
        run_context,
        &context_items,
        "创建",
    )?;
    validate_checkpoint_world_state(run_world_state, model_capabilities)?;
    validate_collaboration_run_snapshot(
        tool_set
            .all_definitions()
            .iter()
            .map(|definition| definition.name.as_str()),
        collaboration_run_snapshot.as_ref(),
    )?;
    let provider_continuation_refs = context.provider_continuation_refs()?;
    let mut queued_tool_calls = tool_batch
        .queue
        .iter()
        .map(|queued| {
            queued_tool_call_checkpoint(
                queued,
                &assistant_turn_identity.assistant_turn_id,
                provider_runtime_capabilities.allows_encrypted_checkpoint_rehydration(),
            )
        })
        .collect::<AgentResult<Vec<_>>>()?;
    attach_queued_file_observations(
        &mut queued_tool_calls,
        file_observations,
        run_id,
        run_context,
        &context_items,
        pending_file_observation,
    )?;
    Ok(AgentRunCheckpoint {
        version: AGENT_RUN_CHECKPOINT_SCHEMA_VERSION,
        run_id: run_id.to_string(),
        context_items,
        next_model_request_index,
        queued_tool_calls,
        deferred_external_tool_call_count: tool_batch.deferred_external_tool_call_count,
        suppressed_narration: tool_batch.suppressed_narration,
        extension_snapshots,
        tool_set: tool_set.checkpoint(),
        run_context: run_context.cloned(),
        collaboration_run_snapshot,
        model_capabilities,
        provider_profile_config: provider_profile_config.clone(),
        provider_protocol_key: provider_protocol_key.clone(),
        assistant_turn_identity,
        provider_continuation_refs,
        run_world_state: run_world_state.clone(),
        pending_action_id: None,
        file_change_run_grant_ref: None,
        pending_file_observation: pending_file_observation.cloned(),
        pending_tool_call_id: pending_tool_call_id.to_string(),
        conversation_trace_items,
        conversation_model_context_items,
        next_conversation_trace_sequence,
        conversation_trace_truncated,
    })
}

fn attach_queued_file_observations(
    queued_calls: &mut [AgentQueuedToolCallCheckpoint],
    registry: &FileObservationRegistry,
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    pending_file_observation: Option<&FileObservationCheckpoint>,
) -> AgentResult<()> {
    let mut observation_ids = BTreeSet::new();
    let mut source_call_ids = BTreeSet::new();
    for queued in queued_calls {
        let Some((observation_id, canonical_target, conversation_id)) =
            queued_apply_patch_observation_request(&queued.call, run_context)?
        else {
            queued.file_observation = None;
            continue;
        };
        if !observation_ids.insert(observation_id.to_string()) {
            // The Runtime batch guard will reject this later call before execution. Keeping a
            // second authority copy would instead let approval restore bypass that guard.
            queued.file_observation = None;
            continue;
        }
        if pending_file_observation.is_some_and(|pending| pending.observation_id == observation_id)
        {
            queued.file_observation = None;
            continue;
        }
        let checkpoint = registry
            .checkpoint_exact(observation_id, conversation_id, run_id, &canonical_target)
            .map_err(|_| {
                AgentError::new("无法创建运行检查点：queued apply_patch 缺少当前且匹配的文件观察。")
            })?;
        if !source_call_ids.insert(checkpoint.source_tool_call_id.clone()) {
            return Err(AgentError::new(
                "无法创建运行检查点：多个文件观察重复绑定同一来源工具调用。",
            ));
        }
        validate_observation_source(context_items, &checkpoint, run_context, "创建")?;
        queued.file_observation = Some(checkpoint);
    }
    Ok(())
}

fn restore_queued_file_observations(
    queued_calls: &[AgentQueuedToolCallCheckpoint],
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    pending_file_observation: Option<&FileObservationCheckpoint>,
) -> AgentResult<Arc<FileObservationRegistry>> {
    let mut checkpoints = Vec::new();
    let mut observation_ids = pending_file_observation
        .map(|checkpoint| BTreeSet::from([checkpoint.observation_id.clone()]))
        .unwrap_or_default();
    let mut source_call_ids = BTreeSet::new();
    let mut expected_conversation_id = None;
    for queued in queued_calls {
        match (
            queued_apply_patch_observation_request(&queued.call, run_context)?,
            queued.file_observation.as_ref(),
        ) {
            (None, None) => {}
            (None, Some(_)) => {
                return Err(AgentError::new(
                    "无法恢复运行检查点：非 apply_patch 调用包含额外的文件观察。",
                ));
            }
            (Some((observation_id, _, _)), None) if observation_ids.contains(observation_id) => {}
            (Some(_), None) => {
                return Err(AgentError::new(
                    "无法恢复运行检查点：queued apply_patch 缺少文件观察。",
                ));
            }
            (Some((observation_id, canonical_target, conversation_id)), Some(checkpoint)) => {
                if checkpoint.observation_id != observation_id
                    || Path::new(&checkpoint.canonical_target) != canonical_target
                    || checkpoint.conversation_id != conversation_id
                    || checkpoint.run_id != run_id
                    || !observation_ids.insert(checkpoint.observation_id.clone())
                    || !source_call_ids.insert(checkpoint.source_tool_call_id.clone())
                {
                    return Err(AgentError::new(
                        "无法恢复运行检查点：queued apply_patch 的文件观察身份、路径或所有者不匹配。",
                    ));
                }
                match expected_conversation_id {
                    Some(expected) if expected != conversation_id => {
                        return Err(AgentError::new(
                            "无法恢复运行检查点：文件观察属于不同会话。",
                        ));
                    }
                    None => expected_conversation_id = Some(conversation_id),
                    _ => {}
                }
                validate_observation_source(context_items, checkpoint, run_context, "恢复")?;
                checkpoints.push(checkpoint.clone());
            }
        }
    }
    if checkpoints.is_empty() {
        return Ok(Arc::new(FileObservationRegistry::default()));
    }
    let conversation_id = expected_conversation_id.expect("non-empty checkpoints have an owner");
    FileObservationRegistry::from_checkpoints(checkpoints, conversation_id, run_id)
        .map(Arc::new)
        .map_err(|_| AgentError::new("无法恢复运行检查点：文件观察已过期或结构无效。"))
}

fn queued_apply_patch_observation_request<'a>(
    call: &'a AgentContextCheckpointToolCall,
    run_context: Option<&'a AgentRunContext>,
) -> AgentResult<Option<(&'a str, PathBuf, &'a str)>> {
    if call.name != "apply_patch" {
        return Ok(None);
    }
    if !crate::tools::apply_patch_wire_is_valid(&call.args) {
        return Err(AgentError::new(
            "运行检查点中的 queued apply_patch 参数不符合当前严格协议。",
        ));
    }
    let object = crate::tools::apply_patch_request(&call.args)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 参数不是严格对象。"))?;
    let action = object
        .get("action")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少当前 action。"))?;
    if !matches!(action, "apply" | "begin") {
        return Ok(None);
    }
    let operation = object
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 operation。"))?;
    if operation == "create" {
        return Ok(None);
    }
    let observation_id = object
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 observationId。"))?;
    let file_path = object
        .get("filePath")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AgentError::new("运行检查点中的 queued apply_patch 缺少 filePath。"))?;
    let run_context = run_context.ok_or_else(|| {
        AgentError::new("运行检查点中的 queued apply_patch 缺少冻结的运行上下文。")
    })?;
    let conversation_id = run_context
        .conversation_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| {
            AgentError::new("运行检查点中的 queued apply_patch 缺少 conversationId。")
        })?;
    let workspace_root = run_context
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.root_path.as_deref())
        .map(PathBuf::from);
    let target = FileChangePathPolicy::new(
        workspace_root.as_deref(),
        run_context.permissions.write == AgentWritePermission::All,
    )
    .resolve(file_path)
    .map_err(|_| AgentError::new("运行检查点中的 queued apply_patch 文件路径无法安全解析。"))?;
    Ok(Some((
        observation_id,
        target.absolute_path().to_path_buf(),
        conversation_id,
    )))
}

fn validate_pending_file_observation(
    pending_call: &AgentContextCheckpointToolCall,
    checkpoint: Option<&FileObservationCheckpoint>,
    run_id: &str,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
    operation: &str,
) -> AgentResult<()> {
    let requested = queued_apply_patch_observation_request(pending_call, run_context)?;
    let is_staged_commit = pending_call.name == "apply_patch"
        && crate::tools::apply_patch_request(&pending_call.args)
            .and_then(|request| request.get("action"))
            .and_then(serde_json::Value::as_str)
            == Some("commit");

    match (requested, checkpoint) {
        (None, None) => Ok(()),
        (Some(_), None) => Err(AgentError::new(format!(
            "无法{operation}运行检查点：待审批 FileChange 缺少冻结的文件观察。"
        ))),
        (None, Some(_)) if !is_staged_commit => Err(AgentError::new(format!(
            "无法{operation}运行检查点：当前待审批调用包含不允许的文件观察。"
        ))),
        (requested, Some(checkpoint)) => {
            let run_context = run_context.ok_or_else(|| {
                AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 FileChange 缺少运行上下文。"
                ))
            })?;
            let conversation_id = run_context
                .conversation_id
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    AgentError::new(format!(
                        "无法{operation}运行检查点：待审批 FileChange 缺少会话上下文。"
                    ))
                })?;
            if !matches!(checkpoint.state, FileObservationState::Existing { .. }) {
                return Err(AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 update/delete 的文件观察状态无效。"
                )));
            }
            let target = requested
                .as_ref()
                .map(|(_, target, _)| target.as_path())
                .unwrap_or_else(|| Path::new(&checkpoint.canonical_target));
            checkpoint
                .validate_frozen_binding(conversation_id, run_id, target)
                .map_err(|_| {
                    AgentError::new(format!(
                        "无法{operation}运行检查点：待审批 FileChange 的文件观察无效。"
                    ))
                })?;
            if requested
                .is_some_and(|(observation_id, _, _)| observation_id != checkpoint.observation_id)
            {
                return Err(AgentError::new(format!(
                    "无法{operation}运行检查点：待审批 FileChange 的 observationId 不一致。"
                )));
            }
            validate_observation_source(context_items, checkpoint, Some(run_context), operation)
        }
    }
}

fn validate_observation_source(
    context_items: &[AgentContextCheckpointItem],
    checkpoint: &FileObservationCheckpoint,
    run_context: Option<&AgentRunContext>,
    operation: &str,
) -> AgentResult<()> {
    validate_model_tool_call_id(&checkpoint.source_tool_call_id)?;
    let run_context = run_context.ok_or_else(|| {
        AgentError::new(format!(
            "无法{operation}运行检查点：文件观察缺少冻结的运行上下文。"
        ))
    })?;
    let matching_source_calls = context_items
        .iter()
        .filter(|item| item.role == "assistant")
        .flat_map(|item| item.tool_calls.iter())
        .filter(|call| {
            call.id == checkpoint.source_tool_call_id
                && matches!(call.name.as_str(), "read_file" | "apply_patch")
        })
        .collect::<Vec<_>>();
    let Some(source_call) = matching_source_calls.first().copied() else {
        return Err(AgentError::new(format!(
            "无法{operation}运行检查点：文件观察缺少唯一且已完成的来源工具调用。"
        )));
    };
    let source_call_shape_matches = match source_call.name.as_str() {
        "read_file" => source_call
            .args
            .as_object()
            .and_then(|args| args.get("path"))
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok())
            .is_some_and(|target| Path::new(&checkpoint.canonical_target) == target),
        "apply_patch" => crate::tools::apply_patch_wire_is_valid(&source_call.args),
        _ => false,
    };
    let matching_results = context_items
        .iter()
        .filter(|item| {
            item.role == "tool"
                && !item.is_error
                && item.tool_call_id.as_deref() == Some(&checkpoint.source_tool_call_id)
                && serde_json::from_str::<serde_json::Value>(&item.content)
                    .ok()
                    .is_some_and(|value| {
                        observation_result_matches_checkpoint(
                            &value,
                            checkpoint,
                            run_context,
                            source_call,
                            context_items,
                        )
                    })
        })
        .count();
    if matching_source_calls.len() != 1 || !source_call_shape_matches || matching_results != 1 {
        return Err(AgentError::new(format!(
            "无法{operation}运行检查点：文件观察没有绑定唯一、同路径且已完成的 read_file 或 apply_patch 调用。"
        )));
    }
    Ok(())
}

fn observation_result_matches_checkpoint(
    value: &serde_json::Value,
    checkpoint: &FileObservationCheckpoint,
    run_context: &AgentRunContext,
    source_call: &AgentContextCheckpointToolCall,
    context_items: &[AgentContextCheckpointItem],
) -> bool {
    if source_call.name == "apply_patch" {
        return apply_patch_observation_result_matches_checkpoint(
            value,
            checkpoint,
            run_context,
            source_call,
            context_items,
        );
    }
    if source_call.name != "read_file" {
        return false;
    }
    let Some(result) = value.as_object() else {
        return false;
    };
    if result
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(result_target) = result
        .get("path")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
        .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok())
    else {
        return false;
    };
    if result_target != Path::new(&checkpoint.canonical_target) {
        return false;
    }
    match &checkpoint.state {
        FileObservationState::Missing => {
            result.get("exists").and_then(serde_json::Value::as_bool) == Some(false)
                && !result.contains_key("revision")
        }
        FileObservationState::Existing { revision, .. } => {
            result.get("exists").and_then(serde_json::Value::as_bool) == Some(true)
                && result.get("revision").and_then(serde_json::Value::as_str) == Some(revision)
        }
    }
}

fn apply_patch_observation_result_matches_checkpoint(
    value: &serde_json::Value,
    checkpoint: &FileObservationCheckpoint,
    run_context: &AgentRunContext,
    source_call: &AgentContextCheckpointToolCall,
    context_items: &[AgentContextCheckpointItem],
) -> bool {
    let Some(result) = value.as_object() else {
        return false;
    };
    if result
        .get("observationId")
        .and_then(serde_json::Value::as_str)
        != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(target) = result
        .get("fileChangeTarget")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    let target_keys = target.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if target_keys != BTreeSet::from(["filePath", "observationId", "state"])
        || target
            .get("observationId")
            .and_then(serde_json::Value::as_str)
            != Some(&checkpoint.observation_id)
    {
        return false;
    }
    let Some(file_path) = target
        .get("filePath")
        .and_then(serde_json::Value::as_str)
        .filter(|path| !path.trim().is_empty())
    else {
        return false;
    };
    let Some(canonical_target) = resolve_checkpoint_file_target(run_context, file_path).ok() else {
        return false;
    };
    if canonical_target != Path::new(&checkpoint.canonical_target) {
        return false;
    }

    let mut terminal = value.clone();
    let Some(terminal_object) = terminal.as_object_mut() else {
        return false;
    };
    terminal_object.remove("observationId");
    terminal_object.remove("fileChangeTarget");
    let Ok(terminal) = serde_json::from_value::<crate::protocol::AgentFileChangeResult>(terminal)
    else {
        return false;
    };
    if !matches!(
        terminal.status,
        crate::protocol::AgentFileChangeResultStatus::Applied
            | crate::protocol::AgentFileChangeResultStatus::AlreadyApplied
    ) || terminal.file_path != file_path
    {
        return false;
    }
    let Some(request) = crate::tools::apply_patch_request(&source_call.args) else {
        return false;
    };
    let terminal_operation = match terminal.operation {
        crate::protocol::AgentFileChangeOperation::Create => "create",
        crate::protocol::AgentFileChangeOperation::Update => "update",
        crate::protocol::AgentFileChangeOperation::Delete => "delete",
    };
    let call_matches = match request.get("action").and_then(serde_json::Value::as_str) {
        Some("apply") => {
            request.get("filePath").and_then(serde_json::Value::as_str) == Some(file_path)
                && request.get("operation").and_then(serde_json::Value::as_str)
                    == Some(terminal_operation)
        }
        Some("commit") => {
            request
                .get("transactionId")
                .and_then(serde_json::Value::as_str)
                == Some(terminal.transaction_id.as_str())
                && staged_begin_source_matches(
                    context_items,
                    run_context,
                    &terminal,
                    &source_call.id,
                )
        }
        _ => false,
    };
    if !call_matches {
        return false;
    }
    match (&checkpoint.state, terminal.operation) {
        (FileObservationState::Missing, crate::protocol::AgentFileChangeOperation::Delete) => {
            target.get("state").and_then(serde_json::Value::as_str) == Some("missing")
                && terminal.revision.is_none()
        }
        (
            FileObservationState::Existing { revision, .. },
            crate::protocol::AgentFileChangeOperation::Create
            | crate::protocol::AgentFileChangeOperation::Update,
        ) => {
            target.get("state").and_then(serde_json::Value::as_str) == Some("existing")
                && terminal.revision.as_deref() == Some(revision)
        }
        _ => false,
    }
}

fn staged_begin_source_matches(
    context_items: &[AgentContextCheckpointItem],
    run_context: &AgentRunContext,
    terminal: &crate::protocol::AgentFileChangeResult,
    commit_call_id: &str,
) -> bool {
    let commit_item_indices = context_items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            item.role == "assistant"
                && item
                    .tool_calls
                    .iter()
                    .any(|call| call.id == commit_call_id && call.name == "apply_patch")
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let [commit_item_index] = commit_item_indices.as_slice() else {
        return false;
    };
    let terminal_target = resolve_checkpoint_file_target(run_context, &terminal.file_path).ok();
    let matches = context_items
        .iter()
        .enumerate()
        .filter(|(index, _)| index < commit_item_index)
        .filter(|(_, item)| item.role == "assistant")
        .flat_map(|(index, item)| item.tool_calls.iter().map(move |call| (index, call)))
        .filter(|(_, call)| call.name == "apply_patch")
        .filter(|(begin_item_index, call)| {
            let Some(request) = crate::tools::apply_patch_request(&call.args) else {
                return false;
            };
            if request.get("action").and_then(serde_json::Value::as_str) != Some("begin")
                || !crate::tools::apply_patch_wire_is_valid(&call.args)
            {
                return false;
            }
            let call_target = request
                .get("filePath")
                .and_then(serde_json::Value::as_str)
                .and_then(|path| resolve_checkpoint_file_target(run_context, path).ok());
            let operation = request.get("operation").and_then(serde_json::Value::as_str);
            let strategy = request.get("strategy").and_then(serde_json::Value::as_str);
            let terminal_strategy = match terminal.update_strategy {
                Some(crate::protocol::AgentFileChangeUpdateStrategy::Modify) => Some("modify"),
                Some(crate::protocol::AgentFileChangeUpdateStrategy::Rewrite) => Some("rewrite"),
                None => None,
            };
            let operation_matches = match terminal.operation {
                crate::protocol::AgentFileChangeOperation::Create => {
                    operation == Some("create")
                        && strategy.is_none()
                        && terminal.update_strategy.is_none()
                }
                crate::protocol::AgentFileChangeOperation::Update => {
                    operation == Some("update") && strategy == terminal_strategy
                }
                crate::protocol::AgentFileChangeOperation::Delete => false,
            };
            if call_target != terminal_target || !operation_matches {
                return false;
            }
            context_items
                .iter()
                .enumerate()
                .filter(|(index, _)| index > begin_item_index && index < commit_item_index)
                .filter(|(_, item)| {
                    item.role == "tool"
                        && !item.is_error
                        && item.tool_call_id.as_deref() == Some(call.id.as_str())
                })
                .filter_map(|(_, item)| {
                    serde_json::from_str::<serde_json::Value>(&item.content).ok()
                })
                .filter(|result| {
                    staged_begin_result_matches_current_shape(result, terminal, operation, strategy)
                })
                .count()
                == 1
        })
        .count();
    matches == 1
}

fn staged_begin_result_matches_current_shape(
    result: &serde_json::Value,
    terminal: &crate::protocol::AgentFileChangeResult,
    operation: Option<&str>,
    strategy: Option<&str>,
) -> bool {
    let Some(result) = result.as_object() else {
        return false;
    };
    let keys = result.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if keys
        != BTreeSet::from([
            "additions",
            "allowedNextActions",
            "byteCount",
            "deletions",
            "draftRevision",
            "filePath",
            "lineCount",
            "mutationCount",
            "nextIndex",
            "operation",
            "requiresCommitBeforeResponse",
            "status",
            "strategy",
            "tail",
            "tailStart",
            "tailTruncated",
            "totalChars",
            "transactionId",
        ])
        || result
            .get("transactionId")
            .and_then(serde_json::Value::as_str)
            != Some(terminal.transaction_id.as_str())
        || result.get("filePath").and_then(serde_json::Value::as_str)
            != Some(terminal.file_path.as_str())
        || result.get("operation").and_then(serde_json::Value::as_str) != operation
        || result.get("strategy").and_then(serde_json::Value::as_str) != strategy
        || result.get("status").and_then(serde_json::Value::as_str) != Some("drafting")
        || result
            .get("draftRevision")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        || result.get("nextIndex").and_then(serde_json::Value::as_u64) != Some(0)
        || result
            .get("mutationCount")
            .and_then(serde_json::Value::as_u64)
            != Some(0)
        || result
            .get("requiresCommitBeforeResponse")
            .and_then(serde_json::Value::as_bool)
            != Some(true)
        || !matches!(result.get("tail"), Some(serde_json::Value::String(_)))
        || ![
            "byteCount",
            "lineCount",
            "additions",
            "deletions",
            "totalChars",
            "tailStart",
        ]
        .iter()
        .all(|key| {
            result
                .get(*key)
                .and_then(serde_json::Value::as_u64)
                .is_some()
        })
        || !matches!(
            result.get("allowedNextActions"),
            Some(serde_json::Value::Array(actions))
                if actions == &[
                    serde_json::Value::String("append".to_string()),
                    serde_json::Value::String("edit".to_string()),
                    serde_json::Value::String("commit".to_string()),
                    serde_json::Value::String("status".to_string()),
                    serde_json::Value::String("abort".to_string()),
                ]
        )
    {
        return false;
    }
    result
        .get("tailStart")
        .and_then(serde_json::Value::as_u64)
        .is_some_and(|tail_start| {
            result
                .get("tailTruncated")
                .and_then(serde_json::Value::as_bool)
                == Some(tail_start > 0)
        })
}

fn resolve_checkpoint_file_target(
    run_context: &AgentRunContext,
    file_path: &str,
) -> Result<PathBuf, crate::file_change::FileChangeError> {
    let workspace_root = run_context
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.root_path.as_deref())
        .map(PathBuf::from);
    FileChangePathPolicy::new(
        workspace_root.as_deref(),
        run_context.permissions.write == AgentWritePermission::All,
    )
    .resolve(file_path)
    .map(|target| target.absolute_path().to_path_buf())
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
    validate_checkpoint_provider_protocol_revision(&checkpoint.provider_protocol_key)?;
    let mut continuation_ref_ids = BTreeSet::new();
    for continuation_ref in &checkpoint.provider_continuation_refs {
        continuation_ref.validate().map_err(|error| {
            AgentError::new(format!(
                "无法恢复运行检查点：Provider continuation ref 无效：{error}"
            ))
        })?;
        if !continuation_ref_ids.insert(continuation_ref.id.as_str()) {
            return Err(AgentError::new(
                "无法恢复运行检查点：Provider continuation ref 重复。",
            ));
        }
    }
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
    if let Some(grant_ref) = checkpoint.file_change_run_grant_ref.as_ref() {
        grant_ref
            .validate()
            .map_err(|_| AgentError::new("无法恢复运行检查点：FileChange Run grant ref 无效。"))?;
        let canonical_pending = checkpoint.pending_action_id.as_deref()
            == Some(
                crate::canonical_pending_action_id(run_id, &checkpoint.pending_tool_call_id)
                    .as_str(),
            );
        let matching_trace_calls = checkpoint
            .conversation_trace_items
            .iter()
            .filter(|item| {
                matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id, .. }
                        if call_id == &checkpoint.pending_tool_call_id
                )
            })
            .collect::<Vec<_>>();
        let exact_apply_patch_call = matching_trace_calls.len() == 1
            && matches!(
                matching_trace_calls[0],
                ConversationTurnTraceItem::ToolCall {
                    tool,
                    provenance: AgentToolIdentity::Builtin { tool_name },
                    approval_status: AgentApprovalStatus::Approved,
                    ..
                } if tool == "apply_patch" && tool_name == "apply_patch"
            );
        let exact_supported_request = checkpoint
            .context_items
            .iter()
            .flat_map(|item| item.tool_calls.iter())
            .filter(|call| call.id == checkpoint.pending_tool_call_id && call.name == "apply_patch")
            .collect::<Vec<_>>();
        let exact_supported_request = exact_supported_request.len() == 1
            && crate::tools::apply_patch_wire_is_valid(&exact_supported_request[0].args)
            && exact_supported_request[0]
                .args
                .get("request")
                .and_then(serde_json::Value::as_object)
                .is_some_and(|request| {
                    match request.get("action").and_then(serde_json::Value::as_str) {
                        Some("apply") => matches!(
                            request.get("operation").and_then(serde_json::Value::as_str),
                            Some("create" | "update")
                        ),
                        Some("commit") => true,
                        _ => false,
                    }
                });
        if !canonical_pending || !exact_apply_patch_call || !exact_supported_request {
            return Err(AgentError::new(
                "无法恢复运行检查点：FileChange Run grant ref 与冻结调用不一致。",
            ));
        }
    }
    validate_tool_set_checkpoint_shape(&checkpoint.tool_set)?;
    validate_collaboration_run_snapshot(
        checkpoint
            .tool_set
            .exposed_tool_names
            .iter()
            .map(String::as_str),
        checkpoint.collaboration_run_snapshot.as_ref(),
    )?;
    validate_checkpoint_world_state(&checkpoint.run_world_state, checkpoint.model_capabilities)?;
    validate_model_tool_call_id(&continuation.call.id)?;
    validate_model_tool_call_id(&continuation.result.call_id)?;
    validate_context_checkpoint_tool_call_ids(&checkpoint.context_items)?;
    validate_queued_checkpoint_tool_call_ids(&checkpoint.queued_tool_calls)?;
    validate_conversation_trace_tool_call_ids(&checkpoint.conversation_trace_items)?;
    let pending_checkpoint_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == checkpoint.pending_tool_call_id)
        .ok_or_else(|| AgentError::new("无法恢复运行检查点：缺少待审批 Tool Call。"))?;
    validate_pending_file_observation(
        pending_checkpoint_call,
        checkpoint.pending_file_observation.as_ref(),
        run_id,
        checkpoint.run_context.as_ref(),
        &checkpoint.context_items,
        "恢复",
    )?;
    let pending_file_observation_id = checkpoint
        .pending_file_observation
        .as_ref()
        .map(|observation| observation.observation_id.clone());
    let file_observations = restore_queued_file_observations(
        &checkpoint.queued_tool_calls,
        run_id,
        checkpoint.run_context.as_ref(),
        &checkpoint.context_items,
        checkpoint.pending_file_observation.as_ref(),
    )?;
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

    let continuation_has_model_only_file_change_projection = continuation.call.tool
        == "apply_patch"
        && continuation_success_can_issue_observation(
            &continuation.call,
            &continuation.result,
            checkpoint.run_context.as_ref(),
            &checkpoint.context_items,
        );
    let continuation_projection = checkpoint_continuation_projection(&checkpoint)?;
    let continuation_result_sequence =
        continuation_result_sequence(&checkpoint, &continuation.call.id);
    let provider_profile_config = checkpoint.provider_profile_config.clone();
    let provider_protocol_key = checkpoint.provider_protocol_key.clone();
    let provider_continuation_refs = checkpoint.provider_continuation_refs.clone();
    let assistant_turn_identity = checkpoint.assistant_turn_identity.clone();
    let tool_set = checkpoint.tool_set;
    let restored_batch_fingerprints =
        restore_batch_fingerprints(&checkpoint.context_items, &checkpoint.pending_tool_call_id)?;
    let mut restored_batch_file_observation_ids = restore_batch_file_observation_ids(
        &checkpoint.context_items,
        &checkpoint.pending_tool_call_id,
    )?;
    if let Some(observation_id) = pending_file_observation_id.as_ref() {
        restored_batch_file_observation_ids.insert(observation_id.clone());
    }
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
        seen_file_observation_ids: restored_batch_file_observation_ids,
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
    let mut continuation_model_result = continuation.result.clone();
    if continuation_has_model_only_file_change_projection {
        let observation_context =
            ToolExecutionContext::from_run_context(checkpoint.run_context.as_ref())
                .with_runtime_services(run_id.to_string(), None)
                .with_file_observation_registry(Arc::clone(&file_observations));
        if let Some(predecessor_observation_id) = pending_file_observation_id.as_deref() {
            crate::tools::attach_successor_observation_to_model_result_with_predecessor(
                &observation_context,
                &continuation.call,
                &continuation.result,
                &mut continuation_model_result,
                predecessor_observation_id,
            );
        } else {
            crate::tools::attach_successor_observation_to_model_result(
                &observation_context,
                &continuation.call,
                &continuation.result,
                &mut continuation_model_result,
            );
        }
    }
    let continuation_call = LlmToolCall {
        id: continuation.call.id.clone(),
        name: continuation.call.tool.clone(),
        args: continuation.call.args.clone(),
    };
    let durable_result = match continuation_projection {
        CheckpointContinuationProjection::Standard => {
            canonical_tool_result_for_context(&continuation.result)
        }
        CheckpointContinuationProjection::ExternalMcp => {
            crate::tools::mcp_tool_result_persistence_projection(&continuation.result)
        }
        CheckpointContinuationProjection::BuiltinCapability => {
            crate::tools::builtin_capability_tool_result_persistence_projection(
                &continuation.result,
            )
        }
    };
    let llm_result = match continuation_projection {
        CheckpointContinuationProjection::ExternalMcp => {
            crate::tools::mcp_tool_result_model_projection(&continuation_model_result)
        }
        CheckpointContinuationProjection::Standard
        | CheckpointContinuationProjection::BuiltinCapability => {
            crate::tools::model_projection_for_persisted_continuation(&continuation_model_result)
        }
    };
    let model_observation = super::finalize_model_tool_observation(
        model_tool_result_gate,
        &continuation.call.id,
        !continuation.result.ok,
        &llm_result,
        archive_metadata,
    )?;
    let persisted_model_observation =
        if continuation_projection != CheckpointContinuationProjection::Standard {
            super::finalize_model_tool_observation(
                model_tool_result_gate,
                &continuation.call.id,
                !continuation.result.ok,
                &durable_result,
                archive_metadata,
            )?
        } else if continuation_has_model_only_file_change_projection {
            // FileChange execution commits its canonical terminal ToolResult and model-context
            // item atomically before an approval continuation is scheduled. The successor
            // Observation is fresh run-local authority for the resumed model (and for a later
            // private checkpoint); it must not rewrite that already committed Trace prefix.
            let canonical_result =
                crate::tools::model_projection_for_persisted_continuation(&continuation.result);
            super::finalize_model_tool_observation(
                model_tool_result_gate,
                &continuation.call.id,
                !continuation.result.ok,
                &canonical_result,
                archive_metadata,
            )?
        } else {
            model_observation.clone()
        };
    context.append_tool_continuation_in_batch(
        &continuation_call,
        model_observation.clone(),
        (continuation_projection != CheckpointContinuationProjection::Standard)
            .then_some(persisted_model_observation.clone()),
        !continuation.result.ok,
        continuation_projection == CheckpointContinuationProjection::ExternalMcp,
        assistant_message_id.map(|assistant_message_id| {
            ContextOrigin::conversation_trace_item(
                assistant_message_id,
                continuation_result_sequence,
            )
        }),
        &queued_tool_call_ids,
    )?;
    conversation_trace
        .require_recorded_tool_call(&continuation.call)
        .map_err(AgentError::new)?;
    if let Some(sequence) = conversation_trace.record_tool_result_with_archive(
        &continuation.call,
        &durable_result,
        archive_metadata.clone(),
    ) {
        conversation_trace
            .record_model_message(
                sequence,
                0,
                &crate::llm::LlmMessage::tool_result(
                    continuation.call.id.clone(),
                    persisted_model_observation,
                    !continuation.result.ok,
                ),
            )
            .map_err(AgentError::new)?;
    }

    Ok(RestoredRunCheckpoint {
        context,
        next_model_request_index: checkpoint.next_model_request_index,
        tool_batch: restored_batch,
        extension_snapshots: checkpoint.extension_snapshots,
        conversation_trace,
        tool_set,
        run_context: checkpoint.run_context,
        collaboration_run_snapshot: checkpoint.collaboration_run_snapshot,
        model_capabilities: checkpoint.model_capabilities,
        run_world_state: checkpoint.run_world_state,
        provider_profile_config,
        provider_protocol_key,
        provider_continuation_refs,
        file_observations,
    })
}

fn continuation_success_can_issue_observation(
    call: &crate::protocol::AgentToolCall,
    result: &crate::protocol::AgentToolResult,
    run_context: Option<&AgentRunContext>,
    context_items: &[AgentContextCheckpointItem],
) -> bool {
    if !result.ok || result.call_id != call.id || result.tool != call.tool {
        return false;
    }
    let Some(request) = crate::tools::apply_patch_request(&call.args) else {
        return false;
    };
    let Some(terminal) = result
        .result
        .clone()
        .and_then(|value| {
            serde_json::from_value::<crate::protocol::AgentFileChangeResult>(value).ok()
        })
        .filter(|terminal| {
            matches!(
                terminal.status,
                crate::protocol::AgentFileChangeResultStatus::Applied
                    | crate::protocol::AgentFileChangeResultStatus::AlreadyApplied
            )
        })
    else {
        return false;
    };
    match request.get("action").and_then(serde_json::Value::as_str) {
        Some("apply") => true,
        Some("commit") => run_context.is_some_and(|run_context| {
            staged_begin_source_matches(context_items, run_context, &terminal, &call.id)
        }),
        _ => false,
    }
}

/// Selects the external-MCP continuation projection from the immutable Tool provenance frozen in
/// the approval checkpoint.
///
/// `pending_action_id` is only the identity of an approval record. External MCP calls and built-in
/// capability activation both need an action UUID distinct from the Provider Tool Call ID, so the
/// field cannot safely double as a tool-kind flag. Re-projecting a built-in result as MCP would
/// rewrite an already committed ToolResult and violate the append-only Trace prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckpointContinuationProjection {
    Standard,
    ExternalMcp,
    BuiltinCapability,
}

pub(super) fn checkpoint_continuation_projection(
    checkpoint: &AgentRunCheckpoint,
) -> AgentResult<CheckpointContinuationProjection> {
    let mut matching_provenance = checkpoint
        .conversation_trace_items
        .iter()
        .filter_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id,
                provenance,
                approval_status,
                ..
            } if call_id == &checkpoint.pending_tool_call_id => {
                Some((provenance, *approval_status))
            }
            _ => None,
        });
    let (provenance, frozen_approval_status) = matching_provenance
        .next()
        .ok_or_else(|| AgentError::new("无法恢复运行检查点：待审批调用缺少冻结的工具来源身份。"))?;
    if matching_provenance.next().is_some() {
        return Err(AgentError::new(
            "无法恢复运行检查点：待审批调用存在重复的工具来源身份。",
        ));
    }
    match (provenance, checkpoint.pending_action_id.as_deref()) {
        (AgentToolIdentity::Mcp { .. }, Some(_)) => {
            Ok(CheckpointContinuationProjection::ExternalMcp)
        }
        (AgentToolIdentity::Mcp { .. }, None) => Err(AgentError::new(
            "无法恢复运行检查点：外部 MCP 审批缺少冻结的动作身份。",
        )),
        (
            AgentToolIdentity::RuntimeExtension {
                extension_id,
                tool_name,
            },
            Some(_),
        ) if extension_id
            == crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
            && tool_name == crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME =>
        {
            Ok(CheckpointContinuationProjection::Standard)
        }
        (
            AgentToolIdentity::RuntimeExtension {
                extension_id,
                tool_name,
            },
            None,
        ) if extension_id
            == crate::builtin_capabilities::BUILTIN_CAPABILITY_RUNTIME_EXTENSION_ID
            && tool_name == crate::builtin_capabilities::ACTIVATE_CAPABILITY_TOOL_NAME =>
        {
            Err(AgentError::new(
                "无法恢复运行检查点：内置能力激活审批缺少冻结的动作身份。",
            ))
        }
        (AgentToolIdentity::BuiltinCapability { .. }, Some(_))
            if frozen_approval_status == AgentApprovalStatus::Required =>
        {
            // Sensitive built-in MCP Tools use the same durable pending-action/checkpoint
            // lifecycle as command and external-MCP approvals. The complete Catalog-bound
            // BuiltinCapability provenance plus a frozen `required` call proves this is the
            // standard built-in projection; the pending-action repository separately binds the
            // action UUID to the exact typed BuiltinMcpToolApproval. Ordinary automatic built-in
            // calls retain `automatic` and therefore fail closed if a bogus action id appears.
            Ok(CheckpointContinuationProjection::BuiltinCapability)
        }
        (AgentToolIdentity::Builtin { tool_name }, Some(_))
            if tool_name == "apply_patch"
                && matches!(
                    frozen_approval_status,
                    AgentApprovalStatus::Required | AgentApprovalStatus::Approved
                ) =>
        {
            Ok(CheckpointContinuationProjection::Standard)
        }
        (
            AgentToolIdentity::Unregistered { .. }
            | AgentToolIdentity::LegacyBuiltinCapability { .. },
            _,
        ) => Err(AgentError::new(
            "无法恢复运行检查点：未注册或旧版工具身份不能获得续跑权限。",
        )),
        (
            AgentToolIdentity::Builtin { .. }
            | AgentToolIdentity::RuntimeExtension { .. }
            | AgentToolIdentity::BuiltinCapability { .. },
            None,
        ) => Ok(CheckpointContinuationProjection::Standard),
        (
            AgentToolIdentity::Builtin { .. }
            | AgentToolIdentity::RuntimeExtension { .. }
            | AgentToolIdentity::BuiltinCapability { .. },
            Some(_),
        ) => Err(AgentError::new(
            "无法恢复运行检查点：审批动作身份与冻结的工具来源不匹配。",
        )),
    }
}

#[cfg(test)]
pub(super) fn checkpoint_continuation_uses_external_mcp_projection(
    checkpoint: &AgentRunCheckpoint,
) -> AgentResult<bool> {
    Ok(checkpoint_continuation_projection(checkpoint)?
        == CheckpointContinuationProjection::ExternalMcp)
}

fn validate_checkpoint_provider_protocol_revision(
    provider_protocol_key: &ProviderProtocolKey,
) -> AgentResult<()> {
    let revision = provider_protocol_key
        .provider_configuration_revision
        .as_deref()
        .ok_or_else(|| {
            AgentError::new("运行检查点必须冻结当前 per-model Provider Protocol revision。")
        })?;
    let Some(suffix) = revision.strip_prefix("provider-protocol-v1:") else {
        return Err(AgentError::new(
            "运行检查点的 Provider Protocol revision 版本不受支持。",
        ));
    };
    if suffix.trim().is_empty() || suffix.trim() != suffix || suffix.chars().any(char::is_control) {
        return Err(AgentError::new(
            "运行检查点的 Provider Protocol revision 无效。",
        ));
    }
    Ok(())
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

fn validate_collaboration_run_snapshot<'a>(
    exposed_tool_names: impl IntoIterator<Item = &'a str>,
    snapshot: Option<&crate::AgentCollaborationRunSnapshot>,
) -> AgentResult<()> {
    let collaboration_tool_count = exposed_tool_names
        .into_iter()
        .filter(|name| AGENT_COLLABORATION_TOOL_NAMES.contains(name))
        .count();
    if collaboration_tool_count != 0
        && collaboration_tool_count != AGENT_COLLABORATION_TOOL_NAMES.len()
    {
        return Err(AgentError::new(
            "运行检查点包含不完整的 Agent collaboration Tool 集。",
        ));
    }
    match (collaboration_tool_count, snapshot) {
        (0, None) => Ok(()),
        (count, Some(snapshot)) if count == AGENT_COLLABORATION_TOOL_NAMES.len() => {
            snapshot.validate()
        }
        (0, Some(_)) => Err(AgentError::new(
            "运行检查点在未暴露 Agent collaboration Tools 时携带了协作授权。",
        )),
        (_, None) => Err(AgentError::new(
            "运行检查点缺少 Agent collaboration Tool 的冻结授权。",
        )),
        _ => unreachable!("partial collaboration tool sets are rejected above"),
    }
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

fn restore_batch_file_observation_ids(
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
    let mut observation_ids = BTreeSet::new();
    for call in &pending_item.tool_calls {
        if let Some(observation_id) = claimed_file_observation_id_from_args(&call.name, &call.args)
        {
            observation_ids.insert(observation_id.to_string());
        }
        if call.id == pending_tool_call_id {
            return Ok(observation_ids);
        }
    }
    Err(AgentError::new(
        "运行检查点的完整 Assistant Turn 缺少待审批 Tool Call。",
    ))
}

fn queued_tool_call_checkpoint(
    call: &QueuedToolCall,
    assistant_turn_id: &str,
    allows_encrypted_checkpoint_rehydration: bool,
) -> AgentResult<AgentQueuedToolCallCheckpoint> {
    let rehydrates_from_encrypted_provider_turn = allows_encrypted_checkpoint_rehydration
        && call.checkpoint_persistence == AgentToolCallCheckpointPersistence::DeniedMcp;
    if call.checkpoint_persistence != AgentToolCallCheckpointPersistence::Allowed
        && !rehydrates_from_encrypted_provider_turn
    {
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
    if call.checkpoint_call.args != call.call.args && !rehydrates_from_encrypted_provider_turn {
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
            provider_identity: call.provider_identity()?,
        },
        file_observation: None,
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
            | ConversationTurnTraceItem::AgentMailboxDelivery { .. }
            | ConversationTurnTraceItem::CommandSessionLifecycle { .. }
            | ConversationTurnTraceItem::ContextCompactionLifecycle { .. }
            | ConversationTurnTraceItem::RuntimeError { .. } => {}
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
                || &queued.call.provider_identity != mapping
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
mod tests;
