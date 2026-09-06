use super::*;
use crate::human_interaction::{
    HumanInteractionIgnoreInput, HumanInteractionMode, HumanInteractionQuestionInput,
    HumanInteractionToolInput,
};
use crate::storage::human_interaction_repository::{self, HostHumanInteractionOwner};

#[test]
fn committed_binding_with_lost_reply_is_adopted_by_stale_terminal_publication_once() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut record = conversation("chat-ignored-finalize", Some("project-1"), "user-ignored");
    record.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: "assistant-ignored".into(),
        role: "assistant".into(),
        content: String::new(),
        created_at: 2,
        status: Some("streaming".into()),
        attachments: vec![],
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(record).unwrap();
    bind_agent_root(&service, "root-ignored", "chat-ignored-finalize");
    let snapshot = crate::ConversationTraceSnapshot::default();
    let mut stale_trace =
        snapshot.in_progress_trace("run-1", "chat-ignored-finalize", "assistant-ignored");
    service
        .append_in_progress_conversation_turn_trace(&stale_trace, 2, 2)
        .unwrap();
    let mut connection = service.state.connection().unwrap();
    let request = human_interaction_repository::create_request(
        &mut connection,
        &HostHumanInteractionOwner {
            agent_id: "root-ignored".into(),
            conversation_id: "chat-ignored-finalize".into(),
            run_id: "run-1".into(),
            assistant_message_id: "assistant-ignored".into(),
            tool_call_id: "ask-ignored".into(),
        },
        HumanInteractionMode::Async,
        &HumanInteractionToolInput {
            questions: vec![HumanInteractionQuestionInput {
                title: "Details?".into(),
                options: None,
            }],
        },
        10,
    )
    .unwrap();
    human_interaction_repository::ignore(
        &mut connection,
        &HumanInteractionIgnoreInput {
            conversation_id: "chat-ignored-finalize".into(),
            request_id: request.request_id,
            expected_revision: request.revision,
            submission_id: "ignore-finalize".into(),
        },
        11,
    )
    .unwrap();
    // Storage committed the binding, but the Runtime never received/adopted its successful reply.
    human_interaction_repository::bind_ignored_at_sampling(
        &mut connection,
        &crate::AgentSamplingBoundaryRequest {
            conversation_id: "chat-ignored-finalize".into(),
            run_id: "run-1".into(),
            assistant_message_id: "assistant-ignored".into(),
            model_batch_index: 1,
            expected_next_trace_sequence: 0,
        },
        12,
    )
    .unwrap();
    drop(connection);
    stale_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Completed;
    let mut usage = agent_usage_record("chat-ignored-finalize", "assistant-ignored");
    usage.started_at = Some(2);
    usage.completed_at = Some(20);
    usage.created_at = 20;
    for _ in 0..2 {
        service
            .finalize_chat_message_with_conversation_trace_model_context_and_usage(
                "chat-ignored-finalize",
                "assistant-ignored",
                "Done",
                Some("sent"),
                "completed",
                &stale_trace,
                Some(&[]),
                2,
                20,
                Some(&usage),
                None,
            )
            .unwrap();
    }
    let stored = service
        .get_conversation_turn_trace("assistant-ignored")
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.terminal_status,
        crate::ConversationTurnTraceTerminalStatus::Completed
    );
    assert_eq!(stored.items.len(), 1);
    let connection = service.state.connection().unwrap();
    let counts: (i64,i64,i64) = connection.query_row(
        "SELECT (SELECT COUNT(*) FROM agent_usage_records),
                (SELECT COUNT(*) FROM human_interaction_ignored_projections WHERE status='materialized'),
                (SELECT COUNT(*) FROM conversation_model_context_items)", [],
        |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)),
    ).unwrap();
    assert_eq!(counts, (1, 1, 1));
    drop(connection);
    assert_eq!(
        service
            .load_agent_usage_for_owner("run-1", "chat-ignored-finalize", "assistant-ignored")
            .unwrap()
            .unwrap()
            .total_tokens,
        Some(20)
    );
}
