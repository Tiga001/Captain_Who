use super::*;
use crate::{
    ContextCompactionGeneration, ContextCompactionPrefix, ContextCompactionSourceItem,
    ContextCompactionSummary, ContextJournalCursor,
};

fn history() -> AgentChatInput {
    let mut user = message("user", "Read the project files.");
    user.message_id = Some("history-user".into());
    user.created_at = Some(1_000);
    conversation_context_input(vec![
        user,
        current_assistant_history_message("Files reviewed."),
    ])
}

fn next_user() -> AgentChatMessage {
    let mut user = message("user", "Continue from the reviewed files.");
    user.message_id = Some("next-user".into());
    user.created_at = Some(3_000);
    user
}

fn add_summary(input: &mut AgentChatInput) {
    let prefix = ContextCompactionPrefix {
        model_source_items: None,
        conversation_id: "conversation-context".into(),
        source_revision: "summary-source".into(),
        covered_through: ContextJournalCursor::message("older-user"),
        previous_summary: None,
        source_items: vec![ContextCompactionSourceItem::Message {
            cursor: ContextJournalCursor::message("older-user"),
            role: "user".into(),
            content: "Keep the established architecture.".into(),
            created_at: 1,
            status: Some("sent".into()),
            terminal_status: None,
            terminal_error: None,
        }],
    };
    input.context_compaction_summary = Some(ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "prepared-summary".into(),
        conversation_id: prefix.conversation_id.clone(),
        source_revision: prefix.source_revision.clone(),
        previous_summary_id: None,
        covered_through: prefix.covered_through.clone(),
        content: "The established architecture must be preserved.".into(),
        continuity: crate::ContextContinuitySnapshot::from_prefix(&prefix).unwrap(),
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 10_000,
        summary_input_tokens: 20,
        continuity_input_tokens: 10,
        uncovered_tail_input_tokens: 0,
        replacement_input_tokens: 20,
        created_at: 500,
    });
}

fn assert_same_request(input: AgentChatInput, baseline: AgentContextBaseline) {
    let stable = prepare_conversation_context(&input).unwrap();
    let bootstrap = WorldStateSnapshot::new(
        "next-run-world",
        0,
        vec![WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::EffectiveTools,
            WorldStateLifetime::Run,
            json!({ "stableTools": ["read_file"] }),
            json!({ "stableTools": ["read_file"] }),
        )
        .unwrap()],
    )
    .unwrap();
    let mut cold = build_llm_request(
        input.clone(),
        &stable.tool_definitions,
        None,
        None,
        Some(bootstrap.clone()),
    )
    .unwrap();
    let mut warm = build_llm_request(
        input.clone(),
        &stable.tool_definitions,
        None,
        Some(baseline),
        Some(bootstrap),
    )
    .unwrap();
    warm.context.validate_cache_layout().unwrap();
    assert_eq!(
        warm.context.clone().into_model_request_messages(),
        cold.context.clone().into_model_request_messages(),
        "warming must preserve exact input, timing, summary and bootstrap order"
    );
    let detector = ContextCapacityDetector::for_model(
        &input.model,
        input.api_style.unwrap(),
        &stable.tool_definitions,
    );
    let estimate = |context: &mut ContextFrame| {
        detector
            .inspect(
                context,
                input.context_window_tokens,
                reserved_output_tokens(&input),
            )
            .snapshot(&input.model)
    };
    assert_eq!(estimate(&mut warm.context), estimate(&mut cold.context));
}

#[test]
fn prepared_history_next_user_preserves_cold_request_and_timing_with_run_overlays() {
    for compacted in [false, true] {
        let mut input = history();
        if compacted {
            add_summary(&mut input);
        }
        let mut state = create_conversation_context_state(input.clone()).unwrap();
        let prepared = state.share_prepared_history().unwrap();
        assert_eq!(
            prepared.configuration_revision(),
            state.configuration_revision()
        );
        let mut adopted = prepared.into_context_state();
        let user = next_user();
        adopted
            .append_user_message(user.message_id.as_deref(), &user.content, user.created_at)
            .unwrap();
        input.messages.push(user);
        input.skill_activation = Some(activated_skill("Read the applicable project instructions."));
        assert_same_request(input, adopted.shared_baseline().unwrap());
    }
}

#[test]
fn prepared_history_independent_adoptions_do_not_retain_another_turn_input() {
    let input = history();
    let mut original = create_conversation_context_state(input.clone()).unwrap();
    let original_snapshot = original.snapshot();
    let prepared = original.share_prepared_history().unwrap();
    let mut first = prepared.clone().into_context_state();
    first
        .append_user_message(Some("first-new-user"), "First distinct input.", Some(3_000))
        .unwrap();
    let mut second = prepared.into_context_state();
    let user = next_user();
    second
        .append_user_message(user.message_id.as_deref(), &user.content, user.created_at)
        .unwrap();
    assert_eq!(original.snapshot(), original_snapshot);
    let mut next = input;
    next.messages.push(user);
    assert_same_request(next, second.shared_baseline().unwrap());
}

#[test]
fn prepared_history_adopts_new_world_state_at_its_exact_input_anchor() {
    let section = |value| {
        WorldStateSectionEnvelope::model_visible(
            WorldStateSectionId::extension("test.current.policy").unwrap(),
            WorldStateLifetime::Conversation,
            json!({ "enabled": value }),
            json!({ "enabled": value }),
        )
        .unwrap()
    };
    let full = WorldStateSnapshot::new("history-world", 0, vec![section(false)]).unwrap();
    let mut input = history();
    input.world_state_records =
        vec![AnchoredWorldStateRecord::new(WorldStateRecord::Full(full.clone()), None).unwrap()];
    let mut state = create_conversation_context_state(input.clone()).unwrap();
    let mut adopted = state.share_prepared_history().unwrap().into_context_state();
    let user = next_user();
    adopted
        .append_user_message(user.message_id.as_deref(), &user.content, user.created_at)
        .unwrap();
    let target = WorldStateSnapshot::new("history-world", 1, vec![section(true)]).unwrap();
    input.world_state_records.push(
        AnchoredWorldStateRecord::new(
            WorldStateRecord::Diff(crate::WorldStateDiff::between(&full, &target).unwrap()),
            user.message_id.clone(),
        )
        .unwrap(),
    );
    adopted
        .sync_conversation_world_state_records(&input.world_state_records)
        .unwrap();
    input.messages.push(user);
    assert_same_request(input, adopted.shared_baseline().unwrap());
}

#[test]
fn prepared_history_rejects_changed_configuration_at_existing_runtime_gate() {
    let mut state = create_conversation_context_state(history()).unwrap();
    let prepared = state.share_prepared_history().unwrap();
    let mut changed = conversation_context_input(vec![next_user()]);
    changed.model = "different-model".into();
    assert_ne!(
        prepared.configuration_revision(),
        conversation_context_configuration_revision(&changed).unwrap()
    );
    let mut adopted = prepared.into_context_state();
    assert_same_request(changed, adopted.shared_baseline().unwrap());
}

#[test]
fn prepared_history_keeps_workflow_delivery_without_replaying_the_last_user_input() {
    let mut input = history();
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-context".into()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    input.assistant_message_id = Some("next-assistant".into());
    let mut state = create_conversation_context_state(input.clone()).unwrap();
    let mut adopted = state.share_prepared_history().unwrap().into_context_state();
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_workflow_delivery(&crate::AgentWorkflowDelivery {
            input_id: "mail-input".into(),
            instance_id: "organization".into(),
            workflow_name: "Organization".into(),
            content: "A colleague requests a review.".into(),
            created_at: 3_000,
            trace_sequence: 0,
        })
        .unwrap();
    let seed = recorder.snapshot();
    let trace = seed.in_progress_audit_trace("next-run", "conversation-context", "next-assistant");
    adopted
        .append_trace_items(&trace, &seed.model_context_items, 0)
        .unwrap();
    input.initial_conversation_trace = Some(seed);
    assert_same_request(input, adopted.shared_baseline().unwrap());
}
