use super::*;

#[test]
fn runtime_command_definition_is_fixed_while_dispatch_uses_current_permissions() {
    let definitions = [
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: Default::default(),
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::AutoApprove,
            builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
            builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
        },
    ]
    .into_iter()
    .map(|permissions| {
        let mut input = conversation_context_input(vec![message("user", "run a command")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions,
        });
        let capabilities =
            prepare_runtime_capabilities(&input, "command-definition", &[], true, None).unwrap();
        capabilities
            .initial_tool_set
            .stable_definitions()
            .iter()
            .find(|definition| definition.name == "run_command")
            .cloned()
            .expect("stable run_command definition")
    })
    .collect::<Vec<_>>();

    assert!(definitions[0].requires_approval);
    assert_eq!(
        definitions[0].approval_mode,
        crate::protocol::AgentToolApprovalMode::Always
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[1]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[2]).unwrap()
    );
}

#[test]
fn model_capabilities_do_not_change_tool_definitions_or_context_revision() {
    let mut input = conversation_context_input(vec![message("user", "Inspect the image")]);
    input.model_capabilities.image_input = false;
    let text_only =
        prepare_runtime_capabilities(&input, "model-capabilities-text-only", &[], true, None)
            .unwrap();
    let text_only_revision = conversation_context_configuration_revision(&input).unwrap();

    input.model_capabilities.image_input = true;
    let image_capable =
        prepare_runtime_capabilities(&input, "model-capabilities-image", &[], true, None).unwrap();
    let image_capable_revision = conversation_context_configuration_revision(&input).unwrap();

    assert!(text_only
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert!(image_capable
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert_eq!(
        serde_json::to_value(&text_only.tool_definitions).unwrap(),
        serde_json::to_value(&image_capable.tool_definitions).unwrap()
    );
    assert_eq!(text_only_revision, image_capable_revision);
}

#[test]
fn run_context_changes_only_world_state_while_prompt_preferences_change_configuration() {
    let mut baseline = conversation_context_input(vec![message("user", "Inspect the project")]);
    baseline.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
            builtin_execution: Default::default(),
        },
    });
    let baseline_revision = conversation_context_configuration_revision(&baseline).unwrap();

    let mut changed_runtime = baseline.clone();
    changed_runtime.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-private".to_string()),
        project_id: Some("project-private".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-private".to_string()),
            display_name: Some("Runtime Workspace".to_string()),
            root_path: Some("/Users/example/runtime-workspace".to_string()),
        }),
        attachment_library: Some(AgentAttachmentLibraryContext {
            root_path: Some("/Users/example/runtime-attachments".to_string()),
            conversation_id: Some("conversation-private".to_string()),
            project_id: Some("project-private".to_string()),
            conversation_attachments: Vec::new(),
            project_attachments: Vec::new(),
        }),
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
            builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
        },
    });
    assert_eq!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let host_services = AgentRuntimeHostServices::new();
    let baseline_projection =
        prepare_context_window_tool_projection(&baseline, &host_services, true).unwrap();
    let changed_projection =
        prepare_context_window_tool_projection(&changed_runtime, &host_services, true).unwrap();
    let baseline_world_state = baseline_projection
        .initial_run_world_state()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    let changed_world_state = changed_projection
        .initial_run_world_state()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    assert_ne!(baseline_world_state, changed_world_state);
    assert!(changed_world_state.contains("\"read\":\"all\""));
    assert!(baseline_world_state.contains("\"builtinExecution\":\"require_approval\""));
    assert!(changed_world_state.contains("\"builtinExecution\":\"auto_approve\""));
    assert!(changed_world_state.contains("\"displayName\":\"Runtime Workspace\""));
    assert_eq!(
        changed_world_state.matches("permissions.effective").count(),
        1
    );
    assert_eq!(changed_world_state.matches("workspace.binding").count(), 1);
    assert!(!changed_world_state.contains("<backend_runtime_context>"));
    assert!(!changed_world_state.contains("<backend_dynamic_tool_availability>"));
    assert!(!changed_world_state.contains("/Users/example/runtime-workspace"));
    assert!(!changed_world_state.contains("/Users/example/runtime-attachments"));
    assert!(!changed_world_state.contains("conversation-private"));
    assert!(!changed_world_state.contains("project-private"));
    let projection_debug = format!("{changed_projection:?}");
    assert!(!projection_debug.contains("/Users/example/runtime-workspace"));
    assert!(!projection_debug.contains("/Users/example/runtime-attachments"));

    let baseline_snapshot =
        inspect_context_window_with_tool_projection(baseline.clone(), &baseline_projection)
            .unwrap()
            .unwrap();
    let changed_snapshot =
        inspect_context_window_with_tool_projection(changed_runtime.clone(), &changed_projection)
            .unwrap()
            .unwrap();
    assert!(baseline_snapshot.input_tokens > 0);
    assert!(changed_snapshot.input_tokens > 0);
    assert_ne!(
        baseline_snapshot.input_tokens,
        changed_snapshot.input_tokens
    );

    changed_runtime.prompt_preferences = Some(AgentPromptPreferences {
        work_mode: Some(crate::protocol::AgentPromptWorkMode::General),
        tone: Some(crate::protocol::AgentPromptTone::Friendly),
        detail_level: None,
        custom_instructions: Some("Use concise domain terminology.".to_string()),
        updated_at: Some(10),
        automation_execution_context: None,
    });
    assert_ne!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let mut timestamp_only_change = changed_runtime.clone();
    timestamp_only_change
        .prompt_preferences
        .as_mut()
        .expect("prompt preferences")
        .updated_at = Some(11);
    assert_eq!(
        conversation_context_configuration_revision(&changed_runtime).unwrap(),
        conversation_context_configuration_revision(&timestamp_only_change).unwrap(),
        "presentation-only settings timestamps must not open a new configuration epoch"
    );
}

#[test]
fn collaboration_identity_is_stable_prompt_configuration_not_run_world_state() {
    let mut root = conversation_context_input(vec![message("user", "Inspect the project")]);
    root.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-root".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    let root_revision = conversation_context_configuration_revision(&root).unwrap();

    let mut child = root.clone();
    child.context.as_mut().unwrap().collaboration_identity =
        Some(crate::AgentCollaborationIdentity {
            agent_id: "agent-child".to_string(),
            root_agent_id: "agent-root".to_string(),
            root_conversation_id: "conversation-root".to_string(),
            parent_agent_id: "agent-root".to_string(),
            parent_task_name: "root".to_string(),
            parent_task_path: "/root".to_string(),
            conversation_id: "conversation-child".to_string(),
            task_name: "review".to_string(),
            task_path: "/root/review".to_string(),
            source_agent_id: "agent-root".to_string(),
            source_kind: crate::AgentMailboxKind::Task,
            source_task_name: "root".to_string(),
            source_task_path: "/root".to_string(),
            source_agent_message_id: "mailbox-task-1".to_string(),
            entrusted_task: "Review the change and report evidence.".to_string(),
            template_instructions: Some("Prefer concrete file references.".to_string()),
        });

    assert_ne!(
        root_revision,
        conversation_context_configuration_revision(&child).unwrap(),
        "a trusted child identity changes the stable system-prompt prefix"
    );

    let mut runtime_only_change = child.clone();
    runtime_only_change
        .context
        .as_mut()
        .unwrap()
        .conversation_id = Some("different-runtime-conversation".to_string());
    assert_eq!(
        conversation_context_configuration_revision(&child).unwrap(),
        conversation_context_configuration_revision(&runtime_only_change).unwrap(),
        "ordinary runtime authority remains outside the stable prompt revision"
    );
}

#[test]
fn durable_conversation_sections_are_not_duplicated_in_run_world_state() {
    let mut input = conversation_context_input(vec![message("user", "Inspect the workspace")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-1".to_string()),
            display_name: Some("Workspace".to_string()),
            root_path: Some("/private/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: AgentPermissions::default(),
    });
    let conversation_snapshot = WorldStateSnapshot::new(
        "conversation-world-state",
        0,
        vec![
            crate::world_state::effective_permissions_section(
                AgentPermissions::default(),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
            crate::world_state::workspace_binding_section(
                input
                    .context
                    .as_ref()
                    .and_then(|context| context.workspace.as_ref()),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
            crate::world_state::interaction_profile_section(
                input.prompt_preferences.as_ref(),
                WorldStateLifetime::Conversation,
            )
            .unwrap(),
        ],
    )
    .unwrap();
    input.world_state_records =
        vec![
            AnchoredWorldStateRecord::new(WorldStateRecord::Full(conversation_snapshot), None)
                .unwrap(),
        ];

    let capabilities =
        prepare_runtime_capabilities(&input, "no-duplicate-world-state", &[], true, None).unwrap();
    let tracker = RunWorldStateTracker::new(
        "no-duplicate-world-state",
        &input,
        &capabilities.initial_tool_set,
    )
    .unwrap();
    let section_ids = tracker
        .snapshot()
        .sections
        .iter()
        .map(|section| section.id.clone())
        .collect::<Vec<_>>();
    assert!(section_ids.contains(&WorldStateSectionId::EffectiveTools));
    assert!(section_ids.contains(&WorldStateSectionId::ModelCapabilities));
    assert!(!section_ids.contains(&WorldStateSectionId::EffectivePermissions));
    assert!(!section_ids.contains(&WorldStateSectionId::WorkspaceBinding));
    assert!(!section_ids.contains(&WorldStateSectionId::InteractionProfile));
}

#[test]
fn settings_capability_changes_open_a_stable_epoch_but_secret_rotation_does_not() {
    let input_with_search = |mode, key: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "Find current evidence")]);
        input.search_config = Some(crate::protocol::AgentSearchConfig {
            mode,
            tavily_api_key: key.map(str::to_string),
        });
        input
    };
    let disabled_input = input_with_search(
        crate::protocol::AgentSearchMode::Disabled,
        Some("tvly-disabled"),
    );
    let enabled_a_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-a"),
    );
    let enabled_b_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-b"),
    );

    let prepare = |input: &AgentChatInput, host_actions_available| {
        prepare_runtime_capabilities(
            input,
            "settings-capability-epoch",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
    };
    let disabled = prepare(&disabled_input, true);
    let enabled_a = prepare(&enabled_a_input, true);
    let enabled_b = prepare(&enabled_b_input, false);

    assert!(!disabled.initial_tool_set.contains("web_search"));
    assert!(!disabled.initial_tool_set.contains("web_fetch"));
    assert!(enabled_a.initial_tool_set.contains("web_search"));
    assert!(enabled_a.initial_tool_set.contains("web_fetch"));
    assert_ne!(
        disabled.initial_tool_set.stable_revision(),
        enabled_a.initial_tool_set.stable_revision(),
        "enabling a model-visible settings capability must open a new stable epoch"
    );
    assert_ne!(
        conversation_context_configuration_revision(&disabled_input).unwrap(),
        conversation_context_configuration_revision(&enabled_a_input).unwrap()
    );

    assert_eq!(
        serde_json::to_vec(enabled_a.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(enabled_b.initial_tool_set.stable_definitions()).unwrap(),
        "rotating a ready capability secret must not rewrite the model-visible contract"
    );
    assert_eq!(
        enabled_a.initial_tool_set.stable_revision(),
        enabled_b.initial_tool_set.stable_revision(),
        "neither secret rotation nor Host executor availability may perturb stable tools"
    );
    assert_eq!(
        conversation_context_configuration_revision(&enabled_a_input).unwrap(),
        conversation_context_configuration_revision(&enabled_b_input).unwrap()
    );
    let serialized_definitions =
        serde_json::to_string(enabled_a.initial_tool_set.stable_definitions()).unwrap();
    assert!(!serialized_definitions.contains("tvly-secret-a"));
    assert!(!serialized_definitions.contains("tvly-secret-b"));
}

#[test]
fn composer_permissions_do_not_change_stable_tools_but_denied_writes_still_fail() {
    let input_with_permissions = |permissions| {
        let mut input = conversation_context_input(vec![message("user", "edit a file")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions,
        });
        input
    };
    let default_permissions = AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::RequireApproval,
        builtin_execution: Default::default(),
    };
    let full_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
        builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
    };
    let custom_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::Denied,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::AutoApprove,
        builtin_execution: AgentBuiltinExecutionPermission::AutoApprove,
    };

    let default_input = input_with_permissions(default_permissions);
    let full_input = input_with_permissions(full_permissions);
    let default =
        prepare_runtime_capabilities(&default_input, "stable-default", &[], true, None).unwrap();
    let full = prepare_runtime_capabilities(&full_input, "stable-full", &[], true, None).unwrap();
    let denied_input = input_with_permissions(custom_permissions);
    let custom =
        prepare_runtime_capabilities(&denied_input, "stable-custom", &[], true, None).unwrap();

    let default_bytes = serde_json::to_vec(default.initial_tool_set.stable_definitions()).unwrap();
    assert_eq!(
        default_bytes,
        serde_json::to_vec(full.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default_bytes,
        serde_json::to_vec(custom.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        full.initial_tool_set.stable_revision()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        custom.initial_tool_set.stable_revision()
    );
    let host_services = AgentRuntimeHostServices::new();
    let default_world_state =
        prepare_context_window_tool_projection(&default_input, &host_services, true)
            .unwrap()
            .initial_run_world_state()
            .model_projection(WorldStateLifetime::Run)
            .unwrap()
            .render_sanitized_text();
    let full_world_state =
        prepare_context_window_tool_projection(&full_input, &host_services, true)
            .unwrap()
            .initial_run_world_state()
            .model_projection(WorldStateLifetime::Run)
            .unwrap()
            .render_sanitized_text();
    let custom_world_state =
        prepare_context_window_tool_projection(&denied_input, &host_services, true)
            .unwrap()
            .initial_run_world_state()
            .model_projection(WorldStateLifetime::Run)
            .unwrap()
            .render_sanitized_text();
    assert!(default_world_state.contains("\"builtinExecution\":\"require_approval\""));
    assert!(full_world_state.contains("\"builtinExecution\":\"auto_approve\""));
    assert!(custom_world_state.contains("\"builtinExecution\":\"auto_approve\""));
    for name in ["apply_patch", "run_command"] {
        assert!(
            custom.initial_tool_set.contains(name),
            "{name} must remain in the stable prefix"
        );
    }

    let error = custom
        .tool_registry
        .proposed_action(
            &ToolExecutionContext::from_run_context(denied_input.context.as_ref()),
            &AgentToolCall {
                id: "denied-stable-write".to_string(),
                tool: "apply_patch".to_string(),
                args: json!({
                    "action": "apply",
                    "operation": "create",
                    "filePath": "denied.txt",
                    "observationId": "fobs_not_reached_because_write_is_denied",
                    "content": "must not be written"
                }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        )
        .unwrap_err();
    assert_eq!(error.code(), Some("agent.apply_patch.permission_denied"));
    assert_eq!(error.to_string(), "当前权限不允许修改此文件。");
}

#[test]
fn conversation_identity_does_not_change_the_stable_history_tool() {
    let input_with_conversation = |conversation_id: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "find earlier evidence")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: conversation_id.map(str::to_string),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        });
        input
    };
    let without_input = input_with_conversation(None);
    let without = prepare_runtime_capabilities(
        &without_input,
        "stable-without-conversation",
        &[],
        true,
        None,
    )
    .unwrap();
    let with_input = input_with_conversation(Some("conversation-1"));
    let with =
        prepare_runtime_capabilities(&with_input, "stable-with-conversation", &[], true, None)
            .unwrap();

    assert!(without.initial_tool_set.contains("conversation_history"));
    assert!(with.initial_tool_set.contains("conversation_history"));
    assert_eq!(
        serde_json::to_vec(without.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(with.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        without.initial_tool_set.stable_revision(),
        with.initial_tool_set.stable_revision()
    );

    let result = without.tool_registry.execute(
        &ToolExecutionContext::from_run_context(without_input.context.as_ref()),
        &AgentToolCall {
            id: "history-without-conversation".to_string(),
            tool: "conversation_history".to_string(),
            args: json!({ "action": "search", "query": "evidence" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        },
    );
    assert!(!result.ok);
    assert!(result.error.is_some());
}

#[test]
fn runtime_structured_writers_share_the_file_edit_approval_policy() {
    let definitions = |patch, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "edit a workbook")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                write: crate::protocol::AgentWritePermission::WorkspaceOnly,
                patch,
                ..Default::default()
            },
        });
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        prepare_runtime_capabilities(
            &input,
            "office-definition",
            &[],
            host_actions_available,
            Some(engine),
        )
        .unwrap()
        .tool_definitions
    };

    let manual = definitions(AgentPatchPermission::RequireApproval, true);
    let automatic = definitions(AgentPatchPermission::AutoApprove, true);
    let without_host = definitions(AgentPatchPermission::AutoApprove, false);

    let manual_apply_patch = manual
        .iter()
        .find(|definition| definition.name == "apply_patch")
        .unwrap();
    let automatic_apply_patch = automatic
        .iter()
        .find(|definition| definition.name == "apply_patch")
        .unwrap();
    let apply_patch_without_host = without_host
        .iter()
        .find(|definition| definition.name == "apply_patch")
        .unwrap();
    assert!(manual_apply_patch.requires_approval);
    assert_eq!(
        serde_json::to_value(manual_apply_patch).unwrap(),
        serde_json::to_value(automatic_apply_patch).unwrap()
    );
    assert_eq!(
        serde_json::to_value(manual_apply_patch).unwrap(),
        serde_json::to_value(apply_patch_without_host).unwrap()
    );

    for name in [
        "skills_materialize_resource",
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(
            manual
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
        let automatic = automatic
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert!(!automatic.requires_approval);
        assert_eq!(
            automatic.approval_mode,
            crate::protocol::AgentToolApprovalMode::Never
        );
        assert!(
            without_host
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
    }
}

#[test]
fn write_denied_keeps_stable_writers_but_filters_dynamic_write_only_tools() {
    let mut input = conversation_context_input(vec![message("user", "inspect a workbook")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("workspace".to_string()),
            root_path: Some("/tmp/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            write: crate::protocol::AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
    });
    let engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
    let definitions = prepare_runtime_capabilities(&input, "write-denied", &[], true, Some(engine))
        .unwrap()
        .tool_definitions;

    assert!(definitions
        .iter()
        .any(|definition| definition.name == "apply_patch"));
    assert!(!definitions
        .iter()
        .any(|definition| definition.name == "write_file"));
    assert!(!definitions
        .iter()
        .any(|definition| definition.name == "skills_materialize_resource"));
    for name in [
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
}

#[test]
fn unavailable_tool_errors_distinguish_activation_permissions_and_runtime_capabilities() {
    let registry = ToolRegistry::defaults_with_search(None);
    let definitions = registry.definitions();

    let inactive = registry
        .effective_tool_set(definitions.clone(), &BTreeSet::new())
        .unwrap();
    let activation_error = unavailable_tool_error(&inactive, "office_document");
    assert_eq!(
        activation_error.code(),
        Some("agent.tool_requires_skill_activation")
    );
    assert_eq!(
        activation_error.details().unwrap()["recovery"],
        "activateSkill"
    );

    let document_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    let active_without_engine = registry
        .effective_tool_set(definitions.clone(), &document_capabilities)
        .unwrap();
    let runtime_error = unavailable_tool_error(&active_without_engine, "office_document");
    assert_eq!(
        runtime_error.code(),
        Some("agent.tool_runtime_capability_unavailable")
    );
    assert_eq!(
        runtime_error.details().unwrap()["code"],
        "toolRuntimeCapabilityUnavailable"
    );
    assert_eq!(
        runtime_error.details().unwrap()["recovery"],
        "configureCapability"
    );

    let permitted_without_script_execution = definitions
        .into_iter()
        .filter(|definition| definition.name != "skills_run_script")
        .collect::<Vec<_>>();
    let script_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::SKILL_SCRIPTS_CAPABILITY,
    )]);
    let permission_filtered = registry
        .effective_tool_set(permitted_without_script_execution, &script_capabilities)
        .unwrap();
    let permission_error = unavailable_tool_error(&permission_filtered, "skills_run_script");
    assert_eq!(
        permission_error.code(),
        Some("agent.tool_blocked_by_permissions")
    );
    assert_eq!(
        permission_error.details().unwrap()["recovery"],
        "changePermissions"
    );
    assert_eq!(permission_error.details().unwrap()["bypassAllowed"], false);

    let unknown_error = unavailable_tool_error(&inactive, "invented_tool");
    assert_eq!(unknown_error.code(), Some("agent.tool_not_registered"));
    assert_eq!(
        unknown_error.details().unwrap()["recovery"],
        "useAvailableTool"
    );
}

#[tokio::test]
async fn effective_tool_definitions_are_also_the_execution_allowlist() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                let body_start = request
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                return request[body_start..].to_vec();
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(Vec::new()));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_body(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = request;
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "hidden-tool-call",
                                "type": "function",
                                "function": {
                                    "name": "skills_preflight_script",
                                    "arguments": "{}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "run the hidden tool")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "done");
    let call = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_preflight_script" => {
                Some(call)
            }
            _ => None,
        })
        .expect("the unavailable tool call remains observable");
    assert_runtime_owned_tool_call_id(&call.id);
    assert_eq!(call.reason, None);
    let request = String::from_utf8(second_request.lock().unwrap().clone()).unwrap();
    assert!(request.contains(&call.id));
    assert!(!request.contains("hidden-tool-call"));
    assert!(request.contains("agent.tool_requires_skill_activation"));
    assert!(request.contains("toolRequiresSkillActivation"));
    assert!(request.contains("activateSkill"));
    assert!(request.contains("skill.scripts"));
}

#[tokio::test]
async fn text_only_model_receives_paired_read_image_capability_failure_without_image_payload() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        serde_json::from_slice(&request[body_start..expected_length.unwrap()]).unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will inspect the image.",
                            "tool_calls": [{
                                "id": "read-image-unsupported",
                                "type": "function",
                                "function": {
                                    "name": "read_image",
                                    "arguments": serde_json::to_string(&json!({
                                        "path": "/path/that/must/not/be-read.png"
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "This model cannot inspect images." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "Inspect this image")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.model_capabilities.image_input = false;
    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "This model cannot inspect images.");
    let (call_index, call_id) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                Some((index, call.id.clone()))
            }
            _ => None,
        })
        .expect("read_image tool call event");
    assert_runtime_owned_tool_call_id(&call_id);
    let (result_index, result) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolResult { result, .. } if result.call_id == call_id => {
                Some((index, result))
            }
            _ => None,
        })
        .expect("paired read_image tool result event");
    assert!(call_index < result_index);
    assert!(!result.ok);
    assert_eq!(result.tool, "read_image");
    let structured = result
        .result
        .as_ref()
        .expect("structured capability failure");
    assert_eq!(structured["code"], "modelCapabilityUnsupported");
    assert_eq!(
        structured["errorCode"],
        "agent.model_capability_unsupported"
    );

    let second_request = second_request
        .lock()
        .unwrap()
        .clone()
        .expect("second model request");
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| {
            message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
        })
        .expect("assistant tool call in provider payload");
    let tool_result_index = messages
        .iter()
        .position(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        .expect("paired tool result in provider payload");
    assert!(tool_call_index < tool_result_index);
    assert!(!serde_json::to_string(messages)
        .unwrap()
        .contains("read-image-unsupported"));

    let request = serde_json::to_string(&second_request).unwrap();
    assert!(request.contains("modelCapabilityUnsupported"));
    assert!(request.contains("agent.model_capability_unsupported"));
    assert!(!request.contains("modelCapabilities"));
    assert!(!request.contains("image_url"));
    assert!(!request.contains("data:image/"));
}

#[tokio::test]
async fn image_capable_read_image_round_trip_is_legal_for_openai_and_anthropic() {
    use base64::Engine;
    use image::ImageEncoder;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut body_start = None;
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or_default();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_length = Some(start + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(
            &request[body_start.unwrap()..expected_length.expect("content length")],
        )
        .unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    async fn run_case(style: crate::protocol::AgentApiStyle) {
        let fixture = tempdir().unwrap();
        let image_path = fixture.path().join("pixel.png");
        let mut image_bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut image_bytes)
            .write_image(
                &[0x10, 0x40, 0x90, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        std::fs::write(&image_path, &image_bytes).unwrap();
        let full_image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let second_request = Arc::new(Mutex::new(None::<Value>));
        let captured_second_request = Arc::clone(&second_request);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                if request_index == 1 {
                    *captured_second_request.lock().unwrap() = Some(request);
                }
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "I will inspect the image.",
                                "tool_calls": [{
                                    "id": "read-image-success",
                                    "type": "function",
                                    "function": {
                                        "name": "read_image",
                                        "arguments": "{\"path\":\"pixel.png\"}"
                                    }
                                }]
                            },
                            "finish_reason": "tool_calls"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "image inspected" },
                            "finish_reason": "stop"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [
                            { "type": "text", "text": "I will inspect the image." },
                            {
                                "type": "tool_use",
                                "id": "read-image-success",
                                "name": "read_image",
                                "input": { "path": "pixel.png" }
                            }
                        ],
                        "stop_reason": "tool_use"
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "image inspected" }],
                        "stop_reason": "end_turn"
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let mut input = conversation_context_input(vec![message("user", "Inspect pixel.png")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        freeze_runtime_test_generic_provider(&mut input, "image-capable-round-trip");
        input.stream = Some(false);
        input.model_capabilities.image_input = true;
        input.assistant_message_id = Some("assistant-image".to_string());
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-image".to_string()),
            project_id: Some("project-image".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-image".to_string()),
                display_name: Some("Image workspace".to_string()),
                root_path: Some(fixture.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default().send_chat(input).await.unwrap();
        server.await.unwrap();
        assert_eq!(output.content, "image inspected");

        let call_id = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                    Some(call.id.clone())
                }
                _ => None,
            })
            .expect("read_image call event");
        assert_runtime_owned_tool_call_id(&call_id);
        let event_result = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult { result, .. } if result.call_id == call_id => Some(result),
                _ => None,
            })
            .expect("read_image result event");
        let event_value = event_result.result.as_ref().unwrap();
        assert!(event_value["thumbnailDataUrl"]
            .as_str()
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));
        assert!(event_value.get("image").is_none());

        let trace = output
            .conversation_turn_trace
            .as_ref()
            .expect("terminal conversation trace");
        trace.validate().unwrap();
        assert!(trace.truncated);
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id: trace_call_id,
                truncated: true,
                ..
            } if trace_call_id == &call_id
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: trace_call_id,
                ..
            } if trace_call_id == &call_id
        )));
        let durable = serde_json::to_string(trace).unwrap();
        assert!(!durable.contains(&full_image_base64));
        assert!(!durable.contains("data:image"));
        assert!(!durable.contains("thumbnailDataUrl"));
        assert!(!durable.contains("dataBase64"));

        let request = second_request
            .lock()
            .unwrap()
            .clone()
            .expect("second provider request");
        let messages = request["messages"].as_array().unwrap();
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
                    })
                    .expect("OpenAI assistant tool call");
                let tool_result_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == call_id
                    })
                    .expect("OpenAI paired tool result");
                let image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| part["type"] == "image_url")
                            })
                    })
                    .expect("OpenAI visual input");
                assert!(tool_call_index < tool_result_index && tool_result_index < image_index);
                assert!(messages[image_index]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| {
                        part["image_url"]["url"]
                            == format!("data:image/png;base64,{full_image_base64}")
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts
                                    .iter()
                                    .any(|part| part["type"] == "tool_use" && part["id"] == call_id)
                            })
                    })
                    .expect("Anthropic assistant tool_use");
                let result_and_image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["type"] == "tool_result" && part["tool_use_id"] == call_id
                                }) && parts.iter().any(|part| {
                                    part["type"] == "image"
                                        && part["source"]["data"] == full_image_base64
                                })
                            })
                    })
                    .expect("Anthropic paired tool_result and visual input");
                assert!(tool_call_index < result_and_image_index);
            }
        }
        assert!(!serde_json::to_string(messages)
            .unwrap()
            .contains("read-image-success"));
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[test]
fn runtime_skill_script_definition_respects_the_host_permission_matrix() {
    use crate::protocol::{
        AgentCommandPermission, AgentReadPermission, AgentToolApprovalMode, AgentWorkspaceContext,
        AgentWritePermission,
    };

    let definitions = |write, command, command_safety, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "run a Skill script")]);
        input.context = Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: if command_safety == AgentCommandSafetyPolicy::FullAccess {
                    AgentReadPermission::All
                } else {
                    AgentReadPermission::WorkspaceOnly
                },
                write,
                command,
                command_safety,
                ..Default::default()
            },
        });
        prepare_runtime_capabilities(
            &input,
            "skill-script-definition",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
        .tool_definitions
    };

    let guarded = definitions(
        AgentWritePermission::WorkspaceOnly,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::Guarded,
        true,
    );
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let write_denied = definitions(
        AgentWritePermission::Denied,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let manual = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(manual
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));
    let manual = manual
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(manual.requires_approval);
    assert_eq!(manual.approval_mode, AgentToolApprovalMode::Always);

    let automatic = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    let automatic = automatic
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(automatic.requires_approval);
    assert_eq!(automatic.approval_mode, AgentToolApprovalMode::Always);

    let without_host = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        false,
    );
    let without_host = without_host
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(without_host.requires_approval);
    assert_eq!(without_host.approval_mode, AgentToolApprovalMode::Always);
}
