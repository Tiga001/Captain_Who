#[test]
fn fork_copies_trace_history_but_reinitializes_the_publication_epoch() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = source_conversation();
    chat_repository::save_conversation(&mut connection, source.clone()).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-source-0".to_string(),
        conversation_id: source.id.clone(),
        assistant_message_id: "assistant-a".to_string(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items: vec![ConversationTurnTraceItem::AssistantNarration {
            sequence: 0,
            provider_turn_id: None,
            first_tool_call_id: None,
            content: "The same visible history survives a fork.".to_string(),
            truncated: false,
        }],
    };
    conversation_trace_repository::replace_trace(&mut connection, &trace, 20, 20).unwrap();
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-trace-journal",
        &source.id,
        "assistant-a",
        20,
    )
    .unwrap();
    let target_message_id = &plan.message_id_map["assistant-a"];
    commit_fork_plan(&mut connection, &plan).unwrap();
    let journal = |id: &str| {
        connection
            .query_row(
                "SELECT epoch, revision FROM conversation_trace_journal_revisions WHERE assistant_message_id = ?1",
                [id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap()
    };
    let source_journal = journal("assistant-a");
    let target_journal = journal(target_message_id);
    assert_ne!(source_journal.0, target_journal.0);
    assert!(target_journal.1 > 0);
    let copied =
        conversation_trace_repository::get_trace_for_message(&connection, target_message_id)
            .unwrap()
            .unwrap();
    assert_eq!(copied.items, trace.items);
    assert_eq!(copied.terminal_status, trace.terminal_status);
    assert_eq!(copied.conversation_id, plan.target.id);
}
