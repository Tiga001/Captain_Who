use super::provider_profiles::{
    collect_until_done, read_provider_request, save_provider_profile_fixture, turn_input,
    write_provider_stream,
};
use super::*;
use tokio::net::TcpListener;

fn continue_input(
    source: &AgentConversationTurnOutput,
    suffix: &str,
) -> AgentConversationTurnContinueInput {
    AgentConversationTurnContinueInput {
        request_id: format!("continue-request-{suffix}"),
        conversation_id: source.conversation_id.clone(),
        source_assistant_message_id: source.assistant_message_id.clone(),
        assistant_message_id: format!("continued-assistant-{suffix}"),
        model_id: "model-1".into(),
        permissions: AgentPermissions::default(),
        context_window_indicator_enabled: true,
        skills: Vec::new(),
    }
}

fn messages_containing<'a>(request: &'a Value, marker: &str) -> Vec<&'a Value> {
    request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|message| message["content"].to_string().contains(marker))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_can_approve_a_command_and_apply_guidance_without_repeating_stop_facts() {
    use super::managed_command_loop::write_tool_call_stream;
    use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (initial_seen_tx, initial_seen_rx) = tokio::sync::oneshot::channel();
    let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
    let (approved_seen_tx, approved_seen_rx) = tokio::sync::oneshot::channel();
    let (guided_tx, guided_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_provider_request(&mut stream).await;
        initial_seen_tx.send(()).unwrap();
        stopped_rx.await.unwrap();
        drop(stream);

        let (mut stream, _) = listener.accept().await.unwrap();
        let resumed = read_provider_request(&mut stream).await;
        write_tool_call_stream(
            &mut stream,
            "command-after-continuation",
            "run_command",
            json!({"command": "printf resumed", "reason": "verify continued task approval"}),
            "Preparing the command.",
        )
        .await;
        drop(stream);

        let (mut stream, _) = listener.accept().await.unwrap();
        let approved = read_provider_request(&mut stream).await;
        approved_seen_tx.send(()).unwrap();
        guided_rx.await.unwrap();
        write_provider_stream(
            &mut stream,
            json!({"role": "assistant", "content": "Command handled."}),
            "stop",
        )
        .await;
        drop(stream);

        let (mut stream, _) = listener.accept().await.unwrap();
        let guided = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({"role": "assistant", "content": "命令已完成。"}),
            "stop",
        )
        .await;
        [resumed, approved, guided]
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("approval.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "continuation-approval-project",
            "Test",
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let mut input = turn_input("model-1");
    input.project_id = Some("continuation-approval-project".into());
    input.content = "Run the requested command after approval.".into();
    let source = service
        .start_conversation_turn(input, notifications.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), initial_seen_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(service.cancel_run_checked(&source.run_id).unwrap());
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(events.last().unwrap()["params"]["status"], "cancelled");
    stopped_tx.send(()).unwrap();

    let mut input = continue_input(&source, "approval");
    input.permissions.command = AgentCommandPermission::RequireApproval;
    input.permissions.command_safety = AgentCommandSafetyPolicy::FullAccess;
    let continued = service
        .continue_conversation_turn(input, notifications.clone())
        .unwrap();
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(
        events.last().unwrap()["params"]["status"],
        "waiting_for_approval",
        "{events:?}"
    );
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    service
        .approve_action(
            &continued.run_id,
            &pending[0].action_id,
            notifications.clone(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), approved_seen_rx)
        .await
        .unwrap()
        .unwrap();
    let guidance = service
        .steer_run(
            AgentSteerRunInput {
                conversation_id: continued.conversation_id.clone(),
                expected_run_id: continued.run_id.clone(),
                client_message_id: "continued-guidance".into(),
                content: "中文回答".into(),
                attachments: Vec::new(),
                folder_references: Vec::new(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(guidance.status, AgentSteerRunResultStatus::Queued);
    guided_tx.send(()).unwrap();
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(
        events.last().unwrap()["params"]["status"],
        "completed",
        "{events:?}"
    );
    assert!(events
        .iter()
        .any(|event| event["params"]["type"] == "guidance_applied"));

    let requests = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    for request in &requests {
        for marker in [
            "user_turn_interrupted",
            "user_turn_continued",
            "Run the requested command after approval.",
        ] {
            assert_eq!(messages_containing(request, marker).len(), 1, "{marker}");
        }
    }
    assert_eq!(messages_containing(&requests[2], "中文回答").len(), 1);
    assert!(requests[1]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|message| message["role"] == "tool"));
    assert_eq!(
        storage
            .load_agent_run_guidance(&guidance.guidance_id)
            .unwrap()
            .unwrap()
            .status,
        AgentGuidanceStatus::Applied
    );
    let trace = storage
        .get_conversation_turn_trace(&continued.assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        trace.terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert!(!trace.user_interrupted());
    let conversation = storage
        .load_conversation(&continued.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "assistant")
            .count(),
        2
    );
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_preserves_stop_history_is_idempotent_and_never_repeats_the_user_message() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrived_tx, mut arrived_rx) = tokio::sync::mpsc::unbounded_channel();
    let (release_tx, mut release_rx) = tokio::sync::mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut requests = Vec::new();
        for index in 0..5 {
            let (mut stream, _) = listener.accept().await.unwrap();
            requests.push(read_provider_request(&mut stream).await);
            if index < 2 {
                arrived_tx.send(()).unwrap();
                release_rx.recv().await.unwrap();
            } else {
                write_provider_stream(
                    &mut stream,
                    json!({"role": "assistant", "content": "Task completed."}),
                    "stop",
                )
                .await;
            }
        }
        requests
    });
    let fixture = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("continuation.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let mut initial = turn_input("model-1");
    initial.conversation_id = Some("conversation-continuation".into());
    initial.content = "Original task must occur exactly once.".into();
    let source = service
        .start_conversation_turn(initial, notifications.clone())
        .unwrap();
    if tokio::time::timeout(Duration::from_secs(10), arrived_rx.recv())
        .await
        .is_err()
    {
        let mut unexpected = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if matches!(event["params"]["type"].as_str(), Some("error" | "done")) {
                unexpected.push(event);
            }
        }
        panic!("provider request never arrived: {unexpected:?}");
    }

    // A running task is not resumable, even when the caller guesses a valid message identity.
    assert!(service
        .continue_conversation_turn(continue_input(&source, "running"), notifications.clone())
        .is_err());
    assert!(service.cancel_run_checked(&source.run_id).unwrap());
    let stopped = collect_until_done(&mut receiver).await;
    assert_eq!(stopped.last().unwrap()["params"]["status"], "cancelled");
    assert_eq!(stopped.last().unwrap()["params"]["userInterrupted"], true);
    release_tx.send(()).unwrap();
    let source_trace = storage
        .get_conversation_turn_trace(&source.assistant_message_id)
        .unwrap()
        .unwrap();
    assert!(source_trace.user_interrupted());
    let fork = storage
        .fork_conversation_request_view(mycopilot_core::storage::models::ForkConversationRequest {
            request_id: "fork-stopped-continuation".into(),
            source_conversation_id: source.conversation_id.clone(),
            fork_point: mycopilot_core::storage::models::ConversationForkPoint::Latest {},
        })
        .unwrap();

    let mut invalid_skill = continue_input(&source, "invalid-skill");
    invalid_skill.skills = vec![mycopilot_protocol_rs::SkillSelectionDto {
        id: "invalid".into(),
        revision: "invalid".into(),
    }];
    let refusal = service
        .continue_conversation_turn(invalid_skill, notifications.clone())
        .unwrap_err();
    assert_eq!(refusal.data().unwrap()["code"], "invalidSelection");
    assert_eq!(
        refusal.data().unwrap()["continuationAdmissionRejected"],
        true
    );
    assert!(storage
        .get_conversation_turn_trace("continued-assistant-invalid-skill")
        .unwrap()
        .is_none());

    let first_request = continue_input(&source, "one");
    let first = service
        .continue_conversation_turn(first_request.clone(), notifications.clone())
        .unwrap();
    assert_eq!(first.user_message_id, source.user_message_id);
    assert_ne!(first.run_id, source.run_id);
    let replay = service
        .continue_conversation_turn(first_request.clone(), notifications.clone())
        .unwrap();
    assert_eq!(replay.run_id, first.run_id);
    let mut conflict = first_request.clone();
    conflict.permissions.write = AgentWritePermission::All;
    assert!(service
        .continue_conversation_turn(conflict, notifications.clone())
        .unwrap_err()
        .to_string()
        .contains("request_conflict"));
    // A different request ID cannot restart the stale source while its continuation is active.
    assert!(service
        .continue_conversation_turn(continue_input(&source, "stale"), notifications.clone())
        .is_err());
    let admitted_log = storage
        .get_conversation_model_context_log(&first.assistant_message_id)
        .unwrap()
        .unwrap();
    assert!(admitted_log
        .items
        .first()
        .unwrap()
        .content
        .contains("user_turn_continued"));
    if tokio::time::timeout(Duration::from_secs(10), arrived_rx.recv())
        .await
        .is_err()
    {
        let mut unexpected = Vec::new();
        while let Ok(event) = receiver.try_recv() {
            if matches!(event["params"]["type"].as_str(), Some("error" | "done")) {
                unexpected.push(event);
            }
        }
        panic!("provider request never arrived: {unexpected:?}");
    }
    assert!(service.cancel_run_checked(&first.run_id).unwrap());
    let stopped = collect_until_done(&mut receiver).await;
    assert_eq!(stopped.last().unwrap()["params"]["status"], "cancelled");
    assert_eq!(stopped.last().unwrap()["params"]["userInterrupted"], true);
    release_tx.send(()).unwrap();

    let second_request = continue_input(&first, "two");
    let second = service
        .continue_conversation_turn(second_request.clone(), notifications.clone())
        .unwrap();
    let completion_events = collect_until_done(&mut receiver).await;
    assert_eq!(
        completion_events.last().unwrap()["params"]["status"],
        "completed",
        "{completion_events:?}"
    );
    let conversation = storage
        .load_conversation(&source.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 4);
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
    assert_eq!(
        storage
            .get_conversation_turn_trace(&source.assistant_message_id)
            .unwrap()
            .unwrap(),
        source_trace
    );
    assert!(!storage
        .get_conversation_turn_trace(&second.assistant_message_id)
        .unwrap()
        .unwrap()
        .user_interrupted());
    assert!(service
        .continue_conversation_turn(continue_input(&second, "completed"), notifications.clone())
        .is_err());

    // Request receipts survive a fresh Host instance and do not need an execution lease to replay.
    let restarted =
        AgentService::try_new_with_startup_reconciliation(Arc::clone(&storage), false, None)
            .unwrap();
    let replay = restarted
        .continue_conversation_turn(second_request, notifications.clone())
        .unwrap();
    assert_eq!(replay.run_id, second.run_id);
    assert_eq!(replay.assistant_message.content, "Task completed.");
    restarted.grant_execution_access_for_test();
    let mut followup = turn_input("model-1");
    followup.conversation_id = Some(source.conversation_id.clone());
    followup.user_message_id = Some("next-user".into());
    followup.assistant_message_id = Some("next-assistant".into());
    followup.content = "What happened earlier?".into();
    restarted
        .start_conversation_turn(followup, notifications.clone())
        .unwrap();
    let completion_events = collect_until_done(&mut receiver).await;
    assert_eq!(
        completion_events.last().unwrap()["params"]["status"],
        "completed",
        "{completion_events:?}"
    );
    assert!(completion_events.last().unwrap()["params"]
        .get("userInterrupted")
        .is_none());
    // A fork retains the certified historical fact; it does not need the source's live stop fence.
    let fork_assistant = fork.conversation.messages.last().unwrap();
    let fork_trace = storage
        .get_conversation_turn_trace(&fork_assistant.id)
        .unwrap()
        .unwrap();
    assert!(fork_trace.user_interrupted(), "fork trace: {fork_trace:?}");
    assert!(storage
        .get_agent_tree_run_stop(&fork_trace.run_id)
        .unwrap()
        .is_none());
    let mut fork_request = continue_input(&source, "fork");
    fork_request.conversation_id = fork.conversation.id;
    fork_request.source_assistant_message_id = fork_assistant.id.clone();
    restarted
        .continue_conversation_turn(fork_request, notifications)
        .unwrap();
    let completion_events = collect_until_done(&mut receiver).await;
    assert_eq!(
        completion_events.last().unwrap()["params"]["status"],
        "completed",
        "{completion_events:?}"
    );
    let requests = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    for request in &requests {
        assert_eq!(
            messages_containing(request, "Original task must occur exactly once.").len(),
            1
        );
        assert!(messages_containing(request, "Continue the interrupted task.").is_empty());
    }
    assert_eq!(
        messages_containing(&requests[1], "user_turn_interrupted").len(),
        1
    );
    assert_eq!(
        messages_containing(&requests[1], "user_turn_continued").len(),
        1
    );
    assert_eq!(
        messages_containing(&requests[2], "user_turn_interrupted").len(),
        2
    );
    assert_eq!(
        messages_containing(&requests[2], "user_turn_continued").len(),
        2
    );
    for marker in ["user_turn_interrupted", "user_turn_continued"] {
        let initial = messages_containing(&requests[1], marker)[0];
        let future = messages_containing(&requests[3], marker);
        assert_eq!(future.len(), 2);
        assert_eq!(initial, future[0]);
    }
    let lifecycle = requests[3]["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| {
            let text = message["content"].to_string();
            if text.contains("user_turn_interrupted") {
                Some("stop")
            } else if text.contains("user_turn_continued") {
                Some("continue")
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(lifecycle, vec!["stop", "continue", "stop", "continue"]);
}

#[tokio::test]
async fn continuation_rejects_generic_cancellation_without_mutating_history() {
    let fixture = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("generic-cancel.sqlite")).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let conversation = ChatConversationRecord {
        id: "generic-cancel".into(),
        project_id: None,
        model_id: Some("model-1".into()),
        title: "Generic cancellation".into(),
        created_at: 1,
        updated_at: 3,
        pinned_at: None,
        archived_at: None,
        unread_at: None,
        messages: vec![
            ChatMessageRecord {
                id: "generic-user".into(),
                role: "user".into(),
                content: "Task".into(),
                created_at: 1,
                status: Some("sent".into()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
                human_interaction_response: None,
            },
            ChatMessageRecord {
                id: "generic-assistant".into(),
                role: "assistant".into(),
                content: String::new(),
                created_at: 2,
                status: Some("sent".into()),
                attachments: Vec::new(),
                folder_references_json: None,
                agent_run_json: None,
                ui_state_json: None,
                human_interaction_response: None,
            },
        ],
    };
    storage.save_conversation(conversation).unwrap();
    let mut trace = mycopilot_core::ConversationTraceSnapshot::default().in_progress_trace(
        "generic-run",
        "generic-cancel",
        "generic-assistant",
    );
    trace.terminal_status = ConversationTurnTraceTerminalStatus::Cancelled;
    storage
        .replace_conversation_turn_trace(&trace, 2, 3)
        .unwrap();
    let (notifications, _) = crate::transport::outbound_channel();
    let error = service
        .continue_conversation_turn(
            AgentConversationTurnContinueInput {
                request_id: "generic-request".into(),
                conversation_id: "generic-cancel".into(),
                source_assistant_message_id: "generic-assistant".into(),
                assistant_message_id: "generic-next".into(),
                model_id: "model-1".into(),
                permissions: AgentPermissions::default(),
                context_window_indicator_enabled: true,
                skills: Vec::new(),
            },
            notifications,
        )
        .unwrap_err();
    assert_eq!(error.data().unwrap()["continuationAdmissionRejected"], true);
    assert_eq!(
        storage
            .load_conversation("generic-cancel")
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2
    );
    assert!(storage
        .get_conversation_turn_trace("generic-next")
        .unwrap()
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn continuation_retains_completed_tool_exchanges_before_the_stop() {
    use super::managed_command_loop::write_tool_call_stream;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (arrived_tx, arrived_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_provider_request(&mut stream).await;
        write_tool_call_stream(
            &mut stream,
            "read-before-stop",
            "read_file",
            json!({"path": "proof.txt"}),
            "Reading the file.",
        )
        .await;
        drop(stream);
        let (mut stream, _) = listener.accept().await.unwrap();
        let before_stop = read_provider_request(&mut stream).await;
        arrived_tx.send(()).unwrap();
        release_rx.await.unwrap();
        drop(stream);
        let (mut stream, _) = listener.accept().await.unwrap();
        let continued = read_provider_request(&mut stream).await;
        write_provider_stream(
            &mut stream,
            json!({"role": "assistant", "content": "I retained the file result."}),
            "stop",
        )
        .await;
        (before_stop, continued)
    });
    let fixture = tempdir().unwrap();
    fs::write(fixture.path().join("proof.txt"), "already read evidence").unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("tool-continuation.sqlite")).unwrap());
    save_provider_profile_fixture(
        &storage,
        &format!("http://{address}/v1/chat/completions"),
        None,
    );
    storage
        .save_project(ProjectRecord::with_primary_folder(
            "continuation-project",
            "Test",
            fixture.path().to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let service = AgentService::new_authorized_for_test(Arc::clone(&storage));
    let (notifications, mut receiver) = crate::transport::outbound_channel();
    let mut input = turn_input("model-1");
    input.project_id = Some("continuation-project".into());
    input.content = "Read the file and use its contents.".into();
    let source = service
        .start_conversation_turn(input, notifications.clone())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), arrived_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(service.cancel_run_checked(&source.run_id).unwrap());
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(events.last().unwrap()["params"]["status"], "cancelled");
    release_tx.send(()).unwrap();
    service
        .continue_conversation_turn(continue_input(&source, "tools"), notifications)
        .unwrap();
    let events = collect_until_done(&mut receiver).await;
    assert_eq!(events.last().unwrap()["params"]["status"], "completed");
    let (before, after) = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .unwrap()
        .unwrap();
    let tool_result = |request: &Value| {
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .cloned()
            .collect::<Vec<_>>()
    };
    let original = tool_result(&before);
    assert_eq!(original.len(), 1);
    assert!(
        original[0]["content"]
            .to_string()
            .contains("already read evidence"),
        "tool result: {original:?}"
    );
    assert_eq!(original, tool_result(&after));
    let calls = after["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|message| message["tool_calls"].as_array())
        .flat_map(|calls| calls.iter())
        .collect::<Vec<_>>();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["id"], original[0]["tool_call_id"]);
}
