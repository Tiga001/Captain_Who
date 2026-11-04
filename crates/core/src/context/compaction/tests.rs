use super::*;

fn item(
    index: usize,
    usage_class: ContextUsageClass,
    tokens: u64,
    role: LlmMessageRole,
    source: ContextSource,
    origin: Option<ContextOrigin>,
) -> ContextFramePlanningItem {
    ContextFramePlanningItem {
        index,
        usage_class,
        estimated_tokens: tokens,
        role,
        sources: vec![source],
        group_id: None,
        tool_names: Vec::new(),
        image_count: 0,
        is_error: false,
        origin,
    }
}

fn query(
    status: ContextBudgetStatus,
    available: Option<u64>,
    fixed: u64,
    durable: u64,
    run_transient: u64,
    request_only: u64,
) -> ContextCompactionQuery {
    let total = fixed
        .saturating_add(durable)
        .saturating_add(run_transient)
        .saturating_add(request_only);
    ContextCompactionQuery {
        status,
        available_input_tokens: available,
        request_input_tokens: total,
        additive_input_tokens: total,
        context_revision: 7,
        persistent_revision: 5,
    }
}

#[test]
fn stays_inert_below_the_soft_trigger() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            100,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            300,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            100,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(1_000),
            100,
            400,
            0,
            0,
        ),
        &items,
        true,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::NotRequired);
    assert!(plan.steps.is_empty());
    assert_eq!(plan.soft_trigger_input_tokens, Some(900));
}

#[test]
fn uses_ninety_percent_as_the_fallback_trigger_without_growth_pressure() {
    let planner = ContextCompactionPlanner::for_tools(&[]);
    let below_items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            100,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            799,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
    ];
    let threshold_items = vec![
        below_items[0].clone(),
        item(
            1,
            ContextUsageClass::Durable,
            800,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
    ];

    let below = planner.plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(1_000),
            100,
            799,
            0,
            0,
        ),
        &below_items,
        false,
    );
    let at_threshold = planner.plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(1_000),
            100,
            800,
            0,
            0,
        ),
        &threshold_items,
        false,
    );

    assert_eq!(below.status, ContextCompactionPlanStatus::NotRequired);
    assert_eq!(
        below.request_input_tokens.saturating_mul(100) / below.available_input_tokens.unwrap(),
        89
    );
    assert_eq!(at_threshold.status, ContextCompactionPlanStatus::Required);
    assert_eq!(
        at_threshold.request_input_tokens.saturating_mul(100)
            / at_threshold.available_input_tokens.unwrap(),
        COMPACTION_TRIGGER_PERCENT
    );
}

#[test]
fn small_windows_scale_recent_history_and_summary_budgets_independently() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            1_000,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            2_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old-1")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            2_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old-2")),
        ),
        item(
            3,
            ContextUsageClass::Durable,
            3_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old-3")),
        ),
        item(
            4,
            ContextUsageClass::RunTransient,
            1_000,
            LlmMessageRole::Tool,
            ContextSource::ToolResult,
            None,
        ),
    ];

    // The run item has no journal origin and remains protected. A small window retains
    // a separate recent-history allowance and nonzero summary budget beside that material.
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(10_000),
            1_000,
            7_000,
            1_000,
            0,
        ),
        &items,
        false,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.soft_trigger_input_tokens, Some(9_000));
    assert_eq!(plan.protected.input_tokens, 2_000);
    assert_eq!(plan.target_input_tokens, Some(5_000));
    assert_eq!(plan.required_reclaimed_tokens, 4_000);
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].source_input_tokens, 7_000);
    assert_eq!(plan.steps[0].target_replacement_tokens, 1_000);
    assert_eq!(plan.projected_request_input_tokens, 3_000);
    assert!(plan.request_target_satisfied);
    assert!(!plan.best_effort);
}

#[test]
fn large_prefix_keeps_the_replacement_target_as_a_projection_only() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            10_000,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            200_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::OverBudget,
            Some(210_000),
            10_000,
            200_000,
            0,
            0,
        ),
        &items,
        false,
    );

    let step = &plan.steps[0];
    assert_eq!(step.target_replacement_tokens, 12_000);
    assert_eq!(step.expected_reclaimed_tokens, 188_000);
}

fn uniform_history(
    fixed_tokens: u64,
    messages: usize,
    tokens_per_message: u64,
) -> Vec<ContextFramePlanningItem> {
    let mut items = vec![item(
        0,
        ContextUsageClass::Fixed,
        fixed_tokens,
        LlmMessageRole::System,
        ContextSource::BackendSystemPrompt,
        None,
    )];
    for index in 1..=messages {
        items.push(item(
            index,
            ContextUsageClass::Durable,
            tokens_per_message,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message(format!(
                "assistant-{index}"
            ))),
        ));
    }
    items
}

#[test]
fn fixed_material_does_not_squeeze_the_summary_or_recent_history_allowances() {
    let planner = ContextCompactionPlanner::for_tools(&[]);
    for fixed_tokens in [20_000, 40_000] {
        let items = uniform_history(fixed_tokens, 130, 4_000);
        let plan = planner.plan(
            &query(
                ContextBudgetStatus::WithinBudget,
                Some(600_000),
                fixed_tokens,
                520_000,
                0,
                0,
            ),
            &items,
            false,
        );

        assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
        assert_eq!(plan.protected.input_tokens, fixed_tokens);
        assert_eq!(plan.target_input_tokens, Some(fixed_tokens + 36_000));
        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert_eq!(step.target_replacement_tokens, 12_000);
        assert_eq!(step.source_input_tokens, 496_000);
        // The latest six 4K messages remain verbatim, independently of the fixed prefix.
        assert_eq!(
            plan.compactable_input_tokens - step.source_input_tokens,
            24_000
        );
        assert_eq!(
            step.durable_prefix.as_ref().unwrap().covered_through,
            ContextJournalCursor::message("assistant-124")
        );
        assert_eq!(plan.projected_request_input_tokens, fixed_tokens + 36_000);
        assert!(plan.request_target_satisfied);
        assert!(!plan.best_effort);
    }
}

#[test]
fn low_usage_does_not_compact_merely_because_it_exceeds_the_post_compaction_target() {
    let planner = ContextCompactionPlanner::for_tools(&[]);
    let items = uniform_history(20_000, 50, 4_000);
    let budget = query(
        ContextBudgetStatus::WithinBudget,
        Some(600_000),
        20_000,
        200_000,
        0,
        0,
    );

    for growth_headroom in [0, 20_000, 80_000] {
        let plan = planner.plan_with_headroom(&budget, &items, false, growth_headroom, false);
        assert_eq!(plan.status, ContextCompactionPlanStatus::NotRequired);
        assert!(plan.request_input_tokens > plan.target_input_tokens.unwrap());
        assert!(plan.steps.is_empty());
        assert_eq!(plan.projected_request_input_tokens, 220_000);
    }
}

#[test]
fn observed_batch_growth_triggers_early_without_reducing_retention_budgets() {
    let planner = ContextCompactionPlanner::for_tools(&[]);
    let items = uniform_history(20_000, 115, 4_000);
    let budget = query(
        ContextBudgetStatus::WithinBudget,
        Some(600_000),
        20_000,
        460_000,
        0,
        0,
    );
    let ordinary = planner.plan_with_headroom(&budget, &items, false, 20_000, false);
    let growing = planner.plan_with_headroom(&budget, &items, false, 160_000, false);

    assert_eq!(ordinary.status, ContextCompactionPlanStatus::NotRequired);
    assert_eq!(ordinary.soft_trigger_input_tokens, Some(540_000));
    assert_eq!(growing.status, ContextCompactionPlanStatus::Required);
    assert_eq!(growing.soft_trigger_input_tokens, Some(440_000));
    assert_eq!(ordinary.target_input_tokens, Some(56_000));
    assert_eq!(growing.target_input_tokens, ordinary.target_input_tokens);
    assert_eq!(growing.steps[0].target_replacement_tokens, 12_000);
    assert_eq!(growing.steps[0].source_input_tokens, 436_000);
    assert_eq!(growing.projected_request_input_tokens, 56_000);
}

#[test]
fn very_large_growth_does_not_zero_the_target_or_consume_all_recent_history() {
    let planner = ContextCompactionPlanner::for_tools(&[]);
    let items = uniform_history(10_000, 20, 4_000);
    let plan = planner.plan_with_headroom(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(100_000),
            10_000,
            80_000,
            0,
            0,
        ),
        &items,
        false,
        60_000,
        false,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.soft_trigger_input_tokens, Some(40_000));
    assert_eq!(plan.target_input_tokens, Some(40_000));
    assert_eq!(plan.steps[0].target_replacement_tokens, 10_000);
    assert_eq!(plan.steps[0].source_input_tokens, 60_000);
    assert_eq!(
        plan.compactable_input_tokens - plan.steps[0].source_input_tokens,
        20_000
    );
    assert_eq!(plan.projected_request_input_tokens, 40_000);
    assert!(plan.request_target_satisfied);
}

#[test]
fn emits_one_stable_log_cursor_for_the_selected_raw_prefix() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            1_000,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            3_000,
            LlmMessageRole::User,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("user-old")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            3_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
        item(
            3,
            ContextUsageClass::Durable,
            1_000,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(8_000),
            1_000,
            7_000,
            0,
            0,
        ),
        &items,
        true,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(
        plan.steps[0]
            .durable_prefix
            .as_ref()
            .map(|prefix| &prefix.covered_through),
        Some(&ContextJournalCursor::message("assistant-old"))
    );
}

#[test]
fn world_state_is_exactly_protected_but_does_not_block_message_prefix_compaction() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            500,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            250,
            LlmMessageRole::System,
            ContextSource::WorldStateSnapshot,
            Some(ContextOrigin::world_state_record("epoch:0")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            3_000,
            LlmMessageRole::User,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("user-old")),
        ),
        item(
            3,
            ContextUsageClass::Durable,
            100,
            LlmMessageRole::System,
            ContextSource::WorldStateDiff,
            Some(ContextOrigin::world_state_record("epoch:1")),
        ),
        item(
            4,
            ContextUsageClass::Durable,
            3_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
        item(
            5,
            ContextUsageClass::Durable,
            500,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(7_350),
            500,
            6_850,
            0,
            0,
        ),
        &items,
        true,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.protected.reasons.get("world_state"), Some(&350));
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(
        plan.steps[0].ranges,
        vec![
            ContextCompactionItemRange {
                start_index: 2,
                end_index_exclusive: 3,
            },
            ContextCompactionItemRange {
                start_index: 4,
                end_index_exclusive: 5,
            },
        ]
    );
    assert_eq!(
        plan.steps[0]
            .durable_prefix
            .as_ref()
            .map(|prefix| &prefix.covered_through),
        Some(&ContextJournalCursor::message("assistant-old"))
    );
}

#[test]
fn a_tool_call_and_result_are_selected_as_one_atomic_log_entry() {
    let tool_origin = ContextOrigin::conversation_trace_item("assistant-current", 2);
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            100,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            500,
            LlmMessageRole::Assistant,
            ContextSource::ConversationTrace,
            Some(ContextOrigin::conversation_trace_item(
                "assistant-current",
                0,
            )),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            2_500,
            LlmMessageRole::Assistant,
            ContextSource::ConversationTrace,
            Some(tool_origin.clone()),
        ),
        item(
            3,
            ContextUsageClass::Durable,
            2_500,
            LlmMessageRole::Tool,
            ContextSource::ConversationTrace,
            Some(tool_origin),
        ),
        item(
            4,
            ContextUsageClass::Durable,
            500,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(6_400),
            100,
            6_000,
            0,
            0,
        ),
        &items,
        true,
    );

    let step = &plan.steps[0];
    assert_eq!(step.atomic_unit_count, 2);
    assert_eq!(step.ranges[0].start_index, 1);
    assert_eq!(step.ranges[0].end_index_exclusive, 4);
    assert_eq!(
        step.durable_prefix
            .as_ref()
            .map(|prefix| &prefix.covered_through),
        Some(&ContextJournalCursor::trace_item("assistant-current", 2))
    );
}

#[test]
fn uncommitted_run_overlay_without_a_history_origin_is_not_compactable() {
    let items = vec![item(
        0,
        ContextUsageClass::RunTransient,
        9_000,
        LlmMessageRole::Tool,
        ContextSource::ToolResult,
        None,
    )];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(8_000), 0, 0, 9_000, 0),
        &items,
        false,
    );

    assert_eq!(
        plan.status,
        ContextCompactionPlanStatus::InsufficientCompactableContext
    );
    assert_eq!(plan.compactable_input_tokens, 0);
    assert_eq!(plan.protected.reasons.get("uncommitted_run"), Some(&9_000));
}

#[test]
fn activated_skill_snapshot_has_explicit_compaction_protection() {
    let items = vec![item(
        0,
        ContextUsageClass::RunTransient,
        2_000,
        LlmMessageRole::User,
        ContextSource::SkillInstructions,
        Some(ContextOrigin::skill("workspace:w:review")),
    )];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(1_000), 0, 0, 2_000, 0),
        &items,
        false,
    );

    assert_eq!(
        plan.status,
        ContextCompactionPlanStatus::InsufficientCompactableContext
    );
    assert_eq!(plan.compactable_input_tokens, 0);
    assert_eq!(
        plan.protected.reasons.get("skill_instructions"),
        Some(&2_000)
    );
}

fn historical_material(
    index: usize,
    source: ContextSource,
    tokens: u64,
) -> ContextFramePlanningItem {
    let mut material = item(
        index,
        ContextUsageClass::Durable,
        tokens,
        LlmMessageRole::User,
        source,
        Some(ContextOrigin::conversation_trace_item(
            "assistant-old",
            index as u64,
        )),
    );
    material.sources.push(ContextSource::HistoricalRunContext);
    material
}

#[test]
fn finished_run_material_is_compactable_despite_its_original_attribution_tags() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Durable,
            100,
            LlmMessageRole::User,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("user-old")),
        ),
        historical_material(1, ContextSource::InputAttachment, 1_000),
        historical_material(2, ContextSource::SkillInstructions, 2_000),
        historical_material(3, ContextSource::WorldStateSnapshot, 3_000),
        historical_material(4, ContextSource::WorldStateDiff, 500),
    ];
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(2_000), 0, 6_600, 0, 0),
        &items,
        true,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    // The historical User-role material must never masquerade as the latest user request.
    assert_eq!(plan.protected.reasons.get("current_user"), Some(&100));
    assert_eq!(plan.compactable_input_tokens, 6_500);
    let step = &plan.steps[0];
    assert_eq!(step.source_input_tokens, 6_600);
    assert_eq!(step.retained_input_tokens, 100);
    assert_eq!(
        step.ranges,
        vec![ContextCompactionItemRange {
            start_index: 0,
            end_index_exclusive: 5
        }]
    );
    assert_eq!(
        step.durable_prefix.as_ref().unwrap().covered_through,
        ContextJournalCursor::trace_item("assistant-old", 4)
    );
}

#[test]
fn historical_images_are_retained_bridges_that_do_not_block_later_text_compaction() {
    let mut image = historical_material(1, ContextSource::InputAttachment, 1_000);
    image.image_count = 2;
    let items = vec![
        historical_material(0, ContextSource::SkillInstructions, 2_000),
        image,
        historical_material(2, ContextSource::WorldStateSnapshot, 3_000),
    ];
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(4_000), 0, 6_000, 0, 0),
        &items,
        false,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.compactable_input_tokens, 5_000);
    assert_eq!(plan.protected.input_tokens, 1_000);
    assert_eq!(plan.protected.reasons.get("visual_input"), Some(&1_000));
    let step = &plan.steps[0];
    assert_eq!(step.source_input_tokens, 6_000);
    assert_eq!(step.retained_input_tokens, 1_000);
    assert_eq!(step.target_replacement_tokens, 400);
    assert_eq!(step.expected_reclaimed_tokens, 4_600);
    assert_eq!(
        step.durable_prefix.as_ref().unwrap().covered_through,
        ContextJournalCursor::trace_item("assistant-old", 2)
    );
    assert_eq!(
        step.ranges,
        vec![ContextCompactionItemRange {
            start_index: 0,
            end_index_exclusive: 3
        }]
    );
}

#[test]
fn historical_images_alone_cannot_claim_a_reduction_in_context_size() {
    let mut image = historical_material(0, ContextSource::InputAttachment, 9_000);
    image.image_count = 1;
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(4_000), 0, 9_000, 0, 0),
        &[image],
        false,
    );
    assert_eq!(
        plan.status,
        ContextCompactionPlanStatus::InsufficientCompactableContext
    );
    assert_eq!(plan.compactable_input_tokens, 0);
    assert_eq!(plan.protected.input_tokens, 9_000);
    assert!(plan.steps.is_empty());
}

#[test]
fn another_compaction_does_not_recount_or_recover_an_already_covered_visual_cursor() {
    let mut retained = historical_material(1, ContextSource::InputAttachment, 1_000);
    retained.image_count = 1;
    retained.sources.push(ContextSource::CompactionRetained);
    let mut items = vec![
        item(
            0,
            ContextUsageClass::Durable,
            500,
            LlmMessageRole::Assistant,
            ContextSource::ConversationSummary,
            Some(ContextOrigin::compaction_summary("summary-previous")),
        ),
        retained,
    ];
    let without_new_history = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(1_000), 0, 1_500, 0, 0),
        &items,
        false,
    );
    // The retained old image must not supply a non-advancing cursor for summary-of-summary.
    assert_eq!(
        without_new_history.status,
        ContextCompactionPlanStatus::InsufficientCompactableContext
    );
    assert!(without_new_history.steps.is_empty());

    let mut new_material = historical_material(2, ContextSource::SkillInstructions, 5_000);
    new_material.origin = Some(ContextOrigin::conversation_trace_item("assistant-new", 0));
    items.push(new_material);
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(ContextBudgetStatus::OverBudget, Some(4_000), 0, 6_500, 0, 0),
        &items,
        false,
    );
    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.protected.input_tokens, 1_000);
    assert_eq!(
        plan.protected.reasons.get("compaction_retained"),
        Some(&1_000)
    );
    let step = &plan.steps[0];
    assert_eq!(step.source_input_tokens, 5_500);
    assert_eq!(step.retained_input_tokens, 0);
    assert_eq!(
        step.ranges,
        vec![
            ContextCompactionItemRange {
                start_index: 0,
                end_index_exclusive: 1
            },
            ContextCompactionItemRange {
                start_index: 2,
                end_index_exclusive: 3
            },
        ]
    );
    let prefix = step.durable_prefix.as_ref().unwrap();
    assert_eq!(
        prefix.previous_summary_id.as_deref(),
        Some("summary-previous")
    );
    assert_eq!(
        prefix.covered_through,
        ContextJournalCursor::trace_item("assistant-new", 0)
    );
}

#[test]
fn current_run_material_remains_protected_even_with_a_durable_trace_identity() {
    for (source, reason) in [
        (ContextSource::InputAttachment, "user_attachment"),
        (ContextSource::SkillInstructions, "skill_instructions"),
        (ContextSource::WorldStateSnapshot, "world_state"),
    ] {
        let mut current = historical_material(0, source, 2_000);
        current.usage_class = ContextUsageClass::RunTransient;
        let plan = ContextCompactionPlanner::for_tools(&[]).plan(
            &query(ContextBudgetStatus::OverBudget, Some(1_000), 0, 0, 2_000, 0),
            &[current],
            false,
        );
        assert_eq!(plan.compactable_input_tokens, 0);
        assert_eq!(plan.protected.reasons.get(reason), Some(&2_000));
        assert!(plan.steps.is_empty());
    }
}

#[test]
fn active_material_is_counted_as_exact_retention_when_compacting_later_tool_results() {
    let mut items = vec![item(
        0,
        ContextUsageClass::Durable,
        100,
        LlmMessageRole::User,
        ContextSource::CurrentTurn,
        Some(ContextOrigin::conversation_message("user-current")),
    )];
    for source in [
        ContextSource::InputAttachment,
        ContextSource::SkillInstructions,
        ContextSource::WorldStateSnapshot,
    ] {
        let index = items.len();
        items.push(item(
            index,
            ContextUsageClass::RunTransient,
            200,
            LlmMessageRole::User,
            source,
            Some(ContextOrigin::conversation_trace_item(
                "assistant-current",
                index as u64 - 1,
            )),
        ));
    }
    for (role, source) in [
        (LlmMessageRole::Assistant, ContextSource::ModelResponse),
        (LlmMessageRole::Tool, ContextSource::ToolResult),
    ] {
        let index = items.len();
        let mut tool_item = item(
            index,
            ContextUsageClass::RunTransient,
            4_000,
            role,
            source,
            Some(ContextOrigin::conversation_trace_item(
                "assistant-current",
                index as u64 - 1,
            )),
        );
        tool_item.group_id = Some("tool:current-call".into());
        items.push(tool_item);
    }
    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::OverBudget,
            Some(6_000),
            0,
            100,
            8_600,
            0,
        ),
        &items,
        true,
    );
    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.protected.reasons.get("current_user"), Some(&100));
    for reason in ["user_attachment", "skill_instructions", "world_state"] {
        assert_eq!(plan.protected.reasons.get(reason), Some(&200));
    }
    assert_eq!(plan.compactable_input_tokens, 8_000);
    let step = &plan.steps[0];
    assert_eq!(step.source_input_tokens, 8_700);
    assert_eq!(step.retained_input_tokens, 700);
    assert_eq!(
        step.ranges,
        vec![ContextCompactionItemRange {
            start_index: 0,
            end_index_exclusive: 6
        }]
    );
    assert_eq!(
        step.durable_prefix.as_ref().unwrap().covered_through,
        ContextJournalCursor::trace_item("assistant-current", 4)
    );
}

#[test]
fn latest_user_is_retained_exactly_while_later_closed_run_history_is_compacted() {
    let mut tool_call = item(
        2,
        ContextUsageClass::RunTransient,
        3_000,
        LlmMessageRole::Assistant,
        ContextSource::ModelResponse,
        Some(ContextOrigin::conversation_trace_item(
            "assistant-current",
            0,
        )),
    );
    tool_call.group_id = Some("tool:call-1".to_string());
    let mut tool_result = item(
        3,
        ContextUsageClass::RunTransient,
        3_000,
        LlmMessageRole::Tool,
        ContextSource::ToolResult,
        Some(ContextOrigin::conversation_trace_item(
            "assistant-current",
            1,
        )),
    );
    tool_result.group_id = tool_call.group_id.clone();
    let items = vec![
        item(
            0,
            ContextUsageClass::Durable,
            2_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
        item(
            1,
            ContextUsageClass::Durable,
            1_000,
            LlmMessageRole::User,
            ContextSource::CurrentTurn,
            Some(ContextOrigin::conversation_message("user-current")),
        ),
        tool_call,
        tool_result,
    ];
    let pressure = query(
        ContextBudgetStatus::OverBudget,
        Some(8_000),
        0,
        3_000,
        6_000,
        0,
    );
    let planner = ContextCompactionPlanner::for_tools(&[]);

    let plan = planner.plan(&pressure, &items, true);

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.protected.reasons.get("current_user"), Some(&1_000));
    assert_eq!(plan.compactable_input_tokens, 8_000);
    assert_eq!(
        plan.steps[0]
            .durable_prefix
            .as_ref()
            .map(|prefix| &prefix.covered_through),
        Some(&ContextJournalCursor::trace_item("assistant-current", 1))
    );
}

#[test]
fn five_hundred_closed_tool_exchanges_remain_one_compactable_prefix() {
    let mut items = vec![item(
        0,
        ContextUsageClass::Durable,
        1_000,
        LlmMessageRole::User,
        ContextSource::CurrentTurn,
        Some(ContextOrigin::conversation_message("user-current")),
    )];
    for exchange in 0..500_u64 {
        let call_index = items.len();
        let group = format!("tool:call-{exchange}");
        let mut call = item(
            call_index,
            ContextUsageClass::RunTransient,
            100,
            LlmMessageRole::Assistant,
            ContextSource::ModelResponse,
            Some(ContextOrigin::conversation_trace_item(
                "assistant-current",
                exchange.saturating_mul(2),
            )),
        );
        call.group_id = Some(group.clone());
        let mut result = item(
            call_index + 1,
            ContextUsageClass::RunTransient,
            100,
            LlmMessageRole::Tool,
            ContextSource::ToolResult,
            Some(ContextOrigin::conversation_trace_item(
                "assistant-current",
                exchange.saturating_mul(2).saturating_add(1),
            )),
        );
        result.group_id = Some(group);
        items.push(call);
        items.push(result);
    }

    let planner = ContextCompactionPlanner::for_tools(&[]);
    let plan = planner.plan(
        &query(
            ContextBudgetStatus::OverBudget,
            Some(100_000),
            0,
            1_000,
            100_000,
            0,
        ),
        &items,
        true,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].atomic_unit_count, 401);
    assert_eq!(
        plan.steps[0]
            .durable_prefix
            .as_ref()
            .map(|prefix| &prefix.covered_through),
        Some(&ContextJournalCursor::trace_item("assistant-current", 799))
    );
    assert_eq!(plan.steps[0].target_replacement_tokens, 10_000);
    assert_eq!(plan.projected_request_input_tokens, 31_000);
}

#[test]
fn an_unreachable_target_still_produces_a_best_effort_plan() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            8_500,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            1_500,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-old")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::OverBudget,
            Some(9_000),
            8_500,
            1_500,
            0,
            0,
        ),
        &items,
        false,
    );

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert!(plan.best_effort);
    assert!(plan.planned_reclaimed_tokens > 0);
    assert_eq!(plan.steps[0].target_replacement_tokens, 375);
    assert_eq!(plan.steps[0].expected_reclaimed_tokens, 1_125);
}

#[test]
fn recursive_compaction_carries_the_previous_summary_identity() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            500,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            2_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationSummary,
            Some(ContextOrigin::compaction_summary("summary-1")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            4_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-new")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(6_800),
            500,
            6_000,
            0,
            0,
        ),
        &items,
        false,
    );
    let prefix = plan.steps[0].durable_prefix.as_ref().unwrap();

    assert_eq!(prefix.previous_summary_id.as_deref(), Some("summary-1"));
    assert_eq!(
        prefix.covered_through,
        ContextJournalCursor::message("assistant-new")
    );
}

#[test]
fn recursive_compaction_accepts_the_production_system_summary_role() {
    let items = vec![
        item(
            0,
            ContextUsageClass::Fixed,
            500,
            LlmMessageRole::System,
            ContextSource::BackendSystemPrompt,
            None,
        ),
        item(
            1,
            ContextUsageClass::Durable,
            2_000,
            LlmMessageRole::System,
            ContextSource::ConversationSummary,
            Some(ContextOrigin::compaction_summary("summary-1")),
        ),
        item(
            2,
            ContextUsageClass::Durable,
            4_000,
            LlmMessageRole::Assistant,
            ContextSource::ConversationHistory,
            Some(ContextOrigin::conversation_message("assistant-new")),
        ),
    ];

    let plan = ContextCompactionPlanner::for_tools(&[]).plan(
        &query(
            ContextBudgetStatus::WithinBudget,
            Some(6_800),
            500,
            6_000,
            0,
            0,
        ),
        &items,
        false,
    );
    let prefix = plan.steps[0].durable_prefix.as_ref().unwrap();

    assert_eq!(plan.status, ContextCompactionPlanStatus::Required);
    assert_eq!(prefix.previous_summary_id.as_deref(), Some("summary-1"));
    assert_eq!(
        prefix.covered_through,
        ContextJournalCursor::message("assistant-new")
    );
}
