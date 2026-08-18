use super::*;

#[test]
fn snapshot_none_all_and_last_use_complete_settled_turns_and_exclude_active_tail() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 3, true);

    for (request, task, selector, expected_history) in [
        ("snapshot-none", "none", AgentForkTurns::None, 0usize),
        ("snapshot-all", "all", AgentForkTurns::All, 6usize),
        ("snapshot-last", "last", AgentForkTurns::Last(2), 4usize),
        ("snapshot-over", "over", AgentForkTurns::Last(99), 6usize),
    ] {
        let mut input = spawn_input(request, task);
        input.fork_turns = selector;
        let child = fixture.service.create_child_agent(&input).unwrap();
        let conversation = fixture
            .service
            .load_conversation(&child.agent.conversation_id)
            .unwrap()
            .unwrap();
        assert_eq!(conversation.messages.len(), expected_history + 1);
        assert_eq!(conversation.messages.last().unwrap().content, input.task);
        assert!(conversation
            .messages
            .iter()
            .all(|message| message.id != "root-user-active"
                && message.content != "must not be copied"));
        for historical in conversation.messages.iter().take(expected_history) {
            assert!(historical.ui_state_json.is_none());
            if let Some(run) = historical.agent_run_json.as_deref() {
                let run = serde_json::from_str::<serde_json::Value>(run).unwrap();
                assert!(
                    run.get("usage").is_none() || run["usage"]["totalTokens"].as_u64() == Some(0),
                    "a context snapshot must not copy parent usage"
                );
            }
        }
    }
}

#[test]
fn grandchild_snapshot_flattens_historical_actor_provenance_without_reusing_mailbox_fk() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    let mut child_input = spawn_input("actor-child", "actor_child");
    child_input.fork_turns = AgentForkTurns::All;
    let child = fixture.service.create_child_agent(&child_input).unwrap();
    let reply_id = "child-assistant-reply";
    {
        let connection = fixture.service.state.connection().unwrap();
        connection
            .execute(
                "INSERT INTO messages (
                         id, conversation_id, role, content, status, created_at, position
                     ) VALUES (?1, ?2, 'assistant', 'child completed task', 'completed', 200,
                               (SELECT COALESCE(MAX(position), -1) + 1 FROM messages
                                WHERE conversation_id = ?2))",
                rusqlite::params![reply_id, &child.agent.conversation_id],
            )
            .unwrap();
        conversation_trace_repository::commit_trace_in_connection(
            &connection,
            &crate::completed_conversation_trace_without_items(
                "child-run",
                &child.agent.conversation_id,
                reply_id,
            ),
            200,
            200,
        )
        .unwrap();
    }
    let child_conversation = fixture
        .service
        .load_conversation(&child.agent.conversation_id)
        .unwrap()
        .unwrap();
    let child_root_user = child_conversation
        .messages
        .iter()
        .find(|message| message.content == "question 0")
        .unwrap()
        .id
        .clone();

    let mut grandchild_input = spawn_input("actor-grandchild", "actor_grandchild");
    grandchild_input.parent_agent_id = child.agent.agent_id.clone();
    grandchild_input.fork_turns = AgentForkTurns::All;
    let grandchild = fixture
        .service
        .create_child_agent(&grandchild_input)
        .unwrap();
    let grandchild_conversation = fixture
        .service
        .load_conversation(&grandchild.agent.conversation_id)
        .unwrap()
        .unwrap();
    let grandchild_root_user = grandchild_conversation
        .messages
        .iter()
        .find(|message| message.content == "question 0")
        .unwrap();
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(
                &grandchild.agent.conversation_id,
                &grandchild_root_user.id,
            )
            .unwrap(),
        crate::ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: child.agent.conversation_id.clone(),
            source_message_id: child_root_user,
            original: Box::new(crate::ConversationMessageOrigin::Human),
        }
    );
    let grandchild_parent_task = grandchild_conversation
        .messages
        .iter()
        .find(|message| message.content == child_input.task)
        .unwrap();
    assert_eq!(
        fixture
            .service
            .conversation_message_origin(
                &grandchild.agent.conversation_id,
                &grandchild_parent_task.id,
            )
            .unwrap(),
        crate::ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: child.agent.conversation_id.clone(),
            source_message_id: child.task_message.projection_message_id.clone(),
            original: Box::new(crate::ConversationMessageOrigin::Agent {
                sender_agent_id: "agent-root".to_string(),
                source_agent_message_id: child.task_message.message_id.clone(),
            }),
        }
    );
    let connection = fixture.service.state.connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT source_agent_message_id FROM messages WHERE id = ?1",
                [&grandchild_parent_task.id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap(),
        None
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE source_agent_message_id = ?1",
                [&child.task_message.message_id],
                |row| row.get::<_, u64>(0),
            )
            .unwrap(),
        1
    );
}

#[test]
fn observer_snapshot_bulk_loads_large_history_and_every_actor_origin_from_one_read_cut() {
    const TURN_COUNT: usize = 1_000;

    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, TURN_COUNT, false);

    let root = fixture
        .service
        .load_conversation_observer_snapshot("root-conversation")
        .unwrap()
        .unwrap();
    assert_eq!(root.conversation.messages.len(), TURN_COUNT * 2);
    assert_eq!(root.input_origins.len(), TURN_COUNT);
    assert!(root
        .input_origins
        .values()
        .all(|origin| matches!(origin, crate::ConversationMessageOrigin::Human)));

    let mut input = spawn_input("bulk-observer-child", "bulk_observer");
    input.fork_turns = AgentForkTurns::All;
    let child = fixture.service.create_child_agent(&input).unwrap();
    let observer = fixture
        .service
        .load_conversation_observer_snapshot(&child.agent.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(observer.conversation.messages.len(), TURN_COUNT * 2 + 1);
    assert_eq!(observer.input_origins.len(), TURN_COUNT + 1);
    assert_eq!(
        observer
            .input_origins
            .get(&child.task_message.projection_message_id),
        Some(&crate::ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root".to_string(),
            source_agent_message_id: child.task_message.message_id.clone(),
        })
    );

    let snapshots = observer
        .input_origins
        .iter()
        .filter(|(message_id, _)| *message_id != &child.task_message.projection_message_id)
        .collect::<Vec<_>>();
    assert_eq!(snapshots.len(), TURN_COUNT);
    for (message_id, origin) in snapshots {
        let crate::ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id,
            source_message_id,
            original,
        } = origin
        else {
            panic!("input {message_id} lost its historical snapshot provenance");
        };
        assert_eq!(source_conversation_id, "root-conversation");
        assert!(source_message_id.starts_with("root-user-"));
        assert!(matches!(
            original.as_ref(),
            crate::ConversationMessageOrigin::Human
        ));
    }
    assert!(observer
        .conversation
        .messages
        .iter()
        .filter(|message| message.role == "user")
        .all(|message| observer.input_origins.contains_key(&message.id)));
}

#[test]
fn user_facing_root_reload_hides_only_agent_projection_while_observer_keeps_origins() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 1, false);
    // Exercise reload projection from the durable trace, not the intentionally minimal
    // legacy AgentRun fixture used by the fork characterization helper.
    fixture
        .service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE messages SET agent_run_json = NULL WHERE id = 'root-assistant-0'",
            [],
        )
        .unwrap();

    let mut child_input = spawn_input("observer-origin-child", "observer_origin");
    child_input.fork_turns = AgentForkTurns::All;
    let child = fixture.service.create_child_agent(&child_input).unwrap();
    let result = EnqueueAgentMessageInput {
        message_id: "mailbox-child-result".to_string(),
        root_agent_id: "agent-root".to_string(),
        sender_agent_id: child.agent.agent_id.clone(),
        recipient_agent_id: "agent-root".to_string(),
        request_id: "request-child-result".to_string(),
        kind: AgentMailboxKind::Result,
        content: "durable child result".to_string(),
        projection_message_id: "projection-child-result".to_string(),
    };
    fixture.service.enqueue_agent_message(&result).unwrap();
    let claimed = fixture
        .service
        .claim_next_agent_message("agent-root", "claim-child-result")
        .unwrap()
        .unwrap();
    assert_eq!(claimed.message_id, result.message_id);
    fixture
        .service
        .acknowledge_agent_message_with_projection(&result.message_id, "claim-child-result")
        .unwrap();

    // The transport fact remains part of the durable Conversation and model context.
    let raw_root = fixture
        .service
        .load_conversation("root-conversation")
        .unwrap()
        .unwrap();
    assert!(raw_root
        .messages
        .iter()
        .any(|message| message.id == result.projection_message_id));

    // Reopen the database to characterize renderer reloads rather than an in-memory cache.
    let reopened = StorageService::open(&fixture._directory.path().join("storage.sqlite")).unwrap();
    let single_root = reopened
        .load_conversation_view("root-conversation")
        .unwrap()
        .unwrap()
        .conversation;
    assert!(single_root
        .messages
        .iter()
        .any(|message| message.id == "root-user-0"));
    assert!(!single_root
        .messages
        .iter()
        .any(|message| message.id == result.projection_message_id));

    let listed_roots = reopened.load_conversation_views().unwrap();
    assert_eq!(listed_roots.len(), 1);
    assert!(listed_roots[0]
        .conversation
        .messages
        .iter()
        .any(|message| message.id == "root-user-0"));
    assert!(!listed_roots[0]
        .conversation
        .messages
        .iter()
        .any(|message| message.id == result.projection_message_id));

    reopened
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE conversations
                 SET title = 'Durable child result title', updated_at = updated_at + 1
                 WHERE id = 'root-conversation'",
            [],
        )
        .unwrap();
    let hidden_transport_hits = reopened
        .search_chats(&ChatSearchInput {
            query: "durable child result".to_string(),
            limit: Some(10),
        })
        .unwrap();
    assert_eq!(hidden_transport_hits.len(), 1);
    assert_eq!(
        hidden_transport_hits[0].conversation_id,
        "root-conversation"
    );
    assert_eq!(hidden_transport_hits[0].message_id, None);
    assert_eq!(hidden_transport_hits[0].snippet, None);
    let human_hits = reopened
        .search_chats(&ChatSearchInput {
            query: "question 0".to_string(),
            limit: Some(10),
        })
        .unwrap();
    assert_eq!(human_hits.len(), 1);
    assert_eq!(human_hits[0].message_id.as_deref(), Some("root-user-0"));

    // The exact child observer read remains unfiltered and preserves both live Agent and
    // historical snapshot provenance for renderer labels and audit.
    let observer = reopened
        .load_conversation_observer_snapshot(&child.agent.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        observer
            .input_origins
            .get(&child.task_message.projection_message_id),
        Some(&crate::ConversationMessageOrigin::Agent {
            sender_agent_id: "agent-root".to_string(),
            source_agent_message_id: child.task_message.message_id.clone(),
        })
    );
    let snapshot_message = observer
        .conversation
        .messages
        .iter()
        .find(|message| message.content == "question 0")
        .expect("historical input remains visible to the child observer");
    assert_eq!(
        observer.input_origins.get(&snapshot_message.id),
        Some(&crate::ConversationMessageOrigin::HistoricalSnapshot {
            source_conversation_id: "root-conversation".to_string(),
            source_message_id: "root-user-0".to_string(),
            original: Box::new(crate::ConversationMessageOrigin::Human),
        })
    );
}
