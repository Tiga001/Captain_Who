#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lazy_root_has_a_stable_parent_name_for_child_messages_during_approval() {
    const CONVERSATION_ID: &str = "conversation-parent-name-approval";
    const PROJECT: &str = "project-parent-name-approval";
    const USER_REQUEST: &str =
        "请指派一个子智能体回复“收到”，然后你运行一个需要我审批的安全命令。不要等待子智能体，先申请审批。";
    const CHILD_REPORT: &str = "收到；父任务等待审批时仍可接收子任务消息。";

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let server_requests = Arc::clone(&requests);
    let allow_child_message = Arc::new(tokio::sync::Notify::new());
    let server_allow_child_message = Arc::clone(&allow_child_message);
    let (stop_sender, mut stop_receiver) = tokio::sync::oneshot::channel::<()>();
    let model_server = tokio::spawn(async move {
        let mut handlers = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                _ = &mut stop_receiver => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    let requests = Arc::clone(&server_requests);
                    let allow_child_message = Arc::clone(&server_allow_child_message);
                    handlers.spawn(async move {
                        let request = read_provider_request(&mut stream).await;
                        requests.lock().unwrap().push(request.clone());
                        let results = tool_results(&request);
                        if request_text(&request).contains("## 子 Agent 协作身份") {
                            if results.is_empty() {
                                allow_child_message.notified().await;
                                write_tool_call(
                                    &mut stream,
                                    "call-report-to-parent",
                                    "send_message",
                                    json!({ "target": "主智能体", "message": CHILD_REPORT }),
                                )
                                .await;
                            } else {
                                // Finish even when the call failed, so the test reports the actual
                                // contract failure instead of hiding it behind retries/timeouts.
                                write_text(&mut stream, "The parent notification attempt finished.")
                                    .await;
                            }
                        } else {
                            match results.len() {
                                0 => {
                                    write_tool_call(
                                        &mut stream,
                                        "call-spawn-reporter",
                                        "spawn_agent",
                                        json!({
                                            "task_name": "reporter",
                                            "message": "向直接父任务发送消息：收到。",
                                            "fork_turns": "none"
                                        }),
                                    )
                                    .await;
                                }
                                1 => {
                                    write_tool_call(
                                        &mut stream,
                                        "call-root-approval",
                                        "run_command",
                                        json!({
                                            "command": "printf 'parent approval resumed\\n'",
                                            "reason": "验证父任务等待审批时子任务仍可发送消息"
                                        }),
                                    )
                                    .await;
                                }
                                2 => write_text(&mut stream, "已收到子任务消息，命令执行完成。").await,
                                count => panic!("unexpected root ToolResult count: {count}"),
                            }
                        }
                    });
                }
            }
        }
        while let Some(result) = handlers.join_next().await {
            result.unwrap();
        }
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("parent-name-approval.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: PROJECT.to_string(),
            name: "Parent name approval".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    assert!(storage
        .get_agent_node_by_conversation(CONVERSATION_ID)
        .unwrap()
        .is_none());

    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        2,
    )
    .unwrap();
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(CONVERSATION_ID.to_string()),
                project_id: Some(PROJECT.to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: USER_REQUEST.to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-parent-name-approval".to_string()),
                assistant_message_id: Some("assistant-parent-name-approval".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications.clone(),
        )
        .unwrap();
    let waiting = collect_root_until_done(&mut receiver, &turn.run_id).await;
    assert_eq!(
        waiting.last().unwrap()["params"]["status"],
        "waiting_for_approval"
    );
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].run_id, turn.run_id);
    let action_id = pending[0].action_id.clone();
    let root = storage
        .get_agent_node_by_conversation(CONVERSATION_ID)
        .unwrap()
        .expect("the first real Harness use creates the root lazily");
    let child = storage
        .list_agent_tree(&root.agent_id)
        .unwrap()
        .into_iter()
        .find(|node| node.task_name == "reporter")
        .unwrap();

    allow_child_message.notify_one();
    wait_for_dispatcher_idle(&database_path).await;
    assert_eq!(service.list_pending_actions().len(), 1);
    let root_trace = storage
        .get_conversation_turn_trace(&turn.assistant_message_id)
        .unwrap()
        .unwrap();
    assert_eq!(
        root_trace.terminal_status,
        ConversationTurnTraceTerminalStatus::InProgress
    );
    let report_count: i64 = rusqlite::Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM agent_mailbox_messages
             WHERE sender_agent_id = ?1 AND recipient_agent_id = ?2
               AND kind = 'message' AND content = ?3",
            rusqlite::params![&child.agent_id, &root.agent_id, CHILD_REPORT],
            |row| row.get(0),
        )
        .unwrap();

    service
        .approve_action(&turn.run_id, &action_id, notifications)
        .unwrap();
    let completed = collect_root_until_done(&mut receiver, &turn.run_id).await;
    assert_eq!(completed.last().unwrap()["params"]["status"], "completed");
    service.shutdown_collaboration_dispatcher().await.unwrap();
    stop_sender.send(()).unwrap();
    model_server.await.unwrap();

    assert_eq!(root.task_name, "主智能体");
    let conversation = storage.load_conversation(CONVERSATION_ID).unwrap().unwrap();
    assert!(conversation.title.contains("“收到”"));
    assert_ne!(conversation.title, root.task_name);
    assert_eq!(
        report_count, 1,
        "the child sends to its parent exactly once"
    );
    let requests = requests.lock().unwrap();
    let child_requests = requests
        .iter()
        .filter(|request| request_text(request).contains("## 子 Agent 协作身份"))
        .collect::<Vec<_>>();
    assert_eq!(
        child_requests.len(),
        2,
        "no target-resolution retry is needed"
    );
    assert!(request_text(child_requests[0]).contains("直接父任务名称：`主智能体`"));
    assert_eq!(
        tool_results(child_requests[1]),
        vec![json!({ "taskName": "主智能体", "deliveryState": "queued" })]
    );
    let root_continuation = requests
        .iter()
        .rev()
        .find(|request| !request_text(request).contains("## 子 Agent 协作身份"))
        .unwrap();
    assert!(request_text(root_continuation).contains(CHILD_REPORT));
}
