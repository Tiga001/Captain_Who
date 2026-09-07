use super::*;

const CONVERSATION_ID: &str = "stream-settlement-conversation";
const ASSISTANT_ID: &str = "stream-settlement-assistant";
const RUN_ID: &str = "stream-settlement-run";

fn partial_answer() -> String {
    "答".repeat(190)
}

fn final_answer() -> String {
    format!("{}那边审批一下，我继续等。", partial_answer())
}

fn stream_projection() -> serde_json::Value {
    serde_json::json!({
        "runId": RUN_ID,
        "status": "running",
        "startedAt": 2,
        "toolDefinitions": [],
        "toolCalls": [],
        "toolResults": [],
        "webSearchActivities": [],
        "readActivities": [],
        "approvals": [],
        "fileChangeProposals": [],
        "fileChanges": [],
        "mcpInvocations": [],
        "messageStreamCheckpoints": {"final-stream": {"previousContent": "earlier answer"}},
        "timeline": [
            {"id": "committed-stream", "type": "message", "streamId": "narration-stream",
             "traceSequence": 0, "content": partial_answer()},
            {"id": "committed-answer-lookalike", "type": "message", "traceSequence": 1,
             "content": final_answer()},
            {"id": "host-note", "type": "message", "content": "Keep an unstreamed presentation note."},
            {"id": "message-stream-final-stream", "type": "message", "streamId": "final-stream",
             "content": partial_answer()}
        ]
    })
}

fn seed_stream_turn(service: &StorageService) -> ConversationTurnTrace {
    assert_eq!(partial_answer().chars().count(), 190);
    assert_eq!(final_answer().chars().count(), 202);
    let mut stored = conversation(CONVERSATION_ID, None, "stream-settlement-user");
    stored.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: ASSISTANT_ID.to_string(),
        role: "assistant".to_string(),
        content: partial_answer(),
        created_at: 2,
        status: Some("pending".to_string()),
        attachments: Vec::new(),
        agent_run_json: Some(stream_projection().to_string()),
        ui_state_json: None,
    });
    service.save_conversation(stored).unwrap();
    let trace = ConversationTurnTrace {
        schema_version: crate::CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: RUN_ID.to_string(),
        conversation_id: CONVERSATION_ID.to_string(),
        assistant_message_id: ASSISTANT_ID.to_string(),
        terminal_status: crate::ConversationTurnTraceTerminalStatus::InProgress,
        terminal_error: None,
        truncated: false,
        items: [partial_answer(), final_answer()]
            .into_iter()
            .enumerate()
            .map(
                |(sequence, content)| ConversationTurnTraceItem::AssistantNarration {
                    provider_turn_id: None,
                    first_tool_call_id: None,
                    sequence: u64::try_from(sequence).unwrap(),
                    content,
                    truncated: false,
                },
            )
            .collect(),
    };
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 3)
        .unwrap();
    trace
}

fn finalize(service: &StorageService, mut trace: ConversationTurnTrace, status: &str) {
    trace.terminal_status = match status {
        "completed" => crate::ConversationTurnTraceTerminalStatus::Completed,
        "failed" => crate::ConversationTurnTraceTerminalStatus::Failed,
        "cancelled" => crate::ConversationTurnTraceTerminalStatus::Cancelled,
        _ => unreachable!(),
    };
    let model_context_items = [partial_answer(), final_answer()]
        .into_iter()
        .enumerate()
        .map(|(sequence, content)| ConversationModelContextItem {
            sequence: u64::try_from(sequence).unwrap(),
            ordinal: 0,
            role: "assistant".to_string(),
            content,
            images: Vec::new(),
            tool_call_id: None,
            tool_calls: Vec::new(),
            is_error: false,
        })
        .collect::<Vec<_>>();
    service
        .finalize_chat_message_with_conversation_trace_model_context_and_usage(
            CONVERSATION_ID,
            ASSISTANT_ID,
            &final_answer(),
            Some(if status == "failed" { "error" } else { "sent" }),
            status,
            &trace,
            Some(&model_context_items),
            2,
            20,
            None,
            None,
        )
        .unwrap();
}

fn assert_settled(message: &ChatMessageRecord) {
    assert_eq!(message.content, final_answer());
    assert_eq!(message.status.as_deref(), Some("sent"));
    let run: serde_json::Value =
        serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
    assert_eq!(run["status"], "completed");
    assert_eq!(run["completedAt"], 20);
    assert_eq!(run["messageStreamCheckpoints"], serde_json::json!({}));
    let timeline = run["timeline"].as_array().unwrap();
    assert_eq!(timeline.len(), 3);
    assert_eq!(timeline[0]["traceSequence"], 0);
    assert_eq!(timeline[0]["content"], partial_answer());
    assert_eq!(timeline[1]["traceSequence"], 1);
    assert_eq!(timeline[1]["content"], final_answer());
    assert_eq!(timeline[2]["id"], "host-note");
}

fn assert_all_views_settled(service: &StorageService) {
    let connection = service.state.connection().unwrap();
    let persisted = chat_repository::get_persisted_conversation(&connection, CONVERSATION_ID)
        .unwrap()
        .unwrap();
    assert_settled(&persisted.messages[1]);
    drop(connection);
    let history = service.load_conversation(CONVERSATION_ID).unwrap().unwrap();
    assert_settled(&history.messages[1]);
    let observer = service
        .load_conversation_observer_snapshot(CONVERSATION_ID)
        .unwrap()
        .unwrap();
    assert_settled(&observer.conversation.messages[1]);
}

#[test]
fn terminal_commit_settles_the_stream_by_identity_and_keeps_real_narration() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trace = seed_stream_turn(&service);
    finalize(&service, trace, "completed");
    assert_all_views_settled(&service);
}

#[test]
fn late_live_and_terminal_checkpoints_cannot_restore_a_partial_completed_answer() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trace = seed_stream_turn(&service);
    finalize(&service, trace, "completed");

    // Renderer snapshots can race with the Host terminal commit even after the Renderer has
    // changed its own status to completed. Neither status nor arrival order makes them final.
    for status in ["running", "completed", "failed", "cancelled"] {
        let mut stale = stream_projection();
        stale["status"] = status.into();
        if status != "running" {
            stale["completedAt"] = 19.into();
        }
        service
            .save_chat_message_state(
                CONVERSATION_ID,
                ChatMessageStateRecord {
                    id: ASSISTANT_ID.to_string(),
                    content: partial_answer(),
                    status: Some(
                        if status == "running" {
                            "pending"
                        } else {
                            "sent"
                        }
                        .into(),
                    ),
                    agent_run_json: Some(stale.to_string()),
                },
            )
            .unwrap();
        assert_all_views_settled(&service);
    }
}

#[test]
fn history_and_observer_rebuild_a_stale_completed_stream_without_mutating_storage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trace = seed_stream_turn(&service);
    finalize(&service, trace, "completed");
    // Reproduce the old durable shape directly: terminal body, but an earlier live suffix.
    let mut old_run = stream_projection();
    old_run["status"] = "completed".into();
    old_run["completedAt"] = 20.into();
    let old_run_json = old_run.to_string();
    {
        let connection = service.state.connection().unwrap();
        connection
            .execute(
                "UPDATE messages SET agent_run_json = ?1 WHERE id = ?2",
                rusqlite::params![old_run_json, ASSISTANT_ID],
            )
            .unwrap();
    }
    let history = service.load_conversation(CONVERSATION_ID).unwrap().unwrap();
    assert_settled(&history.messages[1]);
    let observer = service
        .load_conversation_observer_snapshot(CONVERSATION_ID)
        .unwrap()
        .unwrap();
    assert_settled(&observer.conversation.messages[1]);
    let connection = service.state.connection().unwrap();
    let raw: String = connection
        .query_row(
            "SELECT agent_run_json FROM messages WHERE id = ?1",
            [ASSISTANT_ID],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(raw, old_run_json);
}

#[test]
fn failed_and_cancelled_turns_preserve_partial_output_in_history_and_observer_views() {
    for status in ["failed", "cancelled"] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let trace = seed_stream_turn(&service);
        finalize(&service, trace, status);
        let history = service.load_conversation(CONVERSATION_ID).unwrap().unwrap();
        let observer = service
            .load_conversation_observer_snapshot(CONVERSATION_ID)
            .unwrap()
            .unwrap();
        for message in [&history.messages[1], &observer.conversation.messages[1]] {
            let run: serde_json::Value =
                serde_json::from_str(message.agent_run_json.as_deref().unwrap()).unwrap();
            assert_eq!(run["status"], status);
            assert_eq!(run["timeline"].as_array().unwrap().len(), 4);
            assert_eq!(run["timeline"][3]["streamId"], "final-stream");
            assert_eq!(run["timeline"][3]["content"], partial_answer());
            assert_eq!(
                run["messageStreamCheckpoints"]["final-stream"]["previousContent"],
                "earlier answer"
            );
        }
    }
}
