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

fn async_accepted_receipt(request_id: &str) -> Value {
    json!({"type":"human_interaction_accepted","schemaVersion":1,"requestId":request_id,"status":"accepted"})
}

#[test]
fn inherited_question_receipts_follow_the_branch_identity_in_trace_and_model_context() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-b", "run-source-1", 40);
    let source_request = admit_async_question(
        &mut connection,
        "assistant-b",
        "run-source-1",
        "call-async-question",
        41,
    );
    let receipt = async_accepted_receipt(&source_request.request_id);

    let mut items = staged_tool_exchange(
        0,
        "call-async-question",
        "request_user_input_async",
        question_call_operation(),
    );
    if let ConversationTurnTraceItem::ToolResult { observation, .. } = &mut items[1] {
        *observation = receipt.clone();
    }
    // Text that merely looks like a receipt outside certified machine positions must survive the
    // fork byte-identically.
    items.push(ConversationTurnTraceItem::AssistantNarration {
        sequence: 2,
        content: receipt.to_string(),
        provider_turn_id: None,
        first_tool_call_id: None,
        truncated: false,
    });
    items.push(ConversationTurnTraceItem::AssistantNarration {
        sequence: 3,
        content: source_request.request_id.clone(),
        provider_turn_id: None,
        first_tool_call_id: None,
        truncated: false,
    });
    items.push(ConversationTurnTraceItem::UserGuidance {
        sequence: 4,
        guidance_id: "guidance-opaque-question-id".into(),
        client_message_id: "client-opaque-question-id".into(),
        content: source_request.request_id.clone(),
        attachments: Vec::new(),
        created_at: 41,
        truncated: false,
    });
    items.push(ConversationTurnTraceItem::AgentMailboxDelivery {
        sequence: 5,
        receipt_id: "receipt-opaque-question-id".into(),
        message_id: "message-opaque-question-id".into(),
        sender_agent_id: "agent-opaque-question-id".into(),
        sender_task_name: "opaque".into(),
        sender_task_path: "/root/opaque".into(),
        kind: crate::AgentMailboxKind::Message,
        content: source_request.request_id.clone(),
        created_at: 41,
        truncated: false,
    });
    let mut unrelated = staged_tool_exchange(
        6,
        "call-unrelated-question-id",
        "read_file",
        json!({"requestId": source_request.request_id.clone()}),
    );
    if let ConversationTurnTraceItem::ToolResult { observation, .. } = &mut unrelated[1] {
        *observation = json!({"requestId": source_request.request_id.clone()});
    }
    items.extend(unrelated);
    conversation_trace_repository::commit_trace_in_connection(
        &connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-1".into(),
            conversation_id: "conversation-source".into(),
            assistant_message_id: "assistant-b".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: false,
            items,
        },
        40,
        42,
    )
    .unwrap();

    let context = vec![
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 0,
            ordinal: 0,
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            is_error: false,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "call-async-question".into(),
                name: "request_user_input_async".into(),
                args: question_call_operation(),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 0,
                    provider_call_id: "call-async-question".into(),
                    runtime_call_id: "call-async-question".into(),
                },
            }],
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 1,
            ordinal: 0,
            role: "tool".into(),
            content: serde_json::to_string_pretty(&receipt).unwrap(),
            tool_call_id: Some("call-async-question".into()),
            tool_calls: vec![],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 2,
            ordinal: 0,
            role: "assistant".into(),
            content: receipt.to_string(),
            tool_call_id: None,
            tool_calls: vec![],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 3,
            ordinal: 0,
            role: "assistant".into(),
            content: source_request.request_id.clone(),
            tool_call_id: None,
            tool_calls: vec![],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 4,
            ordinal: 0,
            role: "user".into(),
            content: source_request.request_id.clone(),
            tool_call_id: None,
            tool_calls: vec![],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 5,
            ordinal: 0,
            role: "user".into(),
            content: source_request.request_id.clone(),
            tool_call_id: None,
            tool_calls: vec![],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 6,
            ordinal: 0,
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: vec![crate::AgentContextCheckpointToolCall {
                id: "call-unrelated-question-id".into(),
                name: "read_file".into(),
                args: json!({"requestId": source_request.request_id.clone()}),
                provider_identity: crate::AgentProviderToolCallIdentity {
                    provider_tool_index: 1,
                    provider_call_id: "call-unrelated-question-id".into(),
                    runtime_call_id: "call-unrelated-question-id".into(),
                },
            }],
            is_error: false,
        },
        ConversationModelContextItem {
            images: Vec::new(),
            sequence: 7,
            ordinal: 0,
            role: "tool".into(),
            content: source_request.request_id.clone(),
            tool_call_id: Some("call-unrelated-question-id".into()),
            tool_calls: vec![],
            is_error: false,
        },
    ];
    conversation_model_context_repository::commit_items_in_connection(
        &connection,
        "conversation-source",
        "assistant-b",
        &context,
    )
    .unwrap();

    let mut source_agent_run = source
        .messages
        .iter()
        .find(|message| message.id == "assistant-b")
        .and_then(|message| message.agent_run_json.as_deref())
        .map(|raw| serde_json::from_str::<Value>(raw).unwrap())
        .unwrap();
    source_agent_run["toolCalls"] = serde_json::to_value(vec![
        crate::AgentToolCall {
            id: "call-async-question".into(),
            tool: "request_user_input_async".into(),
            args: question_call_operation(),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        },
        crate::AgentToolCall {
            id: "call-unrelated-question-id".into(),
            tool: "read_file".into(),
            args: json!({"requestId": source_request.request_id.clone()}),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        },
    ])
    .unwrap();
    source_agent_run["toolResults"] = serde_json::to_value(vec![
        AgentToolResult {
            call_id: "call-async-question".into(),
            tool: "request_user_input_async".into(),
            ok: true,
            result: Some(receipt.clone()),
            error: None,
            exact_archive_file: None,
        },
        AgentToolResult {
            call_id: "call-unrelated-question-id".into(),
            tool: "read_file".into(),
            ok: true,
            result: Some(json!({"requestId": source_request.request_id.clone()})),
            error: None,
            exact_archive_file: None,
        },
    ])
    .unwrap();
    connection
        .execute(
            "UPDATE messages SET agent_run_json=?1 WHERE id='assistant-b'",
            [serde_json::to_string(&source_agent_run).unwrap()],
        )
        .unwrap();

    let plan = build_assistant_reply_fork_plan(
        &connection,
        "fork-async-receipt",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &plan).unwrap();
    let target_rows = copied_question_rows(&connection, &plan.target.id);
    assert_eq!(target_rows.len(), 1);
    let copied_request_id = target_rows[0].request_id.clone();
    assert_ne!(copied_request_id, source_request.request_id);

    let target_assistant = &plan.message_id_map["assistant-b"];
    let copied_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        target_assistant,
    )
    .unwrap()
    .unwrap();
    let ConversationTurnTraceItem::ToolResult { observation, .. } = &copied_trace.items[1] else {
        panic!("missing copied question receipt")
    };
    assert_eq!(observation["requestId"], json!(copied_request_id));

    let narration = &copied_trace.items[2];
    let ConversationTurnTraceItem::AssistantNarration { content, .. } = narration else {
        panic!("missing copied narration")
    };
    assert_eq!(content, &receipt.to_string());
    assert!(content.contains(&source_request.request_id));

    for index in [3, 4, 5] {
        let content = match &copied_trace.items[index] {
            ConversationTurnTraceItem::AssistantNarration { content, .. }
            | ConversationTurnTraceItem::UserGuidance { content, .. }
            | ConversationTurnTraceItem::AgentMailboxDelivery { content, .. } => content,
            _ => panic!("missing copied opaque text at trace index {index}"),
        };
        assert_eq!(content, &source_request.request_id);
    }
    let ConversationTurnTraceItem::ToolCall { operation, .. } = &copied_trace.items[6] else {
        panic!("missing copied unrelated ToolCall")
    };
    assert_eq!(operation["requestId"], json!(source_request.request_id));
    let ConversationTurnTraceItem::ToolResult { observation, .. } = &copied_trace.items[7] else {
        panic!("missing copied unrelated ToolResult")
    };
    assert_eq!(observation["requestId"], json!(source_request.request_id));

    let copied_context = conversation_model_context_repository::get_log_for_message(
        &connection,
        target_assistant,
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        copied_context.items[0].tool_calls[0].args,
        question_call_operation()
    );
    let tool_item = &copied_context.items[1];
    let copied_receipt: Value = serde_json::from_str(&tool_item.content).unwrap();
    assert_eq!(copied_receipt["status"], json!("accepted"));
    assert_eq!(copied_receipt["requestId"], json!(copied_request_id));
    for index in [2, 3, 4, 5] {
        let expected = if index == 2 {
            receipt.to_string()
        } else {
            source_request.request_id.clone()
        };
        assert_eq!(copied_context.items[index].content, expected);
    }
    assert_eq!(
        copied_context.items[6].tool_calls[0].args["requestId"],
        json!(source_request.request_id)
    );
    assert_eq!(
        copied_context.items[7].content,
        source_request.request_id
    );

    let copied_agent_run = plan
        .target
        .messages
        .iter()
        .find(|message| message.id == *target_assistant)
        .and_then(|message| message.agent_run_json.as_deref())
        .map(|raw| serde_json::from_str::<Value>(raw).unwrap())
        .unwrap();
    let copied_run_results = copied_agent_run["toolResults"].as_array().unwrap();
    let copied_question_result = copied_run_results
        .iter()
        .find(|result| result["tool"] == "request_user_input_async")
        .unwrap();
    assert_eq!(
        copied_question_result["result"]["requestId"],
        json!(copied_request_id)
    );
    let copied_unrelated_result = copied_run_results
        .iter()
        .find(|result| result["tool"] == "read_file")
        .unwrap();
    assert_eq!(
        copied_unrelated_result["result"]["requestId"],
        json!(source_request.request_id)
    );
}

#[test]
fn file_change_digest_lineage_survives_repeated_forks_when_operations_reference_question_ids() {
    use crate::file_change::{
        FileChangeBase, FileChangeDirectBinding, FileChangeMutation, FileChangeOperation,
        FileChangeOutcome, FileChangePlanRequest, FileChangePlanner, FileChangeProposal,
        FileChangeStatus, FileChangeTransaction, FileObservationCheckpoint,
        FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION, FILE_CHANGE_SCHEMA_VERSION,
    };

    const READ_CALL_ID: &str = "call-lineage-read";
    const APPLY_CALL_ID: &str = "call-lineage-apply";
    const SOURCE_TRANSACTION_ID: &str = "file-change-lineage-source";

    let fixture = tempfile::tempdir().unwrap();
    let canonical_target = fixture.path().join("direct.md");
    std::fs::write(&canonical_target, "before\n").unwrap();
    let base_revision = crate::content_revision(b"before\n");

    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    begin_source_questioning(&connection, "assistant-b", "run-source-1", 40);
    let question = admit_async_question(
        &mut connection,
        "assistant-b",
        "run-source-1",
        "call-lineage-question",
        41,
    );
    finish_source_trace(
        &connection,
        "assistant-b",
        "run-source-1",
        40,
        &["call-lineage-question"],
    );

    let (observation_id, observation_json) = apply_patch_observation(
        &source.id,
        "run-source-0",
        READ_CALL_ID,
        &canonical_target,
        &base_revision,
    );
    let observation: FileObservationCheckpoint = serde_json::from_str(&observation_json).unwrap();
    let read_args = json!({ "path": "direct.md" });
    let apply_args = apply_patch_args(json!({
        "action": "apply",
        "operation": "update",
        "filePath": "direct.md",
        "observationId": observation_id,
        "content": "after\n",
    }));
    let mut trace_operation =
        crate::file_change_support::apply_patch_trace_operation(&apply_args).unwrap();
    trace_operation
        .as_object_mut()
        .unwrap()
        .insert("inheritedQuestionRequestId".into(), json!(question.request_id.clone()));

    let mut items = staged_tool_exchange(0, READ_CALL_ID, "read_file", read_args);
    if let ConversationTurnTraceItem::ToolResult {
        observation: result,
        ..
    } = &mut items[1]
    {
        *result = json!({
            "filePath": "direct.md",
            "revision": base_revision,
            "observationId": observation_id,
        });
    }
    let mut apply = staged_tool_exchange(2, APPLY_CALL_ID, "apply_patch", trace_operation.clone());
    if let ConversationTurnTraceItem::ToolCall {
        approval_status, ..
    } = &mut apply[0]
    {
        *approval_status = AgentApprovalStatus::Required;
    }
    if let ConversationTurnTraceItem::ToolResult {
        approval_status,
        observation: result,
        ..
    } = &mut apply[1]
    {
        *approval_status = AgentApprovalStatus::Approved;
        *result = json!({
            "schemaVersion": 1,
            "status": "applied",
            "outcome": "applied",
            "transactionId": SOURCE_TRANSACTION_ID,
            "operation": "update",
            "updateStrategy": null,
            "filePath": "direct.md",
            "additions": 1,
            "deletions": 1,
            "lineCount": 1,
            "byteCount": 6,
            "revision": crate::content_revision(b"after\n"),
            "errorCode": null,
            "error": null,
            "message": null,
        });
    }
    items.extend(apply);
    conversation_trace_repository::replace_trace(
        &mut connection,
        &ConversationTurnTrace {
            schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
            run_id: "run-source-0".into(),
            conversation_id: source.id.clone(),
            assistant_message_id: "assistant-a".into(),
            terminal_status: ConversationTurnTraceTerminalStatus::Completed,
            terminal_error: None,
            truncated: true,
            items,
        },
        20,
        21,
    )
    .unwrap();

    let frozen_plan = FileChangePlanner
        .plan(FileChangePlanRequest {
            operation: FileChangeOperation::Update,
            file_path: "direct.md",
            base: FileChangeBase::Existing {
                content: "before\n",
                revision: &base_revision,
            },
            mutation: FileChangeMutation::Complete("after\n".to_string()),
        })
        .unwrap();
    let execution = FileChangeDirectBinding {
        schema_version: FILE_CHANGE_DIRECT_BINDING_SCHEMA_VERSION,
        transaction: FileChangeTransaction {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            status: FileChangeStatus::WaitingApproval,
            outcome: FileChangeOutcome::DefinitelyNotExecuted,
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            created_at: 20,
            updated_at: 20,
        },
        proposal: FileChangeProposal {
            schema_version: FILE_CHANGE_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: FileChangeOperation::Update,
            file_path: "direct.md".to_string(),
            base: frozen_plan.base.clone(),
            target: frozen_plan.target.clone(),
            diff_digest: frozen_plan.diff_digest.clone(),
            proposal_digest: frozen_plan.proposal_digest.clone(),
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
        },
        observation_id: observation_id.clone(),
        observation,
        source_tool_name: "apply_patch".to_string(),
        source_call_id: APPLY_CALL_ID.to_string(),
        source_args_digest: crate::file_change::proposal_digest(&apply_args).unwrap(),
        trace_args_digest: crate::file_change::proposal_digest(&trace_operation).unwrap(),
        staged_transaction_id: None,
        conversation_id: source.id.clone(),
        project_id: None,
        run_id: "run-source-0".to_string(),
        staged_transaction_revision: None,
        canonical_target: canonical_target.to_string_lossy().into_owned(),
        base_content: Some("before\n".to_string()),
        target_content: Some("after\n".to_string()),
        delete_journal: None,
        receipt: None,
        permission_revision: "permission-1".to_string(),
        tool_set_revision: "tool-set-1".to_string(),
        provider_wire_revision: "provider-protocol-v1".to_string(),
    };
    execution.validate().unwrap();
    let inline_patch = frozen_plan.diff.clone();
    let terminal_result = crate::AgentFileChangeResult {
        schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
        status: crate::AgentFileChangeResultStatus::Applied,
        outcome: crate::AgentFileChangeOutcome::Applied,
        transaction_id: SOURCE_TRANSACTION_ID.to_string(),
        operation: crate::AgentFileChangeOperation::Update,
        update_strategy: None,
        file_path: "direct.md".to_string(),
        additions: frozen_plan.additions,
        deletions: frozen_plan.deletions,
        line_count: 1,
        byte_count: 6,
        revision: Some(crate::content_revision(b"after\n")),
        error_code: None,
        error: None,
        message: None,
    };
    terminal_result.validate().unwrap();
    let terminal_tool_result = AgentToolResult {
        call_id: APPLY_CALL_ID.to_string(),
        tool: "apply_patch".to_string(),
        ok: true,
        result: Some(serde_json::to_value(&terminal_result).unwrap()),
        error: None,
        exact_archive_file: None,
    };
    let action = AgentProposedAction::FileChange {
        file_change: crate::AgentFileChangeProposal {
            schema_version: crate::AGENT_FILE_CHANGE_PROTOCOL_SCHEMA_VERSION,
            id: APPLY_CALL_ID.to_string(),
            transaction_id: SOURCE_TRANSACTION_ID.to_string(),
            operation: crate::AgentFileChangeOperation::Update,
            update_strategy: None,
            file_path: "direct.md".to_string(),
            inline_diff: Some(crate::AgentGitDiffSnapshot {
                patch: inline_patch,
                truncated: false,
            }),
            base_revision: Some(base_revision),
            summary: None,
            additions: frozen_plan.additions,
            deletions: frozen_plan.deletions,
            line_count: 1,
            byte_count: 6,
            approval_status: AgentApprovalStatus::Approved,
            execution: Box::new(execution),
        },
    };
    assert!(
        agent_action_audit_repository::insert_action_audit_record_if_absent(
            &connection,
            &AgentActionAuditRecord {
                action_id: crate::canonical_pending_action_id("run-source-0", APPLY_CALL_ID),
                run_id: "run-source-0".to_string(),
                conversation_id: Some(source.id.clone()),
                assistant_message_id: Some("assistant-a".to_string()),
                action_type: "file_change".to_string(),
                tool_name: "apply_patch".to_string(),
                decision: Some("approved".to_string()),
                status: "completed".to_string(),
                action_json: serde_json::to_string(&action).unwrap(),
                file_change_result_json: Some(serde_json::to_string(&terminal_result).unwrap()),
                command_result_json: None,
                tool_result_json: Some(serde_json::to_string(&terminal_tool_result).unwrap()),
                error: None,
                created_at: 20,
                decided_at: Some(20),
                completed_at: Some(21),
                effective_permissions_json: Some("{}".to_string()),
                path_scope: Some(canonical_target.to_string_lossy().into_owned()),
                command_cwd_scope: None,
                blocked_reason: None,
                decision_source: Some("manual".to_string()),
            },
        )
        .unwrap()
    );

    // The first fork stores audit digests that the durable trace can reproduce, even though the
    // apply_patch operation references the inherited question id.
    let first = build_assistant_reply_fork_plan(
        &connection,
        "fork-lineage-first",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &first).unwrap();
    assert!(first.file_changes.is_empty());
    assert_eq!(first.action_audits.len(), 1);
    let branch_rows = copied_question_rows(&connection, &first.target.id);
    assert_eq!(branch_rows.len(), 1);
    let branch_request_id = branch_rows[0].request_id.clone();
    assert_ne!(branch_request_id, question.request_id);
    let first_audits =
        agent_action_audit_repository::list_terminal_file_change_action_audits_for_conversation(
            &connection,
            &first.target.id,
        )
        .unwrap();
    assert_eq!(first_audits.len(), 1);
    let first_action: AgentProposedAction =
        serde_json::from_str(&first_audits[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: first_change,
    } = first_action
    else {
        unreachable!()
    };
    let branch_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &first.message_id_map["assistant-a"],
    )
    .unwrap()
    .unwrap();
    let branch_operation = branch_trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                tool, operation, ..
            } if tool == "apply_patch" => Some(operation),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        branch_operation["inheritedQuestionRequestId"],
        json!(question.request_id)
    );
    assert_eq!(
        first_change.execution.trace_args_digest,
        crate::file_change::proposal_digest(branch_operation).unwrap()
    );

    // A second fork must be able to recompute that lineage from the durable trace.
    let second = build_assistant_reply_fork_plan(
        &connection,
        "fork-lineage-second",
        &first.target.id,
        &first.message_id_map["assistant-b"],
        120,
    )
    .unwrap();
    commit_fork_plan(&mut connection, &second).unwrap();
    assert_eq!(second.action_audits.len(), 1);
    let second_rows = copied_question_rows(&connection, &second.target.id);
    assert_eq!(second_rows.len(), 1);
    assert_ne!(second_rows[0].request_id, branch_request_id);
    let second_audits =
        agent_action_audit_repository::list_terminal_file_change_action_audits_for_conversation(
            &connection,
            &second.target.id,
        )
        .unwrap();
    let second_action: AgentProposedAction =
        serde_json::from_str(&second_audits[0].action_json).unwrap();
    let AgentProposedAction::FileChange {
        file_change: second_change,
    } = second_action
    else {
        unreachable!()
    };
    let second_trace = conversation_trace_repository::get_trace_for_message(
        &connection,
        &second.message_id_map[&first.message_id_map["assistant-a"]],
    )
    .unwrap()
    .unwrap();
    let second_operation = second_trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                tool, operation, ..
            } if tool == "apply_patch" => Some(operation),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        second_operation["inheritedQuestionRequestId"],
        json!(question.request_id)
    );
    assert_eq!(
        second_change.execution.trace_args_digest,
        crate::file_change::proposal_digest(second_operation).unwrap()
    );
}
