fn backend_state_content(request_id: &str) -> String {
    json!({"type":"human_interaction_status","requestId":request_id,"status":"ignored"}).to_string()
}

#[test]
fn backend_state_recording_is_atomic_idempotent_and_has_one_exact_model_fact() {
    let content = backend_state_content("request-1");
    let mut recorder = ConversationTraceRecorder::default();
    assert_eq!(
        recorder
            .record_backend_state(
                0,
                "event-1",
                &content,
                20,
                ConversationBackendStatePlacement::Timeline
            )
            .unwrap(),
        Some(content.clone())
    );
    assert_eq!(
        recorder
            .record_backend_state(
                0,
                "event-1",
                &content,
                20,
                ConversationBackendStatePlacement::Timeline
            )
            .unwrap(),
        None
    );
    for (sequence, id, payload, created_at, placement) in [
        (
            1,
            "event-1",
            content.as_str(),
            20,
            ConversationBackendStatePlacement::Timeline,
        ),
        (
            0,
            "event-1",
            content.as_str(),
            21,
            ConversationBackendStatePlacement::Timeline,
        ),
        (
            0,
            "event-1",
            content.as_str(),
            20,
            ConversationBackendStatePlacement::AfterMessage,
        ),
        (
            1,
            "event-2",
            "not JSON",
            20,
            ConversationBackendStatePlacement::Timeline,
        ),
    ] {
        assert!(recorder
            .record_backend_state(sequence, id, payload, created_at, placement)
            .is_err());
    }
    let snapshot = recorder.snapshot();
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.model_context_items.len(), 1);
    assert_eq!(snapshot.model_context_items[0].role, "user");
    assert_eq!(snapshot.model_context_items[0].content, content);
    let trace = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    trace.validate().unwrap();
    trace
        .validate_complete_model_context(&snapshot.model_context_items)
        .unwrap();
    assert!(trace.items[0].is_model_visible());
    assert!(trace.items[0].is_safe_compaction_boundary());
    let mut corrupted = snapshot.model_context_items;
    corrupted.push(ConversationModelContextItem {
        images: Vec::new(),
        ordinal: 1,
        ..corrupted[0].clone()
    });
    assert!(trace.validate_complete_model_context(&corrupted).is_err());
    corrupted.pop();
    corrupted[0].content = backend_state_content("different-request");
    assert!(trace.validate_complete_model_context(&corrupted).is_err());
}

#[test]
fn backend_state_cannot_split_an_unresolved_tool_exchange_or_partially_write() {
    let mut recorder = ConversationTraceRecorder::default();
    record_open_call(&mut recorder, &call("open-call"));
    let before = recorder.checkpoint();
    assert!(recorder
        .record_backend_state(
            recorder.next_sequence(),
            "event",
            &backend_state_content("request"),
            1,
            ConversationBackendStatePlacement::Timeline
        )
        .is_err());
    assert_eq!(recorder.checkpoint(), before);
}

#[test]
fn backend_state_exact_and_fallback_replay_preserve_authority_and_after_message_time() {
    use crate::context::{ContextAssembler, ContextAssemblyInput, ContextAttachments};
    use crate::llm::LlmMessagePlacement;
    let before = backend_state_content("during-run");
    let after = backend_state_content("after-run");
    let mut recorder = ConversationTraceRecorder::default();
    recorder
        .record_backend_state(
            0,
            "before",
            &before,
            1,
            ConversationBackendStatePlacement::Timeline,
        )
        .unwrap();
    recorder
        .record_backend_state(
            1,
            "after",
            &after,
            3,
            ConversationBackendStatePlacement::AfterMessage,
        )
        .unwrap();
    let snapshot = recorder.snapshot();
    let mut trace = snapshot.in_progress_audit_trace("run", "conversation", "assistant");
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Completed;
    let fallback = crate::context::ConversationTraceRenderer::render(&trace).unwrap();
    assert_eq!(
        crate::context::ContextFrame::new(fallback.activity_items).to_messages()[0].content(),
        before
    );
    assert_eq!(
        crate::context::ContextFrame::new(fallback.postlude_items).to_messages()[0].content(),
        after
    );
    assert!(
        crate::context::ConversationTraceRenderer::render_with_model_context(
            &trace,
            &snapshot.model_context_items[..1]
        )
        .is_err()
    );
    for model_context in [snapshot.model_context_items.clone()] {
        let frame = ContextAssembler::assemble(ContextAssemblyInput {
            system_prompt: "rules".to_string(),
            compaction_summary: None,
            world_state_records: Vec::new(),
            initial_run_world_state: None,
            messages: vec![crate::protocol::AgentChatMessage {
                conversation_completion_covered: false,
                message_id: Some("assistant".to_string()),
                role: "assistant".to_string(),
                content: "final reply".to_string(),
                created_at: Some(2),
                conversation_turn_trace: Some(trace.clone()),
                conversation_model_context_items: model_context,
            }],
            skill_discovery: None,
            skill_activation: None,
            attachments: ContextAttachments::default(),
        })
        .unwrap();
        let messages = frame.to_messages();
        let before_index = messages
            .iter()
            .position(|message| message.content() == before)
            .unwrap();
        let final_index = messages
            .iter()
            .position(|message| message.content() == "final reply")
            .unwrap();
        let after_index = messages
            .iter()
            .position(|message| message.content() == after)
            .unwrap();
        assert!(before_index < final_index && final_index < after_index);
        for index in [before_index, after_index] {
            assert_eq!(
                messages[index].placement(),
                LlmMessagePlacement::BackendStateTimeline
            );
            assert_eq!(messages[index].role(), crate::llm::LlmMessageRole::System);
        }
        let restored =
            crate::context::ContextFrame::from_checkpoint_items(frame.checkpoint_items().unwrap())
                .unwrap();
        assert_eq!(restored.to_messages(), messages);
    }
}
