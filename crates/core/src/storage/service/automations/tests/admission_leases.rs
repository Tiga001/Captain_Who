use super::*;
use crate::storage::automation_repository::{
    AutomationRunAdmissionInput, StoredAutomationRunStatus,
};
use std::sync::atomic::{AtomicUsize, Ordering};

fn prepare(
    service: &StorageService,
) -> (
    ChatConversationRecord,
    crate::ConversationTurnTrace,
    AutomationRunAdmissionInput,
) {
    service.create_automation(&input("task")).unwrap();
    service
        .enqueue_manual_automation_run(&NewManualAutomationRunRecord {
            id: "run".into(),
            automation_id: "task".into(),
            manual_request_id: "manual-request".into(),
            scheduled_for: 1,
            config_revision: 1,
            config_snapshot_json: "{}".into(),
            expected_revision: 1,
        })
        .unwrap();
    let run = service
        .claim_ready_automation_runs_now(30_000, 1)
        .unwrap()
        .remove(0);
    let timestamp = now_ms();
    let conversation = ChatConversationRecord {
        id: "conversation".into(),
        project_id: None,
        model_id: Some("fixture-model".into()),
        title: "Automation lease test".into(),
        messages: [("user", "user"), ("assistant", "assistant")]
            .into_iter()
            .map(|(id, role)| ChatMessageRecord {
                id: id.into(),
                role: role.into(),
                content: "synthetic".into(),
                created_at: timestamp,
                status: Some(if role == "user" { "sent" } else { "pending" }.into()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
                human_interaction_response: None,
            })
            .collect(),
        created_at: timestamp,
        updated_at: timestamp,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
    };
    let trace = crate::ConversationTraceSnapshot::default().in_progress_trace(
        "agent-run",
        &conversation.id,
        "assistant",
    );
    let admission = AutomationRunAdmissionInput {
        automation_run_id: run.id,
        admission_token: run.admission_token.unwrap(),
        config_revision: 1,
        permission_mode: "default".into(),
        agent_run_id: trace.run_id.clone(),
        conversation_id: conversation.id.clone(),
        user_message_id: "user".into(),
        assistant_message_id: "assistant".into(),
        admitted_at: timestamp,
    };
    (conversation, trace, admission)
}

fn admit(
    service: &StorageService,
    conversation: ChatConversationRecord,
    trace: &crate::ConversationTurnTrace,
    admission: &AutomationRunAdmissionInput,
    check: &(dyn Fn() -> Result<(), String> + Sync),
) -> Result<(), String> {
    let timestamp = conversation.updated_at;
    service
        .save_automation_conversation_and_begin_turn_with_preloaded_agent_messages(
            conversation,
            None,
            crate::AgentTurnPermissionSource::HostAuthenticatedRoot(
                crate::AgentPermissions::default(),
            ),
            &[],
            trace,
            timestamp,
            timestamp,
            admission,
            check,
        )
        .map(|_| ())
}

fn assert_no_turn(service: &StorageService) {
    let connection = service.state.read_connection().unwrap();
    for table in ["conversations", "messages", "conversation_turn_traces"] {
        assert_eq!(
            connection
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0,
            "{table}"
        );
    }
    drop(connection);
    let run = service.get_automation_run("run").unwrap().unwrap();
    assert_eq!(run.status, StoredAutomationRunStatus::Admitting);
    assert!(run.agent_run_id.is_none());
    assert!(run.conversation_id.is_none());
}

#[test]
fn expired_automation_claim_cannot_create_a_turn() {
    let (_directory, service) = fixture();
    let (conversation, trace, admission) = prepare(&service);
    service
        .state
        .write_sync(1024, StorageWriteClass::Control, |connection| {
            connection.execute(
                "UPDATE automation_runs SET admission_expires_at = 1 WHERE id = 'run'",
                [],
            )
        })
        .unwrap()
        .unwrap();
    let error = admit(&service, conversation, &trace, &admission, &|| Ok(())).unwrap_err();
    assert!(error.contains("claim is stale"), "{error}");
    assert_no_turn(&service);
}

fn queued_claim_change(replace_token: bool) {
    let (_directory, service) = fixture();
    let (conversation, trace, admission) = prepare(&service);
    let (started, entered) = mpsc::channel();
    let (release, wait) = mpsc::channel();
    let writer_service = service.clone();
    let writer = std::thread::spawn(move || {
        writer_service.state.write_sync(1024, StorageWriteClass::Control, move |connection| {
            started.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(5)).unwrap();
            if replace_token {
                connection.execute("UPDATE automation_runs SET admission_token='replacement-token' WHERE id='run'", []).unwrap();
            } else {
                // The deadline falls after prepared admitted_at but before writer acquisition.
                // Using only admitted_at for lease validation would incorrectly accept this.
                connection.execute("UPDATE automation_runs SET admission_expires_at=?1 WHERE id='run'", [now_ms()]).unwrap();
            }
        }).unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let admit_service = service.clone();
    let (done, result) = mpsc::channel();
    let job = std::thread::spawn(move || {
        done.send(admit(
            &admit_service,
            conversation,
            &trace,
            &admission,
            &|| Ok(()),
        ))
        .unwrap();
    });
    assert!(matches!(
        result.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    release.send(()).unwrap();
    writer.join().unwrap();
    let error = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap_err();
    job.join().unwrap();
    assert!(error.contains("claim is stale"), "{error}");
    assert_no_turn(&service);
    if replace_token {
        assert_eq!(
            service
                .get_automation_run("run")
                .unwrap()
                .unwrap()
                .admission_token
                .as_deref(),
            Some("replacement-token")
        );
    }
}

#[test]
fn automation_claim_expiring_while_queued_is_rechecked_after_writer_admission() {
    queued_claim_change(false);
}

#[test]
fn automation_claim_replaced_while_queued_cannot_bind_old_token() {
    queued_claim_change(true);
}

#[test]
fn automation_claim_expiring_during_transaction_rolls_back_all_turn_writes() {
    let (_directory, service) = fixture();
    let (conversation, trace, admission) = prepare(&service);
    let deadline = now_ms() + 2_000;
    service
        .state
        .write_sync(1024, StorageWriteClass::Control, |connection| {
            connection.execute(
                "UPDATE automation_runs SET admission_expires_at=?1 WHERE id='run'",
                [deadline],
            )
        })
        .unwrap()
        .unwrap();
    let checks = AtomicUsize::new(0);
    let error = admit(&service, conversation, &trace, &admission, &|| {
        if checks.fetch_add(1, Ordering::SeqCst) == 1 {
            // This callback executes immediately before final commit, after consuming the token.
            // Only wall time advances; no other connection mutates the in-flight transaction.
            let wait_ms = deadline.saturating_sub(now_ms()).max(0) as u64;
            std::thread::sleep(Duration::from_millis(wait_ms.saturating_add(1)));
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(checks.load(Ordering::SeqCst), 2);
    assert!(error.contains("claim expired before commit"), "{error}");
    assert_no_turn(&service);
    assert_eq!(
        service
            .get_automation_run("run")
            .unwrap()
            .unwrap()
            .admission_token
            .as_deref(),
        Some(admission.admission_token.as_str())
    );
}

#[test]
fn valid_automation_claim_commits_binding_messages_and_trace_together() {
    let (_directory, service) = fixture();
    let (conversation, trace, admission) = prepare(&service);
    admit(&service, conversation, &trace, &admission, &|| Ok(())).unwrap();
    let run = service.get_automation_run("run").unwrap().unwrap();
    assert_eq!(run.status, StoredAutomationRunStatus::Running);
    assert_eq!(run.agent_run_id.as_deref(), Some("agent-run"));
    assert_eq!(run.conversation_id.as_deref(), Some("conversation"));
    assert!(run.admission_token.is_none());
    assert!(run.admission_expires_at.is_none());
    let connection = service.state.read_connection().unwrap();
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE conversation_id='conversation'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        2
    );
    assert_eq!(connection.query_row("SELECT COUNT(*) FROM conversation_turn_traces WHERE run_id='agent-run' AND assistant_message_id='assistant'", [], |row| row.get::<_, i64>(0)).unwrap(), 1);
}
