use super::*;

#[test]
fn short_command_exits_through_the_same_managed_session_entry() {
    let fixture =
        RunningFixture::new_with_initial_yield("short-managed-entry", Duration::from_millis(500));
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        "printf short-managed-output",
        Some(notifications),
    )
    .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("short command must exit inside the initial yield")
    };
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(terminal.execution.stdout, "short-managed-output");

    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("short command still has one durable managed Session receipt");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));

    let event_types = std::iter::from_fn(|| receiver.try_recv().ok())
        .map(|event| event["params"]["type"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(event_types.iter().any(|kind| kind == "command_started"));
    assert!(event_types.iter().any(|kind| kind == "command_exited"));
}

#[test]
fn handed_off_office_artifact_observation_reaches_event_snapshot_and_model_wait() {
    let fixture = RunningFixture::new("office-artifact-observation");
    write_minimal_presentation(&fixture.workspace.path().join("source.pptx"));
    let command_text = "sleep 0.08; cp source.pptx edited.pptx; sleep 0.08";
    let mut command = approved_command(&fixture.call_id, command_text);
    command.observe = Some(AgentCommandArtifactObservationRequest {
        kinds: vec![AgentCommandArtifactObservationKind::Office],
        expected_outputs: vec!["edited.pptx".to_string()],
        additional_roots: Vec::new(),
    });
    let tracker = Arc::new(FileEffectTracker::default());
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let launch = start_owned_session_with_request_and_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &command,
        Some(notifications),
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        mut handoff_guard,
        ..
    } = launch
    else {
        panic!("the delayed presentation copy must hand off")
    };
    adopt_owned_session(
        &fixture.storage,
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        &snapshot,
        command_text,
        &mut handoff_guard,
    );

    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let observation = record
        .snapshot
        .artifact_observation
        .as_ref()
        .expect("terminal Session snapshot retains the validated Office observation");
    let expected = observation
        .expected_outputs
        .first()
        .expect("the approved presentation destination remains explicit");
    assert_eq!(expected.path.as_deref(), Some("edited.pptx"));
    assert_eq!(
        expected.outcome,
        AgentCommandExpectedArtifactOutcomeKind::Created
    );
    assert_eq!(
        expected.artifact_kind,
        Some(AgentCommandArtifactKind::Presentation)
    );
    assert_eq!(
        expected
            .metadata
            .as_ref()
            .map(|metadata| metadata.validation.status),
        Some(AgentCommandArtifactValidationStatus::Valid)
    );

    let reopened = StorageService::open(&fixture.database_path).unwrap();
    let restored = reopened
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("terminal Session survives Host reopen");
    assert_eq!(
        restored.snapshot.artifact_observation,
        Some(observation.clone())
    );

    let model = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert_eq!(model.status, AgentCommandSessionStatus::Exited);
    assert_eq!(model.exit_code, Some(0));
    assert_eq!(model.artifact_observation, Some(observation.clone()));

    let terminal_event = std::iter::from_fn(|| receiver.try_recv().ok())
        .find(|event| event["params"]["type"] == "command_exited")
        .expect("terminal notification");
    assert_eq!(terminal_event["params"]["status"], "exited");
    assert_eq!(terminal_event["params"]["exitCode"], 0);
    assert_eq!(
        terminal_event["params"]["artifactObservation"]["expectedOutputs"][0]["artifactKind"],
        "presentation"
    );
    assert_eq!(
        terminal_event["params"]["artifactObservation"]["expectedOutputs"][0]["metadata"]
            ["validation"]["status"],
        "valid"
    );
}

#[test]
fn short_large_output_is_archived_before_terminal_visibility_without_a_second_exact_body() {
    let fixture = RunningFixture::new_with_initial_yield("short-exact-cut", Duration::from_secs(2));
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        r#"awk 'BEGIN { for (i = 0; i < 20000; i++) printf "archive-line-%05d\n", i }'"#,
        Some(notifications),
    )
    .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("large but short command must exit inside the initial yield")
    };
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert!(terminal.execution.stdout_truncated);
    assert!(!terminal.execution.stdout_spool.is_present());
    assert!(!terminal.execution.stderr_spool.is_present());

    // `start` must not expose the terminal outcome until the immutable archive and authoritative
    // Session/Trace cut have both committed.
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("short command Session receipt");
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("short terminal cut includes its Exact History ref");
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("short command Exact History descriptor");
    let tail_start = descriptor.total_chars.saturating_sub(8 * 1024);
    let tail = fixture
        .storage
        .read_conversation_history_archive_page(
            &fixture.conversation_id,
            archive_ref,
            ConversationHistoryArchivePageUnit::Char,
            tail_start,
            8 * 1024,
        )
        .unwrap()
        .expect("short command Exact History tail");
    assert!(tail.content.contains("archive-line-19999"));

    // The ordinary run_command audit can retain its bounded preview, but the already-consumed
    // complete-output spools must not materialize a second full-body archive sidecar.
    let ordinary_tool_result =
        mycopilot_core::command::command_tool_result(&fixture.call_id, &terminal.execution);
    assert!(ordinary_tool_result.exact_archive_file.is_none());
    let ordinary_body = ordinary_tool_result
        .result
        .as_ref()
        .expect("terminal run_command body");
    let history_open = ordinary_body["historyOpen"]
        .as_str()
        .expect("terminal result carries the authoritative history route");
    assert!(history_open.starts_with("hist_v1_"));
    assert!(ordinary_body.get("continueWith").is_none());
    let routed_page = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("internal archive route resolves");
    assert!(routed_page.content.contains("archive-line-19999"));
    assert_eq!(routed_page.descriptor.archive_ref, archive_ref);
    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(events.iter().any(|event| {
        event["params"]["type"] == "command_exited"
            && event["params"]["sessionId"] == terminal.snapshot.session_id.as_str()
    }));
}

#[test]
fn fast_four_megabyte_output_hands_off_until_the_host_queue_reaches_terminal() {
    let fixture = RunningFixture::new_with_initial_yield(
        "fast-four-megabyte-handoff",
        Duration::from_secs(2),
    );
    let persisted_chunks = Arc::new(AtomicUsize::new(0));
    let observed_chunks = Arc::clone(&persisted_chunks);
    fixture
        .registry
        .set_before_output_persistence_hook(Arc::new(move |_, _| {
            observed_chunks.fetch_add(1, Ordering::Relaxed);
            // A 4+ MiB producer fills the bounded lifecycle queue. Slowing each durable append
            // makes Core's terminal callback observable before the Host writer reaches the final
            // queue item, reproducing the real pdftotext settlement cut deterministically.
            thread::sleep(Duration::from_millis(21));
        }));
    let command = r#"awk 'BEGIN { for (i = 0; i < 100000; i++) printf "pdftotext-page-%06d-abcdefghijklmnopqrstuvwxyz\n", i }'"#;

    let launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        command,
        None,
    )
    .expect("a delayed Host terminal must remain a recoverable launch");
    let AgentCommandSessionLaunch::Running {
        snapshot,
        tool_result,
        mut handoff_guard,
    } = launch
    else {
        panic!("the delayed Host terminal must not be projected as an empty exited result")
    };
    let running = tool_result
        .result
        .as_ref()
        .expect("recoverable running ToolResult");
    assert_eq!(running["status"], "running");
    assert_eq!(running["sessionId"], snapshot.session_id);
    assert_eq!(running["continueWith"]["tool"], "command_session");
    assert_eq!(
        running["continueWith"]["args"],
        json!({
            "sessionId": snapshot.session_id,
            "action": "wait"
        })
    );
    assert!(
        running["outputTruncated"].as_bool().unwrap(),
        "a saturated Host lifecycle queue must be explicit"
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
    let record = wait_for_terminal_record_with_timeout(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
        Duration::from_secs(15),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    assert!(persisted_chunks.load(Ordering::Relaxed) > 1);

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-fast-four-megabyte-terminal-read".to_string(),
        call_id: "call-fast-four-megabyte-terminal-read".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let terminal = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    let history_open = terminal
        .history_open
        .as_deref()
        .expect("terminal wait exposes the exact-output recovery route");
    let tail = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("the exact output remains readable through Host archive storage");
    assert!(tail
        .content
        .contains("pdftotext-page-099999-abcdefghijklmnopqrstuvwxyz"));

    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            rusqlite::params![&fixture.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(replay, terminal);
    assert_eq!(replay.history_open.as_deref(), Some(history_open));
}

#[test]
fn handed_off_terminal_output_is_archived_exactly_once() {
    let fixture = RunningFixture::new("terminal-archive");
    let command = "sleep 0.08; printf archive-marker; sleep 0.08";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let archive_ref = record
        .snapshot
        .archive_ref
        .as_deref()
        .expect("terminal archive ref");
    let descriptor = fixture
        .storage
        .find_conversation_history_archive_by_ref(&fixture.conversation_id, archive_ref)
        .unwrap()
        .expect("terminal archive descriptor");
    let page = fixture
        .storage
        .read_conversation_history_archive_page(
            &fixture.conversation_id,
            archive_ref,
            ConversationHistoryArchivePageUnit::Char,
            0,
            descriptor.total_chars,
        )
        .unwrap()
        .expect("terminal archive page");
    assert!(page.content.contains("archive-marker"));

    let connection = Connection::open(&fixture.database_path).unwrap();
    let archive_count: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs
             WHERE conversation_id = ?1 AND call_id = ?2",
            rusqlite::params![
                &fixture.conversation_id,
                format!("command-session:{}", snapshot.session_id)
            ],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);

    let trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("command session trace");
    let terminal_events = trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id,
                    ..
                } if session_id == &snapshot.session_id
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}

#[test]
fn background_terminal_host_read_retains_deterministic_internal_archive_route() {
    let fixture = RunningFixture::new("terminal-model-history-route");
    let command = r#"sleep 0.08; awk 'BEGIN { for (i = 0; i < 20000; i++) printf "background-line-%05d\n", i }'"#;
    let (snapshot, initial_result) = fixture.start(command, None);
    assert_eq!(initial_result.result.as_ref().unwrap()["status"], "running");

    let running = fixture.poll(&snapshot.session_id, Duration::ZERO);
    assert_eq!(running.status, AgentCommandSessionStatus::Running);
    assert!(running.history_open.is_none());
    fixture.adopt(&snapshot, command);
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );

    let request = AgentCommandSessionExecutionRequest {
        conversation_id: fixture.conversation_id.clone(),
        run_id: "run-terminal-history-route".to_string(),
        call_id: "call-terminal-history-route".to_string(),
        session_id: snapshot.session_id.clone(),
        action: AgentCommandSessionAction::Poll,
        wait_ms: 0,
        max_output_bytes: AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES,
    };
    let terminal = fixture
        .registry
        .execute_command_session(request.clone(), observation_control())
        .unwrap();
    assert_eq!(terminal.status, AgentCommandSessionStatus::Exited);
    assert!(terminal.output.len() <= AGENT_COMMAND_SESSION_MODEL_OUTPUT_BYTES);
    let history_open = terminal
        .history_open
        .as_deref()
        .expect("terminal Host result retains its internal archive route");
    assert!(history_open.starts_with("hist_v1_"));
    let full = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("internal archive route resolves after background completion");
    assert!(full.content.contains("background-line-19999"));

    let restarted = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&fixture.storage),
        CommandSessionManager::new(test_manager_config()).unwrap(),
        INITIAL_YIELD,
    );
    let replay = restarted
        .execute_command_session(request, observation_control())
        .unwrap();
    assert_eq!(replay, terminal);
    let replay_open = replay.history_open.as_deref().unwrap();
    let replayed_full = fixture
        .storage
        .read_conversation_history_archive_page_from_open(
            &fixture.conversation_id,
            replay_open,
            u64::MAX,
        )
        .unwrap()
        .expect("same opaque route remains readable after registry restart");
    assert!(replayed_full.content.contains("background-line-19999"));
}
