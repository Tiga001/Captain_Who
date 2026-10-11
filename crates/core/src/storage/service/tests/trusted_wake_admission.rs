use super::*;
use std::cell::Cell;
use std::time::Duration;

struct PreparedWake {
    candidate: ChatConversationRecord,
    revision: Option<i64>,
    trusted: crate::TrustedAgentWakeTurnAdmission,
    trace: ConversationTurnTrace,
    prepared_at: i64,
}

fn prepare_wake(service: &StorageService) -> PreparedWake {
    prepare_wake_with_remaining_lease(service, 60_000)
}

fn prepare_wake_with_remaining_lease(service: &StorageService, remaining_ms: i64) -> PreparedWake {
    let mut root = conversation("wake-root", None, "root-user");
    service.save_conversation(root.clone()).unwrap();
    bind_agent_root(service, "root-agent", &root.id);
    let revision = service.conversation_revision(&root.id).unwrap();
    let mut assistant = root.messages[0].clone();
    assistant.id = "root-assistant".into();
    assistant.role = "assistant".into();
    assistant.status = Some("pending".into());
    root.messages.push(assistant);
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "root-run",
        &root.id,
        "root-assistant",
    );
    service
        .save_conversation_and_begin_turn(
            root,
            revision,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &trace,
            1,
            1,
        )
        .unwrap();
    let mut child = conversation("wake-child", None, "child-history");
    child.messages.clear();
    service.save_conversation(child).unwrap();
    bind_agent_child(
        service,
        "child-agent",
        "wake-child",
        "root-agent",
        "wake-root",
        "child",
    );
    service
        .follow_up_agent(&crate::SendAgentMessageRequest {
            sender_agent_id: "root-agent".into(),
            recipient_agent_id: "child-agent".into(),
            request_id: "wake-task".into(),
            content: "synthetic lease regression".into(),
        })
        .unwrap();
    // Claim through the real state machine with a synthetic origin, rather than shortening an
    // existing lease (which schema fencing intentionally forbids). Production TTL is unchanged.
    let claimed_at = now_ms() - 60_000 + remaining_ms;
    let wake = service
        .claim_next_dispatchable_agent_wake_at("original-claim", claimed_at)
        .unwrap()
        .unwrap();
    let (candidate, revision) = service.load_conversation_for_turn("wake-child").unwrap();
    let mut candidate = candidate.unwrap();
    let prepared_at = now_ms();
    let mut assistant = candidate.messages[0].clone();
    assistant.id = "child-assistant".into();
    assistant.role = "assistant".into();
    assistant.content.clear();
    assistant.created_at = prepared_at;
    assistant.status = Some("pending".into());
    candidate.messages.push(assistant);
    candidate.updated_at = prepared_at;
    PreparedWake {
        candidate,
        revision,
        trusted: crate::TrustedAgentWakeTurnAdmission {
            agent_id: wake.agent_id,
            wake_id: wake.wake_id,
            claim_token: wake.claim_token.unwrap(),
            source_agent_message_id: wake.source_agent_message_id.unwrap(),
        },
        trace: crate::ConversationTraceSnapshot::default().in_progress_trace(
            "child-run",
            "wake-child",
            "child-assistant",
        ),
        prepared_at,
    }
}

fn admit(
    service: &StorageService,
    prepared: PreparedWake,
    check: Option<&dyn Fn() -> Result<(), String>>,
) -> Result<(ChatConversationRecord, crate::AgentPermissions), String> {
    service.save_conversation_and_begin_turn_with_execution_access(
        prepared.candidate,
        prepared.revision,
        Some(&prepared.trusted),
        crate::AgentTurnPermissionSource::InheritTrustedAncestors,
        std::slice::from_ref(&prepared.trusted.source_agent_message_id),
        &prepared.trace,
        prepared.prepared_at,
        prepared.prepared_at,
        check,
    )
}

fn lease_deadline(service: &StorageService, wake_id: &str) -> i64 {
    service
        .get_agent_wake(wake_id)
        .unwrap()
        .unwrap()
        .lease_expires_at
        .unwrap()
}

fn sleep_until(deadline: i64) {
    while now_ms() < deadline {
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn assert_not_admitted(service: &StorageService, wake_id: &str, revision: Option<i64>) {
    let wake = service.get_agent_wake(wake_id).unwrap().unwrap();
    assert!(wake.run_id.is_none());
    assert!(wake.assistant_message_id.is_none());
    assert_eq!(
        service.conversation_revision("wake-child").unwrap(),
        revision
    );
    assert!(service
        .get_conversation_turn_trace("child-assistant")
        .unwrap()
        .is_none());
    assert!(service
        .load_conversation("wake-child")
        .unwrap()
        .unwrap()
        .messages
        .iter()
        .all(|message| message.id != "child-assistant"));
    assert!(service
        .get_agent_effective_permission_snapshot("child-agent")
        .unwrap()
        .is_none());
    assert!(service
        .load_agent_context_profile_for_run("child-run")
        .unwrap()
        .is_none());
}

#[test]
fn trusted_wake_rejects_lease_expired_after_preparation_with_old_trace_time() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let mut prepared = prepare_wake_with_remaining_lease(&service, -1);
    prepared.prepared_at = now_ms() - 10_000;
    let wake_id = prepared.trusted.wake_id.clone();
    let revision = prepared.revision;
    let error = admit(&service, prepared, None).unwrap_err();
    assert!(error.contains("claim is stale"), "{error}");
    assert_not_admitted(&service, &wake_id, revision);
}

#[test]
fn trusted_wake_rechecks_expiry_after_waiting_for_the_storage_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake_with_remaining_lease(&service, 500);
    let wake_id = prepared.trusted.wake_id.clone();
    let revision = prepared.revision;
    let deadline = lease_deadline(&service, &wake_id);
    let connection = service.state.connection().unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            admit(worker_service, prepared, None)
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        drop(connection);
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.contains("claim is stale"), "{error}");
    assert_not_admitted(&service, &wake_id, revision);
}

#[test]
fn trusted_wake_renewal_rejects_expiry_while_waiting_for_the_storage_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake_with_remaining_lease(&service, 500);
    let wake_id = prepared.trusted.wake_id.clone();
    let deadline = lease_deadline(&service, &wake_id);
    let connection = service.state.connection().unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            worker_service
                .renew_agent_wake_lease(&prepared.trusted.wake_id, &prepared.trusted.claim_token)
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        drop(connection);
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.to_string().contains("already expired"), "{error}");
    assert_eq!(
        service
            .get_agent_wake(&wake_id)
            .unwrap()
            .unwrap()
            .lease_expires_at,
        Some(deadline)
    );
}

#[test]
fn trusted_wake_renewal_rejects_expiry_while_waiting_for_an_external_sqlite_writer() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake_with_remaining_lease(&service, 500);
    let wake_id = prepared.trusted.wake_id.clone();
    let deadline = lease_deadline(&service, &wake_id);
    let mut other_connection =
        rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    let transaction = other_connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            worker_service
                .renew_agent_wake_lease(&prepared.trusted.wake_id, &prepared.trusted.claim_token)
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        transaction.rollback().unwrap();
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.to_string().contains("already expired"), "{error}");
    assert_eq!(
        service
            .get_agent_wake(&wake_id)
            .unwrap()
            .unwrap()
            .lease_expires_at,
        Some(deadline)
    );
}

#[test]
fn trusted_wake_renewal_expiry_before_commit_rolls_back_the_lease_extension() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake(&service);
    let wake_id = prepared.trusted.wake_id;
    let deadline = lease_deadline(&service, &wake_id);
    let checks = Cell::new(0);
    let mut connection = service.state.connection().unwrap();
    let error = agent_graph_repository::renew_agent_wake_lease_with_clock(
        &mut connection,
        &wake_id,
        &prepared.trusted.claim_token,
        || {
            checks.set(checks.get() + 1);
            if checks.get() == 1 {
                deadline - 1
            } else {
                deadline
            }
        },
    )
    .unwrap_err();
    drop(connection);
    assert!(
        error.to_string().contains("before renewal commit"),
        "{error}"
    );
    assert_eq!(checks.get(), 2);
    assert_eq!(
        service
            .get_agent_wake(&wake_id)
            .unwrap()
            .unwrap()
            .lease_expires_at,
        Some(deadline)
    );
}

fn admit_for_execution_fence(service: &StorageService) -> crate::TrustedAgentWakeTurnAdmission {
    admit_for_execution_fence_with_remaining_lease(service, 60_000)
}

fn admit_for_execution_fence_with_remaining_lease(
    service: &StorageService,
    remaining_ms: i64,
) -> crate::TrustedAgentWakeTurnAdmission {
    let prepared = prepare_wake_with_remaining_lease(service, remaining_ms);
    let trusted = prepared.trusted.clone();
    admit(service, prepared, None).unwrap();
    service
        .validate_agent_wake_execution_fence(&trusted, "child-run", "child-assistant")
        .unwrap();
    trusted
}

#[test]
fn trusted_wake_execution_fence_rejects_expired_or_replaced_claim_and_wrong_run() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence_with_remaining_lease(&service, 500);
    for (run_id, assistant_message_id) in [
        ("other-run", "child-assistant"),
        ("child-run", "other-assistant"),
    ] {
        assert!(service
            .validate_agent_wake_execution_fence(&trusted, run_id, assistant_message_id)
            .is_err());
    }
    let original_deadline = service
        .get_agent_wake(&trusted.wake_id)
        .unwrap()
        .unwrap()
        .lease_expires_at
        .unwrap();
    sleep_until(original_deadline);
    assert!(service
        .validate_agent_wake_execution_fence(&trusted, "child-run", "child-assistant")
        .is_err());
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE agent_wake_requests SET claim_token = 'replacement-generation',
                 claimed_at = lease_expires_at, lease_expires_at = lease_expires_at + 60000
             WHERE wake_id = ?1",
            [&trusted.wake_id],
        )
        .unwrap();
    assert!(service
        .validate_agent_wake_execution_fence(&trusted, "child-run", "child-assistant")
        .is_err());
}

#[test]
fn trusted_wake_execution_fence_rejects_stopped_tree_generation() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence(&service);
    service
        .begin_agent_tree_run_stop_by_root_agent("root-agent", "root-run")
        .unwrap()
        .unwrap();
    assert!(service
        .get_agent_tree_run_stop("child-run")
        .unwrap()
        .is_some());
    // A tree stop is an independent durable fence: the Runtime may not yet have projected
    // cancellation onto its live trace or Wake when this final preparation boundary runs.
    assert_eq!(
        service
            .get_agent_wake(&trusted.wake_id)
            .unwrap()
            .unwrap()
            .status,
        crate::AgentWakeStatus::Running
    );
    assert!(service
        .validate_agent_wake_execution_fence(&trusted, "child-run", "child-assistant")
        .is_err());
}

#[test]
fn trusted_wake_execution_fence_rechecks_expiry_after_waiting_for_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence_with_remaining_lease(&service, 500);
    let deadline = lease_deadline(&service, &trusted.wake_id);
    let connection = service.state.connection().unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            worker_service.validate_agent_wake_execution_fence(
                &trusted,
                "child-run",
                "child-assistant",
            )
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        drop(connection);
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.to_string().contains("original live claim"), "{error}");
}

#[test]
fn trusted_wake_approval_wait_rejects_expiry_while_waiting_for_storage_connection() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence_with_remaining_lease(&service, 500);
    let deadline = lease_deadline(&service, &trusted.wake_id);
    let before = service.get_agent_wake(&trusted.wake_id).unwrap().unwrap();
    let connection = service.state.connection().unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            worker_service.mark_agent_wake_waiting_for_pending_approval(
                &trusted.wake_id,
                "child-run",
                "wake-child",
                "child-assistant",
                &trusted.claim_token,
            )
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        drop(connection);
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.contains("expired"), "{error}");
    let after = service.get_agent_wake(&before.wake_id).unwrap().unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.status_revision, before.status_revision);
    assert_eq!(after.lease_expires_at, before.lease_expires_at);
}

#[test]
fn trusted_wake_approval_wait_rejects_expiry_while_waiting_for_external_sqlite_writer() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence_with_remaining_lease(&service, 500);
    let deadline = lease_deadline(&service, &trusted.wake_id);
    let before = service.get_agent_wake(&trusted.wake_id).unwrap().unwrap();
    let mut other_connection =
        rusqlite::Connection::open(fixture.root.join("storage.sqlite")).unwrap();
    let transaction = other_connection
        .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
        .unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            worker_service.mark_agent_wake_waiting_for_pending_approval(
                &trusted.wake_id,
                "child-run",
                "wake-child",
                "child-assistant",
                &trusted.claim_token,
            )
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        transaction.rollback().unwrap();
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.contains("expired"), "{error}");
    let after = service.get_agent_wake(&before.wake_id).unwrap().unwrap();
    assert_eq!(after.status, before.status);
    assert_eq!(after.status_revision, before.status_revision);
    assert_eq!(after.lease_expires_at, before.lease_expires_at);
}

#[test]
fn trusted_wake_expiry_before_commit_rolls_back_messages_trace_and_dispatch_binding() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake_with_remaining_lease(&service, 500);
    let wake_id = prepared.trusted.wake_id.clone();
    let revision = prepared.revision;
    let deadline = lease_deadline(&service, &wake_id);
    let checks = Cell::new(0);
    let check = || {
        checks.set(checks.get() + 1);
        if checks.get() == 2 {
            sleep_until(deadline);
        }
        Ok(())
    };
    let error = admit(&service, prepared, Some(&check)).unwrap_err();
    assert_eq!(checks.get(), 2);
    assert!(error.contains("before Turn admission commit"), "{error}");
    assert_not_admitted(&service, &wake_id, revision);
    assert_eq!(
        service.get_agent_wake(&wake_id).unwrap().unwrap().status,
        crate::AgentWakeStatus::Claimed
    );
}

#[test]
fn trusted_wake_live_lease_preserves_the_original_atomic_admission() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let prepared = prepare_wake(&service);
    let wake_id = prepared.trusted.wake_id.clone();
    admit(&service, prepared, None).unwrap();
    let wake = service.get_agent_wake(&wake_id).unwrap().unwrap();
    assert_eq!(wake.status, crate::AgentWakeStatus::Running);
    assert_eq!(wake.run_id.as_deref(), Some("child-run"));
    drop(service);
    let reopened = fixture.service();
    assert!(reopened
        .get_conversation_turn_trace("child-assistant")
        .unwrap()
        .is_some());
}

#[test]
fn trusted_wake_changed_claim_token_or_tree_stop_cannot_admit_a_prepared_turn() {
    for stopped in [false, true] {
        let fixture = StorageFixture::new();
        let service = fixture.service();
        let prepared = prepare_wake(&service);
        let wake_id = prepared.trusted.wake_id.clone();
        let revision = prepared.revision;
        if stopped {
            service
                .cancel_agent_tree_wakes_by_root_agent("root-agent")
                .unwrap();
        } else {
            let expired_at = lease_deadline(&service, &wake_id);
            service
                .recover_agent_wakes_at("replacement-recovery", expired_at)
                .unwrap();
            let replacement = service
                .claim_next_dispatchable_agent_wake_at("replacement-claim", expired_at)
                .unwrap()
                .unwrap();
            assert_eq!(replacement.wake_id, wake_id);
        }
        assert!(admit(&service, prepared, None).is_err());
        assert_not_admitted(&service, &wake_id, revision);
    }
}

fn finalize_with_wake_fence(
    service: &StorageService,
    trusted: &crate::TrustedAgentWakeTurnAdmission,
    trace: &ConversationTurnTrace,
    usage: &AgentUsageRecordInsert,
) -> Result<(), String> {
    service.finalize_chat_message_with_conversation_trace_model_context_usage_and_notification(
        "wake-child",
        "child-assistant",
        "terminal text",
        Some("sent"),
        trace.terminal_status.as_str(),
        trace,
        Some(&[]),
        1,
        usage.completed_at.unwrap(),
        Some(usage),
        None,
        None,
        Some(trusted),
    )
}

fn terminal_usage() -> AgentUsageRecordInsert {
    let mut usage = agent_usage_record("wake-child", "child-assistant");
    usage.run_id = "child-run".into();
    usage.project_id = None;
    usage.completed_at = Some(now_ms());
    usage
}

fn assert_terminal_fence_unchanged(service: &StorageService) {
    assert_eq!(
        service
            .get_conversation_turn_trace("child-assistant")
            .unwrap()
            .unwrap()
            .terminal_status,
        crate::ConversationTurnTraceTerminalStatus::InProgress
    );
    assert!(service
        .load_agent_usage_for_owner("child-run", "wake-child", "child-assistant")
        .unwrap()
        .is_none());
    let conversation = service.load_conversation("wake-child").unwrap().unwrap();
    assert!(conversation
        .messages
        .iter()
        .find(|message| message.id == "child-assistant")
        .unwrap()
        .content
        .is_empty());
}

#[test]
fn trusted_wake_terminal_rejects_replaced_claim_without_trace_message_or_usage_write() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence(&service);
    service
        .state
        .connection()
        .unwrap()
        .execute(
            "UPDATE agent_wake_requests SET claim_token = 'replacement-generation',
             claimed_at = lease_expires_at, lease_expires_at = lease_expires_at + 60000
         WHERE wake_id = ?1",
            [&trusted.wake_id],
        )
        .unwrap();
    let trace = crate::completed_conversation_trace_without_items(
        "child-run",
        "wake-child",
        "child-assistant",
    );
    let error =
        finalize_with_wake_fence(&service, &trusted, &trace, &terminal_usage()).unwrap_err();
    assert!(error.starts_with("agent_wake_terminal_fenced:"), "{error}");
    assert_terminal_fence_unchanged(&service);
}

#[test]
fn trusted_wake_terminal_rechecks_expiry_after_connection_wait_and_rolls_back_all_facts() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence_with_remaining_lease(&service, 500);
    let deadline = lease_deadline(&service, &trusted.wake_id);
    let trace = crate::completed_conversation_trace_without_items(
        "child-run",
        "wake-child",
        "child-assistant",
    );
    let usage = terminal_usage();
    let connection = service.state.connection().unwrap();
    let error = std::thread::scope(|scope| {
        let (started, ready) = std::sync::mpsc::sync_channel(1);
        let worker_service = &service;
        let worker = scope.spawn(move || {
            started.send(()).unwrap();
            finalize_with_wake_fence(worker_service, &trusted, &trace, &usage)
        });
        ready.recv().unwrap();
        sleep_until(deadline);
        drop(connection);
        worker.join().unwrap().unwrap_err()
    });
    assert!(error.starts_with("agent_wake_terminal_fenced:"), "{error}");
    assert_terminal_fence_unchanged(&service);
}

#[test]
fn trusted_wake_terminal_allows_live_owner_cancel_after_tree_stop_and_exact_noop_replay() {
    let fixture = StorageFixture::new();
    let service = fixture.service();
    let trusted = admit_for_execution_fence(&service);
    service
        .begin_agent_tree_run_stop_by_root_agent("root-agent", "root-run")
        .unwrap()
        .unwrap();
    let mut usage = terminal_usage();
    usage.status = Some("cancelled".into());
    let success_trace = crate::completed_conversation_trace_without_items(
        "child-run",
        "wake-child",
        "child-assistant",
    );
    assert!(finalize_with_wake_fence(&service, &trusted, &success_trace, &usage).is_err());
    assert_terminal_fence_unchanged(&service);
    let trace = crate::cancelled_conversation_trace_without_items(
        "child-run",
        "wake-child",
        "child-assistant",
    );
    finalize_with_wake_fence(&service, &trusted, &trace, &usage).unwrap();
    assert_eq!(
        service
            .get_conversation_turn_trace("child-assistant")
            .unwrap()
            .unwrap(),
        trace
    );
    let before = service
        .load_agent_usage_for_owner("child-run", "wake-child", "child-assistant")
        .unwrap()
        .unwrap();
    finalize_with_wake_fence(&service, &trusted, &trace, &usage).unwrap();
    // Equality must include usage: identical public text and Trace cannot smuggle a changed bill.
    usage.input_tokens = Some(999_999);
    assert!(finalize_with_wake_fence(&service, &trusted, &trace, &usage).is_err());
    let after = service
        .load_agent_usage_for_owner("child-run", "wake-child", "child-assistant")
        .unwrap()
        .unwrap();
    assert_eq!(after.input_tokens, before.input_tokens);
    assert_eq!(after.completed_at, before.completed_at);
}
