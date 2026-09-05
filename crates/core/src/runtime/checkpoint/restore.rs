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
        restored_identity_validated: true,
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
    let is_user_input = matches!(provenance,
        AgentToolIdentity::RuntimeExtension { extension_id, tool_name }
            if extension_id == "human.interaction" && tool_name == "request_user_input"
    );
    match checkpoint.pause_reason {
        crate::AgentRunCheckpointPauseReason::UserInput => {
            if !is_user_input
                || frozen_approval_status != AgentApprovalStatus::NotRequired
                || checkpoint.pending_action_id.is_some()
                || checkpoint.file_change_run_grant_ref.is_some()
                || checkpoint.pending_file_observation.is_some()
            {
                return Err(AgentError::new(
                    "Human input checkpoint authority is invalid.",
                ));
            }
            return Ok(CheckpointContinuationProjection::Standard);
        }
        crate::AgentRunCheckpointPauseReason::Approval if is_user_input => {
            return Err(AgentError::new(
                "A human question cannot carry approval authority.",
            ));
        }
        crate::AgentRunCheckpointPauseReason::Approval => {}
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
        (AgentToolIdentity::Unregistered { .. }, _) => Err(AgentError::new(
            "无法恢复运行检查点：未注册工具身份不能获得续跑权限。",
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
