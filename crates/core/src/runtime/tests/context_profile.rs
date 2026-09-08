use super::*;
use crate::protocol::AgentContextProfile;

fn input(profile: AgentContextProfile) -> AgentChatInput {
    let mut input = conversation_context_input(vec![message("user", "你好")]);
    input.prompt_preferences =
        Some(serde_json::from_value(json!({"contextProfile": profile})).unwrap());
    input
}

#[test]
fn minimal_profile_keeps_core_and_extension_entry_points_without_todo() {
    let full =
        prepare_runtime_capabilities(&input(AgentContextProfile::Full), "full", &[], true, None)
            .unwrap();
    let minimal = prepare_runtime_capabilities(
        &input(AgentContextProfile::Minimal),
        "minimal",
        &[],
        true,
        None,
    )
    .unwrap();
    let names = minimal
        .initial_tool_set
        .stable_definitions()
        .iter()
        .map(|tool| tool.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "apply_patch",
            "attachments_list",
            "attachments_list_project",
            "command_session",
            "conversation_history",
            "read_file",
            "read_image",
            "run_command",
            "search_code",
            "search_files",
            "skills_activate",
            "workspace_map"
        ]
    );
    assert!(full.initial_tool_set.contains("todo_update"));
    assert!(!minimal.initial_tool_set.contains("todo_update"));
    let full_again = prepare_runtime_capabilities(
        &input(AgentContextProfile::Full),
        "full-again",
        &[],
        true,
        None,
    )
    .unwrap();
    assert_eq!(
        full.initial_tool_set.revision(),
        full_again.initial_tool_set.revision()
    );
    assert_ne!(
        full.initial_tool_set.stable_revision(),
        minimal.initial_tool_set.stable_revision()
    );
    assert!(minimal
        .initial_tool_set
        .restore_frozen_checkpoint(&full.initial_tool_set.checkpoint())
        .is_err());
    minimal
        .initial_tool_set
        .validate_checkpoint(&minimal.initial_tool_set.checkpoint())
        .unwrap();
}

#[test]
fn both_profiles_measure_the_same_real_prefix_in_requests_previews_and_compaction() {
    let mut measured = Vec::new();
    for profile in [AgentContextProfile::Full, AgentContextProfile::Minimal] {
        let input = input(profile);
        let capabilities =
            prepare_runtime_capabilities(&input, "profile-budget", &[], true, None).unwrap();
        let tools = capabilities.initial_tool_set.stable_definitions();
        let mut request = build_llm_request(input.clone(), tools, None, None, None).unwrap();
        let detector = ContextCapacityDetector::for_model(
            &input.model,
            crate::protocol::AgentApiStyle::OpenAiCompatible,
            tools,
        );
        // This is the detector consumed by the runtime's compaction planner, not a UI-only count.
        let costs = detector
            .inspect(
                &mut request.context,
                input.context_window_tokens,
                sanitize_max_tokens(input.max_tokens),
            )
            .context_cost_breakdown();
        let preview = inspect_context_window(input.clone()).unwrap().unwrap();
        assert_eq!(preview.cost_breakdown.system_tokens, costs.system_tokens);
        assert_eq!(
            preview.cost_breakdown.tool_schema_tokens,
            costs.tool_schema_tokens
        );
        let projection =
            prepare_context_window_tool_projection(&input, &AgentRuntimeHostServices::new(), true)
                .unwrap();
        let state = preview_conversation_state(&projection)
            .split_whitespace()
            .collect::<String>();
        assert!(state.contains(match profile {
            AgentContextProfile::Full => "\"contextProfile\":\"full\"",
            AgentContextProfile::Minimal => "\"contextProfile\":\"minimal\"",
        }));
        assert!(state.contains("contextProfileDescription"));
        eprintln!(
            "context profile {profile:?}: system={}, tools={}, fixed={}",
            costs.system_tokens,
            costs.tool_schema_tokens,
            costs.system_tokens + costs.tool_schema_tokens
        );
        measured.push(costs);
    }
    assert!(measured[1].system_tokens < measured[0].system_tokens * 3 / 4);
    assert!(measured[1].tool_schema_tokens < measured[0].tool_schema_tokens);
    assert_ne!(
        conversation_context_configuration_revision(&input(AgentContextProfile::Full)).unwrap(),
        conversation_context_configuration_revision(&input(AgentContextProfile::Minimal)).unwrap()
    );
}

fn preview_conversation_state(projection: &AgentContextWindowToolProjection) -> String {
    WorldStateSnapshot::new(
        "preview",
        0,
        projection.conversation_preview_sections().to_vec(),
    )
    .unwrap()
    .model_projection(WorldStateLifetime::Conversation)
    .unwrap()
    .render_sanitized_text()
}

#[test]
fn minimal_profile_preserves_skill_catalog_and_read_only_routes() {
    let mut input = input(AgentContextProfile::Minimal);
    input.skill_discovery = Some(discoverable_skill("PROFILE_SKILL_CATALOG_SENTINEL"));
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions {
            write: crate::protocol::AgentWritePermission::Denied,
            ..AgentPermissions::default()
        },
    });
    let capabilities =
        prepare_runtime_capabilities(&input, "minimal-read-only", &[], true, None).unwrap();
    for tool in [
        "skills_activate",
        "workspace_map",
        "search_files",
        "search_code",
        "read_image",
    ] {
        assert!(capabilities.initial_tool_set.contains(tool));
    }
    let request = build_llm_request(
        input.clone(),
        capabilities.initial_tool_set.stable_definitions(),
        None,
        None,
        None,
    )
    .unwrap();
    assert!(request
        .context
        .to_messages()
        .iter()
        .any(|message| message.content().contains("PROFILE_SKILL_CATALOG_SENTINEL")));
    // Skill discovery is a run overlay; it must not change this mode's stable tool fingerprint.
    input.skill_discovery = None;
    let without =
        prepare_runtime_capabilities(&input, "minimal-no-catalog", &[], true, None).unwrap();
    assert_eq!(
        capabilities.initial_tool_set.stable_revision(),
        without.initial_tool_set.stable_revision()
    );
}
