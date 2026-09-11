use super::managed_command_loop::{read_json_request, write_text_stream, write_tool_call_stream};
use super::*;
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy, AgentWakeStatus};
use std::time::Duration;
use tokio::net::TcpListener;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn child_command_rejection_reaches_its_model_through_the_root_approval_projection() {
    for feedback in [None, Some("请停止这条命令，改用现有信息回答。")] {
        assert_child_command_rejection(feedback).await;
    }
}

async fn assert_child_command_rejection(feedback: Option<&str>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (continuation_tx, continuation_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_json_request(&mut stream).await;
        assert!(request.to_string().contains("## 子 Agent 协作身份"));
        write_tool_call_stream(
            &mut stream,
            "provider-child-rejected-command",
            "run_command",
            json!({
                "command": "printf should-not-run > rejected-child-marker.txt",
                "reason": "verify that root rejection does not dispatch the child command"
            }),
            "Requesting approval for the delegated command.",
        )
        .await;
        drop(stream);
        let (mut stream, _) = listener.accept().await.unwrap();
        let request = read_json_request(&mut stream).await;
        continuation_tx.send(request).unwrap();
        write_text_stream(
            &mut stream,
            "The user rejected the command; it was not executed.",
        )
        .await;
    });

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let storage =
        Arc::new(StorageService::open(&fixture.path().join("child-rejection.sqlite")).unwrap());
    let project_id = "project-child-command-rejection";
    let root_conversation_id = "conversation-child-command-rejection-root";
    let root_agent_id = "agent-child-command-rejection-root";
    storage
        .save_project(ProjectRecord::with_primary_folder(
            project_id.to_string(),
            "Child rejection".to_string(),
            workspace.to_string_lossy().into_owned(),
            1,
        ))
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();
    storage
        .save_conversation(ChatConversationRecord {
            id: root_conversation_id.to_string(),
            project_id: Some(project_id.to_string()),
            model_id: Some("model-1".to_string()),
            title: "Command rejection root".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();
    storage
        .ensure_root_agent(&mycopilot_core::EnsureRootAgentInput {
            agent_id: root_agent_id.to_string(),
            conversation_id: root_conversation_id.to_string(),
            creation_request_id: "ensure-command-rejection-root".to_string(),
            task_name: "Root".to_string(),
        })
        .unwrap();
    seed_root_effective_permissions(
        &storage,
        root_agent_id,
        root_conversation_id,
        "command-rejection",
        AgentPermissions {
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            ..AgentPermissions::default()
        },
    );
    let child =
        crate::application::agent_collaboration::ChildAgentFactory::new(Arc::clone(&storage))
            .create_child(&mycopilot_core::CreateChildAgentInput {
                parent_agent_id: root_agent_id.to_string(),
                creation_request_id: "spawn-command-rejection-child".to_string(),
                task_name: "command_rejection_child".to_string(),
                task: "Request approval for the command, then follow the user's decision."
                    .to_string(),
                template_machine_key: None,
                explicit_model_id: None,
                reasoning_effort: None,
                fork_turns: mycopilot_core::AgentForkTurns::None,
            })
            .unwrap();
    let service = AgentService::try_new_deferred_startup_reconciliation_with_agent_limit(
        Arc::clone(&storage),
        None,
        1,
    )
    .unwrap();
    let (notifications, _receiver) = tokio::sync::mpsc::unbounded_channel();
    service
        .start_collaboration_dispatcher(notifications.clone())
        .unwrap();
    let approval = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(approval) = service
                .list_root_projected_approvals(root_conversation_id)
                .unwrap()
                .into_iter()
                .next()
            {
                let wake = storage
                    .get_agent_wake(&child.initial_wake.wake_id)
                    .unwrap()
                    .unwrap();
                if wake.status == AgentWakeStatus::WaitingForApproval {
                    break approval;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("child command did not reach root approval projection");
    assert_eq!(approval.source_agent_id, child.agent.agent_id);
    let decision = service
        .decide_root_projected_approval(
            root_conversation_id,
            &approval.approval_id,
            ProjectedApprovalDecision::Reject,
            feedback.map(ToString::to_string),
            notifications,
        )
        .unwrap();
    assert!(decision.accepted);
    assert_eq!(decision.status, "rejected");
    let request = tokio::time::timeout(Duration::from_secs(10), continuation_rx)
        .await
        .unwrap()
        .unwrap();
    assert!(request.to_string().contains("## 子 Agent 协作身份"));
    let resumed_model_input = request.to_string();
    assert!(resumed_model_input.contains("审批恢复后若要汇报子 Agent 状态，必须重新查询"));
    assert!(resumed_model_input.contains("command_rejection_child"));
    for private_identity in [
        root_agent_id,
        child.agent.agent_id.as_str(),
        child.agent.task_path.as_str(),
        child.initial_wake.wake_id.as_str(),
    ] {
        assert!(
            !resumed_model_input.contains(private_identity),
            "approval continuation leaked private collaboration identity: {private_identity}"
        );
    }
    let result = request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .expect("child continuation must receive its rejected ToolResult");
    assert_eq!(result["status"], "rejected");
    assert_eq!(result["code"], "command.approval_rejected");
    assert_eq!(result["decisionBy"], "user");
    assert_eq!(result["executionAttempted"], false);
    assert_eq!(result["retryable"], false);
    assert_eq!(result["userFeedback"].as_str(), feedback);
    assert_eq!(
        result["retryPolicy"],
        if feedback.is_some() {
            "follow_user_feedback_without_repeating_same_call"
        } else {
            "new_explicit_user_instruction_required"
        }
    );
    assert!(result["message"]
        .as_str()
        .unwrap()
        .contains("same or an equivalent command"));
    let wake = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let wake = storage
                .get_agent_wake(&child.initial_wake.wake_id)
                .unwrap()
                .unwrap();
            if wake.status.is_terminal() && service.turn_concurrency_gate().active() == 0 {
                break wake;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("rejected child continuation did not settle");
    assert_eq!(
        wake.status,
        AgentWakeStatus::Completed,
        "{:?}",
        wake.terminal_error
    );
    let trace = storage
        .get_conversation_turn_trace(wake.assistant_message_id.as_deref().unwrap())
        .unwrap()
        .unwrap();
    let observation = trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                tool, observation, ..
            } if tool == "run_command" => Some(observation),
            _ => None,
        })
        .expect("child history must retain the rejection decision");
    assert_eq!(observation["decisionBy"], "user");
    assert_eq!(observation["executionAttempted"], false);
    assert_eq!(observation["userFeedback"].as_str(), feedback);
    assert!(storage
        .list_agent_command_sessions(&child.agent.conversation_id, 10)
        .unwrap()
        .is_empty());
    assert!(!workspace.join("rejected-child-marker.txt").exists());
    assert!(service
        .list_root_projected_approvals(root_conversation_id)
        .unwrap()
        .is_empty());
    assert!(service
        .shutdown_collaboration_dispatcher()
        .await
        .unwrap()
        .is_some());
    server.await.unwrap();
}
