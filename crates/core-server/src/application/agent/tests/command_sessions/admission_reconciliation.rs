use super::*;

#[test]
fn short_command_create_failure_returns_failed_execution_without_session_leaks() {
    let fixture =
        RunningFixture::new_with_initial_yield("short-create-failure", Duration::from_secs(1));
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_short_session_create");
    let tracker = Arc::new(FileEffectTracker::default());

    let terminal = assert_session_create_failure_is_fail_closed(
        &fixture,
        &tracker,
        "printf short-create-failure-output",
    );
    // Preserve the real process evidence, but never let exit 0 turn the ToolResult into success.
    assert_eq!(terminal.snapshot.exit_code, Some(0));
    assert_eq!(terminal.execution.exit_code, Some(0));
    assert_eq!(terminal.execution.stdout, "short-create-failure-output");
}

#[test]
fn long_command_create_failure_is_terminated_without_session_leaks() {
    let fixture = RunningFixture::new("long-create-failure");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_long_session_create");
    let tracker = Arc::new(FileEffectTracker::default());

    let terminal = assert_session_create_failure_is_fail_closed(&fixture, &tracker, "sleep 30");
    assert!(terminal.snapshot.exit_code.is_none());
    assert!(terminal.execution.exit_code.is_none());
}

#[test]
fn create_commit_unknown_is_recovered_by_authoritative_owner_readback() {
    let fixture = RunningFixture::new("create-commit-unknown");
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));

    let (snapshot, tool_result) = fixture.start("sleep 30", None);
    assert_eq!(snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(tool_result.result.as_ref().unwrap()["status"], "running");
    let record = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("the post-commit row is authoritatively recovered");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(record.snapshot.conversation_id, fixture.conversation_id);
    assert_eq!(
        record.snapshot.assistant_message_id,
        fixture.assistant_message_id
    );
    assert_eq!(record.snapshot.origin_run_id, fixture.run_id);
    assert_eq!(record.snapshot.call_id, fixture.call_id);

    let terminal = fixture.abort(&snapshot.session_id);
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Interrupted
    ));
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(
        record.snapshot.status,
        AgentCommandSessionStatus::Interrupted
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
}

#[test]
fn indeterminate_reconciliation_cannot_settle_before_file_effect_install() {
    let fixture =
        RunningFixture::new_with_initial_yield("indeterminate-start-fence", Duration::from_secs(2));
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let reconcile_entered = Arc::new(Barrier::new(2));
    let reconcile_release = Arc::new(Barrier::new(2));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let hook_entered = Arc::clone(&reconcile_entered);
    let hook_release = Arc::clone(&reconcile_release);
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |_| {
            match hook_attempts.fetch_add(1, Ordering::SeqCst) {
                0 => return Err("injected initial authoritative read failure".to_string()),
                1 => {
                    hook_entered.wait();
                    hook_release.wait();
                }
                _ => {}
            }
            Ok(())
        }));

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
            "printf indeterminate-start-fence",
            None,
            &tracker_for_start,
        )
    });

    reconcile_entered.wait();
    // The worker has reached authoritative readback but cannot settle yet. File Effect and Host
    // admission must already be installed, closing the exact wait-boundary race.
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    assert_eq!(fixture.registry.retained_live_session_count(), 1);
    assert_eq!(
        tracker.active_run_ids_for_conversation(&fixture.conversation_id),
        vec![fixture.run_id.clone()]
    );
    reconcile_release.wait();

    let launch = start.join().expect("indeterminate start thread").unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("the recovered starting row must settle as one Failed terminal")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    let record = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        terminal.snapshot.session_id.as_str(),
    );
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn indeterminate_then_absent_synthesizes_one_settled_failed_session() {
    let fixture = RunningFixture::new("indeterminate-then-absent");
    let database = Connection::open(&fixture.database_path).unwrap();
    install_session_create_rejection(&database, "reject_indeterminate_absent_create");
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let database_path = fixture.database_path.clone();
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |_| {
            if hook_attempts.fetch_add(1, Ordering::SeqCst) == 0 {
                Err("injected initial authoritative read failure".to_string())
            } else {
                Connection::open(&database_path)
                    .unwrap()
                    .execute_batch("DROP TRIGGER reject_indeterminate_absent_create;")
                    .unwrap();
                Ok(())
            }
        }));
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
            notifications: None,
            cancellation_token: AgentCancellationToken::new(),
            cancel_probe: None,
            file_effect_guard: &mut file_effect_guard,
        })
        .unwrap();
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("authoritative Absent reconciliation must return a failed execution")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    assert!(terminal.snapshot.latest_output_sequence > 0);
    assert!(terminal.snapshot.output_truncated);
    assert!(file_effect_guard.is_none());
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.retained_core_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);

    drop(file_effect_guard);
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("reconciliation synthesizes a recoverable Session row");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert!(record.snapshot.latest_sequence > 0);
    assert!(record.snapshot.output_truncated);
    assert!(record.snapshot.archive_ref.is_some());
    assert!(tracker
        .active_run_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}

#[test]
fn indeterminate_mismatched_start_identity_is_never_adopted() {
    let fixture = RunningFixture::new_with_initial_yield(
        "indeterminate-mismatched-identity",
        Duration::from_secs(2),
    );
    fixture
        .registry
        .set_after_durable_create_hook(Arc::new(|_| {
            Err("injected post-commit create error".to_string())
        }));
    let inspection_attempts = Arc::new(AtomicUsize::new(0));
    let hook_attempts = Arc::clone(&inspection_attempts);
    let database_path = fixture.database_path.clone();
    fixture
        .registry
        .set_durable_start_inspection_hook(Arc::new(move |session_id| {
            let database = Connection::open(&database_path).unwrap();
            match hook_attempts.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    database
                        .execute_batch(
                            "DROP TRIGGER prevent_agent_command_session_identity_update;",
                        )
                        .unwrap();
                    database
                        .execute(
                            "UPDATE agent_command_sessions
                             SET command_projection = 'tampered-command'
                             WHERE session_id = ?1",
                            rusqlite::params![session_id],
                        )
                        .unwrap();
                }
                1 => {
                    database
                        .execute(
                            "DELETE FROM agent_command_sessions WHERE session_id = ?1",
                            rusqlite::params![session_id],
                        )
                        .unwrap();
                }
                _ => {}
            }
            Ok(())
        }));
    let tracker = Arc::new(FileEffectTracker::default());
    let command = "printf identity-mismatch";

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
    let AgentCommandSessionLaunch::Exited(terminal) = launch else {
        panic!("an identity-mismatched durable row must never produce a Running receipt")
    };
    assert!(matches!(
        terminal.snapshot.state,
        mycopilot_core::command::CommandSessionState::Failed
    ));
    let record = fixture
        .storage
        .load_agent_command_session(
            &fixture.conversation_id,
            terminal.snapshot.session_id.as_str(),
        )
        .unwrap()
        .expect("the canonical immutable start identity is reconstructed");
    assert_eq!(record.snapshot.command, command);
    assert_ne!(record.snapshot.command, "tampered-command");
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Failed);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
    assert_eq!(fixture.registry.retained_live_session_count(), 0);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 0);
    assert!(tracker
        .unsettled_effect_ids_for_conversation(&fixture.conversation_id)
        .is_empty());
}
