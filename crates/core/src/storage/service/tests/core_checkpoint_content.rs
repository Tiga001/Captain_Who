use super::*;

const CONVERSATION_ID: &str = "checkpoint-content-conversation";
const ASSISTANT_ID: &str = "checkpoint-content-assistant";
const RUN_ID: &str = "checkpoint-content-run";
const CONFIRMED: &str = "Synthetic text already acknowledged by Core.";

fn setup(service: &StorageService, core_owned: bool) -> ConversationTurnTrace {
    let mut record = conversation(
        CONVERSATION_ID,
        Some("project-1"),
        "checkpoint-content-user",
    );
    record.messages.push(ChatMessageRecord {
        human_interaction_response: None,
        id: ASSISTANT_ID.into(),
        role: "assistant".into(),
        content: CONFIRMED.into(),
        created_at: 2,
        status: Some("streaming".into()),
        attachments: Vec::new(),
        folder_references_json: None,
        agent_run_json: Some(
            chat_repository::canonical_agent_run_lifecycle_projection(
                None, RUN_ID, "running", 2, 2, None,
            )
            .unwrap(),
        ),
        ui_state_json: None,
    });
    service.save_conversation(record).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_ID,
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();
    if core_owned {
        let state = service
            .begin_runtime_checkpoint(CONVERSATION_ID, ASSISTANT_ID, RUN_ID)
            .unwrap()
            .unwrap();
        assert_eq!(
            service
                .apply_runtime_checkpoint(&CoreRuntimeCheckpoint {
                    conversation_id: CONVERSATION_ID.into(),
                    assistant_message_id: ASSISTANT_ID.into(),
                    run_id: RUN_ID.into(),
                    generation: state.generation,
                    expected_sequence: 0,
                    sequence: 1,
                    content: CONFIRMED.into(),
                    projection_json: "{}".into(),
                })
                .unwrap(),
            CoreRuntimeCheckpointOutcome::Persisted
        );
    }
    trace
}

fn stored_message(service: &StorageService) -> (String, Option<String>, serde_json::Value) {
    service
        .state
        .read_connection()
        .unwrap()
        .query_row(
            "SELECT content,status,agent_run_json FROM messages WHERE conversation_id=?1 AND id=?2",
            rusqlite::params![CONVERSATION_ID, ASSISTANT_ID],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    serde_json::from_str(&row.get::<_, String>(2)?).unwrap(),
                ))
            },
        )
        .unwrap()
}

#[test]
fn cancelled_empty_output_preserves_only_core_checkpoint_content_on_disk() {
    for (core_owned, status, output, expected) in [
        (true, "cancelled", "", CONFIRMED),
        (false, "cancelled", "", ""),
        (
            true,
            "cancelled",
            "explicit final text",
            "explicit final text",
        ),
        (true, "completed", "", ""),
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let mut trace = setup(&service, core_owned);
        trace.terminal_status = if status == "cancelled" {
            crate::ConversationTurnTraceTerminalStatus::Cancelled
        } else {
            crate::ConversationTurnTraceTerminalStatus::Completed
        };
        service
            .finalize_chat_message_with_conversation_trace(
                CONVERSATION_ID,
                ASSISTANT_ID,
                output,
                Some("sent"),
                status,
                &trace,
                2,
                20,
            )
            .unwrap();
        drop(service);
        let reopened = fixture.service();
        let (content, message_status, run) = stored_message(&reopened);
        assert_eq!(content, expected, "core={core_owned}, status={status}");
        assert_eq!(message_status.as_deref(), Some("sent"));
        assert_eq!(run["status"], status);
        assert_eq!(run["completedAt"], 20);
        assert_eq!(
            reopened
                .get_conversation_turn_trace(ASSISTANT_ID)
                .unwrap()
                .unwrap(),
            trace
        );
        assert!(reopened
            .get_conversation_model_context_log(ASSISTANT_ID)
            .unwrap()
            .is_none());
    }
}

#[test]
fn approval_empty_output_preserves_only_core_checkpoint_content_on_disk() {
    for (core_owned, output, expected) in [
        (true, "", CONFIRMED),
        (false, "", ""),
        (true, "explicit approval text", "explicit approval text"),
    ] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let trace = setup(&service, core_owned);
        service
            .store_pending_agent_action(AgentPendingActionRecord {
                action_id: "checkpoint-pending-action".into(),
                run_id: RUN_ID.into(),
                conversation_id: Some(CONVERSATION_ID.into()),
                assistant_message_id: Some(ASSISTANT_ID.into()),
                action_type: "command".into(),
                tool_name: "run_command".into(),
                tool_call_id: Some("checkpoint-call".into()),
                status: "pending".into(),
                target_status: None,
                action_json: "{}".into(),
                agent_input_json: "{}".into(),
                created_at: 2,
                updated_at: 2,
            })
            .unwrap();
        assert_eq!(
            service
                .persist_waiting_for_approval_if_run_in_progress(
                    CONVERSATION_ID,
                    ASSISTANT_ID,
                    RUN_ID,
                    "checkpoint-pending-action",
                    output,
                    Some("pending"),
                    20,
                    None,
                )
                .unwrap(),
            AgentWaitingForApprovalPersistenceOutcome::Persisted
        );
        drop(service);
        let reopened = fixture.service();
        let (content, message_status, _) = stored_message(&reopened);
        assert_eq!(content, expected, "core={core_owned}");
        assert_eq!(message_status.as_deref(), Some("pending"));
        assert_eq!(
            reopened
                .get_conversation_turn_trace(ASSISTANT_ID)
                .unwrap()
                .unwrap(),
            trace
        );
        assert!(reopened
            .get_conversation_model_context_log(ASSISTANT_ID)
            .unwrap()
            .is_none());
    }
}
