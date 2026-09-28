use super::*;

fn tool_call() -> AgentToolCall {
    AgentToolCall {
        id: "journal-call".into(),
        tool: "read_file".into(),
        args: json!({ "path": "test.txt" }),
        approval_status: AgentApprovalStatus::Approved,
        reason: None,
    }
}

#[test]
fn failed_model_append_rolls_back_tool_call_sequence_and_truncation() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("original").unwrap();
    let before = recorder.checkpoint();
    {
        let mut transaction = recorder.begin_append();
        let sequence = transaction
            .record_tool_call_with_identity(
                &tool_call(),
                AgentToolIdentity::Builtin {
                    tool_name: "read_file".into(),
                },
            )
            .unwrap();
        transaction.mark_truncated();
        assert!(transaction
            .record_model_tool_call_message(
                sequence,
                0,
                &LlmMessage::text(crate::llm::LlmMessageRole::User, "invalid"),
                AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "journal-call".into(),
                    runtime_call_id: "journal-call".into(),
                },
            )
            .is_err());
    }
    assert_eq!(recorder.checkpoint(), before);
}

#[test]
fn panic_during_multi_item_append_restores_the_live_recorder() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("original").unwrap();
    let before = recorder.checkpoint();
    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut transaction = recorder.begin_append();
        transaction
            .record_context_material(
                "new-material",
                ConversationContextMaterialKind::RunWorldState,
                "material",
                &[],
                1,
            )
            .unwrap();
        panic!("injected after paired trace/model append");
    }));
    assert!(failure.is_err());
    assert_eq!(recorder.checkpoint(), before);
}

#[test]
fn failed_result_append_restores_preexisting_call_approval() {
    let mut recorder = ConversationTraceRecorder::default();
    let mut call = tool_call();
    call.approval_status = AgentApprovalStatus::Required;
    recorder.record_tool_call_with_identity(
        &call,
        AgentToolIdentity::Builtin {
            tool_name: "read_file".into(),
        },
    );
    let before = recorder.checkpoint();
    call.approval_status = AgentApprovalStatus::Approved;
    {
        let mut transaction = recorder.begin_append();
        transaction
            .record_tool_result_with_archive(
                &call,
                &AgentToolResult {
                    call_id: call.id.clone(),
                    tool: call.tool.clone(),
                    ok: true,
                    result: Some(json!({ "text": "result" })),
                    error: None,
                    exact_archive_file: None,
                },
                Default::default(),
            )
            .unwrap();
    }
    assert_eq!(recorder.checkpoint(), before);
}

#[test]
fn successful_append_preserves_old_allocations_and_commits_both_journals() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration(&"history ".repeat(1000)).unwrap();
    let before_pointer = match &recorder.items[0] {
        ConversationTurnTraceItem::AssistantNarration { content, .. } => content.as_ptr(),
        _ => unreachable!(),
    };
    let mut transaction = recorder.begin_append();
    transaction
        .record_context_material(
            "new-material",
            ConversationContextMaterialKind::RunWorldState,
            "material",
            &[],
            1,
        )
        .unwrap();
    transaction.commit();
    assert_eq!(recorder.items.len(), 2);
    assert_eq!(recorder.model_context_items.len(), 2);
    match &recorder.items[0] {
        ConversationTurnTraceItem::AssistantNarration { content, .. } => {
            assert_eq!(content.as_ptr(), before_pointer)
        }
        _ => unreachable!(),
    }
}

#[test]
fn incremental_publication_matches_full_projection_across_split_calls_and_failures() {
    let mut recorder = ConversationTraceRecorder::default();
    for index in 0..3 {
        let mut call = tool_call();
        call.id = format!("call-{index}");
        call.approval_status = AgentApprovalStatus::NotRequired;
        recorder.record_tool_call(&call);
        let published_call = recorder.publication();
        assert_eq!(published_call.items, recorder.snapshot().items);
        recorder.record_tool_result(
            &call,
            &AgentToolResult {
                call_id: call.id.clone(),
                tool: call.tool.clone(),
                ok: false,
                result: Some(json!({ "path": "test.txt", "error": "missing" })),
                error: Some("file is missing".into()),
                exact_archive_file: None,
            },
        );
        let publication = recorder.publication();
        assert_eq!(publication.items, recorder.snapshot().items);
        assert_eq!(publication.truncated, recorder.snapshot().truncated);
        assert!(publication
            .parent
            .as_ref()
            .is_some_and(|parent| std::sync::Arc::ptr_eq(parent, &published_call.identity)));
    }
}

#[test]
fn copied_recorders_and_edited_snapshots_cannot_inherit_append_authority() {
    let mut recorder = ConversationTraceRecorder::default();
    recorder.record_narration("original").unwrap();
    let published = recorder.publication();
    let mut independent = recorder.clone();
    independent.record_narration("different branch").unwrap();
    assert!(independent.publication().parent.is_none());
    let mut detached = published.clone().into_snapshot();
    detached.items.clear();
    assert_eq!(published.trace_items.len(), 1);
    assert_eq!(published.items.len(), 1);
    let restored = ConversationTraceRecorder::from_durable_snapshot(detached);
    assert!(restored.publication().parent.is_none());
}
