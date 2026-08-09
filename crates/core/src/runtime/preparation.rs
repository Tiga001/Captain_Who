use super::*;
use crate::llm::LlmToolCall;
use crate::tools::AgentToolExposure;
use crate::ConversationModelContextItem;
use sha2::{Digest, Sha256};

pub(super) struct RuntimeCapabilityServices {
    pub(super) host_actions_available: bool,
    pub(super) office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    pub(super) image_generation_execution:
        Option<Arc<crate::image_generation::ImageGenerationExecutionService>>,
    pub(super) skill_installation_prepare:
        Option<Arc<dyn crate::tools::AgentSkillInstallationPrepareExecutor>>,
    pub(super) skill_installation_commit:
        Option<Arc<dyn crate::tools::AgentSkillInstallationCommitPreparer>>,
    pub(super) skill_activation_resolver: Option<AgentSkillActivationResolver>,
    pub(super) skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
    pub(super) mcp_tools: Option<crate::tools::McpToolRuntime>,
}

pub(super) struct DurableConversationTimeline {
    pub(super) compaction_summary: Option<crate::ContextCompactionSummary>,
    pub(super) world_state_records: Vec<crate::AnchoredWorldStateRecord>,
    pub(super) goal: Option<crate::ConversationGoal>,
    pub(super) messages: Vec<AgentChatMessage>,
}

pub(super) fn prepare_runtime_capabilities(
    input: &AgentChatInput,
    run_id: &str,
    extension_snapshots: &[AgentExtensionSnapshot],
    host_actions_available: bool,
    office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
) -> AgentResult<PreparedRuntimeCapabilities> {
    prepare_runtime_capabilities_with_skills(
        input,
        run_id,
        extension_snapshots,
        RuntimeCapabilityServices {
            host_actions_available,
            office_engine,
            image_generation_execution: None,
            skill_installation_prepare: None,
            skill_installation_commit: None,
            skill_activation_resolver: None,
            skill_resources: None,
            mcp_tools: None,
        },
    )
}

pub(super) fn prepare_runtime_capabilities_with_skills(
    input: &AgentChatInput,
    run_id: &str,
    extension_snapshots: &[AgentExtensionSnapshot],
    services: RuntimeCapabilityServices,
) -> AgentResult<PreparedRuntimeCapabilities> {
    let RuntimeCapabilityServices {
        host_actions_available,
        office_engine,
        image_generation_execution,
        skill_installation_prepare,
        skill_installation_commit,
        skill_activation_resolver,
        skill_resources,
        mcp_tools,
    } = services;
    let runtime_extensions = RuntimeExtensions::for_run_with_skills(
        run_id,
        input.skill_discovery.clone(),
        input.skill_activation.as_ref(),
        skill_activation_resolver,
        skill_resources,
        extension_snapshots,
    )?;
    let mut tool_registry = ToolRegistry::defaults_with_search_office_and_image(
        input.search_config.as_ref(),
        office_engine,
        image_generation_execution,
    );
    if let Some(executor) = skill_installation_prepare {
        tool_registry.register_skill_installation_prepare(executor);
    }
    if let Some(preparer) = skill_installation_commit {
        tool_registry.register_skill_installation_commit(preparer);
    }
    let context = input.context.as_ref();
    tool_registry.register_conversation_history();
    tool_registry.register_goal_tools();
    runtime_extensions.register_tools(&mut tool_registry)?;
    if let Some(mcp_tools) = mcp_tools.as_ref() {
        tool_registry.register_mcp_runtime(mcp_tools);
    }

    let command_permission = context
        .map(|context| context.permissions.command)
        .unwrap_or(AgentCommandPermission::RequireApproval);
    let command_auto_approve =
        command_permission == AgentCommandPermission::AutoApprove && host_actions_available;
    let command_permissions = context
        .map(|context| context.permissions)
        .unwrap_or_default();
    let command_workspace_root = context
        .and_then(|context| context.workspace.as_ref())
        .and_then(|workspace| workspace.root_path.as_deref())
        .filter(|root| !root.trim().is_empty())
        .map(PathBuf::from);
    let patch_auto_approve = context
        .map(|context| {
            file_write_approval_route(context.permissions) == FileWriteApprovalRoute::AutoApprove
        })
        .unwrap_or(false)
        && host_actions_available;

    let mut permitted_tool_definitions = tool_registry.definitions();
    apply_permission_policy_to_tool_definitions(
        &mut permitted_tool_definitions,
        context,
        &tool_registry,
    );
    if patch_auto_approve {
        for definition in permitted_tool_definitions.iter_mut().filter(|definition| {
            !matches!(
                tool_registry.exposure(&definition.name),
                Some(AgentToolExposure::Stable)
            ) && tool_registry
                .permission_policy(&definition.name)
                .uses_file_write_approval()
        }) {
            definition.requires_approval = false;
            definition.approval_mode = AgentToolApprovalMode::Never;
            definition.description.push_str(
                " The current permission policy automatically approves the final validated file change.",
            );
        }
    }
    let initial_tool_set = tool_registry.effective_tool_set(
        permitted_tool_definitions.iter().cloned(),
        &runtime_extensions.active_tool_capabilities()?,
    )?;

    Ok(PreparedRuntimeCapabilities {
        runtime_extensions,
        tool_registry: Arc::new(tool_registry),
        tool_definitions: permitted_tool_definitions,
        initial_tool_set,
        command_auto_approve,
        command_permissions,
        command_workspace_root,
        patch_auto_approve,
    })
}

/// Repairs only the legacy history that is about to enter this model request.
///
/// New conversations already carry an append-only `ConversationModelContextLog`. Older
/// conversations fall back to Durable Trace, but an archived tool result can be projected again
/// through its owning tool before that lossy fallback is used. The repaired projection is
/// persisted as an immutable prefix so restarts, approval resumes and later turns see one model
/// history instead of repeatedly deriving different representations.
pub(super) fn hydrate_legacy_model_history(
    input: &mut AgentChatInput,
    storage: Option<&crate::storage::service::StorageService>,
    tool_registry: &ToolRegistry,
) -> AgentResult<bool> {
    let (Some(storage), Some(conversation_id)) = (
        storage,
        input
            .context
            .as_ref()
            .and_then(|context| context.conversation_id.as_deref()),
    ) else {
        return Ok(false);
    };
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let model_tool_result_gate =
        ContextCapacityDetector::for_model(input.model.trim(), api_style, &[])
            .model_tool_result_gate();
    let mut changed = false;
    for message in &mut input.messages {
        let Some(trace) = message.conversation_turn_trace.as_ref() else {
            continue;
        };
        if trace.conversation_id != conversation_id
            || message.message_id.as_deref() != Some(trace.assistant_message_id.as_str())
        {
            return Err(AgentError::new(
                "旧会话模型历史重建时，消息与 Trace 身份不一致。",
            ));
        }
        crate::conversation_trace::validate_model_context_prefix(
            trace,
            &message.conversation_model_context_items,
        )
        .map_err(AgentError::new)?;
        let covered_sequence = message
            .conversation_model_context_items
            .last()
            .map(|item| item.sequence);
        let mut rebuilt = message.conversation_model_context_items.clone();
        for (index, trace_item) in trace.items.iter().enumerate() {
            if covered_sequence.is_some_and(|covered| trace_item.sequence() <= covered) {
                continue;
            }
            let model_message = match trace_item {
                crate::ConversationTurnTraceItem::AssistantNarration { content, .. } => {
                    Some(LlmMessage::text(LlmMessageRole::Assistant, content.clone()))
                }
                crate::ConversationTurnTraceItem::UserGuidance {
                    content,
                    attachments,
                    ..
                } => Some(LlmMessage::text(
                    LlmMessageRole::User,
                    crate::conversation_trace::render_user_guidance_content(content, attachments),
                )),
                crate::ConversationTurnTraceItem::ToolCall {
                    call_id,
                    tool,
                    operation,
                    ..
                } => {
                    let paired = trace.items[index + 1..]
                        .iter()
                        .find(|item| item.is_model_visible())
                        .is_some_and(|next| {
                            matches!(
                                next,
                                crate::ConversationTurnTraceItem::ToolResult {
                                    call_id: result_call_id,
                                    ..
                                } if result_call_id == call_id
                            )
                        });
                    if !paired {
                        break;
                    }
                    Some(LlmMessage::assistant(
                        "",
                        vec![LlmToolCall {
                            id: call_id.clone(),
                            name: tool.clone(),
                            args: operation.clone(),
                        }],
                    ))
                }
                crate::ConversationTurnTraceItem::ToolResult {
                    sequence: _,
                    call_id,
                    tool,
                    status,
                    success,
                    observation,
                    error,
                    archive,
                    ..
                } => {
                    let fallback = AgentToolResult {
                        exact_archive_file: None,
                        call_id: call_id.clone(),
                        tool: tool.clone(),
                        ok: *success,
                        result: Some(observation.clone()),
                        error: error.clone(),
                    };
                    let fallback_archive = ConversationHistoryArchiveTraceMetadata {
                        truncated_at_source: archive.truncated_at_source,
                        ..Default::default()
                    };
                    let (projected, verified_archive) = match archived_model_projection(
                        storage,
                        conversation_id,
                        trace,
                        trace_item,
                        tool_registry,
                    ) {
                        Some(restored) => (restored.result, restored.archive),
                        None => (fallback, fallback_archive),
                    };
                    let model_observation = super::finalize_model_tool_observation(
                        &model_tool_result_gate,
                        call_id,
                        matches!(
                            status,
                            crate::ConversationTraceToolResultStatus::Failed
                                | crate::ConversationTraceToolResultStatus::Conflict
                                | crate::ConversationTraceToolResultStatus::Cancelled
                        ),
                        &projected,
                        &verified_archive,
                    )?;
                    Some(LlmMessage::tool_result(
                        call_id.clone(),
                        model_observation,
                        matches!(
                            status,
                            crate::ConversationTraceToolResultStatus::Failed
                                | crate::ConversationTraceToolResultStatus::Conflict
                                | crate::ConversationTraceToolResultStatus::Cancelled
                        ),
                    ))
                }
                crate::ConversationTurnTraceItem::CommandSessionLifecycle { .. } => None,
            };
            let Some(model_message) = model_message else {
                continue;
            };
            let (item, _) = crate::conversation_trace::model_context_item_from_message(
                trace_item.sequence(),
                0,
                &model_message,
            )
            .map_err(AgentError::new)?;
            rebuilt.push(item);
        }
        crate::conversation_trace::validate_model_context_prefix(trace, &rebuilt)
            .map_err(AgentError::new)?;
        let appended = rebuilt.len() != message.conversation_model_context_items.len();
        if appended {
            storage
                .append_reconstructed_conversation_model_context(
                    conversation_id,
                    &trace.assistant_message_id,
                    &rebuilt,
                )
                .map_err(AgentError::new)?;
        }
        let bounded_existing = enforce_existing_model_tool_result_budget(
            storage,
            conversation_id,
            trace,
            tool_registry,
            &model_tool_result_gate,
            &mut rebuilt,
        )?;
        message.conversation_model_context_items = rebuilt;
        changed |= appended || bounded_existing;
    }
    Ok(changed)
}

struct ArchivedModelProjection {
    result: AgentToolResult,
    archive: ConversationHistoryArchiveTraceMetadata,
}

fn archived_model_projection(
    storage: &crate::storage::service::StorageService,
    conversation_id: &str,
    trace: &crate::ConversationTurnTrace,
    trace_item: &crate::ConversationTurnTraceItem,
    tool_registry: &ToolRegistry,
) -> Option<ArchivedModelProjection> {
    let crate::ConversationTurnTraceItem::ToolResult {
        sequence,
        call_id,
        tool,
        archive,
        ..
    } = trace_item
    else {
        return None;
    };
    let archive_ref = archive.archive_ref.as_deref()?;
    if !tool_registry.contains_tool(tool) {
        return None;
    }
    let descriptor = storage
        .find_conversation_history_archive_by_ref(conversation_id, archive_ref)
        .ok()??;
    if !descriptor.archived_completely
        || descriptor.assistant_message_id != trace.assistant_message_id
        || descriptor.sequence != *sequence
        || descriptor.call_id != call_id.as_str()
        || descriptor.tool != tool.as_str()
        || descriptor.content_type != "application/vnd.mycopilot.agent-tool-result+json"
        || archive
            .content_hash
            .as_deref()
            .is_some_and(|hash| hash != descriptor.content_hash)
        || archive
            .archived_bytes
            .is_some_and(|bytes| bytes != descriptor.total_bytes)
    {
        return None;
    }
    let page = storage
        .read_conversation_history_archive_page(
            conversation_id,
            archive_ref,
            crate::storage::conversation_history_archive_repository::ConversationHistoryArchivePageUnit::Byte,
            0,
            descriptor.total_bytes.max(1),
        )
        .ok()??;
    if page.truncated || page.content.len() as u64 != descriptor.total_bytes {
        return None;
    }
    let content_hash = format!("sha256:{:x}", Sha256::digest(page.content.as_bytes()));
    if content_hash != descriptor.content_hash {
        return None;
    }
    let archived = serde_json::from_str::<AgentToolResult>(&page.content).ok()?;
    if archived.call_id != call_id.as_str() || archived.tool != tool.as_str() {
        return None;
    }
    Some(ArchivedModelProjection {
        result: tool_registry.model_projection(&archived),
        archive: ConversationHistoryArchiveTraceMetadata {
            archive_ref: Some(descriptor.archive_ref),
            content_hash: Some(descriptor.content_hash),
            archived_bytes: Some(descriptor.total_bytes),
            archived_completely: Some(descriptor.archived_completely),
            truncated_at_source: descriptor.truncated_at_source,
            model_projection_truncated: descriptor.model_projection_truncated,
            history_projection_truncated: archive.history_projection_truncated,
            archive_projection_truncated: descriptor.archive_projection_truncated,
        },
    })
}

fn enforce_existing_model_tool_result_budget(
    storage: &crate::storage::service::StorageService,
    conversation_id: &str,
    trace: &crate::ConversationTurnTrace,
    tool_registry: &ToolRegistry,
    model_tool_result_gate: &ModelToolResultGate,
    items: &mut [ConversationModelContextItem],
) -> AgentResult<bool> {
    let mut changed = false;
    for item in items.iter_mut().filter(|item| item.role == "tool") {
        let Some(trace_item) = trace
            .items
            .iter()
            .find(|trace_item| trace_item.sequence() == item.sequence)
        else {
            return Err(AgentError::new(
                "旧会话模型历史中的工具结果缺少对应 Trace 记录。",
            ));
        };
        let crate::ConversationTurnTraceItem::ToolResult {
            call_id,
            tool,
            success,
            archive,
            ..
        } = trace_item
        else {
            return Err(AgentError::new(
                "旧会话模型历史中的 tool 消息没有对应工具结果。",
            ));
        };
        let existing_payload = serde_json::from_str::<Value>(&item.content)
            .unwrap_or_else(|_| Value::String(item.content.clone()));
        let existing = AgentToolResult {
            exact_archive_file: None,
            call_id: call_id.clone(),
            tool: tool.clone(),
            ok: *success,
            result: Some(existing_payload),
            error: None,
        };
        if !model_tool_result_gate.would_truncate(call_id, item.is_error, &existing) {
            continue;
        }

        let fallback_archive = ConversationHistoryArchiveTraceMetadata {
            truncated_at_source: archive.truncated_at_source,
            ..Default::default()
        };
        let (projected, verified_archive) = match archived_model_projection(
            storage,
            conversation_id,
            trace,
            trace_item,
            tool_registry,
        ) {
            Some(restored) => (restored.result, restored.archive),
            None => (existing, fallback_archive),
        };
        let bounded = super::finalize_model_tool_observation(
            model_tool_result_gate,
            call_id,
            item.is_error,
            &projected,
            &verified_archive,
        )?;
        if bounded != item.content {
            item.content = bounded;
            changed = true;
        }
    }
    Ok(changed)
}

pub(super) fn assemble_context_preview(
    timeline: DurableConversationTimeline,
    skill_discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    skill_activation: Option<AgentSkillActivation>,
    _context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<crate::context::AssembledContext> {
    let DurableConversationTimeline {
        compaction_summary,
        world_state_records,
        goal,
        messages,
    } = timeline;
    if compaction_summary.is_some()
        || !world_state_records.is_empty()
        || goal.is_some()
        || messages.iter().any(|message| {
            matches!(message.role.trim(), "user" | "assistant")
                && !message.content.trim().is_empty()
        })
    {
        return ContextAssembler::assemble_with_timing(ContextAssemblyInput {
            system_prompt: build_system_prompt(prompt_preferences, tool_definitions),
            compaction_summary,
            world_state_records,
            goal,
            initial_run_world_state: None,
            messages,
            skill_discovery,
            skill_activation,
            attachments: ContextAttachments::default(),
        });
    }

    Ok(crate::context::AssembledContext {
        frame: ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::System,
            build_system_prompt(prompt_preferences, tool_definitions),
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        )]),
        timing: crate::context::ConversationTimingTracker::default(),
    })
}

pub(super) fn build_llm_request(
    input: AgentChatInput,
    tool_definitions: &[AgentToolDefinition],
    restored_checkpoint: Option<RestoredRunCheckpoint>,
    shared_context_baseline: Option<AgentContextBaseline>,
    initial_run_world_state: Option<crate::WorldStateSnapshot>,
) -> AgentResult<PreparedLlmRequest> {
    let api_style = input
        .api_style
        .unwrap_or_else(|| detect_api_style(input.api_url.trim()));
    let provider_dialect = ProviderProtocolDialect::from(api_style);
    let provider_profile_config =
        ProviderProfileConfig::resolve(input.provider_profile_config.as_ref(), provider_dialect)
            .map_err(|error| {
                AgentError::new(format!(
                    "Provider profile configuration is invalid: {error}"
                ))
            })?;
    let provider_protocol_key = match input.provider_protocol_key.clone() {
        Some(key) => {
            key.validate_against_config(&provider_profile_config)
                .map_err(|error| {
                    AgentError::new(format!("Provider protocol key is invalid: {error}"))
                })?;
            if key.model_id != input.model.trim() {
                return Err(AgentError::new(
                    "Provider protocol key does not match the selected model.",
                ));
            }
            if key.provider_configuration_revision != input.provider_configuration_revision {
                return Err(AgentError::new(
                    "Provider protocol key does not match the selected provider configuration revision.",
                ));
            }
            key
        }
        None => ProviderProtocolKey::new(
            provider_dialect,
            &provider_profile_config,
            input.model.trim(),
            input.provider_configuration_revision.clone(),
        )
        .map_err(|error| AgentError::new(format!("Provider protocol key is invalid: {error}")))?,
    };
    let template = LlmRequestTemplate {
        api_url: input.api_url.trim().to_string(),
        api_token: input.api_token.trim().to_string(),
        model: input.model.trim().to_string(),
        api_style,
        context_window_tokens: input.context_window_tokens,
        max_tokens: sanitize_max_tokens(input.max_tokens),
        temperature: sanitize_temperature(input.temperature),
        stream: input.stream.unwrap_or(false),
        stable_tools: tool_definitions.to_vec(),
        provider_profile_config,
        provider_protocol_key,
    };
    let configuration_revision = conversation_context_configuration_revision_from_parts(
        &input,
        api_style,
        tool_definitions,
    )?;
    let shared_context_baseline = shared_context_baseline
        .filter(|baseline| baseline.matches_configuration(&configuration_revision));
    let (context, next_model_request_index, tool_batch, conversation_trace) =
        match restored_checkpoint {
            Some(restored) => {
                let context = match shared_context_baseline {
                    Some(baseline) => baseline.rebase_restored_frame(restored.context),
                    None => restored.context,
                };
                (
                    context,
                    restored.next_model_request_index,
                    restored.tool_batch,
                    restored.conversation_trace,
                )
            }
            None => {
                let attachment_context = build_attachment_context(
                    &input.attachments,
                    input
                        .context
                        .as_ref()
                        .and_then(|context| context.attachment_library.as_ref()),
                )?;
                let skill_activation = input.skill_activation;
                let skill_discovery = input.skill_discovery;
                let world_state_records = input.world_state_records;
                let context = match shared_context_baseline {
                    Some(baseline) => {
                        let mut context = baseline.into_frame();
                        ContextAssembler::append_initial_run_world_state(
                            &mut context,
                            initial_run_world_state.as_ref(),
                        )?;
                        append_attachment_context(&mut context, attachment_context);
                        ContextAssembler::append_skill_overlays(
                            &mut context,
                            skill_discovery.as_ref(),
                            skill_activation.as_ref(),
                        )?;
                        context
                    }
                    None => assemble_initial_context_with_skill_overlays(
                        DurableConversationTimeline {
                            compaction_summary: input.context_compaction_summary,
                            world_state_records,
                            goal: input.goal,
                            messages: input.messages,
                        },
                        initial_run_world_state,
                        InitialSkillOverlays {
                            discovery: skill_discovery,
                            activation: skill_activation,
                        },
                        attachment_context,
                        input.context.as_ref(),
                        input.prompt_preferences.as_ref(),
                        tool_definitions,
                    )?,
                };
                (
                    context,
                    0,
                    ToolCallBatch::default(),
                    ConversationTraceRecorder::default(),
                )
            }
        };

    Ok(PreparedLlmRequest {
        template,
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
    })
}

pub(super) fn append_attachment_context(
    frame: &mut ContextFrame,
    attachment_context: AttachmentContext,
) {
    if attachment_context.text.trim().is_empty() && attachment_context.images.is_empty() {
        return;
    }
    let mut message = LlmMessage::text(LlmMessageRole::User, attachment_context.text);
    *message
        .images_mut()
        .expect("user attachment messages support images") = attachment_context.images;
    frame.push(ContextItem::new(
        message,
        ContextMetadata::new(
            ContextSource::InputAttachment,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    ));
}

pub(super) fn restore_input_checkpoint(
    input: &mut AgentChatInput,
    run_id: &str,
    model_tool_result_gate: &ModelToolResultGate,
    archive_metadata: &ConversationHistoryArchiveTraceMetadata,
) -> AgentResult<Option<RestoredRunCheckpoint>> {
    let checkpoint = input.resume_checkpoint.take();
    let continuation = input.tool_continuation.take();
    let approval_decision = input.approval_decision.take();
    match (checkpoint, continuation, approval_decision) {
        (None, None, None) => Ok(None),
        (Some(checkpoint), Some(continuation), Some(decision)) => {
            let expected_action_id = expected_approval_action_id(
                checkpoint.pending_action_id.as_deref(),
                &checkpoint.pending_tool_call_id,
            );
            if decision.action_id != expected_action_id {
                return Err(AgentError::new(format!(
                    "审批决定 `{}` 与冻结动作 `{expected_action_id}` 不一致。",
                    decision.action_id,
                )));
            }
            restore_run_checkpoint_with_model_projection(
                checkpoint,
                run_id,
                &continuation,
                input.assistant_message_id.as_deref(),
                model_tool_result_gate,
                archive_metadata,
            )
            .map(Some)
        }
        _ => Err(AgentError::new(
            "审批续跑必须同时提供完整运行检查点、审批决定和工具结果。",
        )),
    }
}

fn expected_approval_action_id<'a>(
    pending_action_id: Option<&'a str>,
    pending_tool_call_id: &'a str,
) -> &'a str {
    pending_action_id.unwrap_or(pending_tool_call_id)
}

pub(super) fn suppressed_narration_context_item() -> ContextItem {
    ContextItem::text(
        LlmMessageRole::System,
        "The text emitted alongside the preceding tool calls was not shown to the user because file transactions were unsettled. Do not assume the user saw it. Continue the transaction protocol and generate new text only after every draft has a finish or abort outcome.",
        ContextSource::RuntimeGuard,
        ContextScope::Run,
        ContextRetention::Retained,
    )
}

pub(super) fn deferred_external_tool_calls_context_item(count: u32) -> ContextItem {
    ContextItem::text(
        LlmMessageRole::System,
        format!(
            "MCP_DEFERRED_CALLS_NEED_REPREPARE count={count}. These additional external tool calls from the earlier model response were not executed or persisted because each MCP invocation requires its own one-time preparation and approval. If they are still needed, issue fresh tool calls now, one approval boundary at a time. Do not assume any deferred call ran."
        ),
        ContextSource::RuntimeGuard,
        ContextScope::Run,
        ContextRetention::Retained,
    )
}

pub(super) fn empty_model_action_repair_context_item() -> ContextItem {
    ContextItem::text(
        LlmMessageRole::System,
        "The preceding model response ended normally but contained neither user-visible assistant text nor a tool call. Continue the current task now. If work remains, return valid tool calls using the supplied schemas. If the task is complete or cannot proceed, return a concrete user-visible assistant response. Do not return an empty response, and do not repeat already completed operations merely because this repair request was issued; rely on the existing tool results.",
        ContextSource::RuntimeGuard,
        ContextScope::Run,
        ContextRetention::RequestOnly,
    )
}

pub(super) fn apply_permission_policy_to_tool_definitions(
    definitions: &mut Vec<AgentToolDefinition>,
    context: Option<&AgentRunContext>,
    tool_registry: &ToolRegistry,
) {
    let permissions = context
        .map(|context| context.permissions)
        .unwrap_or_default();

    if permissions.write == crate::protocol::AgentWritePermission::Denied {
        definitions.retain(|definition| {
            matches!(
                tool_registry.exposure(&definition.name),
                Some(AgentToolExposure::Stable)
            ) || (tool_registry
                .permission_policy(&definition.name)
                .is_available_when_write_denied()
                && definition.name != "skills_run_script")
        });
    }

    // A cwd is not a filesystem or network sandbox. Until the host can enforce
    // those boundaries, even dependency inspection remains an unrestricted
    // host capability and every actual script execution stays manual.
    if permissions.command_safety != AgentCommandSafetyPolicy::FullAccess
        || permissions.read != crate::protocol::AgentReadPermission::All
        || permissions.write != crate::protocol::AgentWritePermission::All
    {
        definitions.retain(|definition| {
            definition.name != "skills_preflight_script" && definition.name != "skills_run_script"
        });
    }

    if permissions.read == crate::protocol::AgentReadPermission::All {
        for definition in definitions.iter_mut().filter(|definition| {
            !matches!(
                tool_registry.exposure(&definition.name),
                Some(AgentToolExposure::Stable)
            )
        }) {
            match definition.name.as_str() {
                "read_word" | "read_presentation" | "read_spreadsheet" => {
                    set_schema_property_description(
                        &mut definition.input_schema,
                        "path",
                        "Workspace-relative path, absolute local path, @home/@desktop/@documents/@downloads, or @attachments readPath.",
                    );
                }
                _ => {}
            }
        }
    }
}

pub(super) fn set_schema_property_description(
    schema: &mut Value,
    property: &str,
    description: &str,
) {
    if let Some(property_schema) = schema
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .and_then(|properties| properties.get_mut(property))
        .and_then(Value::as_object_mut)
    {
        property_schema.insert("description".to_string(), json!(description));
    }
}

#[cfg(test)]
pub(super) fn assemble_initial_context(
    compaction_summary: Option<crate::ContextCompactionSummary>,
    messages: Vec<AgentChatMessage>,
    skill_activation: Option<AgentSkillActivation>,
    attachment_context: AttachmentContext,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    assemble_initial_context_with_skill_overlays(
        DurableConversationTimeline {
            compaction_summary,
            world_state_records: Vec::new(),
            goal: None,
            messages,
        },
        None,
        InitialSkillOverlays {
            discovery: None,
            activation: skill_activation,
        },
        attachment_context,
        context,
        prompt_preferences,
        tool_definitions,
    )
}

struct InitialSkillOverlays {
    discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    activation: Option<AgentSkillActivation>,
}

fn assemble_initial_context_with_skill_overlays(
    timeline: DurableConversationTimeline,
    initial_run_world_state: Option<crate::WorldStateSnapshot>,
    skills: InitialSkillOverlays,
    attachment_context: AttachmentContext,
    _context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    let DurableConversationTimeline {
        compaction_summary,
        world_state_records,
        goal,
        messages,
    } = timeline;
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: build_system_prompt(prompt_preferences, tool_definitions),
        compaction_summary,
        world_state_records,
        goal,
        initial_run_world_state,
        messages,
        skill_discovery: skills.discovery,
        skill_activation: skills.activation,
        attachments: ContextAttachments {
            text: attachment_context.text,
            images: attachment_context.images,
        },
    })
}

#[cfg(test)]
mod approval_identity_tests {
    use super::expected_approval_action_id;

    #[test]
    fn mcp_action_identity_is_independent_while_legacy_actions_fall_back_to_call_identity() {
        assert_eq!(
            expected_approval_action_id(Some("mcp-action-uuid"), "model-call-id"),
            "mcp-action-uuid"
        );
        assert_eq!(
            expected_approval_action_id(None, "model-call-id"),
            "model-call-id"
        );
    }
}

#[cfg(test)]
mod legacy_model_history_tests {
    use super::*;
    use crate::storage::conversation_history_archive_repository::ConversationHistoryArchiveInput;
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use tempfile::tempdir;

    fn message(id: &str, role: &str, content: &str, created_at: i64) -> ChatMessageRecord {
        ChatMessageRecord {
            id: id.to_string(),
            role: role.to_string(),
            content: content.to_string(),
            created_at,
            status: Some("sent".to_string()),
            attachments: Vec::new(),
            agent_run_json: None,
            ui_state_json: None,
        }
    }

    #[test]
    fn legacy_tail_rebuilds_from_exact_archive_once_and_survives_restart() {
        let fixture = tempdir().unwrap();
        let database = fixture.path().join("legacy-model-history.sqlite");
        let storage = crate::storage::service::StorageService::open(&database).unwrap();
        storage
            .save_conversation(ChatConversationRecord {
                id: "conversation-legacy".to_string(),
                project_id: None,
                model_id: Some("model-1".to_string()),
                title: "Legacy".to_string(),
                messages: vec![
                    message("user-legacy", "user", "read the legacy file", 1),
                    message("assistant-legacy", "assistant", "done", 2),
                ],
                created_at: 1,
                updated_at: 2,
                pinned_at: None,
                archived_at: None,
                unread_at: None,
            })
            .unwrap();
        let archived_result = AgentToolResult {
            exact_archive_file: None,
            call_id: "call-legacy".to_string(),
            tool: "read_file".to_string(),
            ok: true,
            result: Some(json!({
                "path": "legacy.txt",
                "content": "EXACT_ARCHIVE_MODEL_HISTORY_MARKER",
                "truncated": false
            })),
            error: None,
        };
        let archive = storage
            .archive_conversation_tool_result(ConversationHistoryArchiveInput {
                conversation_id: "conversation-legacy".to_string(),
                assistant_message_id: "assistant-legacy".to_string(),
                sequence: 1,
                call_id: "call-legacy".to_string(),
                tool: "read_file".to_string(),
                content_type: "application/vnd.mycopilot.agent-tool-result+json".to_string(),
                content: serde_json::to_string(&archived_result).unwrap(),
                truncated_at_source: false,
                model_projection_truncated: true,
                archive_projection_truncated: false,
                created_at: 2,
            })
            .unwrap();
        let trace = crate::ConversationTurnTrace {
            schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-legacy".to_string(),
            conversation_id: "conversation-legacy".to_string(),
            assistant_message_id: "assistant-legacy".to_string(),
            terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: true,
            items: vec![
                crate::ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-legacy".to_string(),
                    tool: "read_file".to_string(),
                    provenance: None,
                    operation: json!({ "path": "legacy.txt" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                crate::ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-legacy".to_string(),
                    tool: "read_file".to_string(),
                    status: crate::ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "summary": "LOSSY_DURABLE_TRACE_MARKER" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: true,
                    archive: crate::ConversationHistoryArchiveTraceMetadata {
                        archive_ref: Some(archive.archive_ref),
                        content_hash: Some(archive.content_hash),
                        archived_bytes: Some(archive.total_bytes),
                        archived_completely: Some(true),
                        model_projection_truncated: true,
                        ..Default::default()
                    },
                },
            ],
        };
        let mut in_progress = trace.clone();
        in_progress.terminal_status = crate::ConversationTurnTraceTerminalStatus::InProgress;
        storage
            .append_in_progress_conversation_turn_trace(&in_progress, 2, 2)
            .unwrap();
        storage
            .replace_conversation_turn_trace(&trace, 2, 3)
            .unwrap();

        let mut input: AgentChatInput = serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1",
            "apiToken": "token",
            "model": "model-1",
            "apiStyle": "open_ai_compatible",
            "maxTokens": 1024,
            "context": { "conversationId": "conversation-legacy" },
            "messages": [{
                "messageId": "assistant-legacy",
                "role": "assistant",
                "content": "done",
                "conversationTurnTrace": trace
            }]
        }))
        .unwrap();
        let registry = ToolRegistry::defaults_with_search(None);

        assert!(hydrate_legacy_model_history(&mut input, Some(&storage), &registry).unwrap());
        let restored_content = &input.messages[0].conversation_model_context_items[1].content;
        assert!(restored_content.contains("EXACT_ARCHIVE_MODEL_HISTORY_MARKER"));
        assert!(!restored_content.contains("LOSSY_DURABLE_TRACE_MARKER"));
        assert!(
            !hydrate_legacy_model_history(&mut input, Some(&storage), &registry).unwrap(),
            "persisted reconstruction must be idempotent"
        );
        drop(storage);

        let reopened = crate::storage::service::StorageService::open(&database).unwrap();
        let persisted = reopened
            .get_conversation_model_context_log("assistant-legacy")
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.items,
            input.messages[0].conversation_model_context_items
        );
        assert!(persisted.items[1]
            .content
            .contains("EXACT_ARCHIVE_MODEL_HISTORY_MARKER"));
    }

    #[test]
    fn oversized_legacy_model_result_with_a_missing_archive_fails_closed() {
        let fixture = tempdir().unwrap();
        let storage = crate::storage::service::StorageService::open(
            &fixture.path().join("missing-legacy-archive.sqlite"),
        )
        .unwrap();
        let trace = crate::ConversationTurnTrace {
            schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-missing-archive".to_string(),
            conversation_id: "conversation-missing-archive".to_string(),
            assistant_message_id: "assistant-missing-archive".to_string(),
            terminal_status: crate::ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items: vec![
                crate::ConversationTurnTraceItem::ToolCall {
                    sequence: 0,
                    call_id: "call-missing-archive".to_string(),
                    tool: "read_file".to_string(),
                    provenance: None,
                    operation: json!({ "path": "legacy.txt" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    truncated: false,
                },
                crate::ConversationTurnTraceItem::ToolResult {
                    sequence: 1,
                    call_id: "call-missing-archive".to_string(),
                    tool: "read_file".to_string(),
                    status: crate::ConversationTraceToolResultStatus::Succeeded,
                    success: true,
                    observation: json!({ "summary": "bounded durable fallback" }),
                    approval_status: AgentApprovalStatus::NotRequired,
                    error: None,
                    truncated: true,
                    archive: crate::ConversationHistoryArchiveTraceMetadata {
                        archive_ref: Some("missing-archive".to_string()),
                        archived_completely: Some(true),
                        ..Default::default()
                    },
                },
            ],
        };
        let mut items = vec![ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".to_string(),
            content: serde_json::to_string(&json!({
                "content": "x".repeat(100_000)
            }))
            .unwrap(),
            tool_call_id: Some("call-missing-archive".to_string()),
            tool_calls: Vec::new(),
            is_error: false,
        }];
        let registry = ToolRegistry::defaults_with_search(None);
        let gate = ContextCapacityDetector::for_model(
            "model-1",
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            &[],
        )
        .model_tool_result_gate();

        let error = enforce_existing_model_tool_result_budget(
            &storage,
            "conversation-missing-archive",
            &trace,
            &registry,
            &gate,
            &mut items,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("没有可用的分页游标或 Exact History 恢复位置"));
        assert!(!items[0].content.contains("historyOpen"));
    }
}
