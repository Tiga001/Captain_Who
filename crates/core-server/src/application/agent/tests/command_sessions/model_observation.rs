use super::*;

#[test]
fn silent_long_command_returns_running_and_emits_started() {
    let fixture = RunningFixture::new("silent-started");
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let (snapshot, tool_result) = fixture.start("sleep 5", Some(notifications));

    assert_eq!(snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(
        tool_result.result.as_ref().unwrap()["status"],
        Value::String("running".to_string())
    );
    assert_eq!(
        tool_result.result.as_ref().unwrap()["sessionId"],
        snapshot.session_id
    );

    let started = receiver.try_recv().expect("command_started notification");
    assert_eq!(started["params"]["type"], "command_started");
    assert_eq!(started["params"]["sessionId"], snapshot.session_id);
    assert_eq!(started["params"]["callId"], fixture.call_id);
    assert!(
        receiver.try_recv().is_err(),
        "a silent process must not need output to publish command_started"
    );

    let terminal = fixture.abort(&snapshot.session_id);
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
}

#[test]
fn initial_running_output_cut_is_durable_and_replayable_before_handoff() {
    let fixture = RunningFixture::new_with_initial_yield(
        "initial-running-receipt",
        Duration::from_millis(250),
    );
    let command = "printf initial-receipt-marker; sleep 0.50; printf later-marker; sleep 5";
    let (snapshot, tool_result) = fixture.start(command, None);
    let result = tool_result.result.as_ref().expect("running ToolResult");
    assert!(result["output"]
        .as_str()
        .expect("running output")
        .contains("initial-receipt-marker"));
    assert!(!result["output"]
        .as_str()
        .expect("running output")
        .contains("later-marker"));

    let request = AgentCommandSessionModelReadRequest {
        conversation_id: &fixture.conversation_id,
        session_id: &snapshot.session_id,
        run_id: &fixture.run_id,
        call_id: &fixture.call_id,
        action: AgentCommandSessionAction::Poll,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
        host_output_truncated: false,
        created_at: now_ms(),
    };
    let initial_read = fixture
        .storage
        .load_agent_command_session_model_read(&request)
        .unwrap()
        .expect("initial run_command model-read receipt");
    let initial_output = initial_read
        .chunks
        .iter()
        .map(|chunk| chunk.output.as_str())
        .collect::<String>();
    assert_eq!(initial_output, result["output"]);
    assert_eq!(
        initial_read.receipt.latest_sequence,
        result["latestSequence"].as_u64().unwrap()
    );
    assert_eq!(
        initial_read.receipt.output_truncated,
        result["outputTruncated"].as_bool().unwrap()
    );

    let deadline = Instant::now() + TEST_WAIT;
    loop {
        let host_view = fixture
            .registry
            .get(AgentCommandSessionGetInput {
                conversation_id: fixture.conversation_id.clone(),
                session_id: snapshot.session_id.clone(),
                after_sequence: initial_read.receipt.last_output_sequence,
                max_bytes: None,
            })
            .unwrap();
        if host_view
            .transcript
            .chunks
            .iter()
            .any(|chunk| chunk.output.contains("later-marker"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "later output was not durably visible"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Reopening storage simulates the crash boundary before `commit_handoff`. The original
    // run/call identity must replay the immutable cut, not expand it to include later output.
    let reopened = StorageService::open(&fixture.database_path).unwrap();
    let replay = reopened
        .read_or_create_agent_command_session_model_read(&request)
        .unwrap()
        .expect("replayed initial running receipt");
    assert_eq!(replay, initial_read);

    fixture.abort(&snapshot.session_id);
}

#[test]
fn model_poll_is_incremental_until_terminal() {
    let fixture = RunningFixture::new("incremental-poll");
    let command = "sleep 0.10; printf first; sleep 0.40; printf second";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);

    let first = poll_until(&fixture, &snapshot.session_id, |output| {
        output.output.contains("first")
    });
    assert!(!first.output.contains("second"));

    let second = poll_until(&fixture, &snapshot.session_id, |output| {
        output.output.contains("second")
    });
    assert!(
        !second.output.contains("first"),
        "model polling must advance one independent durable cursor"
    );

    let terminal = poll_until(&fixture, &snapshot.session_id, |output| {
        output.status.is_terminal()
    });
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    assert_eq!(terminal.exit_code, Some(0));
}

#[test]
fn model_wait_ignores_noisy_output_until_its_deadline() {
    let fixture = RunningFixture::new("quiet-noisy-wait");
    let command = "printf 'tick-0\\n'; sleep 0.08; printf 'tick-1\\n'; sleep 0.08; \
                   printf 'tick-2\\n'; sleep 0.08; printf 'tick-3\\n'; sleep 0.08; \
                   printf 'tick-4\\n'; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let started = Instant::now();
    let output = fixture.poll(&snapshot.session_id, Duration::from_millis(300));
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(220),
        "ordinary output returned the model wait early after {elapsed:?}"
    );
    assert_eq!(output.status, AgentCommandSessionStatus::Running);
    assert!(output.output.contains("tick-"));
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
}

#[test]
fn cancelling_model_observation_does_not_terminate_handed_off_process() {
    let fixture = RunningFixture::new("cancel-observation-keeps-process");
    let command = "sleep 0.05; printf before; sleep 0.80; printf after";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);
    let cursor_before = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap()
        .model_read_sequence;

    let cancellation = mycopilot_core::AgentCancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let registry = fixture.registry.clone();
    let conversation_id = fixture.conversation_id.clone();
    let session_id = snapshot.session_id.clone();
    let worker = thread::spawn(move || {
        registry.execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id,
                run_id: "run-cancel-observation".to_string(),
                call_id: "call-cancel-observation".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 300_000,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            AgentCommandSessionExecutionControl::new(worker_cancellation, None),
        )
    });
    thread::sleep(Duration::from_millis(100));
    let cancelled_at = Instant::now();
    cancellation.cancel();
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert!(
        cancelled_at.elapsed() < Duration::from_millis(500),
        "cancelling a model observation must release it promptly"
    );

    let live = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(live.session.status, AgentCommandSessionStatus::Running);
    let durable_after_cancel = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap();
    assert_eq!(durable_after_cancel.model_read_sequence, cursor_before);
    assert!(fixture
        .storage
        .load_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &fixture.conversation_id,
            session_id: &snapshot.session_id,
            run_id: "run-cancel-observation",
            call_id: "call-cancel-observation",
            action: AgentCommandSessionAction::Poll,
            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            host_output_truncated: false,
            created_at: 0,
        })
        .unwrap()
        .is_none());

    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);
    let final_read = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert!(final_read.output.contains("before"));
    assert!(final_read.output.contains("after"));
}

#[test]
fn explicit_run_cancel_releases_model_wait_and_interrupts_its_handed_off_process() {
    let fixture = RunningFixture::new("explicit-cancel-during-model-wait");
    let mut service = AgentService::new_authorized_for_test(Arc::clone(&fixture.storage));
    service.command_sessions = fixture.registry.clone();
    let command = "sleep 5";
    let tracker = Arc::new(FileEffectTracker::default());
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        command,
        None,
        &tracker,
    )
    .unwrap();
    let (snapshot, mut handoff_guard) = match launch {
        AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard,
            ..
        } => (*snapshot, handoff_guard),
        AgentCommandSessionLaunch::Exited(terminal) => panic!(
            "command must still be running, got {:?}",
            terminal.snapshot.state
        ),
    };
    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command,
        &mut handoff_guard,
    );
    let cursor_before = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .unwrap()
        .model_read_sequence;

    let cancellation = mycopilot_core::AgentCancellationToken::new();
    service.register_cancellation(&fixture.run_id, cancellation.clone());
    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: fixture.run_id.clone(),
        call_id: "call-explicit-cancel-wait".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 300_000,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let worker_request = request.clone();
    let worker_registry = fixture.registry.clone();
    let worker_cancellation = cancellation.clone();
    let worker = thread::spawn(move || {
        worker_registry.execute_command_session(
            worker_request,
            AgentCommandSessionExecutionControl::new(worker_cancellation, None),
        )
    });
    thread::sleep(Duration::from_millis(100));

    let cancelled_at = Instant::now();
    assert!(service.cancel_run(&fixture.run_id));
    let error = worker.join().unwrap().unwrap_err();
    assert!(error.is_cancelled());
    assert!(
        cancelled_at.elapsed() < Duration::from_millis(500),
        "input-composer stop must release a quiet model wait promptly"
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
    assert_eq!(terminal.model_read_sequence, cursor_before);
    assert!(fixture
        .storage
        .load_agent_command_session_model_read(&AgentCommandSessionModelReadRequest {
            conversation_id: &request.conversation_id,
            session_id: &request.session_id,
            run_id: &request.run_id,
            call_id: &request.call_id,
            action: request.action,
            max_output_bytes: request.max_output_bytes,
            host_output_truncated: false,
            created_at: 0,
        })
        .unwrap()
        .is_none());
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    service.unregister_cancellation_if_current(&fixture.run_id, &cancellation);
}

#[test]
fn trusted_child_interrupt_terminates_its_handed_off_command_session() {
    let fixture = RunningFixture::new("child-interrupt-adopted-session");
    let mut service = AgentService::new_authorized_for_test(Arc::clone(&fixture.storage));
    service.command_sessions = fixture.registry.clone();
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert!(service
        .interrupt_agent_wake_run(&fixture.run_id)
        .unwrap()
        .any_effect());
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        terminal.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn queued_guidance_releases_a_noisy_running_wait_without_stopping_process() {
    let fixture = RunningFixture::new("guidance-releases-wait");
    let command = "printf 'noise-0\\n'; sleep 0.10; printf 'noise-1\\n'; sleep 0.10; \
                   printf 'noise-2\\n'; sleep 0.10; printf 'noise-3\\n'; sleep 0.10; \
                   printf 'noise-4\\n'; sleep 0.10; printf 'noise-5\\n'; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let guidance = mycopilot_core::AgentSteerInputQueue::new();
    let registry = fixture.registry.clone();
    let conversation_id = fixture.conversation_id.clone();
    let session_id = snapshot.session_id.clone();
    let worker_guidance = guidance.clone();
    let worker = thread::spawn(move || {
        registry.execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id,
                run_id: "run-guidance-observation".to_string(),
                call_id: "call-guidance-observation".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 300_000,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            AgentCommandSessionExecutionControl::new(
                mycopilot_core::AgentCancellationToken::new(),
                Some(worker_guidance),
            ),
        )
    });
    thread::sleep(Duration::from_millis(120));
    let queued_at = Instant::now();
    guidance
        .enqueue(mycopilot_core::AgentSteerInput {
            guidance_id: "guidance-command-wait".to_string(),
            client_message_id: "client-command-wait".to_string(),
            content: "Apply this updated constraint.".to_string(),
            attachments: Vec::new(),
            folder_references: Vec::new(),
            attachment_library: None,
            created_at: 10,
        })
        .unwrap();
    let output = worker.join().unwrap().unwrap();

    assert!(
        queued_at.elapsed() < Duration::from_millis(500),
        "queued guidance must release the current model wait promptly"
    );
    assert_eq!(output.status, AgentCommandSessionStatus::Running);
    assert!(guidance.pending_len() > 0);
    let live = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: snapshot.session_id.clone(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert_eq!(live.session.status, AgentCommandSessionStatus::Running);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
}

#[test]
fn concurrent_model_polls_share_one_serialized_durable_cursor() {
    let fixture = RunningFixture::new("concurrent-model-poll");
    let command = "sleep 0.08; printf serialized-marker; sleep 0.40";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let deadline = Instant::now() + TEST_WAIT;
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
            .any(|chunk| chunk.output.contains("serialized-marker"))
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "command output was not persisted"
        );
        thread::sleep(Duration::from_millis(10));
    }

    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|index| {
            let registry = fixture.registry.clone();
            let barrier = Arc::clone(&barrier);
            let conversation_id = fixture.conversation_id.clone();
            let session_id = snapshot.session_id.clone();
            thread::spawn(move || {
                barrier.wait();
                registry
                    .execute_command_session(
                        AgentCommandSessionExecutionRequest {
                            conversation_id,
                            run_id: format!("run-concurrent-poll-{index}"),
                            call_id: format!("call-concurrent-poll-{index}"),
                            session_id,
                            action: AgentCommandSessionAction::Poll,
                            wait_ms: 0,
                            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                        },
                        observation_control(),
                    )
                    .unwrap()
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let outputs = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        outputs
            .iter()
            .map(|output| output.output.matches("serialized-marker").count())
            .sum::<usize>(),
        1,
        "concurrent polls must not return the same durable output twice"
    );
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
}

#[test]
fn concurrent_terminal_polls_share_one_serialized_durable_cursor() {
    let fixture = RunningFixture::new("concurrent-terminal-poll");
    let command = "sleep 0.08; printf terminal-serialized-marker";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);
    let terminal = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(terminal.snapshot.status, AgentCommandSessionStatus::Exited);

    // A fresh registry has no live HostCommandSession for this already-terminal row. Cursor
    // serialization must therefore belong to the durable Session identity, not the OS-process
    // wrapper which was removed at terminal settlement.
    let registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let barrier = Arc::new(Barrier::new(3));
    let workers = (0..2)
        .map(|index| {
            let registry = registry.clone();
            let barrier = Arc::clone(&barrier);
            let conversation_id = fixture.conversation_id.clone();
            let session_id = snapshot.session_id.clone();
            thread::spawn(move || {
                barrier.wait();
                registry
                    .execute_command_session(
                        AgentCommandSessionExecutionRequest {
                            conversation_id,
                            run_id: format!("run-concurrent-terminal-poll-{index}"),
                            call_id: format!("call-concurrent-terminal-poll-{index}"),
                            session_id,
                            action: AgentCommandSessionAction::Poll,
                            wait_ms: 0,
                            max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
                        },
                        observation_control(),
                    )
                    .expect("concurrent terminal poll must not expose a cursor CAS conflict")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let outputs = workers
        .into_iter()
        .map(|worker| worker.join().unwrap())
        .collect::<Vec<_>>();

    assert!(outputs.iter().all(|output| output.status.is_terminal()));
    assert_eq!(
        outputs
            .iter()
            .map(|output| { output.output.matches("terminal-serialized-marker").count() })
            .sum::<usize>(),
        1,
        "terminal output must be returned exactly once across concurrent model polls"
    );
}

#[test]
fn retried_tool_call_replays_its_committed_cut_after_registry_restart() {
    let fixture = RunningFixture::new("poll-receipt-restart");
    let command = "sleep 0.08; printf crash-safe-marker";
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["output"], "");
    fixture.adopt(&snapshot, command);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-crash-retry".to_string(),
        call_id: "call-crash-retry".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let before_restart = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(before_restart.output, "crash-safe-marker");

    let restarted_registry = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let after_restart = restarted_registry
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(after_restart, before_restart);

    let next_call = restarted_registry
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: "run-crash-retry".to_string(),
                call_id: "call-after-crash-retry".to_string(),
                session_id: snapshot.session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
        .unwrap();
    assert!(next_call.output.is_empty());
}
