use super::*;
use crate::tools::AgentToolExposure;

pub(super) struct RuntimeCapabilityServices {
    pub(super) host_actions_available: bool,
    pub(super) office_engine: Option<Arc<dyn crate::office::OfficeEngine>>,
    pub(super) image_generation_execution:
        Option<Arc<crate::image_generation::ImageGenerationExecutionService>>,
    pub(super) skill_activation_resolver: Option<AgentSkillActivationResolver>,
    pub(super) skill_resources: Option<Arc<crate::skills::SkillResourceSession>>,
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
            skill_activation_resolver: None,
            skill_resources: None,
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
        skill_activation_resolver,
        skill_resources,
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
    let context = input.context.as_ref();
    tool_registry.register_conversation_history();
    tool_registry.register_goal_tools();
    runtime_extensions.register_tools(&mut tool_registry)?;

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
    };
    let configuration_revision = conversation_context_configuration_revision_from_parts(
        &input,
        api_style,
        tool_definitions,
    )?;
    let shared_context_baseline = shared_context_baseline
        .filter(|baseline| baseline.matches_configuration(&configuration_revision));
    let (
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        visible_trace_item_count,
    ) = match restored_checkpoint {
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
                restored.visible_trace_item_count,
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
                0,
            )
        }
    };

    Ok(PreparedLlmRequest {
        template,
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        visible_trace_item_count,
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
    message.images = attachment_context.images;
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
) -> AgentResult<Option<RestoredRunCheckpoint>> {
    let checkpoint = input.resume_checkpoint.take();
    let continuation = input.tool_continuation.take();
    let approval_decision = input.approval_decision.take();
    match (checkpoint, continuation, approval_decision) {
        (None, None, None) => Ok(None),
        (Some(checkpoint), Some(continuation), Some(decision)) => {
            if decision.action_id != continuation.call.id {
                return Err(AgentError::new(format!(
                    "审批决定 `{}` 与工具续跑 `{}` 不一致。",
                    decision.action_id, continuation.call.id
                )));
            }
            restore_run_checkpoint_with_history_ref(
                checkpoint,
                run_id,
                &continuation,
                input.assistant_message_id.as_deref(),
            )
            .map(Some)
        }
        _ => Err(AgentError::new(
            "审批续跑必须同时提供完整运行检查点、审批决定和工具结果。",
        )),
    }
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
