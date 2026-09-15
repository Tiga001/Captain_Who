fn question_batches_input() -> crate::human_interaction::HumanInteractionToolInput {
    crate::human_interaction::HumanInteractionToolInput {
        questions: vec![
            crate::human_interaction::HumanInteractionQuestionInput {
                title: "Pick a color".into(),
                options: Some(vec!["Red".into(), "Blue".into()]),
            },
            crate::human_interaction::HumanInteractionQuestionInput {
                title: "Any details?".into(),
                options: None,
            },
        ],
    }
}

fn question_call_operation() -> Value {
    json!({"questions":[{"title":"Pick a color","options":["Red","Blue"]},{"title":"Any details?"}]})
}

fn fork_source_with_root(connection: &mut Connection) -> ChatConversationRecord {
    let source = source_conversation();
    chat_repository::save_conversation(connection, source.clone()).unwrap();
    crate::storage::agent_graph_repository::ensure_root_agent(
        connection,
        &crate::EnsureRootAgentInput {
            agent_id: crate::root_agent_id_for_conversation("conversation-source"),
            conversation_id: "conversation-source".into(),
            creation_request_id: "root-create-conversation-source".into(),
            task_name: "Root".into(),
        },
        10,
    )
    .unwrap();
    source
}

fn begin_source_questioning(connection: &Connection, assistant: &str, run: &str, created_at: i64) {
    connection
        .execute(
            "INSERT INTO conversation_turn_traces(assistant_message_id,conversation_id,run_id,schema_version,terminal_status,truncated,created_at,updated_at) VALUES(?1,'conversation-source',?2,?3,'in_progress',0,?4,?4)",
            params![
                assistant,
                run,
                CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                created_at
            ],
        )
        .unwrap();
}

fn admit_async_question(
    connection: &mut Connection,
    assistant: &str,
    run: &str,
    call: &str,
    now: i64,
) -> crate::human_interaction::HumanInteractionRequestSnapshot {
    let owner = crate::storage::human_interaction_repository::HostHumanInteractionOwner {
        agent_id: crate::root_agent_id_for_conversation("conversation-source"),
        conversation_id: "conversation-source".into(),
        run_id: run.into(),
        assistant_message_id: assistant.into(),
        tool_call_id: call.into(),
    };
    crate::storage::human_interaction_repository::create_request(
        connection,
        &owner,
        crate::human_interaction::HumanInteractionMode::Async,
        &question_batches_input(),
        now,
    )
    .unwrap()
}

fn finish_source_trace(
    connection: &Connection,
    assistant: &str,
    run: &str,
    created_at: i64,
    calls: &[&str],
) {
    let mut items = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        items.extend(staged_tool_exchange(
            (index as u64) * 2,
            call,
            "request_user_input_async",
            question_call_operation(),
        ));
    }
    let trace = ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: run.into(),
        conversation_id: "conversation-source".into(),
        assistant_message_id: assistant.into(),
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        terminal_error: None,
        truncated: false,
        items,
    };
    conversation_trace_repository::commit_trace_in_connection(
        connection,
        &trace,
        created_at,
        created_at + 2,
    )
    .unwrap();
}

struct CopiedQuestionRow {
    request_id: String,
    agent_id: String,
    run_id: String,
    assistant_message_id: String,
    tool_call_id: String,
    mode: String,
    status: String,
    revision: u64,
    policy_revision: u64,
    questions_json: String,
    created_at: i64,
    updated_at: i64,
}

fn copied_question_rows(connection: &Connection, conversation_id: &str) -> Vec<CopiedQuestionRow> {
    let mut statement = connection
        .prepare(
            "SELECT request_id,agent_id,run_id,assistant_message_id,tool_call_id,mode,status,revision,policy_revision,questions_json,created_at,updated_at FROM human_interaction_requests WHERE conversation_id=?1 ORDER BY sequence",
        )
        .unwrap();
    let rows = statement
        .query_map([conversation_id], |row| {
            Ok(CopiedQuestionRow {
                request_id: row.get(0)?,
                agent_id: row.get(1)?,
                run_id: row.get(2)?,
                assistant_message_id: row.get(3)?,
                tool_call_id: row.get(4)?,
                mode: row.get(5)?,
                status: row.get(6)?,
                revision: row.get(7)?,
                policy_revision: row.get(8)?,
                questions_json: row.get(9)?,
                created_at: row.get(10)?,
                updated_at: row.get(11)?,
            })
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    rows
}

fn skipped_answers(
    snapshot: &crate::human_interaction::HumanInteractionRequestSnapshot,
) -> Vec<crate::human_interaction::HumanInteractionAnswer> {
    snapshot
        .questions
        .iter()
        .map(|question| crate::human_interaction::HumanInteractionAnswer::Skipped {
            question_id: question.id.clone(),
        })
        .collect()
}

#[test]
fn open_async_questions_are_inherited_with_branch_identity_and_frozen_text() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-b", "run-source-1", 40);
    let source_request =
        admit_async_question(&mut connection, "assistant-b", "run-source-1", "call-async-question", 41);
    finish_source_trace(
        &connection,
        "assistant-b",
        "run-source-1",
        40,
        &["call-async-question"],
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-inherit",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_rows = copied_question_rows(&connection, &plan.target.id);
    assert_eq!(target_rows.len(), 1);
    let copied = &target_rows[0];
    assert_eq!(copied.mode, "async");
    assert_eq!(copied.status, "open");
    assert_eq!(copied.revision, 0);
    assert_ne!(copied.request_id, source_request.request_id);
    assert_eq!(
        copied.agent_id,
        crate::root_agent_id_for_conversation(&plan.target.id)
    );
    assert_eq!(copied.assistant_message_id, plan.message_id_map["assistant-b"]);
    assert_ne!(copied.run_id, "run-source-1");
    assert_ne!(copied.tool_call_id, "call-async-question");

    let source_rows = copied_question_rows(&connection, &source.id);
    assert_eq!(copied.questions_json, source_rows[0].questions_json);
    assert_eq!(copied.policy_revision, source_rows[0].policy_revision);
    assert_eq!(copied.created_at, source_rows[0].created_at);
    assert_eq!(copied.updated_at, source_rows[0].updated_at);
    let copied_view: Value = serde_json::from_str(&copied.questions_json).unwrap();
    assert_eq!(copied_view[0]["title"], json!("Pick a color"));
    assert_eq!(copied_view[0]["options"].as_array().unwrap().len(), 2);
    assert_eq!(copied_view[1]["title"], json!("Any details?"));

    let target_assistant = &plan.message_id_map["assistant-b"];
    let copied_trace = conversation_trace_repository::get_trace_for_message(&connection, target_assistant)
        .unwrap()
        .unwrap();
    assert_eq!(copied_trace.run_id, copied.run_id);
    let ConversationTurnTraceItem::ToolCall { call_id, operation, .. } = &copied_trace.items[0] else {
        panic!("missing copied question call")
    };
    assert_eq!(call_id, &copied.tool_call_id);
    assert_eq!(operation, &question_call_operation());
    let ConversationTurnTraceItem::ToolResult {
        call_id: result_call_id,
        ..
    } = &copied_trace.items[1]
    else {
        panic!("missing copied question receipt")
    };
    assert_eq!(result_call_id, &copied.tool_call_id);

    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM human_interaction_responses WHERE request_id=?1",
                [&copied.request_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM human_interaction_responses WHERE request_id=?1",
                [&source_request.request_id],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    let source_now = crate::storage::human_interaction_repository::load_request(
        &connection,
        &source.id,
        &source_request.request_id,
    )
    .unwrap();
    assert_eq!(
        source_now.status,
        crate::human_interaction::HumanInteractionRequestStatus::Open
    );
    assert_eq!(source_now.revision, 0);
}

#[test]
fn settled_questions_are_not_inherited_and_stay_settled() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-b", "run-source-1", 40);
    let open = admit_async_question(&mut connection, "assistant-b", "run-source-1", "call-open", 41);
    let submitted =
        admit_async_question(&mut connection, "assistant-b", "run-source-1", "call-submit", 41);
    let ignored =
        admit_async_question(&mut connection, "assistant-b", "run-source-1", "call-ignore", 41);
    crate::storage::human_interaction_repository::submit(
        &mut connection,
        &crate::human_interaction::HumanInteractionSubmitInput {
            conversation_id: source.id.clone(),
            request_id: submitted.request_id.clone(),
            expected_revision: submitted.revision,
            submission_id: "source-submit".into(),
            answers: skipped_answers(&submitted),
        },
        42,
    )
    .unwrap();
    crate::storage::human_interaction_repository::ignore(
        &mut connection,
        &crate::human_interaction::HumanInteractionIgnoreInput {
            conversation_id: source.id.clone(),
            request_id: ignored.request_id.clone(),
            expected_revision: ignored.revision,
            submission_id: "source-ignore".into(),
        },
        43,
    )
    .unwrap();
    finish_source_trace(
        &connection,
        "assistant-b",
        "run-source-1",
        40,
        &["call-open", "call-submit", "call-ignore"],
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-settled",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();

    let target_rows = copied_question_rows(&connection, &plan.target.id);
    assert_eq!(target_rows.len(), 1);
    assert_ne!(target_rows[0].request_id, open.request_id);
    assert_ne!(target_rows[0].request_id, submitted.request_id);
    assert_ne!(target_rows[0].request_id, ignored.request_id);
    let source_rows = copied_question_rows(&connection, &source.id);
    assert_eq!(source_rows.len(), 3);
    let open_row = source_rows
        .iter()
        .find(|row| row.request_id == open.request_id)
        .unwrap();
    assert_eq!(target_rows[0].questions_json, open_row.questions_json);
}

#[test]
fn questions_beyond_the_fork_boundary_are_not_inherited() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-c", "run-source-2", 60);
    let beyond =
        admit_async_question(&mut connection, "assistant-c", "run-source-2", "call-beyond", 61);
    finish_source_trace(
        &connection,
        "assistant-c",
        "run-source-2",
        60,
        &["call-beyond"],
    );

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-boundary",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    assert_eq!(copied_question_rows(&connection, &plan.target.id).len(), 0);
    let still_open = crate::storage::human_interaction_repository::load_request(
        &connection,
        &source.id,
        &beyond.request_id,
    )
    .unwrap();
    assert_eq!(
        still_open.status,
        crate::human_interaction::HumanInteractionRequestStatus::Open
    );
}

#[test]
fn re_fork_inherits_open_questions_again_but_not_answered_ones() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-b", "run-source-1", 40);
    let source_request =
        admit_async_question(&mut connection, "assistant-b", "run-source-1", "call-async-question", 41);
    finish_source_trace(
        &connection,
        "assistant-b",
        "run-source-1",
        40,
        &["call-async-question"],
    );

    let first = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-first",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &first).unwrap();
    let first_rows = copied_question_rows(&connection, &first.target.id);
    assert_eq!(first_rows.len(), 1);
    let first_request_id = first_rows[0].request_id.clone();
    assert_ne!(first_request_id, source_request.request_id);

    let second = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-second",
        &first.target.id,
        &first.message_id_map["assistant-b"],
        120,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &second).unwrap();
    let second_rows = copied_question_rows(&connection, &second.target.id);
    assert_eq!(second_rows.len(), 1);
    assert_ne!(second_rows[0].request_id, first_request_id);
    assert_ne!(second_rows[0].request_id, source_request.request_id);
    assert_ne!(second_rows[0].run_id, first_rows[0].run_id);
    assert_eq!(
        second_rows[0].questions_json,
        first_rows[0].questions_json
    );

    let branch_snapshot = crate::storage::human_interaction_repository::load_request(
        &connection,
        &first.target.id,
        &first_request_id,
    )
    .unwrap();
    crate::storage::human_interaction_repository::submit(
        &mut connection,
        &crate::human_interaction::HumanInteractionSubmitInput {
            conversation_id: first.target.id.clone(),
            request_id: first_request_id.clone(),
            expected_revision: branch_snapshot.revision,
            submission_id: "branch-submit".into(),
            answers: skipped_answers(&branch_snapshot),
        },
        150,
    )
    .unwrap();

    let third = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-third",
        &first.target.id,
        &first.message_id_map["assistant-b"],
        160,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &third).unwrap();
    assert_eq!(copied_question_rows(&connection, &third.target.id).len(), 0);
}
