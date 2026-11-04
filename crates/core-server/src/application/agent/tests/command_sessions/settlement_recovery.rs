use super::*;

#[test]
fn running_transition_failure_has_one_failed_terminal_across_host_and_storage() {
    let fixture = RunningFixture::new("running-transition-failure");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_running_rejection(&database, "reject_session_running_transition");
    let tracker = Arc::new(FileEffectTracker::default());
    let command = approved_command(&fixture.call_id, "printf early-undurable-output; sleep 30");
    let mut file_effect_guard = tracker.register(
        None,
        Some(&fixture.conversation_id),
        &fixture.run_id,
        &fixture.call_id,
    );
    file_effect_guard.mark_effects_started();
    let mut file_effect_guard = Some(file_effect_guard);
    let (notifications, mut receiver) = crate::transport::outbound_channel();

    let launch = fixture
        .registry
        .start(StartAgentCommandSession {
            owner: CommandSessionOwner {
                conversation_id: fixture.conversation_id.clone(),
                assistant_message_id: fixture.assistant_message_id.clone(),
                origin_run_id: fixture.run_id.clone(),
                call_id: fixture.call_id.clone(),
                project_id: None,
            },
            workspace_root: Some(fixture.workspace.path()),
            command: &command,
            permissions: test_permissions(),
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({
                "source": "explicit_user",
                "status": "approved"
            }),
            artifact_runtime: None,
            office_engine: None,
            file_inputs: None,
            notifications: Some(notifications),
            cancellation_token: AgentCancellationToken::new(),
            cancel_probe: None,
            file_effect_guard: &mut file_effect_guard,
        })
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("a failed running transition must not produce a Running receipt")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(terminal
        .execution
        .error
        .as_deref()
        .is_some_and(|error| error.contains("durable start state could not be persisted")));
    assert!(terminal.snapshot.latest_output_sequence > 0);
    assert!(terminal.snapshot.output_truncated);
    assert!(
        file_effect_guard.is_none(),
        "an existing Session row transfers File Effect ownership to terminal settlement"
    );

    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("the created Session row is terminally settled");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.latest_sequence > 0);
    assert!(record.snapshot.output_truncated);
    assert!(record
        .terminal_reason
        .as_deref()
        .is_some_and(|error| error.contains("durable start state could not be persisted")));
    assert!(record.snapshot.archive_ref.is_some());
    let host = fixture
        .registry
        .get(AgentCommandSessionGetInput {
            conversation_id: fixture.conversation_id.clone(),
            session_id: terminal.snapshot.session_id.to_string(),
            after_sequence: None,
            max_bytes: None,
        })
        .unwrap();
    assert!(host.session.output_truncated);
    assert!(host.transcript.chunks.is_empty());
    let model = fixture
        .registry
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: format!("{}-failure-poll", fixture.run_id),
                call_id: format!("{}-failure-poll", fixture.call_id),
                session_id: terminal.snapshot.session_id.to_string(),
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
        .unwrap();
    assert_eq!(model.status, AgentCommandSessionStatus::Failed);
    assert!(model.output_truncated);
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "command_started"));
    let terminal_events = events
        .iter()
        .filter(|event| event["params"]["type"] == "command_exited")
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(terminal_events[0]["params"]["status"], "failed");
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "command_interrupted"));
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn synchronous_archive_failure_returns_recoverable_session_until_unique_archive_recovers() {
    let fixture = RunningFixture::new_with_initial_yield(
        "synchronous-archive-failure",
        Duration::from_secs(2),
    );
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_synchronous_session_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let command = r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "archive-retry-%05d\n", i }'"#;
    let command_for_start = command.to_string();
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            &command_for_start,
            Some(notifications),
            &tracker_for_start,
        )
    });

    let deadline = Instant::now() + TEST_WAIT;
    let session_id = loop {
        if let Ok(event) = receiver.try_recv() {
            if event["params"]["type"] == "command_started" {
                break event["params"]["sessionId"].as_str().unwrap().to_string();
            }
        }
        assert!(Instant::now() < deadline, "missing command_started");
        thread::sleep(Duration::from_millis(5));
    };
    let launch = start
        .join()
        .expect("synchronous archive failure thread")
        .expect("archive failure remains a recoverable Session launch");
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("an unsettled terminal archive must return a recoverable Session")
    };
    assert_eq!(snapshot.session_id, session_id);
    let running = tool_result.result.as_ref().expect("running receipt");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], session_id);
    assert_eq!(running["continueWith"]["tool"], "command_session");
    assert_eq!(
        running["continueWith"]["args"],
        json!({ "sessionId": session_id, "action": "wait" })
    );
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
    wait_for_settlement_attempts(&fixture.registry, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 1);

    database
        .execute_batch("DROP TRIGGER reject_synchronous_session_archive;")
        .unwrap();
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("recovered Session owns the unique Exact Archive");
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("recovered Exact Archive descriptor");
    assert!(descriptor.total_bytes > 200_000);
    // The SQLite terminal commit precedes release of in-memory process ownership. Wait for that
    // separate cleanup boundary before checking for retained leases and worker entries.
    wait_for_terminal_cleanup(&fixture.registry);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());

    let restarted_storage = Arc::new(StorageService::open(&fixture.database_path).unwrap());
    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&restarted_storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(
            AgentCommandSessionExecutionRequest {
                conversation_id: fixture.conversation_id.clone(),
                run_id: "run-synchronous-archive-recovery-read".to_string(),
                call_id: "call-synchronous-archive-recovery-read".to_string(),
                session_id,
                action: AgentCommandSessionAction::Poll,
                wait_ms: 0,
                max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
            },
            observation_control(),
        )
        .unwrap();
    assert_eq!(replay.status, AgentCommandSessionStatus::Exited);
    let history_open = replay
        .history_open
        .as_deref()
        .expect("restarted Host exposes the recovered archive route");
    let tail = restarted_storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("recovered archive remains readable after a true Storage restart");
    assert!(tail.content.contains("archive-retry-19999"));
}

#[test]
fn synchronous_settlement_failure_serializes_concurrent_cancel_with_recovery_receipt() {
    let fixture = RunningFixture::new_with_initial_yield(
        "synchronous-settlement-cancel-fence",
        Duration::from_secs(2),
    );
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_cancel_fenced_session_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let settlement_entered = Arc::new(Barrier::new(2));
    let settlement_release = Arc::new(Barrier::new(2));
    let entered_for_hook = Arc::clone(&settlement_entered);
    let release_for_hook = Arc::clone(&settlement_release);
    fixture
        .registry
        .set_before_synchronous_settlement_hook(Arc::new(move |_| {
            entered_for_hook.wait();
            release_for_hook.wait();
        }));

    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let command = "printf cancel-fenced-terminal";
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            command,
            Some(notifications),
            &tracker_for_start,
        )
    });

    settlement_entered.wait();
    let cancel_started = Arc::new(Barrier::new(2));
    let cancel_started_in_thread = Arc::clone(&cancel_started);
    let cancel_registry = fixture.registry.clone();
    let cancel_run_id = fixture.run_id.clone();
    let (cancelled_tx, cancelled_rx) = std::sync::mpsc::channel();
    let cancel = thread::spawn(move || {
        cancel_started_in_thread.wait();
        cancelled_tx
            .send(cancel_registry.cancel_pre_handoff_for_run(&cancel_run_id))
            .unwrap();
    });
    cancel_started.wait();
    assert!(
        cancelled_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "cancellation must wait until the recoverable receipt is complete"
    );
    settlement_release.wait();
    assert_eq!(cancelled_rx.recv_timeout(TEST_WAIT).unwrap(), 1);
    cancel.join().unwrap();

    let launch = start.join().unwrap().unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("settlement failure must expose its stable recovery receipt before cancellation")
    };
    let running = tool_result.result.as_ref().expect("running receipt");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], snapshot.session_id);
    assert_eq!(
        running["continueWith"]["args"]["sessionId"],
        snapshot.session_id
    );

    database
        .execute_batch("DROP TRIGGER reject_cancel_fenced_session_archive;")
        .unwrap();
    assert_eq!(
        handoff_guard.commit(|| false, || Ok(())).unwrap(),
        AgentCommandHandoffOutcome::CancelledBeforeCommit
    );
    let terminal = handoff_guard.abort_before_handoff().unwrap();
    assert_eq!(terminal.snapshot.session_id.as_str(), snapshot.session_id);
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);

    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let terminal_events = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| {
            matches!(
                event["params"]["type"].as_str(),
                Some("command_exited" | "command_interrupted")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn cancelled_pending_handoff_guard_drop_retries_one_terminal_settlement() {
    let fixture = RunningFixture::new("cancelled-handoff-drop-retry");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_archive_rejection(&database, "reject_cancelled_handoff_drop_archive");
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        "sleep 30",
        Some(notifications),
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        handoff_guard,
        ..
    } = launch
    else {
        panic!("the pending handoff command must still be running")
    };

    // The explicit cancellation wins first and records Pending -> Aborted. A caller panic then
    // drops the still-armed guard. The Aborted drop path must open the settlement fence; otherwise
    // the terminal process, FileEffect guard, and admission lease remain retained forever.
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );
    drop(handoff_guard);
    wait_for_settlement_attempts(&fixture.registry, 1);
    let pending = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("failed terminal commit keeps the durable running row");
    assert_eq!(pending.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(fixture.registry.retained_live_session_count(), 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );

    database
        .execute_batch("DROP TRIGGER reject_cancelled_handoff_drop_archive;")
        .unwrap();
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let terminal_events = std::iter::from_fn(|| receiver.try_recv().ok())
        .filter(|event| {
            matches!(
                event["params"]["type"].as_str(),
                Some("command_exited" | "command_interrupted")
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn durable_start_failure_archive_retry_returns_one_cleared_failed_execution() {
    let fixture = RunningFixture::new("durable-failure-archive-retry");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_running_rejection(&database, "reject_retry_session_running");
    install_archive_rejection(&database, "reject_durable_failure_archive_once");
    let tracker = Arc::new(FileEffectTracker::default());
    let registry = fixture.registry.clone();
    let workspace = fixture.workspace.path().to_path_buf();
    let conversation_id = fixture.conversation_id.clone();
    let assistant_message_id = fixture.assistant_message_id.clone();
    let run_id = fixture.run_id.clone();
    let call_id = fixture.call_id.clone();
    let tracker_for_start = Arc::clone(&tracker);
    let start = thread::spawn(move || {
        start_owned_session_with_tracker(
            &registry,
            &workspace,
            &conversation_id,
            &assistant_message_id,
            &run_id,
            &call_id,
            r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "durable-failure-%05d\n", i }'; sleep 30"#,
            None,
            &tracker_for_start,
        )
    });

    wait_for_settlement_attempts(&fixture.registry, 1);
    database
        .execute_batch("DROP TRIGGER reject_durable_failure_archive_once;")
        .unwrap();
    let launch = start
        .join()
        .expect("durable failure archive retry")
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("durable-start failure must return one terminal execution after settlement")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        terminal.snapshot.session_id.as_str(),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.archive_ref.is_some());
    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}
