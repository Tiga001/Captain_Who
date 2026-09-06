use super::*;

#[test]
fn file_transaction_correction_is_ephemeral_and_contains_no_frozen_cursor() {
    let correction = file_transaction_protocol_correction_context_item();
    assert_eq!(
        correction.metadata().retention(),
        ContextRetention::RequestOnly
    );
    assert_eq!(correction.metadata().scope(), ContextScope::Run);
    let messages = ContextFrame::new(vec![correction.clone()]).into_messages();
    assert!(messages[0]
        .content()
        .contains("preceding text-only response was not shown"));
    for field in [
        "transactionId",
        "draftRevision",
        "nextIndex",
        "unresolvedTransactions",
    ] {
        assert!(!messages[0].content().contains(field));
    }
    // A correction is added to a request clone only; it cannot become a retained resume guard.
    assert!(ContextFrame::new(vec![correction])
        .checkpoint_items()
        .is_err());
}

#[test]
fn suppressed_file_transaction_text_becomes_one_ordinary_visibility_fact() {
    let trace = Arc::new(Mutex::new(ConversationTraceRecorder::default()));
    let mut frame = ContextFrame::new(vec![]);
    record_suppressed_file_transaction_narration(
        "run-1",
        3,
        &mut frame,
        &trace,
        None,
        Some("assistant-1"),
    )
    .unwrap();
    let snapshot = trace.lock().unwrap().snapshot();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.model_context_items.len(), 1);
    let ConversationTurnTraceItem::BackendState {
        content, placement, ..
    } = &snapshot.items[0]
    else {
        panic!("suppressed text is a backend fact, not a second tool result or a narration");
    };
    assert_eq!(
        *placement,
        crate::ConversationBackendStatePlacement::Timeline
    );
    assert_eq!(
        serde_json::from_str::<Value>(content).unwrap(),
        json!({
            "type": "assistant_text_visibility", "modelRequestIndex": 3,
            "status": "not_shown", "reason": "file_transaction_unsettled",
        })
    );
    let items = frame.model_request_items();
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].metadata().sources(),
        &[ContextSource::BackendState]
    );
    assert_eq!(
        items[0].metadata().origin().unwrap().journal_cursor(),
        Some(crate::ContextJournalCursor::trace_item("assistant-1", 0))
    );
    assert_eq!(frame.clone().into_messages()[0].content(), content);
    let restored = ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap()).unwrap();
    assert_eq!(restored.into_messages(), frame.into_messages());
    snapshot
        .in_progress_audit_trace("run-1", "chat-1", "assistant-1")
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
}
