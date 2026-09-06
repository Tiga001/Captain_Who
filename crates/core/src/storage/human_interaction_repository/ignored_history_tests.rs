use super::*;
use crate::storage::{conversation_model_context_repository, conversation_trace_repository};
use crate::{
    AgentSamplingBoundaryRequest, ConversationBackendStatePlacement, ConversationTurnTraceItem,
};

fn terminal(connection: &Connection, assistant: &str) {
    connection.execute("UPDATE conversation_turn_traces SET terminal_status='completed',completed_at=20 WHERE assistant_message_id=?1",[assistant]).unwrap();
    connection
        .execute(
            "UPDATE messages SET status='sent',content='Done' WHERE id=?1",
            [assistant],
        )
        .unwrap();
}

fn boundary() -> AgentSamplingBoundaryRequest {
    AgentSamplingBoundaryRequest {
        conversation_id: "chat".into(),
        run_id: "run".into(),
        assistant_message_id: "assistant".into(),
        model_batch_index: 1,
        expected_next_trace_sequence: 0,
    }
}

#[test]
fn ignored_idle_history_is_one_postlude_and_survives_reopen() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "ask");
    terminal(&fixture.connect(), "assistant");
    let input = ignored(&request, "ignore");
    ignore(&mut fixture.connect(), &input, 30).unwrap();
    ignore(&mut fixture.connect(), &input, 40).unwrap();
    let connection = fixture.connect();
    let trace = conversation_trace_repository::get_trace_for_message(&connection, "assistant")
        .unwrap()
        .unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [ConversationTurnTraceItem::BackendState {
            placement: ConversationBackendStatePlacement::AfterMessage,
            created_at: 30,
            ..
        }]
    ));
    let log = conversation_model_context_repository::get_log_for_message(&connection, "assistant")
        .unwrap()
        .unwrap();
    assert_eq!(log.items.len(), 1);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&log.items[0].content).unwrap(),
        serde_json::json!({"type":"human_interaction_status","requestId":request.request_id,"status":"ignored"})
    );
    assert_eq!(count(&connection, "human_interaction_deliveries"), 0);
    assert_eq!(
        count(&connection, "human_interaction_ignored_projections"),
        1
    );
}

#[test]
fn ignored_active_history_binds_once_at_an_exact_natural_boundary() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "ask");
    ignore(&mut fixture.connect(), &ignored(&request, "ignore"), 30).unwrap();
    assert_eq!(
        count(&fixture.connect(), "conversation_turn_trace_items"),
        0
    );
    let mut stale = boundary();
    stale.expected_next_trace_sequence = 1;
    assert!(bind_ignored_at_sampling(&mut fixture.connect(), &stale, 31).is_err());
    let events = bind_ignored_at_sampling(&mut fixture.connect(), &boundary(), 32).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].request_id, request.request_id);
    let replay = bind_ignored_at_sampling(&mut fixture.connect(), &boundary(), 33).unwrap();
    assert_eq!(replay[0].event_id, events[0].event_id);
    assert_eq!(
        count(&fixture.connect(), "conversation_turn_trace_items"),
        1
    );
    let mut next = boundary();
    next.model_batch_index = 2;
    next.expected_next_trace_sequence = 1;
    assert!(bind_ignored_at_sampling(&mut fixture.connect(), &next, 34)
        .unwrap()
        .is_empty());
    let trace =
        conversation_trace_repository::get_trace_for_message(&fixture.connect(), "assistant")
            .unwrap()
            .unwrap();
    assert!(matches!(
        trace.items[0],
        ConversationTurnTraceItem::BackendState {
            placement: ConversationBackendStatePlacement::Timeline,
            ..
        }
    ));
}

#[test]
fn ignored_without_further_sampling_flushes_on_terminal_and_never_reappears_after_delete() {
    let fixture = Fixture::new();
    let request = fixture.request(HumanInteractionMode::Async, "ask");
    let connection = fixture.connect();
    terminal(&connection, "assistant");
    connection
        .execute("UPDATE messages SET position=0 WHERE id='assistant'", [])
        .unwrap();
    insert_trace(&connection, "chat", "later-run", "later-assistant");
    drop(connection);
    ignore(&mut fixture.connect(), &ignored(&request, "ignore"), 30).unwrap();
    let connection = fixture.connect();
    assert_eq!(
        connection
            .query_row(
                "SELECT target_assistant_message_id FROM human_interaction_ignored_projections",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "later-assistant"
    );
    terminal(&connection, "later-assistant");
    flush_ignored_for_terminal(&connection, "later-assistant", 40).unwrap();
    flush_ignored_for_terminal(&connection, "later-assistant", 41).unwrap();
    assert_eq!(count(&connection, "conversation_turn_trace_items"), 1);
    let trace =
        conversation_trace_repository::get_trace_for_message(&connection, "later-assistant")
            .unwrap()
            .unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [ConversationTurnTraceItem::BackendState {
            sequence: 0,
            placement: ConversationBackendStatePlacement::AfterMessage,
            created_at: 30,
            ..
        }]
    ));
    let receipt: (String, String, u64, Option<u64>) = connection
        .query_row(
            "SELECT status,placement,trace_sequence,model_batch_index
             FROM human_interaction_ignored_projections",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap();
    assert_eq!(
        receipt,
        ("materialized".into(), "after_message".into(), 0, None)
    );
    connection
        .execute("DELETE FROM messages WHERE id='later-assistant'", [])
        .unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT status FROM human_interaction_ignored_projections",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "materialized"
    );
    assert_eq!(count(&connection, "human_interaction_responses"), 1);
}
