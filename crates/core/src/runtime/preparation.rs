use super::*;

pub(super) fn prepare_runtime_capabilities(
    input: &AgentChatInput,
    run_id: &str,
    extension_snapshots: &[AgentExtensionSnapshot],
    host_actions_available: bool,
) -> AgentResult<PreparedRuntimeCapabilities> {
    let runtime_extensions = RuntimeExtensions::for_run(run_id, extension_snapshots)?;
    let mut tool_registry = ToolRegistry::defaults_with_search(input.search_config.as_ref());
    let context = input.context.as_ref();
    if context
        .and_then(|context| context.conversation_id.as_deref())
        .is_some_and(|conversation_id| !conversation_id.trim().is_empty())
    {
        tool_registry.register_conversation_history();
    }
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
    let command_safety = command_permissions.command_safety;
    let patch_auto_approve = context
        .map(|context| {
            context.permissions.patch == AgentPatchPermission::AutoApprove
                && context.permissions.write != crate::protocol::AgentWritePermission::Denied
        })
        .unwrap_or(false)
        && host_actions_available;

    let mut tool_definitions = tool_registry.definitions();
    apply_permission_policy_to_tool_definitions(&mut tool_definitions, context);
    if command_auto_approve {
        if let Some(definition) = tool_definitions
            .iter_mut()
            .find(|definition| definition.name == "run_command")
        {
            definition.requires_approval = false;
            definition.approval_mode = match command_safety {
                AgentCommandSafetyPolicy::Guarded => AgentToolApprovalMode::Dynamic,
                AgentCommandSafetyPolicy::FullAccess => AgentToolApprovalMode::Never,
            };
            definition.description = match command_safety {
                AgentCommandSafetyPolicy::Guarded => "Run a validated shell command through the host execution layer. Low-risk commands are automatically authorized; high-impact commands are routed to explicit user approval.".to_string(),
                AgentCommandSafetyPolicy::FullAccess => "Run a validated shell command through the host execution layer. Commands are automatically authorized except operations that are always denied or unsupported.".to_string(),
            };
        }
    }
    if patch_auto_approve {
        for definition in tool_definitions.iter_mut().filter(|definition| {
            definition.name == "apply_patch" || definition.name == "write_file"
        }) {
            definition.requires_approval = false;
            definition.description.push_str(
                " The current permission policy automatically approves the final validated file change.",
            );
        }
    }

    Ok(PreparedRuntimeCapabilities {
        runtime_extensions,
        tool_registry: Arc::new(tool_registry),
        tool_definitions,
        command_auto_approve,
        command_permissions,
        command_workspace_root,
        patch_auto_approve,
    })
}

pub(super) fn assemble_context_preview(
    compaction_summary: Option<crate::ContextCompactionSummary>,
    messages: Vec<AgentChatMessage>,
    skill_activation: Option<AgentSkillActivation>,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<crate::context::AssembledContext> {
    if compaction_summary.is_some()
        || messages.iter().any(|message| {
            matches!(message.role.trim(), "user" | "assistant")
                && !message.content.trim().is_empty()
        })
    {
        return ContextAssembler::assemble_with_timing(ContextAssemblyInput {
            system_prompt: build_system_prompt(context, prompt_preferences, tool_definitions),
            compaction_summary,
            messages,
            skill_activation,
            attachments: ContextAttachments::default(),
        });
    }

    Ok(crate::context::AssembledContext {
        frame: ContextFrame::new(vec![ContextItem::text(
            LlmMessageRole::System,
            build_system_prompt(context, prompt_preferences, tool_definitions),
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
        tools: tool_definitions.to_vec(),
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
            let attachment_context = build_attachment_context(&input.attachments)?;
            let skill_activation = input.skill_activation;
            let mut context = match shared_context_baseline {
                Some(baseline) => {
                    let mut context = baseline.into_frame();
                    ContextAssembler::append_skill_activation(
                        &mut context,
                        skill_activation.as_ref(),
                    )?;
                    context
                }
                None => assemble_initial_context(
                    input.context_compaction_summary,
                    input.messages,
                    skill_activation,
                    AttachmentContext {
                        text: String::new(),
                        images: Vec::new(),
                    },
                    input.context.as_ref(),
                    input.prompt_preferences.as_ref(),
                    tool_definitions,
                )?,
            };
            append_attachment_context(&mut context, attachment_context);
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
            restore_run_checkpoint(checkpoint, run_id, &continuation).map(Some)
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

pub(super) fn apply_permission_policy_to_tool_definitions(
    definitions: &mut Vec<AgentToolDefinition>,
    context: Option<&AgentRunContext>,
) {
    let permissions = context
        .map(|context| context.permissions)
        .unwrap_or_default();

    if permissions.write == crate::protocol::AgentWritePermission::Denied {
        definitions.retain(|definition| {
            definition.name != "apply_patch" && definition.name != "write_file"
        });
    }

    if permissions.read == crate::protocol::AgentReadPermission::All {
        for definition in definitions.iter_mut() {
            match definition.name.as_str() {
                "read_file" | "read_image" | "read_pdf" | "read_word" | "read_presentation"
                | "read_spreadsheet" => {
                    set_schema_property_description(
                        &mut definition.input_schema,
                        "path",
                        "Workspace-relative path, absolute local path, @home/@desktop/@documents/@downloads, or @attachments readPath.",
                    );
                }
                "search_code" => set_schema_property_description(
                    &mut definition.input_schema,
                    "path",
                    "Optional workspace-relative or absolute directory/file path, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                "search_files" => set_schema_property_description(
                    &mut definition.input_schema,
                    "path",
                    "Optional workspace-relative or absolute directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                "workspace_map" => set_schema_property_description(
                    &mut definition.input_schema,
                    "focusPath",
                    "Optional workspace-relative or absolute directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
                ),
                _ => {}
            }
        }
    }

    if permissions.write == crate::protocol::AgentWritePermission::All {
        for definition in definitions.iter_mut().filter(|definition| {
            definition.name == "apply_patch" || definition.name == "write_file"
        }) {
            if definition.name == "apply_patch" {
                definition.description = "Create, update, or delete one text/code/config file through structured content or edits; Rust generates the unified diff. The target may be workspace-relative, absolute, or use @home/@desktop/@documents/@downloads. This works without a workspace when write access allows all locations. Do not use run_command to write files. Applying the generated diff still requires host approval.".to_string();
            } else {
                definition.description.push_str(" Targets may be workspace-relative, absolute, or use @home/@desktop/@documents/@downloads when write access allows all locations.");
            }
            set_schema_property_description(
                &mut definition.input_schema,
                "filePath",
                "Workspace-relative or absolute local file path, or @home/@desktop/@documents/@downloads.",
            );
        }
        if let Some(definition) = definitions
            .iter_mut()
            .find(|definition| definition.name == "run_command")
        {
            set_schema_property_description(
                &mut definition.input_schema,
                "cwd",
                "Workspace-relative or absolute working directory, or @home/@desktop/@documents/@downloads. Required when no workspace exists.",
            );
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

pub(super) fn assemble_initial_context(
    compaction_summary: Option<crate::ContextCompactionSummary>,
    messages: Vec<AgentChatMessage>,
    skill_activation: Option<AgentSkillActivation>,
    attachment_context: AttachmentContext,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: build_system_prompt(context, prompt_preferences, tool_definitions),
        compaction_summary,
        messages,
        skill_activation,
        attachments: ContextAttachments {
            text: attachment_context.text,
            images: attachment_context.images,
        },
    })
}
