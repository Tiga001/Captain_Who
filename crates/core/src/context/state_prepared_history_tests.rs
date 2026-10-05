use super::*;
use crate::llm::{LlmAssistantTurn, LlmMessage};
use crate::{AgentApiStyle, ProviderProfileConfig, ProviderProtocolDialect, ProviderProtocolKey};

#[test]
fn prepared_history_shares_measured_chunks_and_keeps_native_assistant_identity() {
    let profile = ProviderProfileConfig::deepseek_flash_default();
    let protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &profile,
        "deepseek-flash",
        Some("protocol-v1".into()),
    )
    .unwrap();
    let turn =
        LlmAssistantTurn::from_provider(protocol, "Native provider answer.", Vec::new()).unwrap();
    let mut original = AgentConversationContextState::new(
        "config".into(),
        "deepseek-flash".into(),
        Some(128_000),
        1024,
        ContextCapacityDetector::for_model("deepseek-flash", AgentApiStyle::OpenAiCompatible, &[]),
        ContextFrame::new(vec![ContextItem::new(
            LlmMessage::from_assistant_turn(turn),
            ContextMetadata::new(
                ContextSource::ConversationHistory,
                ContextScope::Conversation,
                ContextRetention::Retained,
            ),
        )]),
        ConversationTimingTracker::default(),
    );
    let before = original.snapshot();
    let prepared = original.share_prepared_history().unwrap();
    let mut first = prepared.clone().into_context_state();
    let mut second = prepared.into_context_state();
    let old_item = original.frame.model_request_items()[0];
    assert!(std::ptr::eq(old_item, first.frame.model_request_items()[0]));
    assert!(std::ptr::eq(
        old_item,
        second.frame.model_request_items()[0]
    ));
    assert_eq!(first.frame.to_messages(), original.frame.to_messages());
    assert_eq!(first.snapshot(), before);
    first
        .append_user_message(Some("new-user"), "Next task.", None)
        .unwrap();
    assert_eq!(second.snapshot(), before);
    assert_eq!(original.snapshot(), before);
}

#[test]
fn prepared_history_does_not_share_an_active_publication_cursor() {
    let mut state = AgentConversationContextState::new(
        "config".into(),
        "test-model".into(),
        None,
        0,
        ContextCapacityDetector::for_model("test-model", AgentApiStyle::OpenAiCompatible, &[]),
        ContextFrame::new(Vec::new()),
        ConversationTimingTracker::default(),
    );
    let recorder = crate::ConversationTraceRecorder::default();
    state.seed_trace_publication_cursor(&recorder.publication(), "old-assistant", "old-run", 0);
    assert!(state.trace_publication_cursor.is_some());
    let prepared = state.share_prepared_history().unwrap().into_context_state();
    assert!(prepared.trace_publication_cursor.is_none());
    assert!(state.trace_publication_cursor.is_some());
}
