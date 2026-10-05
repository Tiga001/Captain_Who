use super::*;

async fn wait_for_warmed_history(service: &AgentService, conversation_id: &str) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !service.has_prepared_history_for_test(conversation_id) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("idle backend history is prepared without a renderer");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn workflow_capacity_wait_warms_history_without_claiming_pending_letter() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("warm-pending.sqlite");
    let storage = Arc::new(StorageService::open(&path).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let input = synthetic_workflow_input(&storage, &target, "warm-pending-input");
    let db = rusqlite::Connection::open(&path).unwrap();
    insert_workflow_test_input(&db, &input);
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        storage.clone(),
        None,
        1,
    )
    .unwrap();
    service.grant_execution_access_for_test();
    service
        .register_turn_concurrency_permit(
            "warmup-capacity-owner",
            service.turn_concurrency_gate().try_acquire().unwrap(),
        )
        .unwrap();
    let (notifications, mut events) = crate::transport::outbound_channel();
    let before = storage.load_conversation_for_turn(&target).unwrap();
    service.dispatch_workflow_deliveries(notifications.clone());
    wait_for_warmed_history(&service, &target).await;

    let pending = storage
        .workflow_execution_load_input(&input.id)
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, InputStatus::Pending);
    assert_eq!(pending.mail_status, MailStatus::Pending);
    assert!(pending.run_id.is_none());
    assert!(pending.delivery_id.is_none());
    assert_eq!(
        serde_json::to_value(storage.load_conversation_for_turn(&target).unwrap()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert!(storage
        .list_conversation_turn_traces(&target)
        .unwrap()
        .is_empty());
    assert!(!service.has_conversation_turn_occupancy(&target).unwrap());
    assert!(storage
        .list_active_conversation_world_state_records(&target)
        .unwrap()
        .is_empty());

    service.release_turn_concurrency_permit("warmup-capacity-owner");
    service.dispatch_workflow_deliveries(notifications.clone());
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let request = request_body(&mut stream).await;
    assert_eq!(
        request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["content"].to_string().contains(&input.content))
            .count(),
        1,
        "a warmed pending letter enters the first real model request once"
    );
    let admitted = storage
        .workflow_execution_load_input(&input.id)
        .unwrap()
        .unwrap();
    let admitted_run = admitted
        .run_id
        .clone()
        .expect("real dispatch claims one run");
    let admitted_message = admitted
        .delivery_id
        .clone()
        .expect("one projection is bound");
    service.dispatch_workflow_deliveries(notifications.clone());
    assert_eq!(
        storage
            .workflow_execution_load_input(&input.id)
            .unwrap()
            .unwrap()
            .run_id,
        Some(admitted_run.clone()),
        "a second dispatch cannot claim a warmed letter again"
    );
    respond(
        &mut stream,
        json!({"role":"assistant","content":"Mail observed once."}),
        "stop",
    )
    .await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            assert_ne!(event["params"]["type"], "error", "{event}");
            if event["params"]["type"] == "done" {
                break;
            }
        }
        while service.has_conversation_turn_occupancy(&target).unwrap() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    service.dispatch_workflow_deliveries(notifications);
    let traces = storage.list_conversation_turn_traces(&target).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].run_id, admitted_run);
    assert_eq!(
        traces[0].items.iter().filter(|item| matches!(
            item,
            ConversationTurnTraceItem::WorkflowDelivery { input_id, .. } if input_id == &input.id
        )).count(),
        1,
        "the authoritative journal records a single mail injection"
    );
    let conversation = storage.load_conversation(&target).unwrap().unwrap();
    assert_eq!(
        conversation
            .messages
            .iter()
            .filter(|message| message.id == admitted_message)
            .count(),
        1
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn durable_terminal_warms_unopened_conversation_without_starting_another_turn() {
    let directory = tempdir().unwrap();
    let storage =
        Arc::new(StorageService::open(&directory.path().join("warm-terminal.sqlite")).unwrap());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    storage.save_model_settings(settings).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let service = AgentService::new_authorized_for_test(storage.clone());
    let (notifications, mut events) = crate::transport::outbound_channel();
    service
        .start_conversation_turn(root_input(&target, "Only one turn"), notifications)
        .unwrap();
    let (mut stream, _) = tokio::time::timeout(Duration::from_secs(10), listener.accept())
        .await
        .unwrap()
        .unwrap();
    let _ = request_body(&mut stream).await;
    respond(
        &mut stream,
        json!({"role":"assistant","content":"Finished once."}),
        "stop",
    )
    .await;
    drop(stream);
    tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(event) = events.recv().await {
            if event["params"]["type"] == "done" {
                break;
            }
        }
    })
    .await
    .unwrap();
    wait_for_warmed_history(&service, &target).await;
    let traces = storage.list_conversation_turn_traces(&target).unwrap();
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].terminal_status,
        ConversationTurnTraceTerminalStatus::Completed
    );
    assert!(!service.has_conversation_turn_occupancy(&target).unwrap());
    assert!(
        tokio::time::timeout(Duration::from_millis(200), listener.accept())
            .await
            .is_err()
    );
}

#[test]
fn workflow_claim_preserves_warmed_history_and_reuses_measured_prefix_without_human_replay() {
    let directory = tempdir().unwrap();
    let path = directory.path().join("warm-workflow-prefix.sqlite");
    let storage = Arc::new(StorageService::open(&path).unwrap());
    storage.save_model_settings(test_model_settings()).unwrap();
    let (_, target) = workflow_fixture(&storage);
    let conversation = super::super::provider_transition::conversation_with_completed_history(
        &target,
        Some("model-1"),
    );
    let old_user_id = conversation.messages[0].id.clone();
    let old_assistant_id = conversation.messages[1].id.clone();
    storage.save_conversation(conversation).unwrap();
    storage
        .finalize_chat_message_with_conversation_trace(
            &target,
            &old_assistant_id,
            "已完成",
            Some("sent"),
            "completed",
            &completed_conversation_trace_without_items(
                "completed-before-mail",
                &target,
                &old_assistant_id,
            ),
            2,
            3,
        )
        .unwrap();
    let pending = synthetic_workflow_input(&storage, &target, "warmed-workflow-letter");
    insert_workflow_test_input(&rusqlite::Connection::open(&path).unwrap(), &pending);
    let service = AgentService::new_authorized_for_test(storage.clone());
    service
        .get_context_window_snapshot(AgentContextWindowSnapshotInput {
            conversation_id: Some(target.clone()),
            project_id: None,
            model_id: "model-1".into(),
            max_tokens: None,
            prompt_preferences: None,
            permissions: AgentPermissions::default(),
            skills: vec![],
        })
        .unwrap();
    let history = service.prepare_cached_history(&target).unwrap().unwrap();
    let user_id = format!("workflow-message-{}", pending.id);
    storage
        .workflow_execution_bind_run(&target, "warmed-mail-run")
        .unwrap();
    assert!(storage
        .workflow_execution_bind_input(&pending.id, "warmed-mail-run", &user_id)
        .unwrap());
    assert!(
        storage
            .is_conversation_history_snapshot_current(&history.source.version)
            .unwrap(),
        "claiming the next letter must not invalidate the settled history prefix"
    );
    let claimed = storage
        .workflow_execution_load_input(&pending.id)
        .unwrap()
        .unwrap();
    let mut input = root_input(&target, &claimed.content);
    input.context_window_indicator_enabled = true;
    input.user_message_id = Some(user_id.clone());
    input.assistant_message_id = Some("warmed-mail-assistant".into());
    let mut expected =
        crate::application::agent_support::conversation_history_messages_with_model_context(
            &history.source.conversation.to_record(),
            &history.source.traces,
            &history.source.model_context_logs,
            history.source.compaction_summary.as_ref(),
            &[],
        )
        .unwrap();
    storage
        .project_agent_messages_for_model(&target, &mut expected)
        .unwrap();
    let prepared = crate::application::agent_support::prepare_reserved_workflow_turn_with_history(
        &storage,
        &service.skills,
        input,
        "warmed-mail-run",
        Some(history.source.conversation.to_record()),
        Some(history.source.conversation_revision),
        claimed,
        &|| Ok(()),
        Some(history),
    )
    .unwrap();
    assert!(prepared.prepared_history.is_some());
    assert_eq!(
        serde_json::to_value(&prepared.agent_input.messages).unwrap(),
        serde_json::to_value(expected).unwrap()
    );
    assert_eq!(
        prepared
            .agent_input
            .messages
            .iter()
            .filter(|message| message.message_id.as_deref() == Some(old_user_id.as_str()))
            .count(),
        1
    );
    assert!(
        prepared
            .agent_input
            .messages
            .iter()
            .all(|message| message.message_id.as_deref() != Some(user_id.as_str())),
        "the collaborator letter must enter through WorkflowDelivery, not HumanText"
    );
    assert!(
        service.install_prepared_history(&prepared).unwrap(),
        "the measured prefix is really reused, not merely rebuilt correctly"
    );
    let mut cold = create_conversation_context_state(prepared.agent_input.clone()).unwrap();
    let mut states = service.conversation_context_states.lock().unwrap();
    let warm = states.get_mut(&target).unwrap();
    assert_eq!(warm.active_run_id.as_deref(), Some("warmed-mail-run"));
    assert_eq!(
        serde_json::to_value(warm.state.snapshot()).unwrap(),
        serde_json::to_value(cold.snapshot()).unwrap()
    );
}
