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
async fn model_activation_preserves_exposed_siblings_and_discloses_new_tools_next_request() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    const INSTRUCTIONS: &str =
        "DYNAMIC_SKILL_INSTRUCTION_MARKER: verify the document before reporting success.";

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0);
            request.extend_from_slice(&buffer[..read]);
            if expected_length.is_none() {
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
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        let body_start = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .map(|index| index + 4)
            .unwrap();
        serde_json::from_slice(&request[body_start..expected_length.unwrap()]).unwrap()
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let headers = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(headers.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    fn open_ai_tool_names(request: &Value) -> Vec<&str> {
        request["tools"]
            .as_array()
            .expect("OpenAI-compatible request tools")
            .iter()
            .map(|tool| {
                tool["function"]["name"]
                    .as_str()
                    .expect("function tool name")
            })
            .collect()
    }

    let discovery = discoverable_skill("Create and verify Word documents.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [
                                {
                                    "id": "todo-alongside-skill",
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "Verify same-response calls remain executable"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "activate-documents",
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Create and verify the requested document"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "new-tool-before-next-request",
                                    "type": "function",
                                    "function": {
                                        "name": "office_document",
                                        "arguments": serde_json::to_string(&json!({
                                            "operation": "inspect",
                                            "path": "draft.docx"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Skill loaded and applied."
                        },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let entry = discovery.skills[0].clone();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let resolver: AgentSkillActivationResolver = Arc::new(move |selection| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        assert_eq!(selection.skill_id().as_str(), entry.id);
        assert_eq!(selection.expected_revision().as_str(), entry.revision);
        let skill_id = crate::skills::SkillId::parse(entry.id.clone()).unwrap();
        let source_id = skill_id.source_id().clone();
        let resources = crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(entry.revision.clone()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap();
        Ok(AgentResolvedSkillActivation {
            skill: AgentActivatedSkill {
                id: entry.id.clone(),
                name: entry.name.clone(),
                revision: entry.revision.clone(),
                source: "bundled:application".to_string(),
                instructions: INSTRUCTIONS.to_string(),
                source_bytes: u64::try_from(INSTRUCTIONS.len()).unwrap(),
                resources: None,
            },
            resources: Arc::new(resources),
        })
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_for_emitter = Arc::clone(&events);
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        events_for_emitter.lock().unwrap().push(event);
    });
    let context_window_snapshots = Arc::new(Mutex::new(Vec::<AgentContextWindowSnapshot>::new()));
    let context_window_snapshots_for_observer = Arc::clone(&context_window_snapshots);
    let context_window_observer: AgentContextWindowObserver = Arc::new(move |snapshot| {
        context_window_snapshots_for_observer
            .lock()
            .unwrap()
            .push(snapshot);
    });
    let mut input = conversation_context_input(vec![message("user", "Create a Word guide")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.skill_discovery = Some(discovery);
    let office_engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-dynamic-skill".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_activation_resolver(resolver)
                    .with_skill_resources(Arc::new(crate::skills::SkillResourceSession::empty()))
                    .with_office_engine(office_engine)
                    .with_context_window_observer(context_window_observer),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Skill loaded and applied.");
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let context_window_snapshots = context_window_snapshots.lock().unwrap();
    assert_eq!(
        context_window_snapshots.len(),
        2,
        "the observer must receive one exact aggregate snapshot for each sendable request"
    );
    assert!(
        context_window_snapshots[1].input_tokens > context_window_snapshots[0].input_tokens,
        "the post-activation request must account for the paired ToolResult, full Skill instructions, tools.effective World State diff and unlocked Tool schemas"
    );
    drop(context_window_snapshots);
    let activation_call_id = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_activate" => {
                Some(call.id.clone())
            }
            _ => None,
        })
        .expect("canonical skills_activate call");
    assert_runtime_owned_tool_call_id(&activation_call_id);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_serialized = serde_json::to_string(&requests[0]).unwrap();
    assert!(first_serialized.contains("backend_available_skills"));
    assert!(first_serialized.contains("skills_activate"));
    assert!(!first_serialized.contains(INSTRUCTIONS));
    let first_system_prompt = requests[0]["messages"][0]["content"]
        .as_str()
        .expect("backend-owned system prompt");
    assert!(first_system_prompt.contains("Skill 激活、激活后指令和动态工具只在当前 Run 有效"));
    assert!(first_system_prompt.contains("当前 Run 冻结的 available-Skills catalog"));
    assert!(first_system_prompt.contains("每个子 Agent 都有独立 Run"));
    let first_tool_names = open_ai_tool_names(&requests[0]);
    assert!(
        !first_tool_names.contains(&"read_word"),
        "documents tools must remain hidden before Skill activation"
    );
    assert!(
        !first_tool_names.contains(&"office_document"),
        "Office semantic tools must remain hidden before Skill activation"
    );

    let second_tool_names = open_ai_tool_names(&requests[1]);
    assert!(
        second_tool_names.starts_with(&first_tool_names),
        "stable tools must remain an exact prefix after dynamic activation"
    );
    assert!(second_tool_names.contains(&"read_word"));
    assert!(second_tool_names.contains(&"office_document"));
    assert_eq!(
        requests[0]["messages"][0], requests[1]["messages"][0],
        "the backend-owned stable system prompt must not change when a Skill unlocks tools"
    );

    let second_messages = requests[1]["messages"].as_array().unwrap();
    let tool_result_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "tool" && message["tool_call_id"] == activation_call_id
        })
        .unwrap();
    let skill_context_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_str()
                    .is_some_and(|content| content.contains(INSTRUCTIONS))
        })
        .unwrap();
    assert!(tool_result_index < skill_context_index);
    assert!(!second_messages[tool_result_index]["content"]
        .as_str()
        .unwrap()
        .contains(INSTRUCTIONS));
    assert_eq!(
        serde_json::to_string(&requests[1])
            .unwrap()
            .matches(INSTRUCTIONS)
            .count(),
        1
    );
    assert!(
        !serde_json::to_string(&requests[1])
            .unwrap()
            .contains("skillActivationBoundary"),
        "the next request must contain real sibling results, not a synthetic activation barrier"
    );

    let events = events.lock().unwrap();
    let result_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.call_id == activation_call_id))
        .unwrap();
    let activated_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::SkillActivated { skill, .. } if skill.id == "bundled:application:documents"))
        .unwrap();
    assert!(result_index < activated_index);
    let sibling_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" => Some(result),
            _ => None,
        })
        .expect("a tool exposed in the original request must execute alongside Skill activation");
    assert!(sibling_result.ok);
    let newly_unlocked_result = events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "office_document" => {
                Some(result)
            }
            _ => None,
        })
        .expect("a same-response call to a newly unlocked tool must settle deterministically");
    assert!(!newly_unlocked_result.ok);
    assert_eq!(
        newly_unlocked_result
            .result
            .as_ref()
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("agent.tool_requires_skill_activation")
    );
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("skillActivationBoundary"));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("activate-documents"));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains(INSTRUCTIONS));
}

#[derive(Clone, Copy, Debug)]
enum SkillApprovalResumeProviderCase {
    Generic,
    DeepSeekExactGrouped,
}

impl SkillApprovalResumeProviderCase {
    fn label(self) -> &'static str {
        match self {
            Self::Generic => "generic",
            Self::DeepSeekExactGrouped => "deepseek",
        }
    }
}

async fn run_skill_activation_approval_resume_case(case: SkillApprovalResumeProviderCase) {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::protocol::{
        AgentApprovalDecision, AgentApprovalDecisionStatus, AgentCommandPermission,
        AgentPatchPermission, AgentPermissions, AgentReadPermission, AgentToolContinuation,
        AgentWritePermission,
    };
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const INSTRUCTIONS: &str =
        "APPROVAL_RESUME_SKILL_MARKER: Word tools are available on the next request only.";
    const PROVIDER_REASONING: &str =
        "Activate documents, request the existing write approval, then settle all siblings.";

    let label = case.label();
    let run_id = format!("run-skill-approval-{label}");
    let conversation_id = format!("conversation-skill-approval-{label}");
    let assistant_message_id = format!("assistant-skill-approval-{label}");
    let model_id = format!("model-skill-approval-{label}");
    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: conversation_id.clone(),
            project_id: None,
            model_id: Some(model_id.clone()),
            title: format!("Skill Approval {label}"),
            messages: vec![ChatMessageRecord {
                id: assistant_message_id.clone(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );

    let mut provider_profile = match case {
        SkillApprovalResumeProviderCase::Generic => ProviderProfileConfig::generic_for_dialect(
            ProviderProtocolDialect::OpenAiChatCompletions,
        ),
        SkillApprovalResumeProviderCase::DeepSeekExactGrouped => {
            ProviderProfileConfig::deepseek_v4_default()
        }
    };
    if matches!(case, SkillApprovalResumeProviderCase::DeepSeekExactGrouped) {
        provider_profile.reasoning = ReasoningPolicy {
            mode: ReasoningMode::Enabled,
            effort: ReasoningEffort::Max,
        };
    }
    let provider_configuration_revision = format!("provider-protocol-v1:skill-approval-{label}");
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        &model_id,
        Some(provider_configuration_revision.clone()),
    )
    .unwrap();

    let discovery = discoverable_skill("Create and verify Word documents after activation.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let server = tokio::spawn(async move {
        for request_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request.clone());
            let response = match request_index {
                0 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": matches!(
                                case,
                                SkillApprovalResumeProviderCase::DeepSeekExactGrouped
                            ).then_some("Observe the target before proposing the write."),
                            "tool_calls": [{
                                "id": format!("{label}-observe"),
                                "type": "function",
                                "function": {
                                    "name": "read_file",
                                    "arguments": serde_json::to_string(&json!({
                                        "path": "approved.txt"
                                    })).unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => {
                    let observation_id = missing_file_observation_id(&request, "approved.txt");
                    json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": matches!(
                                case,
                                SkillApprovalResumeProviderCase::DeepSeekExactGrouped
                            ).then_some(PROVIDER_REASONING),
                            "tool_calls": [
                                {
                                    "id": format!("{label}-activate"),
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Unlock documents for the next request"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-approval"),
                                    "type": "function",
                                    "function": {
                                        "name": "apply_patch",
                                        "arguments": serde_json::to_string(&json!({
                                            "action": "apply",
                                            "operation": "create",
                                            "filePath": "approved.txt",
                                            "observationId": observation_id,
                                            "content": "approved"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-existing-sibling"),
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "Execute exactly once after resume"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": format!("{label}-guessed-new-tool"),
                                    "type": "function",
                                    "function": {
                                        "name": "read_word",
                                        "arguments": serde_json::to_string(&json!({
                                            "path": "not-created.docx"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                    })
                }
                _ => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Approval resumed under the frozen batch contract.",
                            "reasoning_content": matches!(
                                case,
                                SkillApprovalResumeProviderCase::DeepSeekExactGrouped
                            ).then_some("The next request now includes the activated ToolSet.")
                        },
                        "finish_reason": "stop"
                    }]
                }),
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let entry = discovery.skills[0].clone();
    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let resolver: AgentSkillActivationResolver = Arc::new(move |selection| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        assert_eq!(selection.skill_id().as_str(), entry.id);
        assert_eq!(selection.expected_revision().as_str(), entry.revision);
        let skill_id = crate::skills::SkillId::parse(entry.id.clone()).unwrap();
        let source_id = skill_id.source_id().clone();
        let resources = crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(entry.revision.clone()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap();
        Ok(AgentResolvedSkillActivation {
            skill: AgentActivatedSkill {
                id: entry.id.clone(),
                name: entry.name.clone(),
                revision: entry.revision.clone(),
                source: "bundled:application".to_string(),
                instructions: INSTRUCTIONS.to_string(),
                source_bytes: u64::try_from(INSTRUCTIONS.len()).unwrap(),
                resources: None,
            },
            resources: Arc::new(resources),
        })
    });
    let skill_resources = Arc::new(crate::skills::SkillResourceSession::empty());
    let host_services = AgentRuntimeHostServices::new()
        .with_storage(Arc::clone(&storage))
        .with_skill_activation_resolver(resolver)
        .with_skill_resources(Arc::clone(&skill_resources))
        .with_provider_continuation_vault(Arc::clone(&vault));

    let mut input = conversation_context_input(vec![message(
        "user",
        "Activate documents, request the write, and settle all siblings.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.provider_configuration_revision = Some(provider_configuration_revision);
    input.provider_profile_config = Some(provider_profile);
    input.provider_protocol_key = Some(provider_protocol.clone());
    input.model = model_id;
    input.max_tokens = Some(2_048);
    input.stream = Some(false);
    input.assistant_message_id = Some(assistant_message_id);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(conversation_id.clone()),
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
    });
    input.skill_discovery = Some(discovery);

    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input.clone(),
            Some(run_id.clone()),
            None,
            AgentCancellationToken::new(),
            Some(host_services.clone()),
        )
        .await
        .unwrap();
    assert_eq!(
        waiting.status,
        AgentRunStatus::WaitingForApproval,
        "{case:?}"
    );
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some((**checkpoint).clone()),
            _ => None,
        })
        .expect("approval checkpoint");
    assert!(checkpoint.tool_set.active_capability_ids.is_empty());
    assert!(!checkpoint
        .tool_set
        .exposed_tool_names
        .iter()
        .any(|name| name == "read_word"));
    assert_eq!(checkpoint.queued_tool_calls.len(), 2);
    assert!(serde_json::to_string(&checkpoint.extension_snapshots)
        .unwrap()
        .contains("bundled:application:documents"));
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    let pending_action_id = checkpoint
        .pending_action_id
        .clone()
        .expect("FileChange checkpoint must freeze its canonical pending action id");
    let pending_call = checkpoint
        .context_items
        .iter()
        .flat_map(|item| item.tool_calls.iter())
        .find(|call| call.id == pending_call_id)
        .cloned()
        .expect("pending approval call is frozen");

    let mut resume_input = input;
    resume_input.messages.clear();
    resume_input.skill_discovery = None;
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_action_id,
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
            tool: pending_call.name.clone(),
            args: pending_call.args.clone(),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            exact_archive_file: None,
            call_id: pending_call_id,
            tool: pending_call.name,
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    });
    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some(run_id),
            None,
            AgentCancellationToken::new(),
            Some(host_services),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed, "{case:?}");
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let existing_results = completed
        .events
        .iter()
        .filter(|event| {
            matches!(event, AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" && result.ok)
        })
        .count();
    assert_eq!(existing_results, 1, "{case:?}");
    let guessed_result = completed
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "read_word" => Some(result),
            _ => None,
        })
        .expect("same-response guessed Tool must settle after resume");
    assert!(!guessed_result.ok);
    assert_eq!(
        guessed_result
            .result
            .as_ref()
            .and_then(|result| result.get("errorCode"))
            .and_then(Value::as_str),
        Some("agent.tool_requires_skill_activation")
    );

    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    let request_tool_names = |request: &Value| {
        request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    assert!(!request_tool_names(&requests[0]).contains(&"read_word".to_string()));
    assert!(!request_tool_names(&requests[1]).contains(&"read_word".to_string()));
    assert!(request_tool_names(&requests[2]).contains(&"read_word".to_string()));
    let resumed_request = serde_json::to_string(&requests[2]).unwrap();
    assert!(resumed_request.contains("agent.tool_requires_skill_activation"));
    assert_eq!(resumed_request.matches(INSTRUCTIONS).count(), 1);
    assert!(!resumed_request.contains("skillActivationBoundary"));
    if matches!(case, SkillApprovalResumeProviderCase::DeepSeekExactGrouped) {
        let grouped_turn = requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|message| message["reasoning_content"].as_str() == Some(PROVIDER_REASONING))
            .expect("DeepSeek exact grouped turn must survive Approval resume");
        assert_eq!(grouped_turn["tool_calls"].as_array().unwrap().len(), 4);
        let provider_result_ids = requests[2]["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["role"] == "tool")
            .map(|message| message["tool_call_id"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(
            provider_result_ids,
            [
                "deepseek-observe",
                "deepseek-activate",
                "deepseek-approval",
                "deepseek-existing-sibling",
                "deepseek-guessed-new-tool",
            ]
        );
        assert_eq!(
            vault
                .list_replayable_for_conversation(&conversation_id, &provider_protocol)
                .unwrap()
                .len(),
            2,
            "the target observation and approval batches each retain their exact DeepSeek continuation"
        );
    }
}

#[tokio::test]
async fn skill_activation_before_approval_restores_frozen_batch_for_generic_and_deepseek() {
    for case in [
        SkillApprovalResumeProviderCase::Generic,
        SkillApprovalResumeProviderCase::DeepSeekExactGrouped,
    ] {
        run_skill_activation_approval_resume_case(case).await;
    }
}

#[tokio::test]
async fn deepseek_grouped_activation_failure_settles_exposed_sibling_without_boundary() {
    use crate::image_generation::{CredentialStore, InMemoryCredentialStore};
    use crate::storage::models::{ChatConversationRecord, ChatMessageRecord};
    use crate::storage::service::StorageService;
    use crate::{
        ProviderContinuationVaultFactory, ProviderProfileConfig, ProviderProtocolDialect,
        ProviderProtocolKey, ReasoningEffort, ReasoningMode, ReasoningPolicy,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::tempdir;
    use tokio::net::TcpListener;

    const CONVERSATION_ID: &str = "conversation-deepseek-skill-cocall";
    const ASSISTANT_MESSAGE_ID: &str = "assistant-deepseek-skill-cocall";
    const RUN_ID: &str = "run-deepseek-skill-cocall";
    const MODEL_ID: &str = "deepseek-skill-cocall";
    const REASONING: &str = "Activate the selected Skill and update the existing todo contract.";

    let fixture = tempdir().unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("runtime.sqlite")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: CONVERSATION_ID.to_string(),
            project_id: None,
            model_id: Some(MODEL_ID.to_string()),
            title: "DeepSeek Skill co-call".to_string(),
            messages: vec![ChatMessageRecord {
                id: ASSISTANT_MESSAGE_ID.to_string(),
                role: "assistant".to_string(),
                content: String::new(),
                created_at: 1,
                status: Some("pending".to_string()),
                attachments: Vec::new(),
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
    let credentials = Arc::new(InMemoryCredentialStore::default()) as Arc<dyn CredentialStore>;
    let vault = Arc::new(
        ProviderContinuationVaultFactory::open_or_provision(Arc::clone(&storage), credentials)
            .unwrap(),
    );
    let mut provider_profile = ProviderProfileConfig::deepseek_v4_default();
    provider_profile.reasoning = ReasoningPolicy {
        mode: ReasoningMode::Enabled,
        effort: ReasoningEffort::Max,
    };
    let provider_protocol = ProviderProtocolKey::new(
        ProviderProtocolDialect::OpenAiChatCompletions,
        &provider_profile,
        MODEL_ID,
        None,
    )
    .unwrap();

    let discovery = discoverable_skill("A fixture Skill whose activation fails deterministically.");
    let activation_ref = discovery.skills[0].activation_ref.clone();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": REASONING,
                            "tool_calls": [
                                {
                                    "id": "deepseek-activate-fails",
                                    "type": "function",
                                    "function": {
                                        "name": "skills_activate",
                                        "arguments": serde_json::to_string(&json!({
                                            "skillRef": activation_ref,
                                            "reason": "Exercise independent same-response settlement"
                                        })).unwrap()
                                    }
                                },
                                {
                                    "id": "deepseek-todo-succeeds",
                                    "type": "function",
                                    "function": {
                                        "name": "todo_update",
                                        "arguments": serde_json::to_string(&json!({
                                            "items": [],
                                            "explanation": "The sibling remains independent"
                                        })).unwrap()
                                    }
                                }
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "Grouped sibling settled after activation failure.",
                            "reasoning_content": "Both Tool results are authoritative."
                        },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let resolver_calls = Arc::new(AtomicUsize::new(0));
    let resolver_calls_for_host = Arc::clone(&resolver_calls);
    let failing_resolver: AgentSkillActivationResolver = Arc::new(move |_| {
        resolver_calls_for_host.fetch_add(1, Ordering::SeqCst);
        Err(AgentError::new("fixture Skill activation failed"))
    });
    let mut input = conversation_context_input(vec![message(
        "user",
        "Activate the fixture Skill and update the todo in one response.",
    )]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "deepseek-test-token".to_string();
    input.provider_profile_config = Some(provider_profile);
    input.provider_protocol_key = Some(provider_protocol.clone());
    input.model = MODEL_ID.to_string();
    input.max_tokens = Some(1_024);
    input.stream = Some(false);
    input.assistant_message_id = Some(ASSISTANT_MESSAGE_ID.to_string());
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some(CONVERSATION_ID.to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    input.skill_discovery = Some(discovery);

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some(RUN_ID.to_string()),
            None,
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_activation_resolver(failing_resolver)
                    .with_skill_resources(Arc::new(crate::skills::SkillResourceSession::empty()))
                    .with_provider_continuation_vault(Arc::clone(&vault)),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.status, AgentRunStatus::Completed);
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let activation_result = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "skills_activate" => {
                Some(result)
            }
            _ => None,
        })
        .expect("failed activation result");
    assert!(!activation_result.ok);
    assert!(activation_result
        .error
        .as_deref()
        .is_some_and(|error| error.contains("fixture Skill activation failed")));
    let sibling_result = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolResult { result, .. } if result.tool == "todo_update" => Some(result),
            _ => None,
        })
        .expect("independent sibling result");
    assert!(sibling_result.ok);
    assert!(!serde_json::to_string(&output.events)
        .unwrap()
        .contains("skillActivationBoundary"));

    let second_request = second_request.lock().unwrap().clone().unwrap();
    let messages = second_request["messages"].as_array().unwrap();
    let grouped_turn = messages
        .iter()
        .find(|message| message["reasoning_content"].as_str() == Some(REASONING))
        .expect("exact grouped Provider turn must replay");
    let provider_call_ids = grouped_turn["tool_calls"]
        .as_array()
        .unwrap()
        .iter()
        .map(|call| call["id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        provider_call_ids,
        ["deepseek-activate-fails", "deepseek-todo-succeeds"]
    );
    let result_ids = messages
        .iter()
        .filter(|message| message["role"] == "tool")
        .map(|message| message["tool_call_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        result_ids,
        ["deepseek-activate-fails", "deepseek-todo-succeeds"]
    );
    assert!(!serde_json::to_string(&second_request)
        .unwrap()
        .contains("skillActivationBoundary"));

    let persisted_turns = vault
        .list_replayable_for_conversation(CONVERSATION_ID, &provider_protocol)
        .unwrap();
    assert_eq!(persisted_turns.len(), 1);
    assert_eq!(
        persisted_turns[0]
            .assistant_turn
            .provider_tool_calls()
            .len(),
        2
    );
}

#[tokio::test]
async fn anthropic_payload_keeps_current_user_skill_and_attachment_compatible() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
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

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let captured = Arc::new(std::sync::Mutex::new(None));
    let captured_for_server = captured.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        *captured_for_server.lock().unwrap() = Some(read_json_request(&mut stream).await);
        write_response(
            &mut stream,
            json!({
                "content": [{ "type": "text", "text": "done" }],
                "stop_reason": "end_turn"
            }),
        )
        .await;
    });

    let mut input = conversation_context_input(vec![message("user", "CURRENT_USER_MARKER")]);
    input.api_url = format!("http://{address}/v1/messages");
    input.api_token = "test-token".to_string();
    input.api_style = Some(crate::protocol::AgentApiStyle::AnthropicCompatible);
    freeze_runtime_test_generic_provider(&mut input, "anthropic-user-skill-attachment");
    input.stream = Some(false);
    input.skill_activation = Some(activated_skill("ANTHROPIC_SKILL_MARKER"));
    input.attachments = vec![AgentInputAttachment {
        id: "attachment-anthropic".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 19,
        encoding: AgentInputAttachmentEncoding::Utf8,
        data: "ATTACHMENT_MARKER".to_string(),
        truncated: None,
    }];

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
        )
        .await
        .unwrap();
    server.await.unwrap();
    assert_eq!(output.content, "done");
    let payload = captured.lock().unwrap().take().unwrap();
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["role"], "user");
    let serialized = serde_json::to_string(&messages[0]["content"]).unwrap();
    let current = serialized.find("CURRENT_USER_MARKER").unwrap();
    let skill = serialized.find("ANTHROPIC_SKILL_MARKER").unwrap();
    let attachment = serialized.find("ATTACHMENT_MARKER").unwrap();
    assert!(current < attachment && attachment < skill);
}

#[test]
fn conversation_history_tool_is_stable_even_without_a_persisted_conversation() {
    let mut input = conversation_context_input(vec![message("user", "Current question")]);
    input.context = Some(AgentRunContext {
        collaboration_identity: None,
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let capabilities =
        prepare_runtime_capabilities(&input, "history-capability", &[], true, None).unwrap();
    assert!(capabilities
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "conversation_history"));

    input.context = None;
    let capabilities =
        prepare_runtime_capabilities(&input, "no-history-capability", &[], true, None).unwrap();
    assert!(capabilities
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "conversation_history"));
}
