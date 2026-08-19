use super::*;
use crate::tools::AgentToolExposure;

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
    pub(super) builtin_capabilities: Option<crate::BuiltinCapabilityRuntime>,
    pub(super) agent_collaboration_enabled: bool,
}

pub(super) struct DurableConversationTimeline {
    pub(super) compaction_summary: Option<crate::ContextCompactionSummary>,
    pub(super) world_state_records: Vec<crate::AnchoredWorldStateRecord>,
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
            builtin_capabilities: None,
            agent_collaboration_enabled: false,
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
        builtin_capabilities,
        agent_collaboration_enabled,
    } = services;
    let runtime_extensions = RuntimeExtensions::for_run_with_capabilities(
        run_id,
        input.skill_discovery.clone(),
        input.skill_activation.as_ref(),
        skill_activation_resolver,
        skill_resources,
        builtin_capabilities,
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
    if agent_collaboration_enabled {
        tool_registry.register_agent_collaboration_tools();
    }
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

pub(super) fn assemble_context_preview(
    timeline: DurableConversationTimeline,
    skill_discovery: Option<crate::skills::AgentSkillDiscoverySnapshot>,
    skill_activation: Option<AgentSkillActivation>,
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<crate::context::AssembledContext> {
    let DurableConversationTimeline {
        compaction_summary,
        world_state_records,
        messages,
    } = timeline;
    if compaction_summary.is_some()
        || !world_state_records.is_empty()
        || messages.iter().any(|message| {
            matches!(message.role.trim(), "user" | "assistant")
                && !message.content.trim().is_empty()
        })
    {
        return ContextAssembler::assemble_with_timing(ContextAssemblyInput {
            system_prompt: build_system_prompt_with_collaboration(
                prompt_preferences,
                tool_definitions,
                context.and_then(|context| context.collaboration_identity.as_ref()),
            ),
            compaction_summary,
            world_state_records,
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
            build_system_prompt_with_collaboration(
                prompt_preferences,
                tool_definitions,
                context.and_then(|context| context.collaboration_identity.as_ref()),
            ),
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
    let provider_profile_config = input
        .provider_profile_config
        .clone()
        .ok_or_else(|| AgentError::new("Provider profile configuration is missing."))?;
    provider_profile_config
        .validate_for_dialect(provider_dialect)
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
    let (
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        provider_continuation_refs,
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
                Some(restored.provider_continuation_refs),
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
                None,
            )
        }
    };

    Ok(PreparedLlmRequest {
        template,
        context,
        next_model_request_index,
        tool_batch,
        conversation_trace,
        provider_continuation_refs,
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
            "MCP_PRIVATE_CALLS_NEED_REPREPARE count={count}. These additional private MCP calls from the earlier model response were not executed or persisted because each sensitive invocation requires its own one-time preparation and, where applicable, approval. If they are still needed, issue fresh tool calls now, one approval boundary at a time. Do not assume any deferred call ran."
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
    context: Option<&AgentRunContext>,
    prompt_preferences: Option<&AgentPromptPreferences>,
    tool_definitions: &[AgentToolDefinition],
) -> AgentResult<ContextFrame> {
    let DurableConversationTimeline {
        compaction_summary,
        world_state_records,
        messages,
    } = timeline;
    ContextAssembler::assemble(ContextAssemblyInput {
        system_prompt: build_system_prompt_with_collaboration(
            prompt_preferences,
            tool_definitions,
            context.and_then(|context| context.collaboration_identity.as_ref()),
        ),
        compaction_summary,
        world_state_records,
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
    use super::{
        expected_approval_action_id, prepare_runtime_capabilities_with_skills, AgentChatInput,
        RuntimeCapabilityServices,
    };
    use serde_json::json;
    use std::collections::BTreeSet;

    #[test]
    fn mcp_action_identity_is_independent_while_builtin_actions_use_call_identity() {
        assert_eq!(
            expected_approval_action_id(Some("mcp-action-uuid"), "model-call-id"),
            "mcp-action-uuid"
        );
        assert_eq!(
            expected_approval_action_id(None, "model-call-id"),
            "model-call-id"
        );
    }

    fn minimal_input() -> AgentChatInput {
        serde_json::from_value(json!({
            "apiUrl": "https://example.test/v1/chat/completions",
            "apiToken": "secret",
            "model": "model-1",
            "modelCapabilities": { "imageInput": false },
            "messages": []
        }))
        .unwrap()
    }

    fn services(agent_collaboration_enabled: bool) -> RuntimeCapabilityServices {
        RuntimeCapabilityServices {
            host_actions_available: false,
            office_engine: None,
            image_generation_execution: None,
            skill_installation_prepare: None,
            skill_installation_commit: None,
            skill_activation_resolver: None,
            skill_resources: None,
            mcp_tools: None,
            builtin_capabilities: None,
            agent_collaboration_enabled,
        }
    }

    #[test]
    fn runtime_capability_exposes_exactly_six_collaboration_tools_as_one_group() {
        let disabled = prepare_runtime_capabilities_with_skills(
            &minimal_input(),
            "run-disabled",
            &[],
            services(false),
        )
        .unwrap();
        assert!(crate::AGENT_COLLABORATION_TOOL_NAMES.iter().all(|name| {
            disabled
                .tool_definitions
                .iter()
                .all(|definition| definition.name != *name)
        }));

        let enabled = prepare_runtime_capabilities_with_skills(
            &minimal_input(),
            "run-enabled",
            &[],
            services(true),
        )
        .unwrap();
        let actual = enabled
            .tool_definitions
            .iter()
            .filter(|definition| {
                crate::AGENT_COLLABORATION_TOOL_NAMES.contains(&definition.name.as_str())
            })
            .map(|definition| definition.name.as_str())
            .collect::<BTreeSet<_>>();
        let expected = crate::AGENT_COLLABORATION_TOOL_NAMES
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected);
    }
}
