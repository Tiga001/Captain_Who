use super::*;
use crate::context::ContextCapacityDetector;
use crate::llm::LlmImage;
use crate::protocol::AgentApiStyle;
use serde_json::json;

#[test]
fn checkpoint_round_trip_preserves_messages_images_and_metadata() {
    let group = ContextGroup::tool_exchange("exchange-1");
    let mut image_message = LlmMessage::text(LlmMessageRole::User, "inspect image");
    image_message.images.push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "YWJj".to_string(),
    });
    let frame = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::new(
            image_message,
            ContextMetadata::new(
                ContextSource::CurrentTurn,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_source(ContextSource::InputAttachment),
        ),
        ContextItem::assistant(
            "read it",
            vec![LlmToolCall {
                id: "call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "notes.txt" }),
            }],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::tool_result(
            "call-1",
            "contents",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ),
    ]);

    let checkpoint = frame.checkpoint_items().unwrap();
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();

    restored.validate_complete_tool_protocol().unwrap();
    let messages = restored.to_messages();
    assert_eq!(messages[1].images[0].data_base64, "YWJj");
    assert_eq!(messages[2].tool_calls[0].args["path"], "notes.txt");
    assert_eq!(
        serde_json::to_value(frame.manifest()).unwrap(),
        serde_json::to_value(restored.manifest()).unwrap()
    );
}

#[test]
fn checkpoint_round_trip_preserves_skill_snapshot_source_and_origin() {
    let frame = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::text(
            LlmMessageRole::User,
            "<backend_activated_skill>\nSKILL_CHECKPOINT_MARKER\n</backend_activated_skill>",
        ),
        ContextMetadata::new(
            ContextSource::SkillInstructions,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::skill("workspace:w:review")),
    )]);

    let checkpoint = frame.checkpoint_items().unwrap();
    assert_eq!(checkpoint[0].sources, vec!["skill_instructions"]);
    assert_eq!(checkpoint[0].origin.as_ref().unwrap().kind, "skill");
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert_eq!(restored.to_messages(), frame.to_messages());
    assert_eq!(
        serde_json::to_value(restored.manifest()).unwrap(),
        serde_json::to_value(frame.manifest()).unwrap()
    );
}

#[test]
fn checkpoint_rejects_request_only_context() {
    let frame = ContextFrame::new(vec![ContextItem::text(
        LlmMessageRole::User,
        "transient",
        ContextSource::RuntimeExtension,
        ContextScope::Run,
        ContextRetention::RequestOnly,
    )]);

    assert!(frame.checkpoint_items().is_err());
}

#[test]
fn checkpoint_uses_durable_tool_projection_without_mutating_live_context() {
    let frame = ContextFrame::new(vec![ContextItem::tool_result(
        "read-skill-1",
        "RESOURCE_SECRET_MARKER",
        false,
        ContextMetadata::new(
            ContextSource::ToolResult,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    )
    .with_checkpoint_tool_result(
        "read-skill-1",
        r#"{"contentOmittedFromHistory":true}"#,
        false,
    )]);

    assert_eq!(frame.to_messages()[0].content, "RESOURCE_SECRET_MARKER");
    let checkpoint = frame.checkpoint_items().unwrap();
    assert!(!checkpoint[0].content.contains("RESOURCE_SECRET_MARKER"));
    assert!(checkpoint[0].content.contains("contentOmittedFromHistory"));
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert!(!restored.to_messages()[0]
        .content
        .contains("RESOURCE_SECRET_MARKER"));
}

#[test]
fn checkpoint_message_can_remove_transient_model_images_without_changing_live_context() {
    let mut live = LlmMessage::text(
        LlmMessageRole::User,
        "Inspect the generated image at /managed/generated.png.",
    );
    live.images.push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "YWJj".to_string(),
    });
    let durable = LlmMessage::text(
        LlmMessageRole::User,
        "Inspect the generated image at /managed/generated.png.",
    );
    let frame = ContextFrame::new(vec![ContextItem::new(
        live,
        ContextMetadata::new(
            ContextSource::ToolResult,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    )
    .with_checkpoint_message(durable)]);

    assert_eq!(frame.to_messages()[0].images[0].data_base64, "YWJj");
    let checkpoint = frame.checkpoint_items().unwrap();
    assert!(checkpoint[0].images.is_empty());
    assert!(!serde_json::to_string(&checkpoint).unwrap().contains("YWJj"));
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert!(restored.to_messages()[0].images.is_empty());
    assert!(restored.to_messages()[0]
        .content
        .contains("/managed/generated.png"));
}

#[test]
fn persistent_replacement_keeps_run_overlay_and_discards_old_history() {
    let detector =
        ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]);
    let mut replacement = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "new rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "compacted history",
            ContextSource::ConversationSummary,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "current request",
            ContextSource::CurrentTurn,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
    ]);
    detector.prepare_frame(&mut replacement);
    let replacement = replacement.share_measured_persistent_baseline().unwrap();

    let mut active = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "old rules",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "very old history",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "current run narration",
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    ]);
    detector.prepare_frame(&mut active);

    let replaced = active.replace_persistent_baseline(replacement);
    let messages = replaced.to_messages();

    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].content, "new rules");
    assert_eq!(messages[1].content, "compacted history");
    assert_eq!(messages[2].content, "current request");
    assert_eq!(messages[3].content, "current run narration");
    assert!(messages
        .iter()
        .all(|message| !message.content.contains("very old history")));
}

#[test]
fn cache_layout_rejects_a_durable_item_after_the_run_timeline() {
    let frame = ContextFrame::new(vec![
        ContextItem::text(
            LlmMessageRole::System,
            "stable contract",
            ContextSource::BackendSystemPrompt,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "active run output",
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "late durable history",
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        ),
    ]);

    let error = frame.validate_cache_layout().unwrap_err().to_string();
    assert!(error.contains("上下文缓存分层顺序无效"));
    assert!(error.contains("durable_timeline"));
}
