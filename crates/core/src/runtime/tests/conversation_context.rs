use super::*;

#[test]
fn conversation_context_state_incremental_updates_match_full_rebuilds() {
    let mut first_user = message("user", "Inspect the project and update src/lib.rs");
    first_user.created_at = Some(1_000);
    let mut state =
        create_conversation_context_state(conversation_context_input(vec![first_user.clone()]))
            .unwrap();

    let narration = ConversationTurnTraceItem::AssistantNarration {
        sequence: 0,
        content: "I will inspect the current implementation first.".to_string(),
        truncated: false,
    };
    let narrated_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration.clone()],
    );
    let narrated_message = traced_assistant_message("", narrated_trace.clone());
    let cursor = state
        .append_trace_items(
            &narrated_trace,
            &narrated_message.conversation_model_context_items,
            0,
        )
        .unwrap();
    assert_eq!(cursor, 1);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![first_user.clone(), narrated_message,])
    );

    let context_call_id =
        crate::llm::model_response_tool_call_id("run-context", 0, 0, "call-context");
    assert_runtime_owned_tool_call_id(&context_call_id);
    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: context_call_id.clone(),
        tool: "read_file".to_string(),
        provenance: crate::AgentToolIdentity::Builtin {
            tool_name: "read_file".to_string(),
        },
        operation: json!({ "path": "src/lib.rs" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: context_call_id,
        tool: "read_file".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: json!({ "path": "src/lib.rs", "endLine": 40 }),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
        archive: Default::default(),
    };
    let closed_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration, call, result],
    );
    let closed_message = traced_assistant_message("", closed_trace.clone());
    let cursor = state
        .append_trace_items(
            &closed_trace,
            &closed_message.conversation_model_context_items,
            cursor,
        )
        .unwrap();
    assert_eq!(cursor, 3);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![first_user.clone(), closed_message,])
    );

    let completed_trace = ConversationTurnTrace {
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        ..closed_trace
    };
    let final_content = "I updated the implementation and verified the tests.";
    state
        .finalize_conversation_turn(
            &completed_trace,
            &traced_assistant_message("", completed_trace.clone()).conversation_model_context_items,
            cursor,
            final_content,
            Some(2_000),
        )
        .unwrap();
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message(final_content, completed_trace.clone()),
        ])
    );

    let follow_up = "Now explain the change.";
    state
        .append_user_message(None, follow_up, Some(3_000))
        .unwrap();
    let mut follow_up_message = message("user", follow_up);
    follow_up_message.created_at = Some(3_000);
    assert_eq!(
        state.snapshot(),
        full_conversation_context_snapshot(vec![
            first_user,
            traced_assistant_message(final_content, completed_trace),
            follow_up_message,
        ])
    );
}

#[test]
fn runtime_shared_baseline_matches_full_context_assembly() {
    let input = conversation_context_input(vec![
        message("user", "First question"),
        current_assistant_history_message("First answer"),
        message("user", "Current question"),
    ]);
    let capabilities =
        prepare_runtime_capabilities(&input, "baseline-test", &[], true, None).unwrap();
    let full = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let baseline = durable_state.shared_baseline().unwrap();
    let shared = build_llm_request(
        input,
        &capabilities.tool_definitions,
        None,
        Some(baseline),
        None,
    )
    .unwrap();

    assert_eq!(shared.context.to_messages(), full.context.to_messages());
}

#[test]
fn activated_skill_is_a_measured_dynamic_overlay_not_a_cache_input() {
    const INSTRUCTIONS: &str = "SKILL_DYNAMIC_MARKER: inspect evidence before editing.";
    let mut input = conversation_context_input(vec![
        message("user", "First question"),
        current_assistant_history_message("First answer"),
        message("user", "Current question"),
    ]);
    input.skill_activation = Some(activated_skill(INSTRUCTIONS));

    let mut changed_selection = input.clone();
    changed_selection.skill_activation = Some(activated_skill(
        "SKILL_CHANGED_MARKER: use a different workflow.",
    ));
    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_selection).unwrap()
    );

    let mut stable_input = input.clone();
    stable_input.skill_activation = None;
    stable_input.skill_discovery = None;
    let capabilities =
        prepare_runtime_capabilities(&stable_input, "skill-overlay", &[], true, None).unwrap();
    let mut full = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let manifest = full.context.manifest();
    let skill_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.sources == vec!["skill_instructions"])
        .unwrap();
    assert_eq!(skill_entry.role, "user");
    assert_eq!(skill_entry.scope, "run");
    assert_eq!(skill_entry.retention, "retained");
    assert_eq!(skill_entry.origin_kind, Some("skill"));
    assert_eq!(skill_entry.origin_id, Some("workspace:workspace-1:review"));
    let messages = full.context.to_messages();
    let skill_index = messages
        .iter()
        .position(|message| message.content().contains(INSTRUCTIONS))
        .unwrap();
    let current_user_index = messages
        .iter()
        .position(|message| message.content().contains("Current question"))
        .unwrap();
    assert!(skill_index > current_user_index);
    assert!(!messages[0].content().contains(INSTRUCTIONS));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let skill_report = detector.inspect(
        &mut full.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    assert_eq!(
        skill_report
            .usage
            .breakdown
            .run_transient
            .context_item_count,
        1
    );
    assert!(skill_report.usage.breakdown.run_transient.input_tokens > 0);

    let mut without_skill = input.clone();
    without_skill.skill_activation = None;
    let mut plain = build_llm_request(
        without_skill.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_skill.context_window_tokens,
        sanitize_max_tokens(without_skill.max_tokens),
    );
    assert_eq!(
        skill_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(skill_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens());

    let plain_preview = inspect_context_window(without_skill).unwrap().unwrap();
    let skill_preview = inspect_context_window(input.clone()).unwrap().unwrap();
    assert!(skill_preview.input_tokens > plain_preview.input_tokens);
    let mut dynamic_tool = capabilities
        .tool_definitions
        .iter()
        .find(|definition| definition.name == "read_file")
        .cloned()
        .unwrap();
    dynamic_tool.name = "skill_dynamic_test_tool".to_string();
    dynamic_tool.description =
        "A deliberately verbose Skill-gated Tool schema used for context capacity testing."
            .to_string();
    let tool_state = json!({
        "stableTools": ["read_file"],
        "dynamicTools": ["skill_dynamic_test_tool"],
    });
    let dynamic_run_world_state = WorldStateSnapshot::new(
        "dynamic-context-preview",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectiveTools,
            WorldStateLifetime::Run,
            tool_state.clone(),
            tool_state,
        )
        .unwrap()],
    )
    .unwrap();
    let dynamic_projection = AgentContextWindowToolProjection::new(
        crate::protocol::AgentRunToolSetCheckpoint {
            stable_revision: "stable-test-revision".to_string(),
            dynamic_revision: "dynamic-test-revision".to_string(),
            effective_revision: "effective-test-revision".to_string(),
            active_capability_ids: Vec::new(),
            exposed_tool_names: vec![
                "read_file".to_string(),
                "skill_dynamic_test_tool".to_string(),
            ],
        },
        dynamic_run_world_state,
        vec![dynamic_tool.clone()],
    );
    let dynamic_preview =
        inspect_context_window_with_tool_projection(input.clone(), &dynamic_projection)
            .unwrap()
            .unwrap();
    assert!(dynamic_preview.input_tokens > skill_preview.input_tokens);

    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let cached_plain = durable_state.snapshot();
    let cached_skill = durable_state
        .snapshot_with_skill_activation(input.skill_activation.as_ref())
        .unwrap();
    let cached_plain_after = durable_state.snapshot();
    assert_eq!(cached_plain, cached_plain_after);
    assert!(cached_skill.input_tokens > cached_plain.input_tokens);
    let cached_dynamic_skill = durable_state
        .snapshot_with_skill_overlays_and_tool_projection(
            input.skill_discovery.as_ref(),
            input.skill_activation.as_ref(),
            &dynamic_projection,
        )
        .unwrap();
    assert!(cached_dynamic_skill.input_tokens > cached_skill.input_tokens);
    let baseline = durable_state.shared_baseline().unwrap();
    let shared = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        Some(baseline),
        None,
    )
    .unwrap();
    assert_eq!(shared.context.to_messages(), full.context.to_messages());

    let debug = format!("{:?}", input.skill_activation);
    assert!(!debug.contains(INSTRUCTIONS));
}

#[test]
fn context_preview_counts_only_host_verified_initial_dynamic_tools() {
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillRevision, SkillSourceId,
    };

    let skill_id = SkillId::parse("workspace:workspace-1:preview-resources").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64))).unwrap();
    let source_id = SkillSourceId::parse("workspace:workspace-1").unwrap();
    let resources = Arc::new(
        memory_resource_session_for_test(
            skill_id.clone(),
            revision.clone(),
            source_id,
            vec![(
                "references/guide.md".to_string(),
                SkillResourceKind::Reference,
                b"verified reference".to_vec(),
            )],
        )
        .unwrap(),
    );
    let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
    let instructions = "Read the verified reference before answering.";
    let mut input = conversation_context_input(vec![message("user", "Inspect the reference")]);
    input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-sha256-v1:preview-resources".to_string(),
        skills: vec![AgentActivatedSkill {
            id: skill_id.to_string(),
            name: "preview-resources".to_string(),
            revision: revision.to_string(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: Some(crate::protocol::AgentActivatedSkillResources {
                root_uri: package.to_string(),
                resource_count: 1,
                kinds: vec!["reference".to_string()],
            }),
        }],
    });

    let without_authority =
        prepare_context_window_tool_projection(&input, &AgentRuntimeHostServices::new(), true)
            .unwrap_err();
    assert!(without_authority
        .to_string()
        .contains("no exact Host package authority"));

    let host_services =
        AgentRuntimeHostServices::new().with_skill_resources(Arc::clone(&resources));
    let projection = prepare_context_window_tool_projection(&input, &host_services, true).unwrap();
    let names = projection
        .dynamic_definitions()
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["skills_list_resources", "skills_read_resource"]);

    let conservative = inspect_context_window(input.clone()).unwrap().unwrap();
    let exact = inspect_context_window_with_tool_projection(input, &projection)
        .unwrap()
        .unwrap();
    assert!(exact.input_tokens > conservative.input_tokens);
}

#[test]
fn discoverable_skill_catalog_is_a_measured_dynamic_overlay_not_a_cache_input() {
    let mut input = conversation_context_input(vec![message("user", "Create a document")]);
    input.skill_discovery = Some(discoverable_skill("Create and verify Word documents."));
    let mut changed_catalog = input.clone();
    changed_catalog.skill_discovery = Some(discoverable_skill("Updated routing metadata."));

    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_catalog).unwrap()
    );

    let capabilities =
        prepare_runtime_capabilities(&input, "skill-discovery-overlay", &[], true, None).unwrap();
    let activation_ref = input.skill_discovery.as_ref().unwrap().skills[0]
        .activation_ref
        .clone();
    let mut with_catalog = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let catalog_entry = with_catalog
        .context
        .manifest()
        .entries
        .into_iter()
        .find(|entry| entry.sources == vec!["skill_catalog"])
        .unwrap();
    assert_eq!(catalog_entry.scope, "run");
    assert_eq!(catalog_entry.retention, "retained");

    let rendered_message = with_catalog
        .context
        .to_messages()
        .into_iter()
        .find(|message| message.content().contains("backend_available_skills"))
        .unwrap();
    let rendered = rendered_message.content();
    assert!(rendered.contains(&format!("\"ref\":\"{activation_ref}\"")));
    assert!(rendered.contains("Create and verify Word documents."));
    assert!(!rendered.contains("bundled:application:documents"));
    assert!(!rendered.contains("skill-package-sha256-v2:"));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let catalog_report = detector.inspect(
        &mut with_catalog.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    let mut without_catalog = input.clone();
    without_catalog.skill_discovery = None;
    let mut plain = build_llm_request(
        without_catalog.clone(),
        &capabilities.tool_definitions,
        None,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_catalog.context_window_tokens,
        sanitize_max_tokens(without_catalog.max_tokens),
    );
    assert_eq!(
        catalog_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(
        catalog_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens()
    );
}
