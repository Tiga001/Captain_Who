use super::*;

#[test]
fn observer_pages_preserve_order_provenance_attachments_and_full_projection() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 9, true);
    attach_file_to_first_user_message(&fixture, "observer-page", true);
    let mut input = spawn_input("observer-page-child", "paged_child");
    input.fork_turns = AgentForkTurns::All;
    let child = fixture.service.create_child_agent(&input).unwrap();
    let id = &child.agent.conversation_id;
    let full = fixture
        .service
        .load_conversation_observer_snapshot(id)
        .unwrap()
        .unwrap();
    assert!(full.history.is_none());
    let mut before = None;
    let mut messages = Vec::new();
    let mut origins = std::collections::BTreeMap::new();
    loop {
        let page = fixture
            .service
            .load_conversation_observer_snapshot_page(id, Some(3), before.as_deref())
            .unwrap()
            .unwrap();
        assert!(page.conversation.messages.len() <= 3);
        let history = page.history.unwrap();
        assert_eq!(
            history.before_message_id,
            history
                .has_more
                .then(|| page.conversation.messages[0].id.clone())
        );
        let mut older = page.conversation.messages;
        older.append(&mut messages);
        messages = older;
        origins.extend(page.input_origins);
        if !history.has_more {
            break;
        }
        assert_ne!(before, history.before_message_id);
        before = history.before_message_id;
    }
    assert_eq!(
        serde_json::to_value(messages).unwrap(),
        serde_json::to_value(full.conversation.messages).unwrap()
    );
    assert_eq!(origins, full.input_origins);

    // Active tails also survive a one-message latest window and are not timestamp-sorted.
    let active = fixture
        .service
        .load_conversation_observer_snapshot_page("root-conversation", Some(1), None)
        .unwrap()
        .unwrap();
    assert_eq!(active.conversation.messages[0].id, "root-assistant-active");
}

#[test]
fn observer_page_rejects_invalid_and_foreign_cursors_and_limits() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 2, false);
    let child = fixture
        .service
        .create_child_agent(&spawn_input("cursor-child", "cursor_child"))
        .unwrap();
    for (limit, cursor) in [
        (None, Some("root-user-0")),
        (Some(0), None),
        (Some(101), None),
        (Some(1), Some(" ")),
        (Some(1), Some("missing-message")),
        (
            Some(1),
            Some(child.task_message.projection_message_id.as_str()),
        ),
    ] {
        assert!(fixture
            .service
            .load_conversation_observer_snapshot_page("root-conversation", limit, cursor)
            .is_err());
    }
    let oldest = fixture
        .service
        .load_conversation_observer_snapshot_page("root-conversation", Some(2), Some("root-user-0"))
        .unwrap()
        .unwrap();
    assert!(oldest.conversation.messages.is_empty());
    assert!(!oldest.history.unwrap().has_more);
}

#[test]
fn observer_latest_page_does_not_decode_unrequested_old_trace_payloads() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 50, false);
    {
        let connection = fixture.service.state.connection().unwrap();
        // An incompatible old payload must fail a request for that history, but cannot block
        // opening a recent page. This catches loading every trace and truncating afterwards.
        connection.execute("INSERT INTO conversation_turn_trace_items(assistant_message_id,sequence,item_kind,item_json) VALUES('root-assistant-0',0,'assistant_narration','{}')", []).unwrap();
    }
    assert!(fixture
        .service
        .load_conversation_observer_snapshot("root-conversation")
        .is_err());
    let latest = fixture
        .service
        .load_conversation_observer_snapshot_page("root-conversation", Some(4), None)
        .unwrap()
        .unwrap();
    assert_eq!(latest.conversation.messages.len(), 4);
    assert_eq!(latest.conversation.messages[0].id, "root-user-48");
    assert_eq!(latest.input_origins.len(), 2);
    assert!(latest.history.unwrap().has_more);
    assert!(fixture
        .service
        .load_conversation_observer_snapshot_page("root-conversation", Some(2), Some("root-user-1"))
        .is_err());
}

#[test]
fn observer_latest_page_keeps_active_assistant_when_new_inputs_push_it_outside_window() {
    let fixture = Fixture::new(Some("model-a"));
    save_settled_history(&fixture, 3, true);
    {
        let c = fixture.service.state.connection().unwrap();
        for index in 0..12 {
            c.execute("INSERT INTO messages(id,conversation_id,role,content,status,input_origin_kind,created_at,position) VALUES(?1,'root-conversation','user','input received during active turn','sent','human',?2,?3)", rusqlite::params![format!("in-turn-{index}"), 102 + index, 8 + index]).unwrap();
        }
    }
    let latest = fixture
        .service
        .load_conversation_observer_snapshot_page("root-conversation", Some(2), None)
        .unwrap()
        .unwrap();
    assert_eq!(latest.conversation.messages.len(), 13);
    assert_eq!(latest.conversation.messages[0].id, "root-assistant-active");
    assert_eq!(
        latest.conversation.messages.last().unwrap().id,
        "in-turn-11"
    );
    let history = latest.history.unwrap();
    assert_eq!(
        history.before_message_id.as_deref(),
        Some("root-assistant-active")
    );
    let older = fixture
        .service
        .load_conversation_observer_snapshot_page(
            "root-conversation",
            Some(2),
            history.before_message_id.as_deref(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(older.conversation.messages.len(), 2);
    assert!(older
        .conversation
        .messages
        .iter()
        .all(|message| message.id != "root-assistant-active"));
    assert_eq!(
        older.conversation.messages.last().unwrap().id,
        "root-user-active"
    );
}
