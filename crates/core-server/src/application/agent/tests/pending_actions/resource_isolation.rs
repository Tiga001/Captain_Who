use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;
use std::sync::mpsc;
use std::time::Duration;

fn pending_fixture(
    service: &AgentService,
    storage: &StorageService,
    prefix: &str,
) -> PendingActionRecord {
    let run_id = format!("run-{prefix}");
    let conversation_id = format!("conversation-{prefix}");
    let assistant_id = format!("assistant-{prefix}");
    let call = AgentToolCall {
        id: format!("call-{prefix}"),
        tool: "approval_tool".into(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::Builtin {
        tool_name: call.tool.clone(),
    };
    seed_durable_pending_owner(
        storage,
        &conversation_id,
        &assistant_id,
        &run_id,
        &call,
        provenance.clone(),
        1,
    );
    let mut input: AgentChatInput = serde_json::from_value(json!({
        "apiUrl": "https://example.test/v1/chat/completions", "apiToken": "secret",
        "model": "test-model", "modelCapabilities": { "imageInput": false }, "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(storage, &mut input);
    let context = AgentRunContext {
        conversation_id: Some(conversation_id.clone()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    let mut checkpoint =
        test_pending_resume_checkpoint_for_call(storage, &run_id, None, &call, provenance);
    checkpoint.run_context = Some(context.clone());
    input.context = Some(context);
    input.resume_checkpoint = Some(checkpoint);
    service
        .store_pending_action(
            &run_id,
            &conversation_id,
            &assistant_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            input,
        )
        .unwrap();
    service
        .pending_action_for_decision(&run_id, &call.id)
        .unwrap()
}

fn disk_service() -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
    let fixture = tempdir().unwrap();
    let storage = Arc::new(
        StorageService::open_with_model_credentials(
            &fixture.path().join("storage.sqlite"),
            Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default()),
        )
        .unwrap(),
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    (fixture, storage, service)
}

fn hold_frozen_approval(record: &PendingActionRecord) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    crate::application::agent::approval::install_frozen_approval_hook(
        &record.storage_id,
        Arc::new(move || {
            entered_tx.send(()).unwrap();
            // Disconnection also releases the worker when the main assertion panics.
            let _ = release_rx
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(10));
        }),
    );
    (entered_rx, release_tx)
}

#[test]
fn slow_frozen_approval_does_not_block_other_run_cancel_or_its_own_stop() {
    let (_fixture, storage, service) = disk_service();
    let a = pending_fixture(&service, &storage, "isolated-approval-a");
    let b = pending_fixture(&service, &storage, "isolated-approval-b");
    let (entered, release) = hold_frozen_approval(&a);
    let (service, a, b) = (&service, &a, &b);
    std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            service.queue_action_continuation(
                &a.snapshot.run_id,
                &a.snapshot.action_id,
                AgentApprovalDecisionStatus::Approved,
                None,
                crate::transport::outbound_channel().0,
            )
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let independent = scope.spawn(move || {
            let result = service.cancel_action(&b.snapshot.run_id, &b.snapshot.action_id);
            done_tx.send(result).unwrap();
        });
        let other_result = done_rx.recv_timeout(Duration::from_secs(2));
        // Stop uses the internal cancellation boundary and may not wait on A's user-decision gate.
        let (stop_tx, stop_rx) = mpsc::channel();
        let stop = scope.spawn(move || {
            stop_tx
                .send(service.cancel_action_internal(&a.snapshot.run_id, &a.snapshot.action_id))
                .unwrap();
        });
        let stop_result = stop_rx.recv_timeout(Duration::from_secs(2));
        release.send(()).unwrap();
        independent.join().unwrap();
        stop.join().unwrap();
        assert!(other_result
            .expect("B cancellation must progress during A validation")
            .unwrap());
        assert!(stop_result
            .expect("A Stop must not wait on A's decision gate")
            .unwrap());
        assert!(
            first.join().unwrap().is_err(),
            "stale approval must lose its durable CAS"
        );
    });
    for record in [&a, &b] {
        let durable = storage
            .get_pending_agent_action(&record.storage_id)
            .unwrap()
            .unwrap();
        assert_eq!(durable.status, "cancelled");
        assert_eq!(
            service
                .pending_action_snapshot(&record.storage_id)
                .unwrap()
                .snapshot
                .status,
            PendingActionStatus::Cancelled
        );
        assert_eq!(
            storage
                .get_conversation_turn_trace(
                    record.snapshot.assistant_message_id.as_deref().unwrap()
                )
                .unwrap()
                .unwrap()
                .terminal_status,
            ConversationTurnTraceTerminalStatus::Cancelled
        );
    }
}

#[test]
fn deletion_fences_slow_approval_and_waits_for_its_scope_only() {
    let (_fixture, storage, service) = disk_service();
    let a = pending_fixture(&service, &storage, "deleting-slow-approval-a");
    let b = pending_fixture(&service, &storage, "deleting-slow-approval-b");
    let (entered, release) = hold_frozen_approval(&a);
    let (service, a, b) = (&service, &a, &b);
    std::thread::scope(|scope| {
        let first = scope.spawn(move || {
            service.queue_action_continuation(
                &a.snapshot.run_id,
                &a.snapshot.action_id,
                AgentApprovalDecisionStatus::Approved,
                None,
                crate::transport::outbound_channel().0,
            )
        });
        entered.recv_timeout(Duration::from_secs(2)).unwrap();
        let (deleted_tx, deleted_rx) = mpsc::channel();
        let deletion = scope.spawn(move || {
            deleted_tx
                .send(service.delete_conversation(a.snapshot.conversation_id.as_deref().unwrap()))
                .unwrap();
        });
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !service.is_agent_input_scope_deleting(&a.agent_input)
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        let fenced = service.is_agent_input_scope_deleting(&a.agent_input);
        let not_finished = deleted_rx.try_recv().is_err();
        let pending_still_present = storage
            .get_pending_agent_action(&a.storage_id)
            .unwrap()
            .is_some();
        let independent = service.cancel_action(&b.snapshot.run_id, &b.snapshot.action_id);
        release.send(()).unwrap();
        let approved = first.join().unwrap();
        deletion.join().unwrap();
        let deleted = deleted_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            fenced,
            "delete must install its marker without waiting for Provider validation"
        );
        assert!(
            not_finished && pending_still_present,
            "delete must drain registered approval work before removing its owner"
        );
        assert!(independent.unwrap());
        assert!(approved.is_err());
        assert!(deleted.is_ok(), "{deleted:?}");
    });
    assert!(storage
        .get_pending_agent_action(&a.storage_id)
        .unwrap()
        .is_none());
    assert_eq!(
        storage
            .get_pending_agent_action(&b.storage_id)
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}

#[test]
fn delayed_pending_projection_cannot_regress_a_later_decision() {
    let (_fixture, storage, service) = disk_service();
    let record = pending_fixture(&service, &storage, "delayed-approval-projection");
    service
        .persist_pending_status(
            &record,
            PendingActionStatus::Pending,
            PendingActionStatus::Executing,
        )
        .unwrap();
    service
        .publish_pending_action_status(
            &record,
            PendingActionStatus::Pending,
            PendingActionStatus::Executing,
        )
        .unwrap();
    service
        .persist_pending_target_status(&record, PendingActionStatus::Cancelled)
        .unwrap();
    service
        .transition_pending_status(&record, PendingActionStatus::Cancelled)
        .unwrap();
    assert!(service
        .publish_pending_action_status(
            &record,
            PendingActionStatus::Pending,
            PendingActionStatus::Executing
        )
        .is_err());
    assert_eq!(
        service
            .pending_action_snapshot(&record.storage_id)
            .unwrap()
            .snapshot
            .status,
        PendingActionStatus::Cancelled
    );
    assert_eq!(
        storage
            .get_pending_agent_action(&record.storage_id)
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
}
