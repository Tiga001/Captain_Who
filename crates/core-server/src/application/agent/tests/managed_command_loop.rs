use super::*;
use mycopilot_core::command::CommandSessionManager;
use mycopilot_core::storage::agent_command_session_repository::{
    AgentCommandSessionCreate, AgentCommandSessionCreateOutcome,
};
use mycopilot_core::{AgentCommandPermission, AgentCommandSafetyPolicy, AgentCommandSessionStatus};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const APPROVED_COMMAND_WITH_MIXED_LINE_ENDINGS: &str =
    "\r\n  printf 'approval-first\\n'\r\nprintf 'approval-second\\n'\r  printf 'single-authoritative-command-archive\\n'\r\n";
const APPROVED_COMMAND_CANONICAL: &str =
    "\n  printf 'approval-first\\n'\nprintf 'approval-second\\n'\n  printf 'single-authoritative-command-archive\\n'\n";
const BACKGROUND_COMMAND_WITH_MIXED_LINE_ENDINGS: &str =
    "printf 'ready\\n'\r\nsleep 0.25\rprintf 'progress\\n'\r\nsleep 0.35\r\nexit 7";
const BACKGROUND_COMMAND_CANONICAL: &str =
    "printf 'ready\\n'\nsleep 0.25\nprintf 'progress\\n'\nsleep 0.35\nexit 7";

async fn read_json_request(stream: &mut TcpStream) -> Value {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 4_096];
    let mut body_start = None;
    let mut expected_length = None;
    loop {
        let read = stream.read(&mut buffer).await.unwrap();
        assert!(read > 0, "connection closed before request completed");
        request.extend_from_slice(&buffer[..read]);
        if body_start.is_none() {
            if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let start = index + 4;
                let headers = String::from_utf8_lossy(&request[..index]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                    })
                    .unwrap();
                body_start = Some(start);
                expected_length = Some(start + content_length);
            }
        }
        if expected_length.is_some_and(|length| request.len() >= length) {
            break;
        }
    }
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.unwrap()]).unwrap()
}

async fn write_run_command_stream(stream: &mut TcpStream) {
    write_tool_call_stream(
        stream,
        "provider-managed-command",
        "run_command",
        json!({
            "command": "sleep 0.8",
            "reason": "exercise managed command handoff"
        }),
        "Starting the long-lived command.",
    )
    .await;
}

async fn write_tool_call_stream(
    stream: &mut TcpStream,
    call_id: &str,
    tool: &str,
    arguments: Value,
    narration: &str,
) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let tool_frame = json!({
        "choices": [{
            "delta": {
                "role": "assistant",
                "content": narration,
                "tool_calls": [{
                    "index": 0,
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": tool,
                        "arguments": serde_json::to_string(&arguments).unwrap()
                    }
                }]
            },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "tool_calls" }]
    });
    stream
        .write_all(
            format!("data: {tool_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

async fn write_text_stream(stream: &mut TcpStream, content: &str) {
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
        )
        .await
        .unwrap();
    let content_frame = json!({
        "choices": [{
            "delta": { "role": "assistant", "content": content },
            "finish_reason": null
        }]
    });
    let finish_frame = json!({
        "choices": [{ "delta": {}, "finish_reason": "stop" }]
    });
    stream
        .write_all(
            format!("data: {content_frame}\n\ndata: {finish_frame}\n\ndata: [DONE]\n\n").as_bytes(),
        )
        .await
        .unwrap();
}

struct ApprovedRunningHandoffCase {
    label: &'static str,
    initial_yield: Duration,
    command: &'static str,
}

async fn assert_approved_running_command_handoff(case: ApprovedRunningHandoffCase) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let call_id = format!("provider-approved-running-{}", case.label);
    let server_call_id = call_id.clone();
    let command = case.command.to_string();
    let server = tokio::spawn(async move {
        let (mut approval_stream, _) = listener.accept().await.unwrap();
        let _approval_request = read_json_request(&mut approval_stream).await;
        write_tool_call_stream(
            &mut approval_stream,
            &server_call_id,
            "run_command",
            json!({
                "command": command,
                "reason": "exercise approved Running-receipt handoff"
            }),
            "Preparing the command for approval.",
        )
        .await;
        drop(approval_stream);

        let (mut continuation_stream, _) =
            tokio::time::timeout(Duration::from_secs(5), listener.accept())
                .await
                .expect("approved Running receipt must resume the model")
                .unwrap();
        let continuation_request = read_json_request(&mut continuation_stream).await;
        let running_receipt = continuation_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "tool")
            .and_then(|message| message["content"].as_str())
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .expect("approval continuation must contain the run_command Running receipt");
        assert_eq!(running_receipt["status"], "running");
        let session_id = running_receipt["sessionId"]
            .as_str()
            .expect("Running receipt contains a Session identity")
            .to_string();
        assert_eq!(running_receipt["continueWith"]["tool"], "command_session");
        assert_eq!(
            running_receipt["continueWith"]["args"],
            json!({
                "sessionId": session_id,
                "action": "wait"
            })
        );
        write_text_stream(
            &mut continuation_stream,
            "The approved command was handed off and is still running.",
        )
        .await;
        drop(continuation_stream);

        (session_id, running_receipt)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join(format!("{}.sqlite", case.label));
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    let project_id = format!("project-approved-running-{}", case.label);
    let conversation_id = format!("conversation-approved-running-{}", case.label);
    let assistant_message_id = format!("assistant-approved-running-{}", case.label);
    storage
        .save_project(ProjectRecord {
            id: project_id.clone(),
            name: "Approved Running handoff".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        case.initial_yield,
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some(conversation_id.clone()),
                project_id: Some(project_id),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Run the command after I approve it.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some(format!("user-approved-running-{}", case.label)),
                assistant_message_id: Some(assistant_message_id.clone()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications.clone(),
        )
        .unwrap();

    let mut saw_approval = false;
    let mut saw_waiting_done = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_approval && saw_waiting_done) {
            let notification = receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "{} failed before approval: {notification}",
                case.label
            );
            match notification["params"]["type"].as_str() {
                Some("approval_required") => saw_approval = true,
                Some("done") => saw_waiting_done = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let audited_call_id = pending[0]
        .tool_call_id
        .clone()
        .expect("command approval keeps its Host ToolCall identity");
    let action_id = pending[0].action_id.clone();
    let decision = service
        .approve_action(&turn.run_id, &action_id, notifications)
        .unwrap();
    assert_eq!(decision.agent_output.status, AgentRunStatus::Running);

    let mut saw_completed_done = false;
    let mut saw_command_exit = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_completed_done && saw_command_exit) {
            let notification = receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "{} emitted an error after approval: {notification}",
                case.label
            );
            match notification["params"]["type"].as_str() {
                Some("done") if notification["params"]["status"] == "completed" => {
                    saw_completed_done = true;
                }
                Some("command_exited") => {
                    assert_eq!(notification["params"]["exitCode"], 0);
                    saw_command_exit = true;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let (session_id, running_receipt) = server.await.unwrap();
    let audit = storage
        .get_agent_action_audit(&pending_action_storage_id(&turn.run_id, &action_id))
        .unwrap()
        .expect("approved Running handoff must have a durable audit record");
    assert_eq!(audit.status, "completed");
    assert_eq!(audit.decision.as_deref(), Some("approved"));
    assert!(audit.command_result_json.is_none());
    let audited_tool_result = serde_json::from_str::<AgentToolResult>(
        audit
            .tool_result_json
            .as_deref()
            .expect("Running handoff audit contains its exact ToolResult"),
    )
    .unwrap();
    assert_eq!(audited_tool_result.tool, "run_command");
    assert_eq!(audited_tool_result.call_id, audited_call_id);
    let audited_receipt = audited_tool_result
        .result
        .as_ref()
        .and_then(Value::as_object)
        .expect("Running handoff audit contains the strict object projection");
    assert_eq!(audited_receipt.len(), 7);
    assert_eq!(audited_receipt["status"], "running");
    assert_eq!(audited_receipt["sessionId"], session_id);
    assert_eq!(audited_receipt["output"], "");
    assert_eq!(
        audited_receipt["continueWith"],
        running_receipt["continueWith"]
    );
    for key in [
        "status",
        "sessionId",
        "startedAt",
        "latestSequence",
        "outputTruncated",
    ] {
        assert_eq!(audited_receipt[key], running_receipt[key]);
    }

    let durable_pending: (String, Option<String>) = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status, target_status FROM agent_pending_actions WHERE action_id = ?1",
            [pending_action_storage_id(&turn.run_id, &action_id)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(durable_pending.0, "completed");
    assert_eq!(durable_pending.1.as_deref(), Some("completed"));
    assert!(service.list_pending_actions().is_empty());

    let session = storage
        .load_agent_command_session(&conversation_id, &session_id)
        .unwrap()
        .expect("handed-off command Session remains durably inspectable");
    assert_eq!(session.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(session.snapshot.exit_code, Some(0));
}

#[tokio::test]
async fn approved_running_receipt_is_strictly_audited_before_model_continuation() {
    assert_approved_running_command_handoff(ApprovedRunningHandoffCase {
        label: "well-past-yield",
        initial_yield: Duration::from_millis(20),
        command: "sleep 0.50",
    })
    .await;
}

#[tokio::test]
async fn guidance_releases_a_running_command_wait_and_background_exit_never_wakes_model() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_run_command_stream(&mut first).await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        let running_receipt = second_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "tool")
            .and_then(|message| message["content"].as_str())
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .expect("the second request contains the run_command handoff receipt");
        assert_eq!(running_receipt["status"], "running");
        let session_id = running_receipt["sessionId"].as_str().unwrap().to_string();
        write_tool_call_stream(
            &mut second,
            "provider-managed-command-wait",
            "command_session",
            json!({ "sessionId": session_id }),
            "Waiting quietly for the command while remaining responsive to guidance.",
        )
        .await;
        drop(second);

        let (mut third, _) = listener.accept().await.unwrap();
        let third_request = read_json_request(&mut third).await;
        write_text_stream(
            &mut third,
            "I applied the newer constraint while the command continued.",
        )
        .await;
        drop(third);

        let fourth_request_seen =
            tokio::time::timeout(Duration::from_millis(1_200), listener.accept())
                .await
                .is_ok();
        (session_id, third_request, fourth_request_seen)
    });

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("storage.sqlite")).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-managed-loop".to_string(),
            name: "Managed loop".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(200),
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let permissions = AgentPermissions {
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        ..AgentPermissions::default()
    };
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-managed-loop".to_string()),
                project_id: Some("project-managed-loop".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Start a long command and then continue.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-managed-loop".to_string()),
                assistant_message_id: Some("assistant-managed-loop".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions,
            },
            notifications.clone(),
        )
        .unwrap();

    let session_id = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            if notification["params"]["type"] == "command_started" {
                break notification["params"]["sessionId"]
                    .as_str()
                    .unwrap()
                    .to_string();
            }
        }
    })
    .await
    .unwrap();

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = receiver.recv().await.unwrap();
            if notification["params"]["type"] == "tool_call"
                && notification["params"]["call"]["tool"] == "command_session"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    // The ToolCall event is emitted immediately before execution. This delay makes the guidance
    // arrive while the Host is inside its quiet wait, rather than at the preceding model boundary.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let guidance = service
        .steer_run(
            AgentSteerRunInput {
                conversation_id: turn.conversation_id.clone(),
                expected_run_id: turn.run_id.clone(),
                client_message_id: "client-managed-command-guidance".to_string(),
                content: "Please use the newer constraint.".to_string(),
                attachments: Vec::new(),
            },
            notifications,
        )
        .unwrap();
    assert_eq!(guidance.status, AgentSteerRunResultStatus::Queued);

    let mut saw_done = false;
    let mut saw_exit = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_done && saw_exit) {
            let notification = receiver.recv().await.unwrap();
            match notification["params"]["type"].as_str() {
                Some("done") => {
                    assert_eq!(notification["params"]["status"], "completed");
                    saw_done = true;
                }
                Some("command_exited") => {
                    assert_eq!(notification["params"]["sessionId"], session_id);
                    assert_eq!(notification["params"]["exitCode"], 0);
                    saw_exit = true;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let (server_session_id, third_request, fourth_request_seen) = server.await.unwrap();
    assert_eq!(server_session_id, session_id);
    assert!(
        !fourth_request_seen,
        "background command exit must not create an automatic model request"
    );
    let messages = third_request["messages"].as_array().unwrap();
    let tool_message = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .expect("guidance must release command_session back through the ordinary Agent loop");
    let receipt: Value = serde_json::from_str(tool_message["content"].as_str().unwrap()).unwrap();
    assert_eq!(receipt["status"], "running");
    assert_eq!(receipt["sessionId"], session_id);
    assert!(messages.iter().any(|message| {
        message["role"] == "user" && message["content"] == "Please use the newer constraint."
    }));

    let record = storage
        .load_agent_command_session(&turn.conversation_id, &session_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(0));
    let conversation = storage
        .load_conversation(&turn.conversation_id)
        .unwrap()
        .unwrap();
    assert_eq!(conversation.messages.len(), 2);
}

#[tokio::test]
async fn model_poll_observes_nonzero_terminal_result_without_background_continuation() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut first, _) = listener.accept().await.unwrap();
        let _first_request = read_json_request(&mut first).await;
        write_tool_call_stream(
            &mut first,
            "provider-build-command",
            "run_command",
            json!({
                "command": BACKGROUND_COMMAND_WITH_MIXED_LINE_ENDINGS,
                "reason": "exercise explicit terminal polling"
            }),
            "Starting the build.",
        )
        .await;
        drop(first);

        let (mut second, _) = listener.accept().await.unwrap();
        let second_request = read_json_request(&mut second).await;
        let running_receipt = second_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["role"] == "tool")
            .and_then(|message| message["content"].as_str())
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .expect("the second model request must contain the run_command running receipt");
        assert_eq!(running_receipt["status"], "running");
        let session_id = running_receipt["sessionId"].as_str().unwrap().to_string();
        write_tool_call_stream(
            &mut second,
            "provider-command-poll",
            "command_session",
            json!({
                "sessionId": session_id
            }),
            "Waiting for the build's terminal result.",
        )
        .await;
        drop(second);

        let (mut third, _) = listener.accept().await.unwrap();
        let third_request = read_json_request(&mut third).await;
        let poll_result = third_request["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|message| message["role"] == "tool")
            .and_then(|message| message["content"].as_str())
            .and_then(|content| serde_json::from_str::<Value>(content).ok())
            .expect("the third model request must contain the terminal command_session result");
        assert_eq!(poll_result["status"], "exited");
        assert_eq!(poll_result["exitCode"], 7);
        assert!(poll_result["output"].as_str().unwrap().contains("progress"));
        assert!(poll_result["historyOpen"].as_str().is_some());
        assert_eq!(
            poll_result["continueWith"]["args"]["open"],
            poll_result["historyOpen"]
        );
        write_text_stream(&mut third, "The build exited with status 7.").await;
        drop(third);

        let fourth_request_seen =
            tokio::time::timeout(Duration::from_millis(800), listener.accept())
                .await
                .is_ok();
        (session_id, third_request, fourth_request_seen)
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("managed-poll.sqlite");
    let storage = Arc::new(StorageService::open(&database_path).unwrap());
    storage
        .save_project(ProjectRecord {
            id: "project-managed-poll".to_string(),
            name: "Managed poll".to_string(),
            path: Some(fixture.path().to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let mut service = AgentService::new(Arc::clone(&storage));
    service.command_sessions = AgentCommandSessionRegistry::with_manager(
        Arc::clone(&storage),
        CommandSessionManager::default(),
        Duration::from_millis(100),
    );
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-managed-poll".to_string()),
                project_id: Some("project-managed-poll".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Run the build and verify its final exit code.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-managed-poll".to_string()),
                assistant_message_id: Some("assistant-managed-poll".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::AutoApprove,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications,
        )
        .unwrap();

    let mut exited_events = 0;
    let mut saw_done = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_done && exited_events == 1) {
            let notification = receiver.recv().await.unwrap();
            match notification["params"]["type"].as_str() {
                Some("done") => saw_done = true,
                Some("command_exited") => {
                    assert_eq!(notification["params"]["exitCode"], 7);
                    exited_events += 1;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap();

    let (session_id, third_request, fourth_request_seen) = server.await.unwrap();
    assert!(
        !fourth_request_seen,
        "neither the terminal event nor a finished Agent run may create another model request"
    );
    let poll_result = third_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .expect("the third model request must contain the command_session result");
    assert_eq!(poll_result["sessionId"], session_id);
    assert_eq!(poll_result["status"], "exited");
    assert_eq!(poll_result["exitCode"], 7);
    assert!(poll_result["output"].as_str().unwrap().contains("progress"));

    let record = storage
        .load_agent_command_session(&turn.conversation_id, &session_id)
        .unwrap()
        .unwrap();
    assert_eq!(record.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(record.snapshot.exit_code, Some(7));
    assert_eq!(record.snapshot.command, BACKGROUND_COMMAND_CANONICAL);
    let background_history_open = poll_result["historyOpen"]
        .as_str()
        .expect("terminal background Session returns an exact-history route");
    let background_page = storage
        .read_conversation_history_archive_page_from_open(
            &turn.conversation_id,
            background_history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("background command historyOpen resolves in the current conversation");
    assert!(background_page.content.contains("progress"));
    let trace = storage
        .get_conversation_turn_trace("assistant-managed-poll")
        .unwrap()
        .expect("background command keeps its durable Trace");
    assert!(trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolCall {
            call_id,
            operation,
            ..
        } if call_id == &record.snapshot.call_id
            && operation["command"] == BACKGROUND_COMMAND_CANONICAL
    )));
    assert_eq!(
        storage
            .load_conversation(&turn.conversation_id)
            .unwrap()
            .unwrap()
            .messages
            .len(),
        2,
        "command polling and terminal events stay inside the original assistant turn"
    );

    let background_history_open = background_history_open.to_string();
    drop(service);
    drop(storage);
    let restarted_storage = StorageService::open(&database_path).unwrap();
    let restarted_session = restarted_storage
        .load_agent_command_session(&turn.conversation_id, &session_id)
        .unwrap()
        .expect("terminal background Session survives restart");
    assert_eq!(
        restarted_session.snapshot.command,
        BACKGROUND_COMMAND_CANONICAL
    );
    let restarted_page = restarted_storage
        .read_conversation_history_archive_page_from_open(
            &turn.conversation_id,
            &background_history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("background historyOpen is deterministic after restart");
    assert!(restarted_page.content.contains("progress"));
}

#[tokio::test]
async fn approved_command_reuses_one_session_archive_across_restart_and_runtime_resume() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (continuation_tx, continuation_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut approval_stream, _) = listener.accept().await.unwrap();
        let _approval_request = read_json_request(&mut approval_stream).await;
        write_tool_call_stream(
            &mut approval_stream,
            "provider-approved-command-archive",
            "run_command",
            json!({
                "command": APPROVED_COMMAND_WITH_MIXED_LINE_ENDINGS,
                "reason": "exercise approval resume archive identity"
            }),
            "Preparing the command for approval.",
        )
        .await;
        drop(approval_stream);

        let (mut continuation_stream, _) = listener.accept().await.unwrap();
        let continuation_request = read_json_request(&mut continuation_stream).await;
        continuation_tx.send(continuation_request).unwrap();
        release_rx.await.unwrap();
        write_text_stream(
            &mut continuation_stream,
            "The approved command completed successfully.",
        )
        .await;
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("approved-command-archive.sqlite");
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let model_credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, model_credentials.clone())
            .unwrap(),
    );
    storage
        .save_project(ProjectRecord {
            id: "project-approved-command-archive".to_string(),
            name: "Approved command archive".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let service = AgentService::new(Arc::clone(&storage));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-approved-command-archive".to_string()),
                project_id: Some("project-approved-command-archive".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Run the command after I approve it.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-approved-command-archive".to_string()),
                assistant_message_id: Some("assistant-approved-command-archive".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications,
        )
        .unwrap();

    let mut saw_approval = false;
    let mut saw_waiting_done = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_approval && saw_waiting_done) {
            let notification = receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "run failed before approval: {notification}"
            );
            match notification["params"]["type"].as_str() {
                Some("approval_required") => saw_approval = true,
                Some("done") => saw_waiting_done = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let AgentProposedAction::Command { command } = &pending[0].action else {
        panic!("approval must freeze a command action");
    };
    assert_eq!(command.command, APPROVED_COMMAND_CANONICAL);
    let action_id = pending[0].action_id.clone();
    let call_id = pending[0]
        .tool_call_id
        .clone()
        .expect("command approval keeps its ToolCall identity");

    let persisted_pending = storage.list_pending_agent_actions().unwrap();
    assert_eq!(persisted_pending.len(), 1);
    assert!(matches!(
        storage
            .store_pending_agent_action(persisted_pending[0].clone())
            .unwrap(),
        PendingActionStoreOutcome::Idempotent
    ));
    let persisted_action =
        serde_json::from_str::<AgentProposedAction>(&persisted_pending[0].action_json).unwrap();
    let AgentProposedAction::Command { command } = persisted_action else {
        panic!("pending action JSON must retain the command variant");
    };
    assert_eq!(command.command, APPROVED_COMMAND_CANONICAL);
    let persisted_resume =
        PersistedAgentResumeInput::decode(&persisted_pending[0].agent_input_json)
            .unwrap()
            .agent_input;
    let checkpoint = persisted_resume
        .resume_checkpoint
        .expect("approval pending row keeps its resume checkpoint");
    let checkpoint_command = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == call_id)
        .and_then(|call| call.args.get("command"))
        .and_then(Value::as_str)
        .expect("checkpoint keeps the canonical command ToolCall");
    assert_eq!(checkpoint_command, APPROVED_COMMAND_CANONICAL);
    assert!(checkpoint
        .conversation_trace_items
        .iter()
        .any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: item_call_id,
                operation,
                ..
            } if item_call_id == &call_id
                && operation["command"] == APPROVED_COMMAND_CANONICAL
        )));
    assert_eq!(
        checkpoint.conversation_trace_items,
        storage
            .get_conversation_turn_trace("assistant-approved-command-archive")
            .unwrap()
            .expect("waiting approval has a durable in-progress trace")
            .items,
        "the frozen approval checkpoint and durable Trace must share one exact prefix"
    );

    drop(receiver);
    drop(service);
    drop(storage);

    let restarted_storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, model_credentials.clone())
            .unwrap(),
    );
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&restarted_storage))
            .unwrap();
    let restarted_pending = restarted.list_pending_actions();
    assert_eq!(restarted_pending.len(), 1);
    let AgentProposedAction::Command { command } = &restarted_pending[0].action else {
        panic!("restarted pending action must retain the command variant");
    };
    assert_eq!(command.command, APPROVED_COMMAND_CANONICAL);
    {
        let records = restarted
            .pending_actions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let record = records
            .values()
            .find(|record| record.snapshot.action_id == action_id)
            .expect("restarted Host reloads the pending command record");
        let checkpoint_command = record
            .agent_input
            .resume_checkpoint
            .as_ref()
            .expect("restarted pending record keeps its checkpoint")
            .context_items
            .iter()
            .flat_map(|item| item.tool_calls.iter())
            .find(|call| call.id == call_id)
            .and_then(|call| call.args.get("command"))
            .and_then(Value::as_str)
            .expect("restarted checkpoint keeps the canonical command");
        assert_eq!(checkpoint_command, APPROVED_COMMAND_CANONICAL);
    }
    let (restart_notifications, mut restart_receiver) = tokio::sync::mpsc::unbounded_channel();
    let approval = restarted
        .approve_action(&turn.run_id, &action_id, restart_notifications)
        .unwrap();
    assert_eq!(approval.agent_output.status, AgentRunStatus::Running);

    let continuation_request = tokio::time::timeout(Duration::from_secs(5), continuation_rx)
        .await
        .unwrap()
        .unwrap();
    let model_result = continuation_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rev()
        .find(|message| {
            message["role"] == "tool" && message["tool_call_id"].as_str() == Some(call_id.as_str())
        })
        .and_then(|message| message["content"].as_str())
        .and_then(|content| serde_json::from_str::<Value>(content).ok())
        .expect("approval continuation request contains the terminal run_command result");
    let history_open = model_result["historyOpen"]
        .as_str()
        .expect("terminal run_command result keeps its authoritative history route")
        .to_string();
    assert_eq!(model_result["continueWith"]["args"]["open"], history_open);

    // Runtime publishes its setup snapshot before issuing the continuation model request. Holding
    // the response lets this test inspect that first publication before any new narration exists.
    let setup_snapshot = restarted
        .trace_snapshots
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(&turn.run_id)
        .cloned()
        .expect("runtime approval resume publishes its setup Trace snapshot");
    let committed_trace = restarted_storage
        .get_conversation_turn_trace("assistant-approved-command-archive")
        .unwrap()
        .expect("approval continuation committed Trace");
    assert_eq!(setup_snapshot.items, committed_trace.items);
    assert_eq!(
        setup_snapshot.model_context_items,
        restarted_storage
            .get_conversation_model_context_log("assistant-approved-command-archive")
            .unwrap()
            .expect("approval continuation model log")
            .items
    );

    let session = restarted_storage
        .list_agent_command_sessions(&turn.conversation_id, 10)
        .unwrap()
        .into_iter()
        .find(|record| record.snapshot.call_id == call_id)
        .expect("approved command owns a durable terminal Session");
    assert_eq!(session.snapshot.status, AgentCommandSessionStatus::Exited);
    assert_eq!(session.snapshot.command, APPROVED_COMMAND_CANONICAL);
    assert_eq!(
        session.snapshot.command_digest,
        format!(
            "sha256:{:x}",
            Sha256::digest(APPROVED_COMMAND_CANONICAL.as_bytes())
        )
    );
    let mut replay_snapshot = session.snapshot.clone();
    replay_snapshot.status = AgentCommandSessionStatus::Starting;
    replay_snapshot.ended_at = None;
    replay_snapshot.exit_code = None;
    replay_snapshot.latest_sequence = 0;
    replay_snapshot.output_truncated = false;
    replay_snapshot.outputs.clear();
    replay_snapshot.archive_ref = None;
    assert_eq!(
        restarted_storage
            .create_agent_command_session(&AgentCommandSessionCreate {
                snapshot: replay_snapshot,
                authorization_source: session.authorization_source,
                approval_provenance: session.approval_provenance.clone(),
                permission_provenance: session.permission_provenance.clone(),
                created_at: session.created_at,
            })
            .unwrap(),
        AgentCommandSessionCreateOutcome::Idempotent
    );
    let archive_ref = session
        .snapshot
        .archive_ref
        .clone()
        .expect("terminal command Session owns Archive B");
    let trace_archive_ref = committed_trace
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolResult {
                call_id: item_call_id,
                archive,
                ..
            } if item_call_id == &call_id => archive.archive_ref.clone(),
            _ => None,
        })
        .expect("committed ToolResult carries an Archive ref");
    assert_eq!(trace_archive_ref, archive_ref);
    assert!(committed_trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolCall {
            call_id: item_call_id,
            operation,
            ..
        } if item_call_id == &call_id
            && operation["command"] == APPROVED_COMMAND_CANONICAL
    )));
    let routed_page = restarted_storage
        .read_conversation_history_archive_page_from_open(
            &turn.conversation_id,
            &history_open,
            u64::MAX,
        )
        .unwrap()
        .expect("model historyOpen resolves inside the current conversation");
    assert_eq!(routed_page.descriptor.archive_ref, archive_ref);
    assert!(routed_page
        .content
        .contains("single-authoritative-command-archive"));
    let archive_count: u64 = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            [&turn.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count, 1, "approval must not create Archive A");
    let audited_action_json: String = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT action_json FROM agent_action_audit
             WHERE run_id = ?1 AND tool_name = 'run_command'",
            [&turn.run_id],
            |row| row.get(0),
        )
        .unwrap();
    let audited_action = serde_json::from_str::<AgentProposedAction>(&audited_action_json).unwrap();
    let AgentProposedAction::Command { command } = audited_action else {
        panic!("command audit must retain its frozen action");
    };
    assert_eq!(command.command, APPROVED_COMMAND_CANONICAL);

    release_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = restart_receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "approval continuation failed: {notification}"
            );
            if notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();

    drop(restarted);
    drop(restarted_storage);
    let after_second_restart =
        StorageService::open_with_model_credentials(&database_path, model_credentials).unwrap();
    let restarted_trace = after_second_restart
        .get_conversation_turn_trace("assistant-approved-command-archive")
        .unwrap()
        .expect("terminal Trace survives the second restart");
    assert!(restarted_trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolResult { call_id: item_call_id, archive, .. }
            if item_call_id == &call_id
                && archive.archive_ref.as_deref() == Some(archive_ref.as_str())
    )));
    assert!(restarted_trace.items.iter().any(|item| matches!(
        item,
        ConversationTurnTraceItem::ToolCall {
            call_id: item_call_id,
            operation,
            ..
        } if item_call_id == &call_id
            && operation["command"] == APPROVED_COMMAND_CANONICAL
    )));
    let restarted_model_log = after_second_restart
        .get_conversation_model_context_log("assistant-approved-command-archive")
        .unwrap()
        .expect("model projection survives the second restart");
    let restarted_model_result = restarted_model_log
        .items
        .iter()
        .find(|item| item.tool_call_id.as_deref() == Some(call_id.as_str()))
        .and_then(|item| serde_json::from_str::<Value>(&item.content).ok())
        .expect("restarted model log keeps the run_command result");
    assert_eq!(restarted_model_result["historyOpen"], history_open);
    let archive_count_after_restart: u64 = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM conversation_history_blobs WHERE conversation_id = ?1",
            [&turn.conversation_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(archive_count_after_restart, 1);
}

#[tokio::test]
async fn rejected_command_after_restart_resumes_the_model_without_starting_a_session() {
    for (message, expected_message) in [
        (None, None),
        (Some(""), None),
        (Some("\t\n"), None),
        (
            Some("  Do not execute this command.  "),
            Some("  Do not execute this command.  "),
        ),
    ] {
        assert_rejected_command_after_restart(message, expected_message).await;
    }
}

async fn assert_rejected_command_after_restart(
    message: Option<&str>,
    expected_message: Option<&str>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (continuation_tx, continuation_rx) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut approval_stream, _) = listener.accept().await.unwrap();
        let _approval_request = read_json_request(&mut approval_stream).await;
        write_tool_call_stream(
            &mut approval_stream,
            "provider-rejected-command-restart",
            "run_command",
            json!({
                "command": "printf should-not-run > rejected-command-marker.txt",
                "reason": "prove that rejection remains pre-dispatch across restart"
            }),
            "Preparing the command for approval.",
        )
        .await;
        drop(approval_stream);

        let (mut continuation_stream, _) = listener.accept().await.unwrap();
        let continuation_request = read_json_request(&mut continuation_stream).await;
        continuation_tx.send(continuation_request).unwrap();
        write_text_stream(
            &mut continuation_stream,
            "The command was rejected and was not executed.",
        )
        .await;
    });

    let fixture = tempdir().unwrap();
    let database_path = fixture.path().join("rejected-command-restart.sqlite");
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let model_credentials =
        Arc::new(mycopilot_core::image_generation::InMemoryCredentialStore::default());
    let storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, model_credentials.clone())
            .unwrap(),
    );
    storage
        .save_project(ProjectRecord {
            id: "project-rejected-command-restart".to_string(),
            name: "Rejected command restart".to_string(),
            path: Some(workspace.to_string_lossy().into_owned()),
            created_at: 1,
            pinned_at: None,
        })
        .unwrap();
    let mut settings = test_model_settings();
    settings.api_url = format!("http://{address}/v1/chat/completions");
    storage.save_model_settings(settings).unwrap();

    let service = AgentService::new(Arc::clone(&storage));
    let (notifications, mut receiver) = tokio::sync::mpsc::unbounded_channel();
    let turn = service
        .start_conversation_turn(
            AgentConversationTurnInput {
                conversation_id: Some("conversation-rejected-command-restart".to_string()),
                project_id: Some("project-rejected-command-restart".to_string()),
                model_id: "model-1".to_string(),
                context_window_indicator_enabled: true,
                content: "Prepare the command, but wait for my decision.".to_string(),
                attachments: Vec::new(),
                skills: Vec::new(),
                title: None,
                user_message_id: Some("user-rejected-command-restart".to_string()),
                assistant_message_id: Some("assistant-rejected-command-restart".to_string()),
                max_tokens: None,
                temperature: None,
                prompt_preferences: None,
                permissions: AgentPermissions {
                    write: AgentWritePermission::WorkspaceOnly,
                    command: AgentCommandPermission::RequireApproval,
                    command_safety: AgentCommandSafetyPolicy::FullAccess,
                    ..AgentPermissions::default()
                },
            },
            notifications,
        )
        .unwrap();

    let mut saw_approval = false;
    let mut saw_waiting_done = false;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !(saw_approval && saw_waiting_done) {
            let notification = receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "run failed before approval: {notification}"
            );
            match notification["params"]["type"].as_str() {
                Some("approval_required") => saw_approval = true,
                Some("done") => saw_waiting_done = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    let pending = service.list_pending_actions();
    assert_eq!(pending.len(), 1);
    let action_id = pending[0].action_id.clone();
    let call_id = pending[0]
        .tool_call_id
        .clone()
        .expect("rejected command retains its ToolCall identity");
    let persisted_checkpoint = storage
        .list_pending_agent_actions()
        .unwrap()
        .into_iter()
        .next()
        .and_then(|record| {
            PersistedAgentResumeInput::decode(&record.agent_input_json)
                .ok()
                .and_then(|input| input.agent_input.resume_checkpoint)
        })
        .expect("rejected command pending row keeps its frozen checkpoint");
    assert_eq!(
        persisted_checkpoint.conversation_trace_items,
        storage
            .get_conversation_turn_trace("assistant-rejected-command-restart")
            .unwrap()
            .expect("waiting rejection has a durable in-progress trace")
            .items,
        "the rejected-command checkpoint and durable Trace must share one exact prefix"
    );
    assert!(!workspace.join("rejected-command-marker.txt").exists());

    drop(receiver);
    drop(service);
    drop(storage);

    let restarted_storage = Arc::new(
        StorageService::open_with_model_credentials(&database_path, model_credentials.clone())
            .unwrap(),
    );
    let restarted =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&restarted_storage))
            .unwrap();
    assert_eq!(restarted.list_pending_actions().len(), 1);
    let (restart_notifications, mut restart_receiver) = tokio::sync::mpsc::unbounded_channel();
    let rejection = restarted
        .reject_action(
            &turn.run_id,
            &action_id,
            message.map(ToString::to_string),
            restart_notifications,
        )
        .unwrap();
    assert_eq!(rejection.agent_output.status, AgentRunStatus::Running);
    let expected_decision_result = json!({
        "status": "rejected",
        "message": expected_message,
    });
    let rejection_result = rejection
        .tool_result
        .as_ref()
        .expect("command rejection returns its exact Host ToolResult");
    assert_eq!(rejection_result.call_id, call_id);
    assert_eq!(rejection_result.tool, "run_command");
    assert!(rejection_result.ok);
    assert_eq!(
        rejection_result.result.as_ref(),
        Some(&expected_decision_result),
        "Host rejection reason must normalize only blank input: {message:?}"
    );

    let continuation_request = tokio::time::timeout(Duration::from_secs(5), continuation_rx)
        .await
        .unwrap()
        .unwrap();
    let rejected_tool_result = continuation_request["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|message| {
            message["role"] == "tool" && message["tool_call_id"].as_str() == Some(call_id.as_str())
        })
        .and_then(|message| message["content"].as_str())
        .expect("rejection continuation contains the paired ToolResult");
    let rejected_tool_result = serde_json::from_str::<Value>(rejected_tool_result).unwrap();
    // The command model projection deliberately retains status but not message. Verify the
    // exact Host and audit payloads separately so model compaction cannot hide blank reasons.
    assert_eq!(
        rejected_tool_result,
        json!({ "status": "rejected" }),
        "Provider must receive the existing rejected-command projection: {message:?}"
    );

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let notification = restart_receiver.recv().await.unwrap();
            assert_ne!(
                notification["params"]["type"], "error",
                "rejection continuation failed: {notification}"
            );
            if notification["params"]["type"] == "done"
                && notification["params"]["status"] == "completed"
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    server.await.unwrap();

    let audit = restarted_storage
        .get_agent_action_audit(&pending_action_storage_id(&turn.run_id, &action_id))
        .unwrap()
        .expect("command rejection keeps a durable audit record after restart");
    assert_eq!(audit.status, "rejected");
    assert_eq!(audit.decision.as_deref(), Some("rejected"));
    let audited_tool_result = serde_json::from_str::<AgentToolResult>(
        audit
            .tool_result_json
            .as_deref()
            .expect("command rejection audit contains its exact ToolResult"),
    )
    .unwrap();
    assert_eq!(audited_tool_result.call_id, call_id);
    assert_eq!(audited_tool_result.tool, "run_command");
    assert!(audited_tool_result.ok);
    assert_eq!(
        audited_tool_result.result.as_ref(),
        Some(&expected_decision_result),
        "durable rejection reason must normalize only blank input: {message:?}"
    );

    assert!(restarted.list_pending_actions().is_empty());
    assert!(restarted_storage
        .list_agent_command_sessions(&turn.conversation_id, 10)
        .unwrap()
        .is_empty());
    assert!(!workspace.join("rejected-command-marker.txt").exists());
    let pending_status: String = Connection::open(&database_path)
        .unwrap()
        .query_row(
            "SELECT status FROM agent_pending_actions WHERE action_id = ?1",
            [pending_action_storage_id(&turn.run_id, &action_id)],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(pending_status, "rejected");

    drop(restarted);
    drop(restarted_storage);
    let after_second_restart = Arc::new(
        StorageService::open_with_model_credentials(&database_path, model_credentials).unwrap(),
    );
    let reconciled =
        AgentService::try_new_deferred_startup_reconciliation(Arc::clone(&after_second_restart))
            .unwrap();
    assert!(reconciled.list_pending_actions().is_empty());
    assert!(after_second_restart
        .list_agent_command_sessions(&turn.conversation_id, 10)
        .unwrap()
        .is_empty());
    assert!(!workspace.join("rejected-command-marker.txt").exists());
}
