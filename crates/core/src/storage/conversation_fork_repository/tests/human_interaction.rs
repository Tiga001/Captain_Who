fn frozen_answer() -> crate::human_interaction::HumanInteractionResponseDisplay {
    serde_json::from_value(json!({
        "type":"human_interaction_response","schemaVersion":1,
        "requestId":"request-frozen","responseId":"response-frozen",
        "answers":[{"questionId":"question-frozen","question":"assistant-a",
            "kind":"text","answer":"run-source-0"}]
    }))
    .unwrap()
}

#[test]
fn fork_copies_only_verified_answer_history_within_boundary_and_survives_source_deletion() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let mut source = source_conversation();
    let display = frozen_answer();
    for message in source
        .messages
        .iter_mut()
        .filter(|message| message.role == "user")
    {
        message.content = serde_json::to_string(&display).unwrap();
        // A struct/input property is never accepted as proof by the ordinary writer.
        message.human_interaction_response = Some(display.clone());
    }
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM human_interaction_message_projections",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    for message_id in ["user-a", "user-c"] {
        crate::storage::human_interaction_repository::store_message_projection(
            &connection,
            message_id,
            &display,
        )
        .unwrap();
    }
    let plan =
        build_assistant_reply_fork_plan(&connection, "fork-answer", &source.id, "assistant-b", 90)
            .unwrap();
    let first_answer = plan.message_id_map["user-a"].clone();
    let ordinary_json = plan.message_id_map["user-b"].clone();
    let boundary = plan.message_id_map["assistant-b"].clone();
    commit_fork_plan(&mut connection, &plan).unwrap();
    connection
        .execute("DELETE FROM conversations WHERE id=?1", [&source.id])
        .unwrap();
    let target = chat_repository::get_conversation(&connection, &plan.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(target.messages.len(), 4);
    assert_eq!(
        target
            .messages
            .iter()
            .find(|m| m.id == first_answer)
            .unwrap()
            .human_interaction_response
            .as_ref(),
        Some(&display)
    );
    assert!(target
        .messages
        .iter()
        .find(|m| m.id == ordinary_json)
        .unwrap()
        .human_interaction_response
        .is_none());
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM human_interaction_message_projections",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    let again = build_assistant_reply_fork_plan(
        &connection,
        "fork-answer-again",
        &target.id,
        &boundary,
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &again).unwrap();
    let final_history = chat_repository::get_conversation(&connection, &again.target.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        final_history
            .messages
            .iter()
            .filter(|m| m.human_interaction_response.is_some())
            .count(),
        1
    );
    assert_eq!(
        final_history
            .messages
            .iter()
            .find_map(|m| m.human_interaction_response.as_ref()),
        Some(&display)
    );
}

#[test]
fn fork_and_compaction_preserve_sync_answer_text_without_adding_user_context() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let answer = serde_json::to_value(frozen_answer()).unwrap();
    let mut items = staged_tool_exchange(
        0,
        "sync-call",
        "request_user_input",
        json!({"questions":[{"title":"assistant-a"}]}),
    );
    if let ConversationTurnTraceItem::ToolResult { observation, .. } = &mut items[1] {
        *observation = answer.clone();
    }
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-source-0".into(),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items,
    };
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 20, 21).unwrap();
    let context = vec![
        ConversationModelContextItem {
            sequence: 0,
            ordinal: 0,
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            is_error: false,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "sync-call".into(),
                name: "request_user_input".into(),
                args: json!({"questions":[{"title":"assistant-a"}]}),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "sync-call".into(),
                    runtime_call_id: "sync-call".into(),
                },
            }],
        },
        ConversationModelContextItem {
            sequence: 1,
            ordinal: 0,
            role: "tool".into(),
            content: answer.to_string(),
            tool_call_id: Some("sync-call".into()),
            tool_calls: vec![],
            is_error: false,
        },
    ];
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        &source.id,
        "assistant-a",
        &context,
    )
    .unwrap();
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-sync-answer",
        &source.id,
        "assistant-a",
        90,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let assistant = &plan.message_id_map["assistant-a"];
    let copied_trace = conversation_trace_repository::get_trace_for_message(&connection, assistant)
        .unwrap()
        .unwrap();
    let ConversationTurnTraceItem::ToolResult {
        call_id,
        observation,
        ..
    } = &copied_trace.items[1]
    else {
        panic!("missing original ToolResult")
    };
    assert_ne!(call_id, "sync-call");
    assert_eq!(observation, &answer);
    let ConversationTurnTraceItem::ToolCall { operation, .. } = &copied_trace.items[0] else {
        panic!("missing question call")
    };
    assert_eq!(operation, &json!({"questions":[{"title":"assistant-a"}]}));
    let copied_context =
        conversation_model_context_repository::get_log_for_message(&connection, assistant)
            .unwrap()
            .unwrap();
    assert_eq!(
        copied_context
            .items
            .iter()
            .map(|m| m.role.as_str())
            .collect::<Vec<_>>(),
        vec!["assistant", "tool"]
    );
    assert_eq!(
        serde_json::from_str::<Value>(&copied_context.items[1].content).unwrap(),
        answer
    );
    assert_eq!(copied_context.items[0].tool_calls[0].args, *operation);
    let prefix = context_compaction_repository::prepare_prefix(
        &connection,
        &plan.target.id,
        &ContextJournalCursor::trace_item(assistant, 1),
    )
    .unwrap();
    assert_eq!(prefix.source_items.iter().filter(|item|matches!(item, crate::ContextCompactionSourceItem::Message{role,..} if role=="user")).count(),1);
    assert_eq!(prefix.source_items.iter().filter(|item|matches!(item, crate::ContextCompactionSourceItem::TraceItem{item,..} if matches!(item.as_ref(),ConversationTurnTraceItem::ToolResult{observation,..} if observation==&answer))).count(),1);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM human_interaction_message_projections",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn latest_fork_keeps_idle_backend_postlude_but_reply_fork_stops_before_it() {
    use crate::{ConversationBackendStatePlacement, ConversationTraceSnapshot};
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let mut recorder = crate::conversation_trace::ConversationTraceRecorder::from_durable_snapshot(
        ConversationTraceSnapshot::default(),
    );
    let content = json!({"type":"human_interaction_status","requestId":"question-original","status":"ignored"}).to_string();
    for (sequence, created_at, placement) in [
        (0, 79, ConversationBackendStatePlacement::Timeline),
        (1, 90, ConversationBackendStatePlacement::AfterMessage),
    ] {
        recorder
            .record_backend_state(
                sequence,
                &format!("event-{sequence}"),
                &content,
                created_at,
                placement,
            )
            .unwrap();
    }
    let snapshot = recorder.snapshot();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-source-3".into(),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-d".into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: snapshot.items,
    };
    // The idle postlude occurs after completed_at, so using the last completion for Latest loses it.
    conversation_trace_repository::commit_trace_in_connection(&connection, &trace, 80, 81).unwrap();
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        &source.id,
        "assistant-d",
        &snapshot.model_context_items,
    )
    .unwrap();
    for (request_id, point, expected_count) in [
        ("fork-latest-postlude", ConversationForkPoint::Latest {}, 2),
        (
            "fork-reply-before-postlude",
            ConversationForkPoint::AssistantReply {
                assistant_message_id: "assistant-d".into(),
            },
            1,
        ),
    ] {
        let plan =
            build_fork_plan_at_point(&connection, request_id, &source.id, &point, 100).unwrap();
        commit_fork_plan(&mut connection, &plan).unwrap();
        let target_assistant = &plan.message_id_map["assistant-d"];
        let cloned_trace =
            conversation_trace_repository::get_trace_for_message(&connection, target_assistant)
                .unwrap()
                .unwrap();
        assert_eq!(cloned_trace.items.len(), expected_count);
        let cloned_context = conversation_model_context_repository::get_log_for_message(
            &connection,
            target_assistant,
        )
        .unwrap()
        .unwrap();
        assert_eq!(cloned_context.items.len(), expected_count);
        assert_eq!(cloned_context.items.last().unwrap().content, content);
        if expected_count == 2 {
            assert!(matches!(
                cloned_trace.items[1],
                ConversationTurnTraceItem::BackendState {
                    placement: ConversationBackendStatePlacement::AfterMessage,
                    created_at: 90,
                    ..
                }
            ));
        }
    }
}
