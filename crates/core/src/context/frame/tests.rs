use super::*;
use crate::context::ContextCapacityDetector;
use crate::llm::LlmImage;
use crate::llm::{
    LlmAssistantTurn, LlmRuntimeToolCallBinding, ProviderContinuation,
    ProviderContinuationAttachment, ProviderContinuationFragment, ProviderContinuationPosition,
    ProviderContinuationReplayScope,
};
use crate::protocol::{AgentApiStyle, ProviderContinuationRef};
use crate::provider_profile::{
    ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey,
};
use crate::ProviderContinuationProjection;
use serde_json::json;

#[test]
fn checkpoint_round_trip_preserves_messages_images_and_metadata() {
    let group = ContextGroup::tool_exchange("exchange-1");
    let mut image_message = LlmMessage::text(LlmMessageRole::User, "inspect image");
    image_message.images_mut().unwrap().push(LlmImage {
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
    assert_eq!(messages[1].images()[0].data_base64, "YWJj");
    assert_eq!(
        messages[2].tool_calls().next().unwrap().args["path"],
        "notes.txt"
    );
    assert_eq!(
        serde_json::to_value(frame.manifest()).unwrap(),
        serde_json::to_value(restored.manifest()).unwrap()
    );
}

#[test]
fn encrypted_provider_turn_replaces_split_durable_projection_without_leaking_payload() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-1".to_string()),
    )
    .unwrap();
    let provider_calls = vec![
        LlmToolCall {
            id: "provider-call-a".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "a.txt" }),
        },
        LlmToolCall {
            id: "provider-call-b".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "b.txt" }),
        },
    ];
    let runtime_calls = [
        LlmToolCall {
            id: "runtime-call-a".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "a.txt" }),
        },
        LlmToolCall {
            id: "runtime-call-b".to_string(),
            name: "read_file".to_string(),
            args: json!({ "path": "b.txt" }),
        },
    ];
    let mut turn = LlmAssistantTurn::from_provider(protocol.clone(), "", provider_calls.clone())
        .unwrap()
        .with_runtime_tool_bindings(
            provider_calls
                .iter()
                .zip(runtime_calls.iter().cloned())
                .enumerate()
                .map(|(index, (provider_call, runtime_call))| {
                    LlmRuntimeToolCallBinding::new(index, provider_call, runtime_call)
                })
                .collect(),
        )
        .unwrap();
    let position =
        ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn);
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::InteractionV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            position,
            b"\x01private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let metadata = |sequence| {
        ContextMetadata::new(
            ContextSource::ConversationTrace,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_trace_item(
            "assistant-deepseek",
            sequence,
        ))
    };
    let mut frame = ContextFrame::new(vec![
        ContextItem::assistant(
            "",
            vec![runtime_calls[0].clone()],
            metadata(1).with_group(ContextGroup::tool_exchange("split-a")),
        ),
        ContextItem::tool_result(
            "runtime-call-a",
            "a",
            false,
            metadata(2).with_group(ContextGroup::tool_exchange("split-a")),
        ),
        ContextItem::assistant(
            "",
            vec![runtime_calls[1].clone()],
            metadata(3).with_group(ContextGroup::tool_exchange("split-b")),
        ),
        ContextItem::tool_result(
            "runtime-call-b",
            "b",
            false,
            metadata(4).with_group(ContextGroup::tool_exchange("split-b")),
        ),
    ]);

    frame
        .restore_provider_assistant_turn("assistant-deepseek", None, turn)
        .unwrap();
    frame.validate_complete_tool_protocol().unwrap();
    let messages = frame.to_messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].tool_calls().count(), 2);
    assert!(messages[0]
        .assistant_turn()
        .unwrap()
        .provider_continuation()
        .is_some());
    assert_eq!(frame.provider_continuation_refs().unwrap().len(), 1);
    let checkpoint_json = serde_json::to_string(&frame.checkpoint_items().unwrap()).unwrap();
    assert!(!checkpoint_json.contains("private-reasoning-canary"));
}

#[test]
fn encrypted_ordinary_provider_turn_replaces_visible_history_projection() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-ordinary".to_string()),
    )
    .unwrap();
    let mut turn =
        LlmAssistantTurn::from_provider(protocol.clone(), "visible ordinary answer", Vec::new())
            .unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01ordinary-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let mut frame = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::text(LlmMessageRole::Assistant, "visible ordinary answer"),
        ContextMetadata::new(
            ContextSource::ConversationHistory,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_message("assistant-ordinary")),
    )]);

    frame
        .restore_provider_assistant_turn(
            "assistant-ordinary",
            Some(ProviderContinuationProjection::ConversationMessage),
            turn,
        )
        .unwrap();
    let restored = frame.to_messages();
    assert_eq!(restored.len(), 1);
    let restored_turn = restored[0].assistant_turn().unwrap();
    assert_eq!(restored_turn.visible_text(), "visible ordinary answer");
    assert!(restored_turn.provider_continuation().is_some());
    assert!(restored_turn.provider_continuation_ref().is_some());
    assert_eq!(frame.provider_continuation_refs().unwrap().len(), 1);
    let checkpoint_json = serde_json::to_string(&frame.checkpoint_items().unwrap()).unwrap();
    assert!(!checkpoint_json.contains("ordinary-private-reasoning-canary"));
}

#[test]
fn encrypted_ordinary_provider_turn_binds_to_terminal_message_not_trace_text() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-terminal-owner".to_string()),
    )
    .unwrap();
    let mut turn =
        LlmAssistantTurn::from_provider(protocol.clone(), "provider final answer", Vec::new())
            .unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01terminal-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let mut frame = ContextFrame::new(vec![
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::Assistant, "sanitized trace text"),
            ContextMetadata::new(
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item(
                "assistant-terminal-owner",
                7,
            )),
        ),
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::Assistant, "provider final answer"),
            ContextMetadata::new(
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_message(
                "assistant-terminal-owner",
            )),
        ),
        ContextItem::new(
            LlmMessage::text(
                LlmMessageRole::Assistant,
                "Historical agent activity terminal record",
            ),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_message(
                "assistant-terminal-owner",
            )),
        ),
    ]);

    frame
        .restore_provider_assistant_turn(
            "assistant-terminal-owner",
            Some(ProviderContinuationProjection::ConversationMessage),
            turn,
        )
        .unwrap();
    let restored = frame.to_messages();
    assert_eq!(restored[0].content(), "sanitized trace text");
    let restored_turn = restored[1].assistant_turn().unwrap();
    assert_eq!(
        restored_turn.provider_visible_text(),
        "provider final answer"
    );
    assert!(restored_turn.provider_continuation().is_some());
    assert_eq!(
        restored[2].content(),
        "Historical agent activity terminal record"
    );
}

#[test]
fn encrypted_empty_final_provider_turn_is_inserted_before_terminal_audit() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-empty-terminal".to_string()),
    )
    .unwrap();
    let mut turn = LlmAssistantTurn::from_provider(protocol.clone(), "", Vec::new()).unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01empty-terminal-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let mut frame = ContextFrame::new(vec![ContextItem::new(
        LlmMessage::text(
            LlmMessageRole::Assistant,
            "Historical agent activity terminal record",
        ),
        ContextMetadata::new(
            ContextSource::ConversationTrace,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_message(
            "assistant-empty-terminal",
        )),
    )]);

    frame
        .restore_provider_assistant_turn(
            "assistant-empty-terminal",
            Some(ProviderContinuationProjection::ConversationMessage),
            turn,
        )
        .unwrap();
    let restored = frame.to_messages();
    assert_eq!(restored.len(), 2);
    let restored_turn = restored[0].assistant_turn().unwrap();
    assert_eq!(restored_turn.visible_text(), "");
    assert!(restored_turn.provider_continuation().is_some());
    assert_eq!(
        restored[1].content(),
        "Historical agent activity terminal record"
    );
    let checkpoint_json = serde_json::to_string(&frame.checkpoint_items().unwrap()).unwrap();
    assert!(!checkpoint_json.contains("empty-terminal-private-reasoning-canary"));
}

#[test]
fn encrypted_ordinary_provider_turn_restores_exact_steer_narration_sequence() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-steer-owner".to_string()),
    )
    .unwrap();
    let mut turn = LlmAssistantTurn::from_provider(
        protocol.clone(),
        "provider text is intentionally not used for selection",
        Vec::new(),
    )
    .unwrap();
    turn.set_runtime_visible_text("first sanitized narration");
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01steer-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let trace_item = |sequence, content| {
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::Assistant, content),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item(
                "assistant-steer-owner",
                sequence,
            )),
        )
    };
    let mut frame = ContextFrame::new(vec![
        trace_item(4, "duplicate visible text"),
        trace_item(7, "duplicate visible text"),
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::Assistant, "terminal answer"),
            ContextMetadata::new(
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_message("assistant-steer-owner")),
        ),
    ]);

    frame
        .restore_provider_assistant_turn(
            "assistant-steer-owner",
            Some(ProviderContinuationProjection::ConversationTraceItem {
                sequence: 7,
                ordinal: 0,
            }),
            turn,
        )
        .unwrap();
    let restored = frame.to_messages();
    assert!(restored[0]
        .assistant_turn()
        .unwrap()
        .provider_continuation()
        .is_none());
    assert!(restored[1]
        .assistant_turn()
        .unwrap()
        .provider_continuation()
        .is_some());
    assert!(restored[2]
        .assistant_turn()
        .unwrap()
        .provider_continuation()
        .is_none());
    let checkpoint_json = serde_json::to_string(&frame.checkpoint_items().unwrap()).unwrap();
    assert!(!checkpoint_json.contains("steer-private-reasoning-canary"));
}

#[test]
fn encrypted_empty_provider_turn_restores_immediately_before_exact_steer_boundary() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-empty-steer".to_string()),
    )
    .unwrap();
    let mut turn = LlmAssistantTurn::from_provider(protocol.clone(), "", Vec::new()).unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01empty-steer-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();

    let mut frame = ContextFrame::new(vec![
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::User, "earlier guidance"),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item(
                "assistant-empty-steer",
                3,
            )),
        ),
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::User, "exact guidance"),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item(
                "assistant-empty-steer",
                8,
            )),
        ),
    ]);

    frame
        .restore_provider_assistant_turn(
            "assistant-empty-steer",
            Some(ProviderContinuationProjection::ConversationSteerBoundary {
                guidance_sequence: 8,
            }),
            turn,
        )
        .unwrap();
    let restored = frame.to_messages();
    assert_eq!(restored.len(), 3);
    assert_eq!(restored[0].content(), "earlier guidance");
    let restored_turn = restored[1].assistant_turn().unwrap();
    assert_eq!(restored_turn.visible_text(), "");
    assert!(restored_turn.provider_continuation().is_some());
    assert_eq!(restored[2].content(), "exact guidance");
    let checkpoint_json = serde_json::to_string(&frame.checkpoint_items().unwrap()).unwrap();
    assert!(!checkpoint_json.contains("empty-steer-private-reasoning-canary"));
}

#[test]
fn encrypted_empty_provider_turn_rejects_duplicate_steer_boundaries() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("provider-revision-duplicate-steer".to_string()),
    )
    .unwrap();
    let mut turn = LlmAssistantTurn::from_provider(protocol.clone(), "", Vec::new()).unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::AssistantTurnV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01duplicate-steer-private-reasoning-canary".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();
    let guidance = || {
        ContextItem::new(
            LlmMessage::text(LlmMessageRole::User, "duplicate guidance"),
            ContextMetadata::new(
                ContextSource::ConversationTrace,
                ContextScope::Conversation,
                ContextRetention::Retained,
            )
            .with_origin(ContextOrigin::conversation_trace_item(
                "assistant-duplicate-steer",
                5,
            )),
        )
    };
    let mut frame = ContextFrame::new(vec![guidance(), guidance()]);

    let error = frame
        .restore_provider_assistant_turn(
            "assistant-duplicate-steer",
            Some(ProviderContinuationProjection::ConversationSteerBoundary {
                guidance_sequence: 5,
            }),
            turn,
        )
        .expect_err("duplicate durable guidance must fail closed");
    assert_eq!(error.code(), Some("provider_continuation_corrupt"));
}

#[test]
fn pending_tool_batch_accepts_exact_unresolved_calls_from_one_assistant_turn() {
    let group = ContextGroup::tool_exchange("batch-approval");
    let frame = ContextFrame::new(vec![ContextItem::assistant(
        "I will run both checks.",
        vec![
            LlmToolCall {
                id: "runtime-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "one.txt" }),
            },
            LlmToolCall {
                id: "runtime-call-2".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "two.txt" }),
            },
        ],
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        )
        .with_group(group),
    )]);

    let queued = vec!["runtime-call-2".to_string()];
    let pending = frame
        .validate_pending_tool_batch("runtime-call-1", &queued)
        .unwrap();
    assert_eq!(pending.id, "runtime-call-1");
    assert!(frame
        .validate_pending_tool_batch("runtime-call-1", &[])
        .is_err());
    assert!(frame
        .validate_pending_tool_batch("runtime-call-1", &["other-call".to_string()])
        .is_err());
}

#[test]
fn complete_turn_allows_runtime_context_between_settled_tool_results() {
    let group = ContextGroup::tool_exchange("multi-call-with-runtime-context");
    let mut image_message = LlmMessage::text(LlmMessageRole::User, "first result image");
    image_message.images_mut().unwrap().push(LlmImage {
        mime_type: "image/png".to_string(),
        data_base64: "AA==".to_string(),
    });
    let frame = ContextFrame::new(vec![
        ContextItem::assistant(
            "Inspect both files.",
            vec![
                LlmToolCall {
                    id: "runtime-call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "one.png" }),
                },
                LlmToolCall {
                    id: "runtime-call-2".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "two.txt" }),
                },
            ],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::tool_result(
            "runtime-call-1",
            "first result",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::new(
            image_message,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::text(
            LlmMessageRole::Assistant,
            "runtime extension context",
            ContextSource::RuntimeGuard,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::tool_result(
            "runtime-call-2",
            "second result",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ),
    ]);

    frame.validate_complete_tool_protocol().unwrap();
}

#[test]
fn complete_turn_still_rejects_context_before_its_first_tool_result() {
    let group = ContextGroup::tool_exchange("invalid-pre-result-context");
    let frame = ContextFrame::new(vec![
        ContextItem::assistant(
            "Inspect the file.",
            vec![LlmToolCall {
                id: "runtime-call-1".to_string(),
                name: "read_file".to_string(),
                args: json!({ "path": "one.txt" }),
            }],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::text(
            LlmMessageRole::User,
            "interleaved too early",
            ContextSource::RuntimeGuard,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
        ContextItem::tool_result(
            "runtime-call-1",
            "result",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ),
    ]);

    assert!(frame.validate_complete_tool_protocol().is_err());
}

#[test]
fn complete_turn_rejects_tool_results_out_of_provider_order() {
    let group = ContextGroup::tool_exchange("out-of-order-results");
    let frame = ContextFrame::new(vec![
        ContextItem::assistant(
            "Inspect both files.",
            vec![
                LlmToolCall {
                    id: "runtime-call-1".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "one.txt" }),
                },
                LlmToolCall {
                    id: "runtime-call-2".to_string(),
                    name: "read_file".to_string(),
                    args: json!({ "path": "two.txt" }),
                },
            ],
            ContextMetadata::new(
                ContextSource::ModelResponse,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group.clone()),
        ),
        ContextItem::tool_result(
            "runtime-call-2",
            "second result arrived first",
            false,
            ContextMetadata::new(
                ContextSource::ToolResult,
                ContextScope::Run,
                ContextRetention::Retained,
            )
            .with_group(group),
        ),
    ]);

    assert!(frame.validate_complete_tool_protocol().is_err());
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
        ContextSource::RuntimeTodo,
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

    assert_eq!(frame.to_messages()[0].content(), "RESOURCE_SECRET_MARKER");
    let checkpoint = frame.checkpoint_items().unwrap();
    assert!(!checkpoint[0].content.contains("RESOURCE_SECRET_MARKER"));
    assert!(checkpoint[0].content.contains("contentOmittedFromHistory"));
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert!(!restored.to_messages()[0]
        .content()
        .contains("RESOURCE_SECRET_MARKER"));
}

#[test]
fn checkpoint_message_can_remove_transient_model_images_without_changing_live_context() {
    let mut live = LlmMessage::text(
        LlmMessageRole::User,
        "Inspect the generated image at /managed/generated.png.",
    );
    live.images_mut().unwrap().push(LlmImage {
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

    assert_eq!(frame.to_messages()[0].images()[0].data_base64, "YWJj");
    let checkpoint = frame.checkpoint_items().unwrap();
    assert!(checkpoint[0].images.is_empty());
    assert!(!serde_json::to_string(&checkpoint).unwrap().contains("YWJj"));
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert!(restored.to_messages()[0].images().is_empty());
    assert!(restored.to_messages()[0]
        .content()
        .contains("/managed/generated.png"));
}

#[test]
fn checkpoint_message_can_redact_tool_arguments_without_mutating_live_context() {
    let secret = "MCP_LIVE_ARGUMENT_CANARY";
    let live = LlmMessage::assistant(
        "I will inspect the requested file.",
        vec![LlmToolCall {
            id: "mcp-call-1".to_string(),
            name: "mcp__fixture__read_text_file".to_string(),
            args: json!({ "path": secret }),
        }],
    );
    let frame = ContextFrame::new(vec![ContextItem::new(
        live,
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    )
    .with_checkpoint_tool_calls(vec![LlmToolCall {
        id: "mcp-call-1".to_string(),
        name: "mcp__fixture__read_text_file".to_string(),
        args: json!({}),
    }])]);

    assert_eq!(
        frame.to_messages()[0].tool_calls().next().unwrap().args["path"],
        secret
    );
    let checkpoint = frame.checkpoint_items().unwrap();
    assert_eq!(checkpoint[0].content, "I will inspect the requested file.");
    assert_eq!(checkpoint[0].tool_calls[0].args, json!({}));
    assert!(!serde_json::to_string(&checkpoint).unwrap().contains(secret));
    let restored = ContextFrame::from_checkpoint_items(checkpoint).unwrap();
    assert_eq!(
        restored.to_messages()[0].tool_calls().next().unwrap().args,
        json!({})
    );
}

#[test]
fn checkpoint_message_cannot_change_tool_call_identity() {
    let live = LlmMessage::assistant(
        "",
        vec![LlmToolCall {
            id: "mcp-call-1".to_string(),
            name: "mcp__fixture__read_text_file".to_string(),
            args: json!({ "path": "live-only" }),
        }],
    );
    let durable = LlmMessage::assistant(
        "",
        vec![LlmToolCall {
            id: "different-call".to_string(),
            name: "mcp__fixture__read_text_file".to_string(),
            args: json!({}),
        }],
    );
    let frame = ContextFrame::new(vec![ContextItem::new(
        live,
        ContextMetadata::new(
            ContextSource::ModelResponse,
            ContextScope::Run,
            ContextRetention::Retained,
        ),
    )
    .with_checkpoint_message(durable)]);

    assert!(frame.checkpoint_items().is_err());
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
    assert_eq!(messages[0].content(), "new rules");
    assert_eq!(messages[1].content(), "compacted history");
    assert_eq!(messages[2].content(), "current request");
    assert_eq!(messages[3].content(), "current run narration");
    assert!(messages
        .iter()
        .all(|message| !message.content().contains("very old history")));
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

#[test]
fn provider_turn_hydration_removes_only_owned_narration_with_identical_text() {
    const TEXT: &str = "The same sentence can belong to different responses.";
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        None,
    )
    .unwrap();
    let provider_call = LlmToolCall {
        id: "provider-narration-owner".to_string(),
        name: "read_file".to_string(),
        args: json!({"path":"a.txt"}),
    };
    let runtime_call = LlmToolCall {
        id: "runtime-narration-owner".to_string(),
        ..provider_call.clone()
    };
    let mut turn =
        LlmAssistantTurn::from_provider(protocol.clone(), TEXT, vec![provider_call.clone()])
            .unwrap()
            .with_runtime_tool_bindings(vec![LlmRuntimeToolCallBinding::new(
                0,
                &provider_call,
                runtime_call.clone(),
            )])
            .unwrap();
    let continuation = ProviderContinuation::new(
        protocol,
        ProviderContinuationReplayScope::InteractionV1,
        turn.digest(),
        vec![ProviderContinuationFragment::new(
            ProviderContinuationPosition::new(0, ProviderContinuationAttachment::AssistantTurn),
            b"\x01narration-private".to_vec(),
        )],
    )
    .unwrap();
    turn = turn
        .with_provider_continuation(continuation)
        .unwrap()
        .with_provider_continuation_ref(ProviderContinuationRef::new())
        .unwrap();
    let metadata = |sequence| {
        ContextMetadata::new(
            ContextSource::ConversationTrace,
            ContextScope::Conversation,
            ContextRetention::Retained,
        )
        .with_origin(ContextOrigin::conversation_trace_item(
            "assistant-narration-owner",
            sequence,
        ))
    };
    let narration = |sequence, owner: Option<(&str, &str)>| {
        let mut metadata = metadata(sequence);
        if let Some((owner, first_call)) = owner {
            metadata = metadata
                .with_source(ContextSource::ToolTurnNarration)
                .with_group(ContextGroup::tool_exchange(format!(
                    "narration-owner:{owner}:{first_call}"
                )));
        }
        ContextItem::new(LlmMessage::text(LlmMessageRole::Assistant, TEXT), metadata)
    };
    let mut frame = ContextFrame::new(vec![
        narration(0, None),
        narration(1, Some(("different-provider-turn", "another-call"))),
        // Provider payload identity can repeat; canonical runtime call identity cannot.
        narration(2, Some((&turn.stable_id(), "different-runtime-call"))),
        narration(3, Some((&turn.stable_id(), &runtime_call.id))),
        ContextItem::assistant(
            "",
            vec![runtime_call.clone()],
            metadata(4).with_group(ContextGroup::tool_exchange("split-owned")),
        ),
        ContextItem::tool_result(
            runtime_call.id,
            "file",
            false,
            metadata(5).with_group(ContextGroup::tool_exchange("split-owned")),
        ),
    ]);
    frame
        .restore_provider_assistant_turn("assistant-narration-owner", None, turn)
        .unwrap();
    frame.validate_complete_tool_protocol().unwrap();
    let messages = frame.to_messages();
    assert_eq!(messages.len(), 5);
    assert_eq!(
        messages
            .iter()
            .filter(|message| message.content() == TEXT)
            .count(),
        4
    );
    assert_eq!(messages[0].tool_calls().count(), 0);
    assert_eq!(messages[1].tool_calls().count(), 0);
    assert_eq!(messages[2].tool_calls().count(), 0);
    assert_eq!(messages[3].tool_calls().count(), 1);
    assert_eq!(messages[4].content(), "file");
    assert_eq!(
        frame.iter_items().next().unwrap().metadata.origin(),
        Some(&ContextOrigin::conversation_trace_item(
            "assistant-narration-owner",
            0
        ))
    );
    assert_eq!(
        frame.iter_items().nth(1).unwrap().metadata.origin(),
        Some(&ContextOrigin::conversation_trace_item(
            "assistant-narration-owner",
            1
        ))
    );
}
