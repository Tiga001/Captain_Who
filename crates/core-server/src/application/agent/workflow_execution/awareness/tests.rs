use super::*;
use mycopilot_core::human_interaction::{HumanInteractionMode, HumanInteractionToolInput};
use mycopilot_core::storage::human_interaction_repository::{
    HostHumanInteractionOwner, HumanInteractionSyncAdmission,
};
use mycopilot_core::storage::models::{
    AgentPendingActionRecord, ChatConversationRecord, ChatMessageRecord,
};
use mycopilot_core::{ConversationTraceSnapshot, EnsureRootAgentInput};

fn fixture() -> (tempfile::TempDir, Arc<StorageService>, AgentService) {
    let directory = tempfile::tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("awareness.sqlite")).unwrap());
    let service = AgentService::new_authorized_for_test(storage.clone());
    (directory, storage, service)
}

fn seed_chat(storage: &StorageService, id: &str, active: bool) -> HostHumanInteractionOwner {
    let assistant = format!("assistant-{id}");
    storage
        .save_conversation(ChatConversationRecord {
            id: id.into(),
            project_id: None,
            model_id: None,
            title: id.into(),
            messages: vec![ChatMessageRecord {
                human_interaction_response: None,
                id: assistant.clone(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".into()),
                attachments: vec![],
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
    let owner = HostHumanInteractionOwner {
        agent_id: format!("agent-{id}"),
        conversation_id: id.into(),
        run_id: format!("run-{id}"),
        assistant_message_id: assistant,
        tool_call_id: format!("question-{id}"),
    };
    storage
        .ensure_root_agent(&EnsureRootAgentInput {
            agent_id: owner.agent_id.clone(),
            conversation_id: id.into(),
            creation_request_id: format!("create-{id}"),
            task_name: id.into(),
        })
        .unwrap();
    if active {
        let trace = ConversationTraceSnapshot::default().in_progress_trace(
            &owner.run_id,
            id,
            &owner.assistant_message_id,
        );
        storage
            .append_in_progress_conversation_turn_trace(&trace, 1, 1)
            .unwrap();
    }
    owner
}

fn resident(service: &AgentService, owner: &HostHumanInteractionOwner) {
    service.register_active_run_control(
        &owner.run_id,
        &owner.conversation_id,
        &owner.assistant_message_id,
        None,
        ModelCapabilities::default(),
        AgentPermissions::default(),
    );
}

#[test]
fn awareness_host_clears_stale_estimates_and_marks_missing_bindings_unknown() {
    let (_directory, storage, service) = fixture();
    seed_chat(&storage, "idle", false);
    let mut projection = json!({"runtime":{"nodes":[
        {"conversationId":"idle","state":"failed","latestRun":{"status":"failed"},"waitingForApproval":true,"waitingForInteraction":true},
        {"conversationId":"idle","state":"running","lastRunStatus":"in_progress","pendingCount":2},
        {"conversationId":"idle","state":"running","processingCount":1},
        {"conversationId":null,"state":"idle"},
        {"conversationId":"deleted","state":"running"}
    ]}});
    service.enrich_workflow_awareness(&mut projection).unwrap();
    let nodes = projection["runtime"]["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["state"], "idle");
    assert_eq!(nodes[0]["latestRun"]["status"], "failed");
    assert_eq!(nodes[1]["state"], "queued");
    assert_eq!(nodes[2]["state"], "unknown");
    for node in nodes {
        assert_eq!(node["waitingForApproval"], false);
        assert_eq!(node["waitingForInteraction"], false);
        assert_eq!(node["hasPendingInteraction"], false);
        assert!(node["activeRunId"].is_null());
    }
    for node in &nodes[3..] {
        assert_eq!(node["state"], "unknown");
        assert_eq!(node["bindingAvailable"], false);
    }
}

#[test]
fn awareness_host_requires_live_worker_evidence_and_preserves_pause_and_compaction() {
    let (_directory, storage, service) = fixture();
    let owner = seed_chat(&storage, "member", true);
    service
        .restore_durable_conversation_turn_occupancies()
        .unwrap();
    // The recovery occupancy map is populated, but no worker is actually attached.
    let mut projection =
        json!({"nodes":[{"conversationId":"member","state":"running","processingCount":1}]});
    service.enrich_workflow_awareness(&mut projection).unwrap();
    assert_eq!(projection["nodes"][0]["state"], "unknown");
    assert_eq!(projection["nodes"][0]["activeRunId"], owner.run_id);
    resident(&service, &owner);
    service.enrich_workflow_awareness(&mut projection).unwrap();
    assert_eq!(projection["nodes"][0]["state"], "running");
    service
        .manual_context_compaction_cancellations
        .lock()
        .unwrap()
        .insert("member".into(), AgentCancellationToken::new());
    service.enrich_workflow_awareness(&mut projection).unwrap();
    assert_eq!(projection["nodes"][0]["state"], "compacting");
    projection["nodes"][0]["paused"] = json!(true);
    service.enrich_workflow_awareness(&mut projection).unwrap();
    assert_eq!(projection["nodes"][0]["state"], "stopped");
    assert_eq!(
        storage
            .get_in_progress_conversation_turn_identity("member")
            .unwrap()
            .unwrap()
            .run_id,
        owner.run_id
    );
}

#[test]
fn awareness_host_distinguishes_async_questions_from_blocking_sync_waits_and_approvals() {
    let (_directory, storage, service) = fixture();
    let asynchronous = seed_chat(&storage, "async", true);
    let synchronous = seed_chat(&storage, "sync", true);
    let approval = seed_chat(&storage, "approval", true);
    for owner in [&asynchronous, &synchronous, &approval] {
        resident(&service, owner);
    }
    let question: HumanInteractionToolInput =
        serde_json::from_value(json!({"questions":[{"title":"Continue?"}]})).unwrap();
    storage
        .create_human_interaction_request(&asynchronous, HumanInteractionMode::Async, &question)
        .unwrap();
    storage
        .admit_sync_human_interaction(&HumanInteractionSyncAdmission {
            owner: synchronous,
            input: question,
            checkpoint: json!({"schemaVersion":1}),
            usage: None,
            content: String::new(),
            predecessor: None,
            pending_action_predecessor: None,
        })
        .unwrap();
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: "pending-approval".into(),
            run_id: approval.run_id,
            conversation_id: Some(approval.conversation_id),
            assistant_message_id: Some(approval.assistant_message_id),
            action_type: "tool_call".into(),
            tool_name: "apply_patch".into(),
            tool_call_id: Some("approval-call".into()),
            status: "pending".into(),
            target_status: None,
            action_json: "{}".into(),
            agent_input_json: "{}".into(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    let mut projection = json!({"runtime":{"nodes":[{"conversationId":"async"},{"conversationId":"sync"},{"conversationId":"approval"}]}});
    service.enrich_workflow_awareness(&mut projection).unwrap();
    let nodes = &projection["runtime"]["nodes"];
    assert_eq!(nodes[0]["state"], "running");
    assert_eq!(nodes[0]["hasPendingInteraction"], true);
    assert_eq!(nodes[0]["waitingForInteraction"], false);
    assert_eq!(nodes[1]["state"], "waiting_interaction");
    assert_eq!(nodes[1]["hasPendingInteraction"], true);
    assert_eq!(nodes[1]["waitingForInteraction"], true);
    assert_eq!(nodes[2]["state"], "waiting_approval");
    assert_eq!(nodes[2]["waitingForApproval"], true);
    assert_eq!(nodes[2]["waitingForInteraction"], false);
}

#[test]
fn batched_awareness_keeps_sync_wait_boundaries_and_approval_over_pause_priority() {
    let (directory, storage, service) = fixture();
    let owner = seed_chat(&storage, "sync-batch", true);
    resident(&service, &owner);
    storage
        .admit_sync_human_interaction(&HumanInteractionSyncAdmission {
            owner: owner.clone(),
            input: serde_json::from_value(json!({"questions":[{"title":"Continue?"}]})).unwrap(),
            checkpoint: json!({"schemaVersion":1}),
            usage: None,
            content: String::new(),
            predecessor: None,
            pending_action_predecessor: None,
        })
        .unwrap();
    let connection = rusqlite::Connection::open(directory.path().join("awareness.sqlite")).unwrap();
    for status in ["waiting", "claimed", "executing", "model_in_flight"] {
        connection
            .execute(
                "UPDATE human_interaction_suspensions SET status=?1",
                [status],
            )
            .unwrap();
        let mut projection =
            json!({"runtime":{"nodes":[{"conversationId":"sync-batch","paused":true}]}});
        service.enrich_workflow_awareness(&mut projection).unwrap();
        let node = &projection["runtime"]["nodes"][0];
        let waiting = storage
            .is_sync_human_interaction_run_waiting(&owner.run_id)
            .unwrap();
        assert_eq!(node["waitingForInteraction"], waiting);
        assert_eq!(
            node["state"],
            if waiting {
                "waiting_interaction"
            } else {
                "stopped"
            }
        );
        assert_eq!(node["hasPendingInteraction"], true);
    }
    connection
        .execute(
            "UPDATE human_interaction_suspensions SET status='waiting'",
            [],
        )
        .unwrap();
    storage
        .store_pending_agent_action(AgentPendingActionRecord {
            action_id: "batch-approval".into(),
            run_id: owner.run_id,
            conversation_id: Some(owner.conversation_id),
            assistant_message_id: Some(owner.assistant_message_id),
            action_type: "tool_call".into(),
            tool_name: "apply_patch".into(),
            tool_call_id: Some("approval-call".into()),
            status: "pending".into(),
            target_status: None,
            action_json: "{}".into(),
            agent_input_json: "{}".into(),
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    let mut projection = json!({"nodes":[{"conversationId":"sync-batch","paused":true}]});
    service.enrich_workflow_awareness(&mut projection).unwrap();
    assert_eq!(projection["nodes"][0]["state"], "waiting_approval");
    assert_eq!(projection["nodes"][0]["waitingForInteraction"], true);
}
