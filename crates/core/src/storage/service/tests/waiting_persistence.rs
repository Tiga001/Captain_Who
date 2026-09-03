use super::*;

const CONVERSATION_ID: &str = "conversation-waiting-persistence";
const ASSISTANT_MESSAGE_ID: &str = "assistant-waiting-persistence";
const RUN_ID: &str = "run-waiting-persistence";
const PENDING_ACTION_ID: &str = "pending-action-waiting-persistence";

fn setup_waiting_turn(service: &StorageService) {
    let mut record = conversation(
        CONVERSATION_ID,
        Some("project-1"),
        "user-waiting-persistence",
    );
    record.messages.push(ChatMessageRecord {
        id: ASSISTANT_MESSAGE_ID.to_string(),
        role: "assistant".to_string(),
        content: "original pending content".to_string(),
        created_at: 2,
        status: Some("streaming".to_string()),
        attachments: Vec::new(),
        agent_run_json: None,
        ui_state_json: None,
    });
    service.save_conversation(record).unwrap();
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        RUN_ID,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
    );
    service
        .append_in_progress_conversation_turn_trace(&trace, 2, 2)
        .unwrap();
    service
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: PENDING_ACTION_ID.to_string(),
            run_id: RUN_ID.to_string(),
            conversation_id: Some(CONVERSATION_ID.to_string()),
            assistant_message_id: Some(ASSISTANT_MESSAGE_ID.to_string()),
            action_type: "command".to_string(),
            tool_name: "run_command".to_string(),
            tool_call_id: Some("call-waiting-persistence".to_string()),
            status: "pending".to_string(),
            target_status: None,
            action_json: "{}".to_string(),
            agent_input_json: "{}".to_string(),
            created_at: 2,
            updated_at: 2,
        })
        .unwrap();
}

fn waiting_usage() -> AgentUsageRecordInsert {
    let mut usage = agent_usage_record(CONVERSATION_ID, ASSISTANT_MESSAGE_ID);
    usage.run_id = RUN_ID.to_string();
    usage.status = Some("waiting_for_approval".to_string());
    usage.completed_at = None;
    usage
}

#[test]
fn waiting_segment_usage_commits_for_the_exact_in_progress_turn() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    let usage = waiting_usage();

    assert_eq!(
        service
            .persist_waiting_segment_usage_if_run_in_progress(&usage)
            .unwrap(),
        AgentWaitingSegmentUsagePersistenceOutcome::Persisted
    );

    let stored = service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status.as_deref(), Some("waiting_for_approval"));
    assert_eq!(stored.completed_at, None);
    assert_eq!(stored.total_tokens, usage.total_tokens);
}

#[test]
fn terminal_turn_rejects_late_waiting_segment_usage_without_regressing_usage() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);

    let mut terminal_trace = service
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    terminal_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Cancelled;
    terminal_trace.terminal_error = Some("cancelled first".to_string());
    service
        .replace_conversation_turn_trace(&terminal_trace, 2, 20)
        .unwrap();

    let mut terminal_usage = waiting_usage();
    terminal_usage.status = Some("cancelled".to_string());
    terminal_usage.completed_at = Some(20);
    terminal_usage.error = Some("cancelled first".to_string());
    terminal_usage.input_tokens = Some(101);
    terminal_usage.output_tokens = Some(29);
    terminal_usage.total_tokens = Some(130);
    terminal_usage.billable_request_count = 2;
    service.upsert_agent_usage(terminal_usage).unwrap();

    let mut late_waiting_usage = waiting_usage();
    late_waiting_usage.input_tokens = Some(999);
    late_waiting_usage.output_tokens = Some(999);
    late_waiting_usage.total_tokens = Some(1_998);
    late_waiting_usage.billable_request_count = 99;
    assert_eq!(
        service
            .persist_waiting_segment_usage_if_run_in_progress(&late_waiting_usage)
            .unwrap(),
        AgentWaitingSegmentUsagePersistenceOutcome::TurnTerminal
    );

    let stored = service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(stored.status.as_deref(), Some("cancelled"));
    assert_eq!(stored.completed_at, Some(20));
    assert_eq!(stored.error.as_deref(), Some("cancelled first"));
    assert_eq!(stored.input_tokens, Some(101));
    assert_eq!(stored.output_tokens, Some(29));
    assert_eq!(stored.total_tokens, Some(130));
    assert_eq!(stored.billable_request_count, 2);
}

fn assistant_message(service: &StorageService) -> ChatMessageRecord {
    service
        .load_conversation(CONVERSATION_ID)
        .unwrap()
        .unwrap()
        .messages
        .into_iter()
        .find(|message| message.id == ASSISTANT_MESSAGE_ID)
        .unwrap()
}

fn raw_message_and_conversation_state(service: &StorageService) -> (String, Option<String>, i64) {
    service
        .state
        .connection()
        .unwrap()
        .query_row(
            "
            SELECT message.content, message.status, conversation.updated_at
            FROM messages AS message
            JOIN conversations AS conversation ON conversation.id = message.conversation_id
            WHERE message.id = ?1 AND message.conversation_id = ?2
            ",
            rusqlite::params![ASSISTANT_MESSAGE_ID, CONVERSATION_ID],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

#[test]
fn waiting_projection_and_usage_commit_for_the_exact_in_progress_turn() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    let usage = waiting_usage();

    assert_eq!(
        service
            .persist_waiting_for_approval_if_run_in_progress(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                PENDING_ACTION_ID,
                "approval required",
                Some("pending"),
                50,
                Some(&usage),
            )
            .unwrap(),
        AgentWaitingForApprovalPersistenceOutcome::Persisted
    );

    let message = assistant_message(&service);
    assert_eq!(message.content, "approval required");
    assert_eq!(message.status.as_deref(), Some("pending"));
    assert_eq!(
        service
            .load_conversation(CONVERSATION_ID)
            .unwrap()
            .unwrap()
            .updated_at,
        50
    );
    let stored_usage = service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(stored_usage.status.as_deref(), Some("waiting_for_approval"));
}

#[test]
fn exact_terminal_turn_rejects_late_waiting_projection_without_overwriting_state() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    let mut terminal_trace = service
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    terminal_trace.terminal_status = crate::ConversationTurnTraceTerminalStatus::Cancelled;
    terminal_trace.terminal_error = Some("cancelled first".to_string());
    service
        .replace_conversation_turn_trace(&terminal_trace, 2, 20)
        .unwrap();
    let state_before_late_write = raw_message_and_conversation_state(&service);
    let usage = waiting_usage();

    assert_eq!(
        service
            .persist_waiting_for_approval_if_run_in_progress(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                PENDING_ACTION_ID,
                "stale waiting content",
                Some("pending"),
                50,
                Some(&usage),
            )
            .unwrap(),
        AgentWaitingForApprovalPersistenceOutcome::TurnTerminal
    );

    assert_eq!(
        raw_message_and_conversation_state(&service),
        state_before_late_write
    );
    assert!(service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .is_none());
}

#[test]
fn missing_or_mismatched_turn_identity_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);

    for (conversation_id, assistant_message_id, run_id) in [
        (CONVERSATION_ID, "assistant-missing", RUN_ID),
        ("conversation-foreign", ASSISTANT_MESSAGE_ID, RUN_ID),
        (CONVERSATION_ID, ASSISTANT_MESSAGE_ID, "run-foreign"),
    ] {
        let error = service
            .persist_waiting_for_approval_if_run_in_progress(
                conversation_id,
                assistant_message_id,
                run_id,
                PENDING_ACTION_ID,
                "must not persist",
                Some("pending"),
                50,
                None,
            )
            .unwrap_err();
        assert!(
            error.contains("does not exist") || error.contains("owned by another"),
            "{error}"
        );
    }

    let message = assistant_message(&service);
    assert_eq!(message.content, "original pending content");
}

#[test]
fn mismatched_usage_owner_fails_closed_without_updating_the_message() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    let mut usage = waiting_usage();
    usage.run_id = "run-foreign".to_string();

    let error = service
        .persist_waiting_for_approval_if_run_in_progress(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            RUN_ID,
            PENDING_ACTION_ID,
            "must not persist",
            Some("pending"),
            50,
            Some(&usage),
        )
        .unwrap_err();
    assert!(error.contains("usage owner"), "{error}");
    assert_eq!(
        raw_message_and_conversation_state(&service),
        (
            "original pending content".to_string(),
            Some("streaming".to_string()),
            1,
        )
    );
}

#[test]
fn usage_failure_rolls_back_the_waiting_message_and_conversation_updates() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);

    let other_conversation_id = "conversation-existing-usage";
    let other_message_id = "message-existing-usage";
    service
        .save_conversation(conversation(
            other_conversation_id,
            Some("project-1"),
            other_message_id,
        ))
        .unwrap();
    let mut existing_usage = agent_usage_record(other_conversation_id, other_message_id);
    existing_usage.id = "colliding-usage-id".to_string();
    existing_usage.run_id = "run-existing-usage".to_string();
    service.upsert_agent_usage(existing_usage).unwrap();

    let mut colliding_usage = waiting_usage();
    colliding_usage.id = "colliding-usage-id".to_string();
    assert!(service
        .persist_waiting_for_approval_if_run_in_progress(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            RUN_ID,
            PENDING_ACTION_ID,
            "must roll back",
            Some("pending"),
            50,
            Some(&colliding_usage),
        )
        .is_err());

    assert_eq!(
        raw_message_and_conversation_state(&service),
        (
            "original pending content".to_string(),
            Some("streaming".to_string()),
            1,
        )
    );
    assert!(service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .is_none());
}

#[test]
fn advanced_approval_rejects_the_old_waiting_projection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    service
        .transition_pending_agent_action(PENDING_ACTION_ID, "pending", "approved", "{}", 20)
        .unwrap();
    let state_before_late_write = raw_message_and_conversation_state(&service);
    let usage = waiting_usage();

    assert_eq!(
        service
            .persist_waiting_for_approval_if_run_in_progress(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                PENDING_ACTION_ID,
                "stale waiting content",
                Some("pending"),
                50,
                Some(&usage),
            )
            .unwrap(),
        AgentWaitingForApprovalPersistenceOutcome::PendingActionAdvanced
    );
    assert_eq!(
        raw_message_and_conversation_state(&service),
        state_before_late_write
    );
    assert!(service
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .is_none());
}

#[test]
fn targeted_pending_action_rejects_the_old_waiting_projection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    service
        .set_pending_agent_action_target_status(PENDING_ACTION_ID, "pending", "cancelled", 20)
        .unwrap();
    let state_before_late_write = raw_message_and_conversation_state(&service);

    assert_eq!(
        service
            .persist_waiting_for_approval_if_run_in_progress(
                CONVERSATION_ID,
                ASSISTANT_MESSAGE_ID,
                RUN_ID,
                PENDING_ACTION_ID,
                "stale waiting content",
                Some("pending"),
                50,
                None,
            )
            .unwrap(),
        AgentWaitingForApprovalPersistenceOutcome::PendingActionAdvanced
    );
    assert_eq!(
        raw_message_and_conversation_state(&service),
        state_before_late_write
    );
}

#[test]
fn missing_or_mismatched_pending_action_generation_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);

    let missing_error = service
        .persist_waiting_for_approval_if_run_in_progress(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            RUN_ID,
            "pending-action-missing",
            "must not persist",
            Some("pending"),
            50,
            None,
        )
        .unwrap_err();
    assert!(missing_error.contains("does not exist"), "{missing_error}");

    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE agent_pending_actions SET assistant_message_id = 'assistant-foreign' WHERE action_id = ?1",
            [PENDING_ACTION_ID],
        )
        .unwrap();
    let mismatched_error = service
        .persist_waiting_for_approval_if_run_in_progress(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            RUN_ID,
            PENDING_ACTION_ID,
            "must not persist",
            Some("pending"),
            50,
            None,
        )
        .unwrap_err();
    assert!(
        mismatched_error.contains("owned by another Turn"),
        "{mismatched_error}"
    );
    assert_eq!(
        raw_message_and_conversation_state(&service),
        (
            "original pending content".to_string(),
            Some("streaming".to_string()),
            1,
        )
    );
}

#[test]
fn invalid_pending_action_lifecycle_fails_closed() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    setup_waiting_turn(&service);
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE agent_pending_actions SET status = 'unknown' WHERE action_id = ?1",
            [PENDING_ACTION_ID],
        )
        .unwrap();

    let error = service
        .persist_waiting_for_approval_if_run_in_progress(
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            RUN_ID,
            PENDING_ACTION_ID,
            "must not persist",
            Some("pending"),
            50,
            None,
        )
        .unwrap_err();
    assert!(error.contains("invalid status"), "{error}");
    assert_eq!(
        raw_message_and_conversation_state(&service),
        (
            "original pending content".to_string(),
            Some("streaming".to_string()),
            1,
        )
    );
}
