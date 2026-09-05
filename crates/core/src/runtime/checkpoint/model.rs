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
    /// Generic restored turns retain the fully validated call mapping but reconstruct a local
    /// synthetic turn digest. This in-memory fact permits another pause in the same frozen batch.
    restored_identity_validated: bool,
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
            restored_identity_validated: false,
            deferred_external_tool_call_count: 0,
            suppressed_narration,
            seen_semantic_fingerprints: BTreeSet::new(),
            seen_file_observation_ids: BTreeSet::new(),
        })
    }

    pub(super) fn claim(&mut self, call: &LlmToolCall) -> ToolCallBatchClaim {
        // Distinct question call identities each await their own answer, including identical
        // question text. Storage deduplicates the exact call; semantic side-effect guards do not.
        if call.name == "request_user_input" {
            return ToolCallBatchClaim::Execute;
        }
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
        !tool_batch.restored_identity_validated
            || provider_runtime_capabilities.allows_encrypted_checkpoint_rehydration(),
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
        pause_reason: crate::AgentRunCheckpointPauseReason::Approval,
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
