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
            | ConversationTurnTraceItem::BackendState { .. }
                    | ConversationTurnTraceItem::ContextMaterial { .. }
            | ConversationTurnTraceItem::UserGuidance { .. }
            | ConversationTurnTraceItem::AgentMailboxDelivery { .. }
                | ConversationTurnTraceItem::WorkflowDelivery { .. }
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
