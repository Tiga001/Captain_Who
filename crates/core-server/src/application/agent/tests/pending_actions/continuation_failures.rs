use super::fixtures::{seed_durable_pending_owner, test_pending_resume_checkpoint_for_call};
use super::*;

#[tokio::test]
async fn pre_runtime_continuation_failure_terminalizes_turn_and_releases_occupancy() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let run_id = "pre-runtime-continuation-failure-run";
    let conversation_id = "pre-runtime-continuation-failure-conversation";
    let assistant_message_id = "pre-runtime-continuation-failure-assistant";
    let call = AgentToolCall {
        id: "pre-runtime-continuation-failure-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);

    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();
    service.register_usage_context(
        run_id,
        AgentRunUsageContext {
            conversation_id: conversation_id.to_string(),
            assistant_message_id: assistant_message_id.to_string(),
            run_id: run_id.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );

    // The pending row contains a valid frozen checkpoint. Corrupt only the reconstructed
    // continuation so restoration fails after the exact durable Turn owner has been acquired,
    // but before any Runtime/model request can start.
    let mut resumed_input = agent_input;
    resumed_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .extension_snapshots
        .push(mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: u32::MAX,
            state: json!({}),
        });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            None,
        )
        .await;
    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();

    let pending_status: String = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [&storage_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "failed", "events: {events:#?}");
    let trace = storage
        .get_conversation_turn_trace(assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Failed
    );
    assert_eq!(trace.run_id, run_id);
    assert_eq!(trace.conversation_id, conversation_id);
    assert!(matches!(
        trace.items.first(),
        Some(ConversationTurnTraceItem::ToolCall { call_id, .. }) if call_id == &call.id
    ));
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    let conversation = storage.load_conversation(conversation_id).unwrap().unwrap();
    let assistant = conversation
        .messages
        .iter()
        .find(|message| message.id == assistant_message_id)
        .unwrap();
    assert_eq!(assistant.status.as_deref(), Some("error"));
    assert!(
        assistant.content.contains("Skill extension version"),
        "unexpected terminal assistant content: {}",
        assistant.content
    );
    let usage = storage
        .load_agent_usage_for_owner(run_id, conversation_id, assistant_message_id)
        .unwrap()
        .expect("pre-Runtime failure must settle the existing run Usage owner");
    assert_eq!(usage.status.as_deref(), Some("failed"));
    assert!(usage
        .error
        .as_deref()
        .is_some_and(|error| error.contains("Skill extension version")));

    assert!(!service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    service
        .reserve_conversation_turn(conversation_id, "next-run", "next-assistant")
        .expect("a durable terminal continuation must admit the next Turn");
    service.release_conversation_turn_if_current(conversation_id, "next-run");

    let error = events
        .iter()
        .find(|event| event["params"]["type"] == "error")
        .expect("terminal failure must emit an error event");
    assert_eq!(
        error["params"]["code"],
        "skill_resource_snapshot_unavailable"
    );
    assert_eq!(error["params"]["recoverable"], false);
    let done = events
        .iter()
        .find(|event| event["params"]["type"] == "done")
        .expect("terminal failure must emit a Done event");
    assert_eq!(done["params"]["status"], "failed");
    assert_eq!(done["params"]["success"], false);
}

#[tokio::test]
async fn pre_runtime_continuation_failure_cas_conflict_preserves_turn_for_recovery() {
    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let run_id = "pre-runtime-continuation-conflict-run";
    let conversation_id = "pre-runtime-continuation-conflict-conversation";
    let assistant_message_id = "pre-runtime-continuation-conflict-assistant";
    let call = AgentToolCall {
        id: "pre-runtime-continuation-conflict-call".to_string(),
        tool: "approval_tool".to_string(),
        args: json!({}),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        run_id,
        None,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(conversation_id.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        conversation_id,
        assistant_message_id,
        run_id,
        &call,
        AgentToolIdentity::Unregistered {
            tool_name: call.tool.clone(),
        },
        1,
    );
    service
        .store_pending_action(
            run_id,
            conversation_id,
            assistant_message_id,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(run_id, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();

    let mut resumed_input = agent_input;
    resumed_input
        .resume_checkpoint
        .as_mut()
        .unwrap()
        .extension_snapshots
        .push(mycopilot_core::AgentExtensionSnapshot {
            extension_id: "skills".to_string(),
            version: u32::MAX,
            state: json!({}),
        });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            // Deliberately stale: the durable executor target is completed. The terminalization
            // transaction must lose this CAS without altering any durable fact or Turn lease.
            PendingActionStatus::Rejected,
            None,
        )
        .await;

    let (status, target_status): (String, Option<String>) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(target_status.as_deref(), Some("completed"));
    assert_eq!(
        storage
            .get_conversation_turn_trace(assistant_message_id)
            .unwrap()
            .unwrap()
            .terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let conversation = storage.load_conversation(conversation_id).unwrap().unwrap();
    assert_eq!(conversation.messages[0].status.as_deref(), Some("pending"));
    assert!(service
        .has_conversation_turn_occupancy(conversation_id)
        .unwrap());
    assert!(service
        .reserve_conversation_turn(conversation_id, "conflicting-next-run", "next-assistant")
        .is_err());

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    let error = events
        .iter()
        .find(|event| event["params"]["type"] == "error")
        .expect("CAS conflict must emit a recoverable persistence error");
    assert_eq!(
        error["params"]["code"],
        "conversation_trace_persistence_failed"
    );
    assert_eq!(error["params"]["recoverable"], true);
    assert!(!events.iter().any(|event| event["params"]["type"] == "done"));
}

#[tokio::test]
async fn pre_spawn_cancelled_continuation_retries_real_pending_target_and_releases_resources() {
    const RUN_ID: &str = "pre-spawn-cancelled-continuation-run";
    const CONVERSATION_ID: &str = "pre-spawn-cancelled-continuation-conversation";
    const ASSISTANT_MESSAGE_ID: &str = "pre-spawn-cancelled-continuation-assistant";

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("storage.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    service
        .reserve_conversation_turn(CONVERSATION_ID, RUN_ID, ASSISTANT_MESSAGE_ID)
        .unwrap();
    service.ensure_turn_concurrency_permit(RUN_ID).unwrap();
    let gate = service.turn_concurrency_gate();
    assert_eq!(gate.active(), 1);

    let call = AgentToolCall {
        id: "pre-spawn-cancelled-continuation-call".to_string(),
        tool: "todo_update".to_string(),
        args: json!({ "items": [] }),
        approval_status: AgentApprovalStatus::Required,
        reason: None,
    };
    let provenance = AgentToolIdentity::RuntimeExtension {
        extension_id: "todo".to_string(),
        tool_name: call.tool.clone(),
    };
    let mut agent_input = serde_json::from_value::<AgentChatInput>(json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "secret",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "messages": []
    }))
    .unwrap();
    save_test_pending_provider_for_input(&storage, &mut agent_input);
    agent_input.resume_checkpoint = Some(test_pending_resume_checkpoint_for_call(
        &storage,
        RUN_ID,
        None,
        &call,
        provenance.clone(),
    ));
    let run_context = AgentRunContext {
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions::default(),
        collaboration_identity: None,
    };
    agent_input.context = Some(run_context.clone());
    agent_input.resume_checkpoint.as_mut().unwrap().run_context = Some(run_context);
    seed_durable_pending_owner(
        &storage,
        CONVERSATION_ID,
        ASSISTANT_MESSAGE_ID,
        RUN_ID,
        &call,
        provenance,
        1,
    );
    service
        .store_pending_action(
            RUN_ID,
            CONVERSATION_ID,
            ASSISTANT_MESSAGE_ID,
            AgentProposedAction::ToolCall { call: call.clone() },
            agent_input.clone(),
        )
        .unwrap();
    let storage_id = pending_action_storage_id(RUN_ID, &call.id);
    let pending = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .transition_pending_status(&pending, PendingActionStatus::Approved)
        .unwrap();
    let approved = service
        .pending_actions
        .lock()
        .unwrap_or_else(|error| error.into_inner())[&storage_id]
        .clone();
    service
        .persist_pending_target_status(&approved, PendingActionStatus::Completed)
        .unwrap();

    let usage = AgentUsage {
        input_tokens: Some(20),
        output_tokens: Some(8),
        output_thinking_tokens: Some(3),
        total_tokens: Some(28),
        cached_input_tokens: Some(2),
        cache_creation_input_tokens: None,
        billable_request_count: Some(1),
    };
    service.register_usage_context(
        RUN_ID,
        AgentRunUsageContext {
            conversation_id: CONVERSATION_ID.to_string(),
            assistant_message_id: ASSISTANT_MESSAGE_ID.to_string(),
            run_id: RUN_ID.to_string(),
            project_id: None,
            model_id: "test-model".to_string(),
            model_name: "Test Model".to_string(),
            provider_usage_semantics: ProviderUsageSemantics::StandardAdditive,
            input_price: None,
            cached_input_price: None,
            output_price: None,
            started_at: 1,
        },
    );
    service
        .persist_run_usage(
            RUN_ID,
            AgentRunStatus::WaitingForApproval,
            Some(usage.clone()),
            None,
        )
        .unwrap();

    let mut resumed_input = agent_input;
    resumed_input.approval_decision = Some(AgentApprovalDecision {
        action_id: call.id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resumed_input.tool_continuation = Some(AgentToolContinuation {
        call: call.clone(),
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: call.id.clone(),
            tool: call.tool.clone(),
            ok: true,
            result: Some(json!({ "revision": 1, "items": [], "updatedAt": 1 })),
            error: None,
        },
    });
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .commit_trace_snapshot_with_continuation(&approved, &resumed_input, &notifications)
        .unwrap();

    let cancellation = AgentCancellationToken::new();
    service.register_cancellation(RUN_ID, cancellation.clone());
    cancellation.cancel();
    inject_pending_status_transition_failure(&storage_id, "completed");
    service
        .run_action_continuation(
            approved,
            resumed_input,
            notifications,
            PendingActionStatus::Completed,
            Some(cancellation),
        )
        .await;

    let (pending_status, pending_target_status): (String, String) =
        rusqlite::Connection::open(&database_path)
            .unwrap()
            .query_row(
                "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
                [&storage_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
    assert_eq!(pending_status, "completed");
    assert_eq!(pending_target_status, "completed");
    assert_eq!(
        service
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner())[&storage_id]
            .snapshot
            .status,
        PendingActionStatus::Completed
    );

    let trace = storage
        .get_conversation_turn_trace(ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Cancelled
    );
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id, .. } if call_id == &call.id
    )));
    let persisted_usage = storage
        .load_agent_usage_for_owner(RUN_ID, CONVERSATION_ID, ASSISTANT_MESSAGE_ID)
        .unwrap()
        .unwrap();
    assert_eq!(persisted_usage.status.as_deref(), Some("cancelled"));
    assert_eq!(persisted_usage.total_tokens, usage.total_tokens);
    assert_eq!(persisted_usage.billable_request_count, 1);
    assert!(!service
        .has_conversation_turn_occupancy(CONVERSATION_ID)
        .unwrap());
    assert_eq!(gate.active(), 0);
    assert!(!service
        .active_turn_permits
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));
    assert!(!service
        .usage_contexts
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));
    assert!(!service
        .cancellations
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .contains_key(RUN_ID));

    let events = std::iter::from_fn(|| receiver.try_recv().ok()).collect::<Vec<_>>();
    assert!(!events
        .iter()
        .any(|event| event["params"]["type"] == "error"));
    let done = events
        .iter()
        .filter(|event| event["params"]["type"] == "done")
        .collect::<Vec<_>>();
    assert_eq!(done.len(), 1, "events: {events:#?}");
    assert_eq!(done[0]["params"]["status"], "cancelled");
    assert_eq!(done[0]["params"]["usage"]["totalTokens"], 28);
    assert_eq!(done[0]["params"]["usage"]["billableRequestCount"], 1);
}
