thread_local! {
    static FORK_PLAN_SQL: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn record_fork_plan_sql(sql: &str) {
    FORK_PLAN_SQL.with(|statements| statements.borrow_mut().push(sql.to_string()));
}

fn trace_read_count(statements: &[String], assistant_message_id: &str) -> usize {
    statements
        .iter()
        .filter(|sql| {
            sql.contains("SELECT sequence, item_kind, item_json")
                && sql.contains("FROM conversation_turn_trace_items")
                && sql.contains(&format!("'{assistant_message_id}'"))
        })
        .count()
}

fn populate_reuse_test_traces(
    connection: &mut Connection,
    source: &ChatConversationRecord,
    extra_completion_delay: i64,
) {
    for message in source
        .messages
        .iter()
        .filter(|message| message.role == "assistant")
    {
        let run_id = agent_run_id(message.agent_run_json.as_deref()).unwrap();
        conversation_trace_repository::replace_trace(
            connection,
            &ConversationTurnTrace {
                schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
                run_id: run_id.clone(),
                conversation_id: source.id.clone(),
                assistant_message_id: message.id.clone(),
                terminal_status: ConversationTurnTraceTerminalStatus::Completed,
                terminal_error: None,
                truncated: false,
                items: staged_tool_exchange(
                    0,
                    &format!("call-{}", message.id),
                    "read_file",
                    json!({"path":"README.md"}),
                ),
            },
            message.created_at,
            message.created_at + extra_completion_delay,
        )
        .unwrap();
        crate::storage::agent_workspace_repository::freeze_run(connection, &run_id, None, None)
            .unwrap();
    }
}

#[test]
fn fork_plan_reuses_parsed_history_across_tree_identity_allocation_and_building() {
    let mut connection = Connection::open_in_memory().unwrap();
    migrations::run_migrations(&connection).unwrap();
    let source = fork_source_with_root(&mut connection);
    populate_reuse_test_traces(&mut connection, &source, 0);
    let root_id = crate::root_agent_id_for_conversation(&source.id);

    let mut child_source = source.clone();
    child_source.id = "reuse-child-conversation".into();
    for message in &mut child_source.messages {
        message.id = format!("child-{}", message.id);
        if message.role == "assistant" {
            message.agent_run_json = Some(
                json!({"runId":format!("run-{}",message.id),"status":"completed"}).to_string(),
            );
        }
    }
    let mut empty_child = child_source.clone();
    empty_child.messages.clear();
    chat_repository::save_conversation(&mut connection, empty_child).unwrap();
    agent_graph_repository::create_agent_node(
        &mut connection,
        &crate::CreateAgentNodeInput {
            agent_id: "reuse-child-agent".into(),
            root_agent_id: root_id.clone(),
            parent_agent_id: root_id.clone(),
            conversation_id: child_source.id.clone(),
            creation_request_id: "create-reuse-child".into(),
            task_name: "reader".into(),
            task_path: "/root/reader".into(),
            template_snapshot: None,
            model_snapshot: crate::AgentModelSelectionSnapshot {
                model_config_id: "model-1".into(),
                display_name: "Model 1".into(),
                supports_image: false,
                effective_context_window_tokens: 64_000,
                model_settings_configuration_revision: "model-settings-v1:test".into(),
                provider_connection_revision: "provider-connection-v1:test".into(),
                provider_protocol_revision: "provider-protocol-v1:test".into(),
            },
        },
        15,
    )
    .unwrap();
    for (position, message) in child_source.messages.iter().enumerate() {
        if message.role == "user" {
            let mailbox_id = format!("mailbox-{}", message.id);
            let claim = format!("claim-{}", message.id);
            agent_graph_repository::enqueue_agent_message(
                &mut connection,
                &crate::EnqueueAgentMessageInput {
                    message_id: mailbox_id.clone(),
                    root_agent_id: root_id.clone(),
                    sender_agent_id: root_id.clone(),
                    recipient_agent_id: "reuse-child-agent".into(),
                    request_id: format!("request-{}", message.id),
                    kind: crate::AgentMailboxKind::Message,
                    content: message.content.clone(),
                    projection_message_id: message.id.clone(),
                },
                message.created_at,
            )
            .unwrap();
            agent_graph_repository::claim_next_agent_message(
                &mut connection,
                "reuse-child-agent",
                &claim,
                message.created_at,
            )
            .unwrap()
            .unwrap();
            agent_graph_repository::acknowledge_agent_message_with_projection(
                &mut connection,
                &mailbox_id,
                &claim,
                message.created_at,
            )
            .unwrap();
        } else {
            connection.execute(
                "INSERT INTO messages (id, conversation_id, role, content, created_at, status, agent_run_json, position) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![message.id, child_source.id, message.role, message.content, message.created_at, message.status, message.agent_run_json, position],
            ).unwrap();
        }
    }
    // The child second reply exists at the root boundary but finishes afterwards. It must
    // establish the cutoff without entering the copied snapshot or being parsed twice.
    populate_reuse_test_traces(&mut connection, &child_source, 5);

    FORK_PLAN_SQL.with(|statements| statements.borrow_mut().clear());
    connection.trace(Some(record_fork_plan_sql));
    let plan = build_assistant_reply_fork_plan(
        &connection,
        "reuse-tree-plan",
        &source.id,
        "assistant-b",
        100,
    )
    .unwrap();
    connection.trace(None);
    let statements = FORK_PLAN_SQL.with(|statements| std::mem::take(&mut *statements.borrow_mut()));
    for message_id in [
        "assistant-a",
        "assistant-b",
        "child-assistant-a",
        "child-assistant-b",
    ] {
        assert_eq!(
            trace_read_count(&statements, message_id),
            1,
            "{message_id} must be decoded once across all planning stages"
        );
    }
    for message_id in [
        "assistant-c",
        "assistant-d",
        "child-assistant-c",
        "child-assistant-d",
    ] {
        assert_eq!(
            trace_read_count(&statements, message_id),
            0,
            "history beyond the selected boundary must not be decoded"
        );
    }
    for conversation_id in [&source.id, &child_source.id] {
        assert_eq!(statements.iter().filter(|sql| sql.contains("SELECT id, project_id, model_id, title, created_at, updated_at, pinned_at, archived_at, unread_at") && sql.contains("FROM conversations") && sql.contains(&format!("'{conversation_id}'"))).count(), 1, "conversation content is reused rather than loaded for every planning stage");
    }
    assert_eq!(plan.target.messages.len(), 4);
    assert_eq!(plan.traces.len(), 2);
    assert_eq!(plan.members.len(), 1);
    let child = &plan.members[0];
    assert_eq!(child.history.target.messages.len(), 3);
    assert_eq!(child.history.traces.len(), 1);
    assert!(child.history.message_id_map.contains_key("child-user-b"));
    assert!(!child
        .history
        .message_id_map
        .contains_key("child-assistant-b"));
    assert_eq!(
        child.target_agent.parent_agent_id.as_deref(),
        plan.collaboration_root
            .as_ref()
            .map(|root| root.target_root.agent_id.as_str())
    );
    commit_fork_plan(&mut connection, &plan).unwrap();
    let recursive = build_assistant_reply_fork_plan(
        &connection,
        "reuse-tree-plan-recursive",
        &plan.target.id,
        &plan.message_id_map["assistant-b"],
        110,
    )
    .unwrap();
    assert_eq!(recursive.target.messages.len(), 4);
    assert_eq!(recursive.members[0].history.target.messages.len(), 3);
    assert_eq!(recursive.members[0].history.traces.len(), 1);
    assert_ne!(
        recursive.members[0].target_agent.agent_id,
        child.target_agent.agent_id
    );
}
