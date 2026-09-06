use super::*;
use crate::context::ContextCapacityDetector;
use crate::protocol::AgentApiStyle;

fn text(content: &str, source: ContextSource, scope: ContextScope) -> ContextItem {
    ContextItem::text(
        LlmMessageRole::User,
        content,
        source,
        scope,
        ContextRetention::Retained,
    )
}

fn input(content: &str, marked: bool) -> ContextItem {
    let mut metadata = ContextMetadata::new(
        ContextSource::ConversationHistory,
        ContextScope::Conversation,
        ContextRetention::Retained,
    )
    .with_origin(ContextOrigin::conversation_message(content));
    if marked {
        metadata = metadata.with_source(ContextSource::RunInput);
    }
    ContextItem::new(LlmMessage::text(LlmMessageRole::User, content), metadata)
}

fn input_world_diff(marked: bool) -> ContextItem {
    let mut item = text(
        "input world diff",
        ContextSource::WorldStateDiff,
        ContextScope::Conversation,
    )
    .with_origin(ContextOrigin::world_state_record("input-world-diff"));
    if marked {
        item = item.with_source(ContextSource::RunInput);
    }
    item
}

fn attachment() -> ContextItem {
    let mut item = text(
        "input attachment",
        ContextSource::InputAttachment,
        ContextScope::Run,
    )
    .with_source(ContextSource::RunBootstrap);
    item.message
        .images_mut()
        .unwrap()
        .push(crate::llm::LlmImage {
            mime_type: "image/png".to_string(),
            data_base64: "YWJj".to_string(),
        });
    item
}

fn narration(sequence: u64, durable: bool) -> ContextItem {
    ContextItem::new(
        LlmMessage::text(LlmMessageRole::Assistant, format!("narration-{sequence}")),
        trace_metadata(sequence, durable, ContextSource::ModelResponse),
    )
}

fn trace_metadata(sequence: u64, durable: bool, source: ContextSource) -> ContextMetadata {
    ContextMetadata::new(
        if durable {
            ContextSource::ConversationTrace
        } else {
            source
        },
        if durable {
            ContextScope::Conversation
        } else {
            ContextScope::Run
        },
        ContextRetention::Retained,
    )
    .with_origin(ContextOrigin::conversation_trace_item(
        "active-assistant",
        sequence,
    ))
}

fn exchange(sequence: u64, durable: bool) -> Vec<ContextItem> {
    let call_id = format!("call-{sequence}");
    let group = ContextGroup::tool_exchange(&call_id);
    vec![
        ContextItem::assistant(
            format!("call-{sequence}"),
            vec![LlmToolCall {
                id: call_id.clone(),
                name: "fixture".to_string(),
                args: serde_json::json!({}),
            }],
            trace_metadata(sequence, durable, ContextSource::ModelResponse)
                .with_group(group.clone()),
        ),
        ContextItem::tool_result(
            &call_id,
            format!("result-{sequence}"),
            false,
            trace_metadata(sequence, durable, ContextSource::ToolResult).with_group(group),
        ),
    ]
}

fn baseline(extra: bool) -> MeasuredContextBaseline {
    let mut items = vec![
        text(
            "stable",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
        ),
        text(
            "summary",
            ContextSource::ConversationSummary,
            ContextScope::Conversation,
        ),
        input("input-one", false),
        input_world_diff(false),
        input("input-two", false),
        narration(1, true),
    ];
    items.extend(exchange(2, true));
    items.extend(exchange(3, true));
    items.push(ContextItem::new(
        LlmMessage::text(LlmMessageRole::User, "answer"),
        trace_metadata(4, true, ContextSource::UserGuidance),
    ));
    items.push(narration(5, true));
    if extra {
        items.extend(exchange(6, true));
        items.push(narration(7, true));
    }
    let mut frame = ContextFrame::new(items);
    ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[])
        .prepare_frame(&mut frame);
    frame.share_measured_persistent_baseline().unwrap()
}

fn active_frame() -> ContextFrame {
    let mut items = vec![
        text(
            "stable",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
        ),
        text(
            "old history",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
        ),
        input("input-one", true),
        input_world_diff(true),
        input("input-two", true),
        text(
            "initial world",
            ContextSource::WorldStateSnapshot,
            ContextScope::Run,
        )
        .with_source(ContextSource::RunBootstrap),
        attachment(),
        text("catalog", ContextSource::SkillCatalog, ContextScope::Run)
            .with_source(ContextSource::RunBootstrap),
        text(
            "initial skill",
            ContextSource::SkillInstructions,
            ContextScope::Run,
        )
        .with_source(ContextSource::RunBootstrap),
        narration(1, false),
    ];
    items.extend(exchange(2, false));
    items.push(text(
        "live skill",
        ContextSource::SkillInstructions,
        ContextScope::Run,
    ));
    items.push(text(
        "world diff",
        ContextSource::WorldStateDiff,
        ContextScope::Run,
    ));
    items.extend(exchange(3, false));
    items.push(text(
        "resume world",
        ContextSource::WorldStateSnapshot,
        ContextScope::Run,
    ));
    items.push(ContextItem::new(
        LlmMessage::text(LlmMessageRole::User, "answer"),
        trace_metadata(4, false, ContextSource::UserGuidance),
    ));
    items.push(narration(5, false));
    ContextFrame::new(items)
}

fn contents(frame: &ContextFrame) -> Vec<String> {
    frame
        .model_request_items()
        .iter()
        .map(|item| item.message.content().to_string())
        .collect()
}

#[test]
fn compaction_layout_keeps_current_inputs_and_retained_observations_at_causal_boundaries() {
    let original_baseline = baseline(false);
    let frame = active_frame().replace_compacted_model_history(original_baseline.clone());
    assert_eq!(
        contents(&frame),
        [
            "stable",
            "catalog",
            "summary",
            "input-one",
            "input world diff",
            "input-two",
            "input attachment",
            "initial skill",
            "initial world",
            "narration-1",
            "call-2",
            "result-2",
            "live skill",
            "world diff",
            "call-3",
            "result-3",
            "resume world",
            "answer",
            "narration-5",
        ]
    );
    frame.validate_cache_layout().unwrap();
    let request_attachment = frame
        .model_request_items()
        .into_iter()
        .find(|item| item.message.content() == "input attachment")
        .unwrap();
    assert_eq!(
        request_attachment.message.images(),
        attachment().message.images()
    );
    // The backend baseline remains shareable and receives no current-run placement metadata.
    let original = ContextFrame::from_measured_baseline(original_baseline);
    assert!(original
        .model_request_items()
        .iter()
        .all(|item| item.metadata.request_order().is_none()));
    assert!(original.model_request_items().iter().all(|item| !item
        .metadata
        .sources()
        .contains(&ContextSource::RunTimeline)));
    let canonical = frame.to_messages();
    let result_index = canonical
        .iter()
        .position(|message| message.content() == "result-3")
        .unwrap();
    let overlay_index = canonical
        .iter()
        .position(|message| message.content() == "live skill")
        .unwrap();
    assert!(
        result_index < overlay_index,
        "the canonical journal remains before its overlay"
    );
}

#[test]
fn compaction_layout_survives_checkpoint_and_a_second_compaction_with_new_live_items() {
    let frame = active_frame().replace_compacted_model_history(baseline(false));
    let mut restored =
        ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap()).unwrap();
    assert_eq!(contents(&restored), contents(&frame));
    for item in exchange(6, false) {
        restored.push(item);
    }
    restored.push(text(
        "later world diff",
        ContextSource::WorldStateDiff,
        ContextScope::Run,
    ));
    restored.push(narration(7, false));
    let expected = contents(&restored);
    let twice_compacted = restored.replace_compacted_model_history(baseline(true));
    assert_eq!(contents(&twice_compacted), expected);
    let round_trip =
        ContextFrame::from_checkpoint_items(twice_compacted.checkpoint_items().unwrap()).unwrap();
    assert_eq!(contents(&round_trip), expected);
    let contents = contents(&round_trip);
    for unique in [
        "input-one",
        "input world diff",
        "input-two",
        "input attachment",
        "initial skill",
        "initial world",
        "answer",
        "call-2",
        "result-2",
        "live skill",
        "resume world",
        "later world diff",
    ] {
        assert_eq!(
            contents
                .iter()
                .filter(|item| item.as_str() == unique)
                .count(),
            1
        );
    }
    let round_trip_attachment = round_trip
        .model_request_items()
        .into_iter()
        .find(|item| item.message.content() == "input attachment")
        .unwrap();
    assert_eq!(
        round_trip_attachment.message.images(),
        attachment().message.images()
    );
}

#[test]
fn compaction_layout_maps_a_complete_multi_call_turn_to_each_durable_split_exchange() {
    let calls = (1..=2)
        .map(|index| LlmToolCall {
            id: format!("batch-call-{index}"),
            name: "fixture".to_string(),
            args: serde_json::json!({ "index": index }),
        })
        .collect::<Vec<_>>();
    let group = ContextGroup::tool_exchange("complete-batch");
    let active = ContextFrame::new(vec![
        text(
            "stable",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
        ),
        input("input-one", true),
        ContextItem::assistant(
            "both calls",
            calls.clone(),
            trace_metadata(2, false, ContextSource::ModelResponse).with_group(group.clone()),
        ),
        ContextItem::tool_result(
            "batch-call-1",
            "first result",
            false,
            trace_metadata(3, false, ContextSource::ToolResult).with_group(group.clone()),
        ),
        text(
            "skill loaded between calls",
            ContextSource::SkillInstructions,
            ContextScope::Run,
        ),
        ContextItem::tool_result(
            "batch-call-2",
            "second result",
            false,
            trace_metadata(5, false, ContextSource::ToolResult).with_group(group),
        ),
    ]);
    active.validate_complete_tool_protocol().unwrap();
    let mut durable_items = vec![
        text(
            "stable",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
        ),
        input("input-one", false),
    ];
    for (index, call) in calls.into_iter().enumerate() {
        let group = ContextGroup::tool_exchange(&call.id);
        let sequence = 2 + index as u64 * 2;
        let call_id = call.id.clone();
        durable_items.push(ContextItem::assistant(
            if index == 0 { "both calls" } else { "" },
            vec![call],
            trace_metadata(sequence, true, ContextSource::ModelResponse).with_group(group.clone()),
        ));
        durable_items.push(ContextItem::tool_result(
            call_id,
            if index == 0 {
                "first result"
            } else {
                "second result"
            },
            false,
            trace_metadata(sequence + 1, true, ContextSource::ToolResult).with_group(group),
        ));
    }
    let mut durable = ContextFrame::new(durable_items);
    ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[])
        .prepare_frame(&mut durable);
    let replaced = active
        .replace_compacted_model_history(durable.share_measured_persistent_baseline().unwrap());
    let projected = ContextFrame::new(
        replaced
            .model_request_items()
            .into_iter()
            .cloned()
            .collect(),
    );
    projected.validate_complete_tool_protocol().unwrap();
    assert_eq!(
        contents(&replaced),
        [
            "stable",
            "input-one",
            "both calls",
            "first result",
            "skill loaded between calls",
            "",
            "second result",
        ]
    );
    let checkpoint =
        ContextFrame::from_checkpoint_items(replaced.checkpoint_items().unwrap()).unwrap();
    assert_eq!(contents(&checkpoint), contents(&replaced));
}
