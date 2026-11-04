use super::*;

#[test]
fn caller_panic_drops_pending_handoff_and_settles_its_process_and_file_effect() {
    let fixture = RunningFixture::new("handoff-panic");
    let tracker = Arc::new(FileEffectTracker::default());
    let captured_session_id = Arc::new(Mutex::new(None::<String>));
    let unwind_session_id = Arc::clone(&captured_session_id);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let launch = start_owned_session_with_tracker(
            &fixture.registry,
            fixture.workspace.path(),
            &fixture.conversation_id,
            &fixture.assistant_message_id,
            &fixture.run_id,
            &fixture.call_id,
            "sleep 5",
            None,
            &tracker,
        )
        .unwrap();
        let AgentCommandSessionLaunch::Running {
            snapshot,
            handoff_guard: _handoff_guard,
            ..
        } = launch
        else {
            panic!("panic-boundary command must still be running")
        };
        *unwind_session_id
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(snapshot.session_id.clone());
        panic!("injected caller panic after the running receipt")
    }));
    assert!(result.is_err());
    let session_id = captured_session_id
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .expect("running Session identity survives caller unwind");
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    // The durable terminal row is committed immediately before the worker releases its
    // in-memory admission lease; observe both boundaries instead of racing the latter.
    wait_for_retained_admission_count(&fixture.registry, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn initial_yield_cancellation_cannot_settle_before_file_effect_ownership_is_installed() {
    let fixture =
        RunningFixture::new_with_initial_yield("initial-yield-cancel", Duration::from_secs(2));
    let tracker = Arc::new(FileEffectTracker::default());
    let hook_entered = Arc::new(Barrier::new(2));
    let hook_release = Arc::new(Barrier::new(2));
    let hook_entered_for_start = Arc::clone(&hook_entered);
    let hook_release_for_start = Arc::clone(&hook_release);
    fixture
        .registry
        .set_before_file_effect_install_hook(Arc::new(move |_| {
            hook_entered_for_start.wait();
            hook_release_for_start.wait();
        }));

    let (notifications, mut receiver) = crate::transport::outbound_channel();
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
            "sleep 5",
            Some(notifications),
            &tracker_for_start,
        )
    });

    let event_deadline = Instant::now() + TEST_WAIT;
    let session_id = loop {
        if let Ok(event) = receiver.try_recv() {
            if event["params"]["type"] == "command_started" {
                break event["params"]["sessionId"]
                    .as_str()
                    .expect("command_started Session identity")
                    .to_string();
            }
        }
        assert!(
            Instant::now() < event_deadline,
            "initial-yield command did not publish command_started"
        );
        thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(
        fixture.registry.cancel_pre_handoff_for_run(&fixture.run_id),
        1
    );

    // The hook is reached only after the Core manager has observed the interrupted terminal result,
    // while the caller-owned FileEffectGuard is deliberately still outside Host state.
    hook_entered.wait();
    assert!(fixture
        .registry
        .wait_for_live_terminal(&session_id, TEST_WAIT));
    thread::sleep(Duration::from_millis(100));
    let pre_install = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &session_id)
        .unwrap()
        .expect("pre-install Session row remains owned by the Host");
    assert_eq!(
        pre_install.snapshot.status,
        AgentCommandSessionStatus::Running
    );
    assert!(pre_install.snapshot.archive_ref.is_none());
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );

    hook_release.wait();
    let launch = start.join().expect("command start thread").unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("initial-yield cancellation must return a terminal launch outcome")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
    let record = wait_for_terminal_record(&fixture.storage, &fixture.conversation_id, &session_id);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert!(record.snapshot.archive_ref.is_some());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn absolute_handoff_deadline_reclaims_a_noisy_session_while_guard_is_alive() {
    let fixture = RunningFixture::new_with_handoff_timeout(
        "handoff-deadline",
        Duration::from_millis(10),
        Duration::from_millis(500),
    );
    let tracker = Arc::new(FileEffectTracker::default());
    let launch = start_owned_session_with_tracker(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        &fixture.run_id,
        &fixture.call_id,
        r#"awk 'BEGIN { for (i = 0; i < 12000; i++) print "deadline-noise" }'; sleep 5"#,
        None,
        &tracker,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot,
        handoff_guard,
        ..
    } = launch
    else {
        panic!("deadline-bound command must still be running")
    };

    // Keep the caller token alive and deliberately omit commit/drop. The Host's absolute deadline
    // must still win over an always-readable output queue.
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    std::hint::black_box(&handoff_guard);
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    // The terminal transaction commits before the lifecycle writer releases its admission lease.
    wait_for_retained_admission_count(&fixture.registry, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    drop(handoff_guard);
}
