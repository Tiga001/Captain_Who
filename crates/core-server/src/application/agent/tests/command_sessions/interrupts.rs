use super::*;

#[test]
fn cross_conversation_session_access_is_rejected() {
    let fixture = RunningFixture::new("conversation-isolation");
    let other_conversation = "conversation-command-session-other";
    seed_conversation(
        &fixture.storage,
        other_conversation,
        "assistant-command-session-other",
    );
    let (snapshot, _) = fixture.start("sleep 5", None);

    let get = fixture.registry.get(AgentCommandSessionGetInput {
        conversation_id: other_conversation.to_string(),
        session_id: snapshot.session_id.clone(),
        after_sequence: None,
        max_bytes: None,
    });
    assert!(get.is_err());

    let model_access = fixture.registry.execute_command_session(
        AgentCommandSessionExecutionRequest {
            conversation_id: other_conversation.to_string(),
            run_id: "run-other".to_string(),
            call_id: "call-other".to_string(),
            session_id: snapshot.session_id.clone(),
            action: AgentCommandSessionAction::Poll,
            wait_ms: 0,
            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
        },
        observation_control(),
    );
    assert_eq!(
        model_access.unwrap_err().code(),
        Some("agent.command_session_not_found")
    );

    fixture.abort(&snapshot.session_id);
}

#[test]
fn adopted_session_ignores_origin_run_pre_handoff_cancellation() {
    let fixture = RunningFixture::new("adopted-cancel");
    let command = "sleep 0.50";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        0
    );
    let current = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(current.session.status, AgentCommandSessionStatus::Running);

    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(terminal.snapshot.exit_code, Some(0));
}

#[test]
fn explicit_run_interrupt_is_scoped_by_conversation_and_origin_run() {
    let fixture = RunningFixture::new("explicit-run-interrupt-scope");
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run("conversation-other", &fixture.run_id),
        0
    );
    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, "run-other"),
        0
    );
    let current = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(current.session.status, AgentCommandSessionStatus::Running);

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        1
    );
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
}

#[test]
fn explicit_run_interrupt_closes_pending_handoff_before_receipt_commit() {
    let fixture = RunningFixture::new("explicit-stop-before-handoff");
    let (snapshot, _) = fixture.start("sleep 5", None);

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        1
    );
    let mut handoff_guard = fixture
        .handoff_guards
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .remove(&snapshot.session_id)
        .expect("running fixture retains the handoff ownership token");
    let outcome = handoff_guard
        .commit(
            || false,
            || panic!("an explicit stop which wins the fence must forbid receipt persistence"),
        )
        .unwrap();
    assert_eq!(outcome, AgentCommandHandoffOutcome::CancelledBeforeCommit);
    let terminal = handoff_guard.abort_before_handoff().unwrap();
    assert_eq!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    );
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
}

#[test]
fn explicit_run_interrupt_stops_every_session_owned_by_the_turn() {
    let fixture = RunningFixture::new("explicit-stop-multiple");
    let command = "sleep 5";
    let (first, _) = fixture.start(command, None);
    fixture.adopt(&first, command);

    let second_assistant_message_id = format!("{}-second", fixture.assistant_message_id);
    let second_call_id = format!("{}-second", fixture.call_id);
    fixture
        .storage
        .upsert_chat_messages(
            &fixture.conversation_id,
            vec![ChatMessageRecord {
                human_interaction_response: None,
                id: second_assistant_message_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 2,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            1,
        )
        .unwrap();
    let second_launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &second_assistant_message_id,
        &fixture.run_id,
        &second_call_id,
        command,
        None,
    )
    .unwrap();
    let (second, mut second_handoff_guard) = match second_launch {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (*snapshot, handoff_guard),
        AgentCommandSessionLaunch::Exited(terminal) => panic!(
            "second command must still be running, got {:?}",
            terminal.snapshot.state
        ),
    };
    assert_eq!(
        second_handoff_guard.commit(|| false, || Ok(())).unwrap(),
        AgentCommandHandoffOutcome::Adopted
    );

    assert_eq!(
        fixture
            .registry
            .interrupt_origin_run(&fixture.conversation_id, &fixture.run_id),
        2
    );
    for session_id in [&first.session_id, &second.session_id] {
        let terminal =
            wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, session_id);
        assert_eq!(
            terminal.snapshot.status,
            AgentCommandSessionStatus::Interrupted
        );
    }
}

#[test]
fn pre_handoff_cancellation_terminates_the_process() {
    let fixture = RunningFixture::new("pending-cancel");
    let (snapshot, _) = fixture.start("sleep 5", None);

    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    let terminal = fixture.abort(&snapshot.session_id);
    assert_eq!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    );
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
}

#[test]
fn pre_handoff_abort_uses_host_terminal_after_core_cleanup() {
    let fixture = RunningFixture::new("pending-cancel-core-cleanup");
    let (snapshot, _) = fixture.start("sleep 5", None);
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    assert!(fixture
        .registry
        .wait_for_live_terminal(&snapshot.session_id, TEST_WAIT));
    let deadline = Instant::now() + TEST_WAIT;
    while !fixture
        .registry
        .remove_terminal_core_session_for_test(&snapshot.session_id)
    {
        assert!(
            Instant::now() < deadline,
            "Core did not release its active process lease"
        );
        thread::yield_now();
    }
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    let terminal = fixture.abort(&snapshot.session_id);
    assert_eq!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    );
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert!(record.snapshot.archive_ref.is_some());
    wait_for_retained_admission_count(&fixture.registry, 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn model_interrupt_terminates_an_adopted_session_and_publishes_one_terminal_event() {
    let fixture = RunningFixture::new("model-interrupt");
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, Some(notifications));
    fixture.adopt(&snapshot, command);

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-model-interrupt".to_string(),
        call_id: "call-model-interrupt".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Interrupt,
        wait_ms: 2_000,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let output = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(output.status, AgentCommandSessionStatus::Interrupted);
    assert_eq!(output.exit_code, None);
    Connection::open(&fixture.database_path)
        .unwrap()
        .execute(
            "DELETE FROM agent_command_session_output_chunks WHERE session_id = ?1",
            [&snapshot.session_id],
        )
        .unwrap();
    assert_eq!(
        fixture
            .registry
            .execute_command_session(request, observation_control())
            .expect("an interrupt ToolCall retry replays its receipt without signaling twice"),
        output
    );

    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert_eq!(
        events
            .iter()
            .filter(|event| event["params"]["type"] == "command_interrupted")
            .count(),
        1
    );
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "command_interrupted"
            && event["params"]["sessionId"] == snapshot.session_id
            && event["params"]["callId"] == fixture.call_id
    }));
}

#[test]
fn interrupt_is_sticky_when_the_first_receipt_write_fails() {
    let fixture = RunningFixture::new("interrupt-receipt-failure");
    let marker_path = fixture.workspace.path().join("interrupt-count.txt");
    let script_path = fixture.workspace.path().join("interrupt-target.sh");
    std::fs::write(
        &script_path,
        format!(
            "#!/bin/sh\nmarker='{}'\ntrap 'printf x >> \"$marker\"' INT\nprintf ready\n\
             while true; do sleep 1; done\n",
            marker_path.to_string_lossy().replace('\'', "'\\''")
        ),
    )
    .unwrap();
    let command = format!(
        "sh '{}'",
        script_path.to_string_lossy().replace('\'', "'\\''")
    );
    let (snapshot, _) = fixture.start(&command, None);
    fixture.adopt(&snapshot, &command);

    let ready_deadline = Instant::now() + TEST_WAIT;
    loop {
        let current = fixture
            .registry
            .get(AgentCommandSessionGetInput {
                conversation_id: fixture.conversation_id.clone(),
                session_id: snapshot.session_id.clone(),
                after_sequence: Some(0),
                max_bytes: None,
            })
            .unwrap();
        if current
            .transcript
            .chunks
            .iter()
            .any(|chunk| chunk.output.contains("ready"))
        {
            break;
        }
        assert!(
            Instant::now() < ready_deadline,
            "command did not become ready"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let failure_connection = Connection::open(&fixture.database_path).unwrap();
    failure_connection
        .execute_batch(
            "CREATE TRIGGER fail_test_command_session_receipt_insert
             BEFORE INSERT ON agent_command_session_model_read_receipts
             BEGIN
                 SELECT RAISE(ABORT, 'injected receipt persistence failure');
             END;",
        )
        .unwrap();
    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-interrupt-receipt-failure".to_string(),
        call_id: "call-interrupt-receipt-failure".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Interrupt,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    assert!(fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .is_err());
    failure_connection
        .execute_batch("DROP TRIGGER fail_test_command_session_receipt_insert;")
        .unwrap();

    let retry = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .expect("retry should observe the sticky interrupt and persist its receipt");
    assert_eq!(
        fixture
            .registry
            .execute_command_session(request, observation_control())
            .expect("the committed interrupt receipt must replay exactly"),
        retry
    );
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(std::fs::read_to_string(marker_path).unwrap(), "x");
}
