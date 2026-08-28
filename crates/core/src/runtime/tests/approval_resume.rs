use super::*;

fn missing_file_observation_id(request: &Value, expected_path: &str) -> String {
    fn find(value: &Value, expected_path: &str) -> Option<String> {
        match value {
            Value::Object(object) => {
                if object.get("path").and_then(Value::as_str) == Some(expected_path)
                    && object.get("exists").and_then(Value::as_bool) == Some(false)
                {
                    return object
                        .get("observationId")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                }
                object.values().find_map(|value| find(value, expected_path))
            }
            Value::Array(values) => values.iter().find_map(|value| find(value, expected_path)),
            Value::String(text) if text.starts_with('{') || text.starts_with('[') => {
                serde_json::from_str::<Value>(text)
                    .ok()
                    .and_then(|value| find(&value, expected_path))
            }
            _ => None,
        }
    }

    find(request, expected_path).unwrap_or_else(|| {
        panic!(
            "Provider request must contain the missing read_file observation for {expected_path}"
        )
    })
}

#[tokio::test]
async fn approval_resume_restores_prior_context_and_continues_queued_tools() {
    use crate::protocol::{
        AgentApprovalDecision, AgentApprovalDecisionStatus, AgentCommandPermission,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentToolContinuation,
        AgentWritePermission,
    };
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn native_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::write(workspace.join("source.txt"), "evidence-before-approval").unwrap();
    std::fs::write(workspace.join("queued.txt"), "evidence-from-queued-tool").unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let final_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let server_final_request = final_request.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will read the source first.",
                            "tool_calls": [
                                native_tool_call(
                                    "todo-before",
                                    "todo_update",
                                    json!({
                                        "items": [{
                                            "title": "Collect evidence and write report",
                                            "status": "in_progress"
                                        }]
                                    })
                                ),
                                native_tool_call(
                                    "read-before",
                                    "read_file",
                                    json!({ "path": "source.txt" })
                                ),
                                native_tool_call(
                                    "read-report-before",
                                    "read_file",
                                    json!({ "path": "report.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => {
                    let observation_id = missing_file_observation_id(&request, "report.txt");
                    json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "I have the evidence and will prepare the report.",
                                "tool_calls": [
                                    native_tool_call(
                                        "patch-approval",
                                        "apply_patch",
                                        json!({
                                            "action": "apply",
                                            "operation": "create",
                                            "filePath": "report.txt",
                                            "observationId": observation_id,
                                            "content": "draft report"
                                        })
                                    ),
                                    native_tool_call(
                                        "read-queued",
                                        "read_file",
                                        json!({ "path": "queued.txt" })
                                    )
                                ]
                            },
                            "finish_reason": "tool_calls"
                        }]
                    })
                }
                _ => {
                    *server_final_request.lock().unwrap() = Some(request);
                    json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "resumed with evidence" },
                            "finish_reason": "stop"
                        }]
                    })
                }
            };
            write_response(&mut stream, response).await;
        }
    });

    let mut base_input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-checkpoint".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-checkpoint".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: Some(activated_skill("SKILL_SNAPSHOT_BEFORE_APPROVAL")),
        skill_discovery: None,
        messages: vec![message("user", "collect evidence and write report.txt")],
    };
    freeze_runtime_test_generic_provider(&mut base_input, "approval-resume");
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base_input.clone(),
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    assert!(waiting.conversation_turn_trace.is_none());
    assert!(
        !workspace.join("report.txt").exists(),
        "an apply_patch proposal must not change the workspace before approval"
    );
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
    let todo_call_id = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "todo_update" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("todo call event");
    let read_before_call_id = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "read_file" && call.args["path"] == "source.txt" =>
            {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("first read call event");
    let read_target_call_id = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "read_file" && call.args["path"] == "report.txt" =>
            {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("target observation read call event");
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let pending_action_id = checkpoint
        .pending_action_id
        .clone()
        .expect("FileChange checkpoint must freeze its canonical pending action id");
    let pending_checkpoint_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_call_id)
        .cloned()
        .expect("checkpoint must freeze the pending call");
    let queued_call_id = checkpoint.queued_tool_calls[0].call.id.clone();
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &read_target_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert_runtime_owned_tool_call_id(call_id);
    }
    assert_eq!(
        [
            &todo_call_id,
            &read_before_call_id,
            &read_target_call_id,
            &pending_call_id,
            &queued_call_id,
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .len(),
        5
    );
    for completed_call_id in [&todo_call_id, &read_before_call_id, &read_target_call_id] {
        assert!(waiting.events.iter().any(|event| matches!(
            event,
            AgentEvent::ToolResult { result, .. }
                if result.call_id == completed_call_id.as_str()
        )));
    }
    assert!(checkpoint
        .extension_snapshots
        .iter()
        .any(|snapshot| snapshot.extension_id == "todo"));
    assert!(checkpoint.context_items.iter().any(|item| {
        item.sources == vec!["skill_instructions"]
            && item.content.contains("SKILL_SNAPSHOT_BEFORE_APPROVAL")
            && item.origin.as_ref().is_some_and(|origin| {
                origin.kind == "skill" && origin.id == "workspace:workspace-1:review"
            })
    }));
    assert!(!format!("{checkpoint:?}").contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(!format!("{:?}", waiting.events).contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(checkpoint
        .conversation_trace_items
        .iter()
        .any(|item| matches!(
            item,
            ConversationTurnTraceItem::AssistantNarration { content, .. }
                if content == "I have the evidence and will prepare the report."
        )));
    assert!(
        !checkpoint.conversation_model_context_items.is_empty(),
        "the approval checkpoint must preserve its exact active-run model projection"
    );

    let mut resume_input = base_input;
    resume_input.messages.clear();
    // The checkpoint is authoritative for the logical run; a changed resume payload must not
    // replace the frozen Skill snapshot selected before approval.
    resume_input.skill_activation = Some(activated_skill("SKILL_CHANGED_DURING_RESUME"));
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_action_id,
        status: AgentApprovalDecisionStatus::Rejected,
        message: Some("Keep the evidence but revise the report first.".to_string()),
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: pending_checkpoint_call.name,
            args: pending_checkpoint_call.args,
            approval_status: AgentApprovalStatus::Rejected,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id.clone(),
            tool: "apply_patch".to_string(),
            ok: false,
            result: None,
            error: Some(
                "user rejected: Keep the evidence but revise the report first.".to_string(),
            ),
        },
    });
    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "resumed with evidence");
    assert!(
        !workspace.join("report.txt").exists(),
        "a rejected apply_patch proposal must remain definitely unexecuted"
    );
    assert_eq!(completed.todo.as_ref().unwrap().revision, 1);
    let request = final_request.lock().unwrap().take().unwrap();
    let messages = serde_json::to_string(&request["messages"]).unwrap();
    assert!(messages.contains("evidence-before-approval"));
    assert!(messages.contains("evidence-from-queued-tool"));
    assert!(messages.contains("user rejected"));
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &read_target_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert!(messages.contains(call_id.as_str()));
    }
    for provider_call_id in [
        "todo-before",
        "read-before",
        "read-report-before",
        "patch-approval",
        "read-queued",
    ] {
        assert!(!messages.contains(provider_call_id));
    }
    assert!(messages.contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(!messages.contains("SKILL_CHANGED_DURING_RESUME"));
    let trace = completed.conversation_turn_trace.as_ref().unwrap();
    trace.validate().unwrap();
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &read_target_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolCall { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool call {call_id} should be retained exactly once"
        );
        assert_eq!(
            trace
                .items
                .iter()
                .filter(|item| matches!(
                    item,
                    ConversationTurnTraceItem::ToolResult { call_id: item_id, .. }
                        if item_id == call_id
                ))
                .count(),
            1,
            "tool result {call_id} should be retained exactly once"
        );
    }
}

#[tokio::test]
async fn skill_resource_text_survives_approval_checkpoint_but_is_omitted_from_durable_history() {
    use crate::protocol::{
        AgentActivatedSkillResources, AgentApprovalDecision, AgentApprovalDecisionStatus,
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentToolContinuation, AgentWritePermission,
    };
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillResourcePath, SkillRevision, SkillSourceId,
    };
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const RESOURCE_MARKER: &str = "SKILL_RESOURCE_CHECKPOINT_SECRET_MARKER";

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut body_start = None;
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if body_start.is_none() {
                if let Some(header_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_len = Some(start + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(&request[body_start.unwrap()..expected_len.unwrap()]).unwrap()
    }

    async fn write_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn native_tool_call(id: &str, name: &str, args: Value) -> Value {
        json!({
            "id": id,
            "type": "function",
            "function": {
                "name": name,
                "arguments": serde_json::to_string(&args).unwrap()
            }
        })
    }

    let source_id = SkillSourceId::parse("workspace:workspace-1").unwrap();
    let skill_id = SkillId::parse("workspace:workspace-1:resource-checkpoint").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64))).unwrap();
    let resource_path = SkillResourcePath::parse("references/guide.md").unwrap();
    let bytes = RESOURCE_MARKER.as_bytes().to_vec();
    let session = memory_resource_session_for_test(
        skill_id.clone(),
        revision.clone(),
        source_id,
        vec![(
            resource_path.as_str().to_string(),
            SkillResourceKind::Reference,
            bytes,
        )],
    )
    .unwrap();
    let resources = Arc::new(session);
    let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
    let resource_uri = package.resource(resource_path);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_second_request = Arc::clone(&second_request);
    let resumed_request = Arc::new(std::sync::Mutex::new(None::<Value>));
    let captured_resumed_request = Arc::clone(&resumed_request);
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [
                                native_tool_call(
                                    "read-skill-resource",
                                    "skills_read_resource",
                                    json!({ "uri": resource_uri.as_str() })
                                ),
                                native_tool_call(
                                    "read-report-before-materialize",
                                    "read_file",
                                    json!({ "path": "report.txt" })
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => {
                    let observation_id = missing_file_observation_id(&request, "report.txt");
                    *captured_second_request.lock().unwrap() = Some(request);
                    json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [native_tool_call(
                                "materialize-after-read",
                                "apply_patch",
                                json!({
                                    "action": "apply",
                                    "operation": "create",
                                    "filePath": "report.txt",
                                    "observationId": observation_id,
                                    "content": "report"
                                })
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                    })
                }
                _ => {
                    *captured_resumed_request.lock().unwrap() = Some(request);
                    json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "continued after approval"
                            },
                            "finish_reason": "stop"
                        }]
                    })
                }
            };
            write_response(&mut stream, response).await;
        }
    });

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let mut input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        provider_configuration_revision: None,
        provider_connection_revision: None,
        search_connection_revision: None,
        provider_profile_config: None,
        provider_protocol_key: None,
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            collaboration_identity: None,
            conversation_id: Some("conversation-skill-resource-checkpoint".to_string()),
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some(workspace.to_string_lossy().into_owned()),
            }),
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: Default::default(),
                patch: AgentPatchPermission::RequireApproval,
                builtin_execution: Default::default(),
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-skill-resource-checkpoint".to_string()),
        context_compaction_summary: None,
        world_state_records: Vec::new(),
        skill_activation: Some(AgentSkillActivation {
            activation_revision: "activation-sha256-v1:resource-checkpoint".to_string(),
            skills: vec![AgentActivatedSkill {
                id: skill_id.as_str().to_string(),
                name: "resource-checkpoint".to_string(),
                revision: revision.as_str().to_string(),
                source: "workspace:workspace-1".to_string(),
                instructions: "Read references progressively.".to_string(),
                source_bytes: 30,
                resources: Some(AgentActivatedSkillResources {
                    root_uri: package.to_string(),
                    resource_count: 1,
                    kinds: vec!["reference".to_string()],
                }),
            }],
        }),
        skill_discovery: None,
        messages: vec![message(
            "user",
            "Read the Skill reference, then prepare a report.",
        )],
    };
    freeze_runtime_test_generic_provider(&mut input, "skill-resource-approval");

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input.clone(),
            Some("run-skill-resource-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(Arc::clone(&resources))),
        )
        .await
        .unwrap();

    assert_eq!(output.status, AgentRunStatus::WaitingForApproval);
    let model_request = second_request.lock().unwrap().clone().unwrap();
    assert!(model_request.to_string().contains(RESOURCE_MARKER));
    let checkpoint = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some(checkpoint.as_ref()),
            _ => None,
        })
        .unwrap();
    let resource_read_call_id = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_read_resource" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("Skill resource read call");
    let target_read_call_id = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. }
                if call.tool == "read_file" && call.args["path"] == "report.txt" =>
            {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("target observation read call");
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let pending_action_id = checkpoint
        .pending_action_id
        .clone()
        .expect("FileChange checkpoint must freeze its canonical pending action id");
    let pending_checkpoint_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_call_id)
        .cloned()
        .expect("checkpoint must freeze the strict apply_patch call");
    assert_runtime_owned_tool_call_id(&resource_read_call_id);
    assert_runtime_owned_tool_call_id(&target_read_call_id);
    assert_runtime_owned_tool_call_id(&pending_call_id);
    assert_ne!(resource_read_call_id, pending_call_id);
    assert_ne!(target_read_call_id, pending_call_id);
    let model_request_messages = serde_json::to_string(&model_request["messages"]).unwrap();
    assert!(model_request_messages.contains(&resource_read_call_id));
    assert!(model_request_messages.contains(&target_read_call_id));
    assert!(!model_request_messages.contains("read-skill-resource"));
    assert!(!model_request_messages.contains("read-report-before-materialize"));
    let checkpoint_json = serde_json::to_string(checkpoint).unwrap();
    assert!(checkpoint_json.contains(RESOURCE_MARKER));
    assert!(!serde_json::to_string(&output.events)
        .unwrap()
        .contains(RESOURCE_MARKER));
    assert!(
        !checkpoint.conversation_model_context_items.is_empty(),
        "the approval checkpoint must preserve its exact active-run model projection"
    );

    let mut resume_input = input;
    resume_input.messages.clear();
    resume_input.skill_activation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_action_id,
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: pending_checkpoint_call.name,
            args: pending_checkpoint_call.args,
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id.clone(),
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    });
    let resumed_trace_snapshots = Arc::new(Mutex::new(Vec::new()));
    let resumed_trace_snapshots_for_observer = Arc::clone(&resumed_trace_snapshots);
    let resumed_trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        resumed_trace_snapshots_for_observer
            .lock()
            .unwrap()
            .push(snapshot);
        Ok(None)
    });

    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-skill-resource-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_resources(resources)
                    .with_trace_observer(resumed_trace_observer),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "continued after approval");
    let resumed_trace_snapshots = resumed_trace_snapshots.lock().unwrap();
    let setup_snapshot = resumed_trace_snapshots
        .first()
        .expect("approval resume must publish a setup trace snapshot");
    assert!(
        !setup_snapshot.model_context_items.is_empty(),
        "the setup publication must not discard the checkpoint's exact model projection"
    );
    let resumed_request = resumed_request.lock().unwrap().clone().unwrap();
    let resumed_messages = serde_json::to_string(&resumed_request["messages"]).unwrap();
    assert!(resumed_messages.contains(&resource_read_call_id));
    assert!(resumed_messages.contains(&target_read_call_id));
    assert!(resumed_messages.contains(&pending_call_id));
    assert!(!resumed_messages.contains("read-skill-resource"));
    assert!(!resumed_messages.contains("read-report-before-materialize"));
    assert!(!resumed_messages.contains("materialize-after-read"));
    assert!(resumed_messages.contains("applied"));
    assert!(resumed_messages.contains(RESOURCE_MARKER));
    let trace = completed
        .conversation_turn_trace
        .as_ref()
        .expect("terminal conversation trace");
    assert!(!serde_json::to_string(trace)
        .unwrap()
        .contains(RESOURCE_MARKER));
    for call_id in [
        &resource_read_call_id,
        &target_read_call_id,
        &pending_call_id,
    ] {
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: trace_call_id,
                ..
            } if trace_call_id == call_id
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id: trace_call_id,
                ..
            } if trace_call_id == call_id
        )));
    }
}
