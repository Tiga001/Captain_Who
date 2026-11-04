use super::*;

#[test]
fn transient_terminal_cas_failure_retries_without_duplicate_archive_or_lifecycle() {
    let fixture = RunningFixture::new("terminal-retry");
    let command = "sleep 0.20; printf retry-archive-marker";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    database.busy_timeout(Duration::from_secs(2)).unwrap();
    database
        .execute_batch(
            "CREATE TRIGGER reject_command_session_terminal_once_enabled
             BEFORE UPDATE OF status ON agent_command_sessions
             WHEN OLD.status IN ('starting', 'running')
              AND NEW.status NOT IN ('starting', 'running')
             BEGIN
               SELECT RAISE(ABORT, 'injected transient terminal CAS failure');
             END;",
        )
        .unwrap();
    fixture.adopt(&snapshot, command);

    let archive_call_id = format!("command-session:{}", snapshot.session_id);
    let retry_deadline = Instant::now() + TEST_WAIT;
    loop {
        let archive_count: u64 = database
            .query_row(
                "SELECT COUNT(*) FROM conversation_history_blobs
                 WHERE conversation_id = ?1 AND call_id = ?2",
                rusqlite::params![&fixture.conversation_id, &archive_call_id],
                |row| row.get(0),
            )
            .unwrap();
        if archive_count == 1 {
            break;
        }
        assert!(
            Instant::now() < retry_deadline,
            "the first settlement attempt never reached Exact Archive"
        );
        thread::sleep(Duration::from_millis(10));
    }

    // Keep the trigger installed across several retry windows. The immutable Archive has already
    // committed, while the operational row and terminal lifecycle must remain uncommitted.
    thread::sleep(Duration::from_millis(150));
    let blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("blocked command session row");
    assert_eq!(blocked.snapshot.status, AgentCommandSessionStatus::Running);
    assert!(blocked.snapshot.archive_ref.is_none());
    let blocked_trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("blocked command trace");
    assert_eq!(
        blocked_trace
            .items
            .iter()
            .filter(|item| matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id,
                    ..
                } if session_id == &snapshot.session_id
            ))
            .count(),
        0
    );

    database
        .execute_batch("DROP TRIGGER reject_command_session_terminal_once_enabled;")
        .unwrap();
    let settled = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(settled.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(settled.snapshot.exit_code, Some(0));
    assert!(settled.snapshot.archive_ref.is_some());

    let archive_count: u64 = database
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs
             WHERE conversation_id = ?1 AND call_id = ?2",
            rusqlite::params![&fixture.conversation_id, &archive_call_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1);
    let settled_trace = fixture
        .storage
        .get_conversation_turn_trace(&fixture.assistant_message_id)
        .unwrap()
        .expect("settled command trace");
    let terminal_events = settled_trace
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
fn permanently_unsettled_session_keeps_host_admission_without_spawning_retry_threads() {
    let fixture = RunningFixture::new_with_admission_limits("settlement-admission", 2, 1);
    let command = "sleep 0.20; printf permanently-blocked";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_permanent_settlement",
        &snapshot.session_id,
    );
    fixture.adopt(&snapshot, command);
    wait_for_settlement_attempts(&fixture.registry, 3);

    let blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &snapshot.session_id)
        .unwrap()
        .expect("unsettled command Session");
    assert_eq!(blocked.snapshot.status, AgentCommandSessionStatus::Running);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    let stats = fixture.registry.settlement_scheduler_stats();
    assert_eq!(stats.0, 1, "retries must reuse the one scheduler worker");
    assert_eq!(stats.1, 1, "one failed Session must occupy one retry slot");

    let rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        "run-over-conversation-limit",
        "call-over-conversation-limit",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(rejected.contains("当前会话"));
    assert_eq!(fixture.registry.retained_admission_count(), 1);

    let other_conversation = "conversation-command-session-admission-other";
    let other_assistant = "assistant-command-session-admission-other";
    seed_conversation(&fixture.storage, other_conversation, other_assistant);
    let other = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        other_conversation,
        other_assistant,
        "run-admission-other",
        "call-admission-other",
        "sleep 5",
        None,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot: _other_snapshot,
        handoff_guard: mut other_handoff_guard,
        ..
    } = other
    else {
        panic!("the second admitted command must still be running");
    };
    assert_eq!(fixture.registry.retained_admission_count(), 2);
    let globally_rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        "conversation-not-created-because-global-limit-wins",
        "assistant-not-created-because-global-limit-wins",
        "run-over-global-limit",
        "call-over-global-limit",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(globally_rejected.contains("全局"));
    assert_eq!(fixture.registry.retained_admission_count(), 2);

    other_handoff_guard.abort_before_handoff().unwrap();
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    database
        .execute_batch("DROP TRIGGER reject_permanent_settlement;")
        .unwrap();
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(fixture.registry.retained_admission_count(), 0);
}

#[test]
fn delayed_failed_settlement_does_not_starve_a_healthy_session() {
    let fixture = RunningFixture::new_with_admission_limits("settlement-fairness", 2, 2);
    let healthy_conversation = "conversation-command-session-settlement-healthy";
    let healthy_assistant = "assistant-command-session-settlement-healthy";
    seed_conversation(&fixture.storage, healthy_conversation, healthy_assistant);

    let bad_command = "sleep 0.20; printf bad-terminal";
    let (bad_snapshot, _) = fixture.start(bad_command, None);
    let healthy_command = "sleep 0.24; printf healthy-terminal";
    let healthy_launch = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        healthy_conversation,
        healthy_assistant,
        "run-settlement-healthy",
        "call-settlement-healthy",
        healthy_command,
        None,
    )
    .unwrap();
    let AgentCommandSessionLaunch::Running {
        snapshot: healthy_snapshot,
        handoff_guard: mut healthy_handoff_guard,
        ..
    } = healthy_launch
    else {
        panic!("healthy command must survive the initial yield");
    };

    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_only_bad_settlement",
        &bad_snapshot.session_id,
    );
    fixture.adopt(&bad_snapshot, bad_command);
    adopt_owned_session(
        &fixture.storage,
        healthy_conversation,
        healthy_assistant,
        "run-settlement-healthy",
        "call-settlement-healthy",
        &healthy_snapshot,
        healthy_command,
        &mut healthy_handoff_guard,
    );

    let healthy = wait_for_terminal_record(
        &fixture.storage,
        healthy_conversation,
        &healthy_snapshot.session_id,
    );
    assert_eq!(healthy.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(healthy.snapshot.exit_code, Some(0));
    let still_blocked = fixture
        .storage
        .load_agent_command_session(&fixture.conversation_id, &bad_snapshot.session_id)
        .unwrap()
        .expect("failed settlement Session");
    assert_eq!(
        still_blocked.snapshot.status,
        AgentCommandSessionStatus::Running
    );
    // A durable terminal commit precedes the worker's admission release. Await the same
    // ownership boundary used below before checking scheduler fairness under parallel load.
    wait_for_retained_admission_count(&fixture.registry, 1);
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);

    database
        .execute_batch("DROP TRIGGER reject_only_bad_settlement;")
        .unwrap();
    wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &bad_snapshot.session_id,
    );
    // The durable terminal row commits immediately before the worker releases its admission
    // lease. Wait for that second, in-memory ownership boundary instead of racing it.
    wait_for_retained_admission_count(&fixture.registry, 0);
}

#[test]
fn host_shutdown_terminates_and_settles_an_adopted_session() {
    let fixture = RunningFixture::new("adopted-shutdown");
    let command = "sleep 5";
    let (snapshot, _) = fixture.start(command, None);
    fixture.adopt(&snapshot, command);

    assert!(fixture.registry.shutdown(Duration::from_secs(3)));
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
}

#[test]
fn shutdown_is_bounded_when_terminal_settlement_keeps_failing() {
    let fixture = RunningFixture::new("settlement-shutdown");
    let command = "sleep 0.15; printf shutdown-blocked";
    let (snapshot, _) = fixture.start(command, None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_shutdown_settlement",
        &snapshot.session_id,
    );
    fixture.adopt(&snapshot, command);
    wait_for_settlement_attempts(&fixture.registry, 2);

    let started = Instant::now();
    assert!(!fixture.registry.shutdown(Duration::from_millis(150)));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "shutdown must not join an unbounded settlement retry"
    );
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
}

#[test]
fn synchronous_terminal_failure_uses_the_bounded_shared_retry_scheduler() {
    let fixture = RunningFixture::new_with_admission_limits("synchronous-retry", 1, 1);
    let (snapshot, _) = fixture.start("sleep 0.15; printf synchronous-blocked", None);
    let database = Connection::open(&fixture.database_path).unwrap();
    install_terminal_rejection(
        &database,
        "reject_synchronous_settlement",
        &snapshot.session_id,
    );
    thread::sleep(Duration::from_millis(250));

    let error = fixture.abort_result(&snapshot.session_id).unwrap_err();
    assert!(error.contains("完整输出由后台 Session 继续持有"));
    wait_for_settlement_attempts(&fixture.registry, 3);
    assert_eq!(fixture.registry.settlement_scheduler_stats().0, 1);
    assert_eq!(fixture.registry.settlement_scheduler_stats().1, 1);
    assert_eq!(fixture.registry.retained_admission_count(), 1);
    let rejected = start_owned_session(
        &fixture.registry,
        fixture.workspace.path(),
        &fixture.conversation_id,
        &fixture.assistant_message_id,
        "run-after-synchronous-failure",
        "call-after-synchronous-failure",
        "sleep 5",
        None,
    )
    .unwrap_err();
    assert!(rejected.contains("全局"));

    database
        .execute_batch("DROP TRIGGER reject_synchronous_settlement;")
        .unwrap();
    let settled = wait_for_terminal_record(
        &fixture.storage,
        &fixture.conversation_id,
        &snapshot.session_id,
    );
    assert_eq!(settled.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(fixture.registry.retained_admission_count(), 0);
}

#[test]
fn every_restart_restores_outcome_unknown_conversation_and_project_fences() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let conversation_id = "conversation-command-session-restart";
    let assistant_message_id = "assistant-command-session-restart";
    let session_id = "cmd_0123456789abcdef0123456789abcdef";
    let project_id = "project-command-session-restart";
    storage
        .save_project(ProjectRecord::with_primary_folder(
            project_id.to_string(),
            "Command session restart".to_string(),
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("test-model".to_string()),
            title: "Managed command session restart".to_string(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant_message_id.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
            }],
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .replace_conversation_turn_trace(
            &completed_running_trace(
                conversation_id,
                assistant_message_id,
                "run-restart",
                "call-restart",
                session_id,
                "sleep 30",
            ),
            1,
            2,
        )
        .unwrap();
    storage
        .create_agent_command_session(&AgentCommandSessionCreate {
            snapshot: AgentCommandSessionSnapshot {
                schema_version: AGENT_COMMAND_SESSION_SCHEMA_VERSION,
                session_id: session_id.to_string(),
                conversation_id: conversation_id.to_string(),
                assistant_message_id: assistant_message_id.to_string(),
                origin_run_id: "run-restart".to_string(),
                call_id: "call-restart".to_string(),
                project_id: Some(project_id.to_string()),
                command: "sleep 30".to_string(),
                cwd: fixture.path().to_string_lossy().into_owned(),
                command_digest: ZERO_DIGEST.to_string(),
                status: AgentCommandSessionStatus::Starting,
                started_at: 10,
                ended_at: None,
                exit_code: None,
                latest_sequence: 0,
                output_truncated: false,
                outputs: Vec::new(),
                artifact_observation: None,
                archive_ref: None,
            },
            authorization_source: CommandAuthorizationSource::ExplicitUser,
            approval_provenance: json!({"status": "approved"}),
            permission_provenance: json!({"command": "auto_approve"}),
            created_at: 10,
        })
        .unwrap();
    storage
        .mark_agent_command_session_running(conversation_id, session_id, 11)
        .unwrap();

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let reconciled = storage
        .load_agent_command_session(conversation_id, session_id)
        .unwrap()
        .expect("startup-reconciled command Session");
    assert_eq!(
        reconciled.snapshot.status,
        AgentCommandSessionStatus::OutcomeUnknown
    );
    assert!(reconciled.snapshot.ended_at.is_some());
    assert!(reconciled.snapshot.archive_ref.is_none());
    let restored_effect = format!(
        "run-restart/{}",
        pending_action_storage_id("run-restart", "call-restart")
    );
    assert_eq!(
        service.unsettled_file_effect_ids_for_conversation(conversation_id),
        vec![restored_effect.clone()],
        "an outcome-unknown process must keep its durable deletion fence after restart"
    );
    let deletion_error = service.delete_conversation(conversation_id).unwrap_err();
    assert!(deletion_error.contains(&restored_effect));
    let project_deletion_error = service.delete_project(project_id).unwrap_err();
    assert!(project_deletion_error.contains(&restored_effect));
    assert!(storage
        .load_conversation(conversation_id)
        .unwrap()
        .is_some());
    drop(service);

    let restarted_again = AgentService::new_authorized_for_test(Arc::clone(&storage));
    assert_eq!(
        restarted_again.unsettled_file_effect_ids_for_conversation(conversation_id),
        vec![restored_effect.clone()],
        "a second restart must rebuild the fence from the persisted outcome-unknown row"
    );
    let second_conversation_error = restarted_again
        .delete_conversation(conversation_id)
        .unwrap_err();
    assert!(second_conversation_error.contains(&restored_effect));
    let second_project_error = restarted_again.delete_project(project_id).unwrap_err();
    assert!(second_project_error.contains(&restored_effect));

    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .expect("restart trace");
    let terminal_events = trace
        .items
        .iter()
        .filter(|item| {
            matches!(
                item,
                ConversationTurnTraceItem::CommandSessionLifecycle {
                    phase: ConversationCommandSessionLifecyclePhase::Terminal,
                    session_id: persisted_session_id,
                    status: AgentCommandSessionStatus::OutcomeUnknown,
                    ..
                } if persisted_session_id == session_id
            )
        })
        .count();
    assert_eq!(terminal_events, 1);
}
