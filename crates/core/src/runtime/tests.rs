// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentActivatedSkill, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentPatchPermission, AgentRunContext, AgentSkillActivation,
    AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;
use crate::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        message_id: None,
        role: role.to_string(),
        content: content.to_string(),
        created_at: None,
        conversation_turn_trace: None,
    }
}

fn empty_attachment_context() -> AttachmentContext {
    AttachmentContext {
        text: String::new(),
        images: Vec::new(),
    }
}

#[test]
fn skill_activation_barrier_clears_committed_stream_write_previews() {
    let captured = Arc::new(Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = Arc::clone(&captured);
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let event_stream = AgentEventStream::new(Some(emitter));
    let mut committed_preview = Some(("run-1-stream-1".to_string(), 2));

    clear_deferred_tool_input_preview(&event_stream, "run-1", 1, &mut committed_preview);

    assert!(committed_preview.is_none());
    let captured = captured.lock().unwrap();
    assert_eq!(captured.len(), 1);
    assert!(matches!(
        &captured[0],
        AgentEvent::FileWritePreviewCleared {
            run_id,
            stream_id,
            attempt: 2,
        } if run_id == "run-1" && stream_id == "run-1-stream-1"
    ));
    drop(captured);
    assert!(event_stream.into_events().is_empty());
}

#[test]
fn skill_activation_capacity_measures_only_calls_retained_by_the_barrier() {
    let detector = ContextCapacityDetector::for_model(
        "test-model",
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &[],
    );
    let activation = LlmToolCall {
        id: "activate-documents".to_string(),
        name: "skills_activate".to_string(),
        args: json!({
            "skillRef": "s_000000000000000000000000",
            "reason": "Need document guidance"
        }),
    };
    let discarded = LlmToolCall {
        id: "discarded-read".to_string(),
        name: "read_file".to_string(),
        args: json!({ "path": "丢".repeat(10_000) }),
    };
    let original = vec![discarded, activation.clone()];
    let original_tokens =
        detector.estimate_assistant_tool_batch_tokens("Activating first.", &original);
    let (retained, deferred) = enforce_skill_activation_barrier(original);
    let retained_tokens =
        detector.estimate_assistant_tool_batch_tokens("Activating first.", &retained);

    assert_eq!(deferred, 1);
    assert_eq!(retained, vec![activation]);
    assert!(retained_tokens < original_tokens);
}

fn activated_skill(instructions: &str) -> AgentSkillActivation {
    AgentSkillActivation {
        activation_revision: "activation-sha256-v1:test".to_string(),
        skills: vec![AgentActivatedSkill {
            id: "workspace:workspace-1:review".to_string(),
            name: "repository-review".to_string(),
            revision: "skill-sha256-v1:test".to_string(),
            source: "workspace".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: None,
        }],
    }
}

fn discoverable_skill(description: &str) -> crate::skills::AgentSkillDiscoverySnapshot {
    const CATALOG_REVISION: &str = "skill-enabled-catalog-sha256-v1:test";
    const SKILL_ID: &str = "bundled:application:documents";
    let revision = format!("skill-package-sha256-v2:{}", "d".repeat(64));
    crate::skills::AgentSkillDiscoverySnapshot {
        schema_version: crate::skills::AGENT_SKILL_DISCOVERY_SCHEMA_VERSION,
        catalog_revision: CATALOG_REVISION.to_string(),
        prompt_token_budget: crate::skills::DEFAULT_SKILL_DISCOVERY_PROMPT_TOKENS,
        skills: vec![crate::skills::AgentDiscoverableSkill {
            activation_ref: crate::skills::derive_skill_activation_ref(
                CATALOG_REVISION,
                SKILL_ID,
                &revision,
            ),
            id: SKILL_ID.to_string(),
            revision,
            name: "documents".to_string(),
            description: description.to_string(),
            source_kind: "bundled".to_string(),
        }],
        max_activated_skills: 8,
        max_total_source_bytes: 512 * 1024,
    }
}

fn command_dispatch_fixture(command: &str) -> (AgentToolCall, AgentProposedAction) {
    let call = AgentToolCall {
        id: "command-dispatch".to_string(),
        tool: "run_command".to_string(),
        args: json!({ "command": command }),
        approval_status: AgentApprovalStatus::NotRequired,
        reason: None,
    };
    let action = AgentProposedAction::Command {
        command: crate::protocol::AgentCommandRequest {
            id: call.id.clone(),
            command: command.to_string(),
            cwd: None,
            timeout_ms: None,
            approval_status: AgentApprovalStatus::NotRequired,
            risk_level: None,
            reason: None,
            observe: None,
            runtime: None,
            runtime_binding: None,
        },
    };
    (call, action)
}

fn command_permissions(command_safety: AgentCommandSafetyPolicy) -> AgentPermissions {
    AgentPermissions {
        command_safety,
        ..AgentPermissions::default()
    }
}

#[test]
fn command_dispatch_guarded_automatic_routes_high_impact_work_to_approval() {
    let (call, action) = command_dispatch_fixture("python3 -m pip install openpyxl");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::Guarded),
        None,
        true,
    );

    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn command_dispatch_applies_read_scope_before_automatic_execution() {
    let (call, action) = command_dispatch_fixture("cat /etc/passwd");
    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::Guarded),
        None,
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));

    let (call, action) = command_dispatch_fixture("cat /etc/passwd");
    let dispatch = prepare_command_dispatch(
        &call,
        action,
        AgentPermissions {
            read: crate::protocol::AgentReadPermission::All,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..AgentPermissions::default()
        },
        None,
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::ExecuteAutomatically(_)));
}

#[test]
fn command_dispatch_routes_workspace_external_cwd_to_approval() {
    let (call, mut action) = command_dispatch_fixture("cat local.txt");
    let AgentProposedAction::Command { command } = &mut action else {
        unreachable!("fixture always produces a command")
    };
    command.cwd = Some("/tmp".to_string());

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        AgentPermissions {
            read: crate::protocol::AgentReadPermission::WorkspaceOnly,
            write: crate::protocol::AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            ..AgentPermissions::default()
        },
        Some(Path::new("/workspace")),
        true,
    );
    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn command_dispatch_full_access_automatic_executes_high_impact_work() {
    let (call, action) = command_dispatch_fixture("python3 -m pip install openpyxl");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        true,
    );

    assert!(matches!(dispatch, CommandDispatch::ExecuteAutomatically(_)));
}

#[test]
fn command_dispatch_full_access_returns_structured_rejection_for_catastrophic_command() {
    let (call, action) = command_dispatch_fixture("sudo rm -rf /");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        true,
    );

    let CommandDispatch::Reject(result) = dispatch else {
        panic!("catastrophic command must be rejected");
    };
    assert!(!result.ok);
    let payload = result.result.expect("policy rejection payload");
    assert_eq!(payload["type"], "command_policy");
    assert_eq!(payload["decision"], "deny");
    assert!(payload["code"]
        .as_str()
        .is_some_and(|code| !code.is_empty()));
    assert!(payload["findings"]
        .as_array()
        .is_some_and(|items| !items.is_empty()));
}

#[test]
fn command_dispatch_require_approval_never_auto_executes_an_allowed_command() {
    let (call, action) = command_dispatch_fixture("git status --short");

    let dispatch = prepare_command_dispatch(
        &call,
        action,
        command_permissions(AgentCommandSafetyPolicy::FullAccess),
        None,
        false,
    );

    assert!(matches!(dispatch, CommandDispatch::RequireApproval(_)));
}

#[test]
fn runtime_messages_add_backend_system_prompt() {
    let context = AgentRunContext {
        conversation_id: Some("conversation-1".to_string()),
        project_id: Some("project-1".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-1".to_string()),
            display_name: Some("Workspace".to_string()),
            root_path: Some("/private/path".to_string()),
        }),
        attachment_library: None,
        permissions: Default::default(),
    };
    let context = assemble_initial_context(
        None,
        vec![message("user", "Read src/main.rs")],
        None,
        empty_attachment_context(),
        Some(&context),
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert_eq!(messages[0].role.as_str(), "system");
    assert!(messages[0].content.contains("MyCopilot"));
    assert!(!messages[0].content.contains("/private/path"));
    assert_eq!(messages[1].role.as_str(), "user");
}

#[test]
fn runtime_messages_include_text_attachment_content() {
    let context = assemble_initial_context(
        None,
        vec![message("user", "Summarize this attachment")],
        None,
        AttachmentContext {
            text: "用户输入框附件内容如下。\n\n### notes.txt\nhello from attachment".to_string(),
            images: Vec::new(),
        },
        None,
        None,
        &ToolRegistry::defaults_with_search(None).definitions(),
    )
    .unwrap();
    let messages = context.to_messages();

    assert!(messages
        .iter()
        .any(|message| message.role == LlmMessageRole::User
            && message.content.contains("hello from attachment")));
}

#[test]
fn attachment_context_reads_text_with_registered_tool() {
    let context = build_attachment_context(&[AgentInputAttachment {
        id: "attachment-1".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "notes.txt".to_string(),
        mime_type: Some("text/plain".to_string()),
        size_bytes: 16,
        encoding: AgentInputAttachmentEncoding::Utf8,
        data: "hello from file".to_string(),
        truncated: None,
    }])
    .unwrap();

    assert!(context.text.contains("读取工具：read_file"));
    assert!(context.text.contains("hello from file"));
}

#[test]
fn read_image_tool_result_is_redacted_but_creates_visual_message() {
    let result = AgentToolResult {
        call_id: "call-image".to_string(),
        tool: "read_image".to_string(),
        ok: true,
        result: Some(json!({
            "path": "@attachments/image1/pixel.png",
            "format": "png",
            "mimeType": "image/png",
            "sizeBytes": 3,
            "thumbnailDataUrl": "data:image/png;base64,dGh1bWI=",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    let registry = ToolRegistry::defaults_with_search(None);
    let event_result = redact_tool_result_for_event(&registry.event_projection(&result));
    let event_value = event_result.result.as_ref().unwrap();
    assert_eq!(
        event_value["thumbnailDataUrl"],
        "data:image/png;base64,dGh1bWI="
    );
    assert!(event_value.get("image").is_none());

    let image_message =
        llm_image_message_from_tool_result(&result, crate::ModelCapabilities { image_input: true })
            .unwrap();
    assert_eq!(image_message.role, LlmMessageRole::User);
    assert_eq!(image_message.images.len(), 1);
    assert_eq!(image_message.images[0].mime_type, "image/png");
    assert_eq!(image_message.images[0].data_base64, "YWJj");
}

#[test]
fn failed_read_image_capability_result_never_creates_visual_input() {
    let result = AgentToolResult {
        call_id: "call-image-unsupported".to_string(),
        tool: "read_image".to_string(),
        ok: false,
        result: Some(json!({
            "type": "model_capability",
            "code": "modelCapabilityUnsupported",
            "errorCode": "agent.model_capability_unsupported",
            "capability": "imageInput"
        })),
        error: Some("当前模型不支持图片输入；文件尚未读取。".to_string()),
    };

    assert!(llm_image_message_from_tool_result(
        &result,
        crate::ModelCapabilities { image_input: false }
    )
    .is_none());
}

#[test]
fn generated_image_visual_input_is_capability_gated() {
    let result = AgentToolResult {
        call_id: "call-generated-image".to_string(),
        tool: "image_generation".to_string(),
        ok: true,
        result: Some(json!({
            "schemaVersion": 1,
            "status": "succeeded",
            "savedPath": "/managed/generated-image.png",
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    assert!(llm_image_message_from_tool_result(
        &result,
        crate::ModelCapabilities { image_input: false }
    )
    .is_none());

    let image_message =
        llm_image_message_from_tool_result(&result, crate::ModelCapabilities { image_input: true })
            .expect("image-capable models receive generated pixels");
    assert_eq!(image_message.role, LlmMessageRole::User);
    assert!(image_message
        .content
        .contains("/managed/generated-image.png"));
    assert_eq!(image_message.images.len(), 1);
    assert_eq!(image_message.images[0].mime_type, "image/png");
    assert_eq!(image_message.images[0].data_base64, "YWJj");
}

#[test]
fn file_write_tail_is_available_to_llm_but_not_persisted_in_events() {
    let result = AgentToolResult {
        call_id: "call-write".to_string(),
        tool: "write_file".to_string(),
        ok: true,
        result: Some(json!({
            "draft": { "draftId": "draft-1" },
            "tail": "private generated content"
        })),
        error: None,
    };

    let llm_result = canonical_tool_result_for_context(&result);
    let event_result = redact_tool_result_for_event(&result);

    assert_eq!(
        llm_result.result.as_ref().unwrap()["tail"],
        "private generated content"
    );
    assert!(event_result.result.as_ref().unwrap().get("tail").is_none());
    assert_eq!(
        event_result.result.as_ref().unwrap()["draft"]["draftId"],
        "draft-1"
    );
}

#[test]
fn parses_plain_and_fenced_tool_calls() {
    let plain = parse_tool_call_request(
        r#"{"type":"tool_call","tool":"search_files","args":{"query":"main"}}"#,
    )
    .unwrap();
    let fenced = parse_tool_call_request(
            "```json\n{\"type\":\"tool_call\",\"tool\":\"read_file\",\"args\":{\"path\":\"src/lib.rs\"}}\n```",
        )
        .unwrap();

    assert_eq!(plain.tool, "search_files");
    assert_eq!(plain.args["query"], "main");
    assert_eq!(fenced.tool, "read_file");
    assert_eq!(fenced.args["path"], "src/lib.rs");
}

#[test]
fn transient_events_are_emitted_without_entering_output_history() {
    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let stream = AgentEventStream::new(Some(emitter));

    stream.emit_transient(AgentEvent::ToolInputProgress {
        run_id: "run-1".to_string(),
        stream_id: "stream-1".to_string(),
        attempt: 1,
        tool_call_index: 0,
        tool_call_id: Some("call-1".to_string()),
        tool: "write_file".to_string(),
        received_bytes: 128,
    });

    assert_eq!(captured.lock().unwrap().len(), 1);
    assert!(stream.into_events().is_empty());
}

#[test]
fn context_window_preview_is_available_independently_of_indicator_events() {
    let input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": false,
        "messages": []
    }))
    .unwrap();

    assert!(inspect_context_window(input).unwrap().is_some());
}

fn conversation_context_input(messages: Vec<AgentChatMessage>) -> AgentChatInput {
    AgentChatInput {
        api_url: "https://example.test/v1/chat/completions".to_string(),
        api_token: String::new(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(30_000),
        temperature: None,
        stream: Some(true),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        skill_activation: None,
        skill_discovery: None,
        messages,
    }
}

#[test]
fn runtime_command_definition_advertises_effective_approval_routing() {
    for (safety, expected_mode) in [
        (
            AgentCommandSafetyPolicy::Guarded,
            crate::protocol::AgentToolApprovalMode::Dynamic,
        ),
        (
            AgentCommandSafetyPolicy::FullAccess,
            crate::protocol::AgentToolApprovalMode::Never,
        ),
    ] {
        let mut input = conversation_context_input(vec![message("user", "run a command")]);
        input.context = Some(AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                command: crate::protocol::AgentCommandPermission::AutoApprove,
                command_safety: safety,
                ..Default::default()
            },
        });

        let capabilities =
            prepare_runtime_capabilities(&input, "command-definition", &[], true, None).unwrap();
        let definition = capabilities
            .tool_definitions
            .iter()
            .find(|definition| definition.name == "run_command")
            .expect("run_command definition");

        assert!(!definition.requires_approval);
        assert_eq!(definition.approval_mode, expected_mode);
    }
}

#[test]
fn model_capabilities_do_not_change_tool_definitions_or_context_revision() {
    let mut input = conversation_context_input(vec![message("user", "Inspect the image")]);
    input.model_capabilities.image_input = false;
    let text_only =
        prepare_runtime_capabilities(&input, "model-capabilities-text-only", &[], true, None)
            .unwrap();
    let text_only_revision = conversation_context_configuration_revision(&input).unwrap();

    input.model_capabilities.image_input = true;
    let image_capable =
        prepare_runtime_capabilities(&input, "model-capabilities-image", &[], true, None).unwrap();
    let image_capable_revision = conversation_context_configuration_revision(&input).unwrap();

    assert!(text_only
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert!(image_capable
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "read_image"));
    assert_eq!(
        serde_json::to_value(&text_only.tool_definitions).unwrap(),
        serde_json::to_value(&image_capable.tool_definitions).unwrap()
    );
    assert_eq!(text_only_revision, image_capable_revision);
}

#[test]
fn runtime_structured_writers_share_the_file_edit_approval_policy() {
    let definitions = |patch, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "edit a workbook")]);
        input.context = Some(AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                write: crate::protocol::AgentWritePermission::WorkspaceOnly,
                patch,
                ..Default::default()
            },
        });
        let engine =
            crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
        prepare_runtime_capabilities(
            &input,
            "office-definition",
            &[],
            host_actions_available,
            Some(engine),
        )
        .unwrap()
        .tool_definitions
    };

    let manual = definitions(AgentPatchPermission::RequireApproval, true);
    let automatic = definitions(AgentPatchPermission::AutoApprove, true);
    let without_host = definitions(AgentPatchPermission::AutoApprove, false);

    for name in [
        "apply_patch",
        "write_file",
        "skills_materialize_resource",
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(
            manual
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
        assert!(
            !automatic
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
        assert_eq!(
            automatic
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .approval_mode,
            crate::protocol::AgentToolApprovalMode::Never
        );
        assert!(
            without_host
                .iter()
                .find(|definition| definition.name == name)
                .unwrap()
                .requires_approval
        );
    }
}

#[test]
fn write_denied_hides_write_only_tools_but_keeps_office_reads_available() {
    let mut input = conversation_context_input(vec![message("user", "inspect a workbook")]);
    input.context = Some(AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: Some(AgentWorkspaceContext {
            project_id: None,
            display_name: Some("workspace".to_string()),
            root_path: Some("/tmp/workspace".to_string()),
        }),
        attachment_library: None,
        permissions: crate::protocol::AgentPermissions {
            write: crate::protocol::AgentWritePermission::Denied,
            patch: AgentPatchPermission::AutoApprove,
            ..Default::default()
        },
    });
    let engine =
        crate::office::resolve_office_engine(&crate::office::OfficeCliDiscoveryOptions::new());
    let definitions = prepare_runtime_capabilities(&input, "write-denied", &[], true, Some(engine))
        .unwrap()
        .tool_definitions;

    for name in ["apply_patch", "write_file", "skills_materialize_resource"] {
        assert!(!definitions.iter().any(|definition| definition.name == name));
    }
    for name in [
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
}

#[tokio::test]
async fn effective_tool_definitions_are_also_the_execution_allowlist() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
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
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                let body_start = request
                    .windows(4)
                    .position(|part| part == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                return request[body_start..].to_vec();
            }
        }
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

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(Vec::new()));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_http_body(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = request;
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "hidden-tool-call",
                                "type": "function",
                                "function": {
                                    "name": "skills_preflight_script",
                                    "arguments": "{}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "run the hidden tool")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "done");
    let call = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.id == "hidden-tool-call" => Some(call),
            _ => None,
        })
        .expect("the unavailable tool call remains observable");
    assert_eq!(call.reason, None);
    let request = String::from_utf8(second_request.lock().unwrap().clone()).unwrap();
    assert!(request.contains("agent.tool_not_available"));
    assert!(request.contains("toolNotAvailable"));
}

#[tokio::test]
async fn text_only_model_receives_paired_read_image_capability_failure_without_image_payload() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_json_request(stream: &mut TcpStream) -> Value {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            assert!(read > 0, "connection closed before request completed");
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
                        .unwrap_or_default();
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
            .unwrap()
            + 4;
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

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let captured_second_request = Arc::clone(&second_request);
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_json_request(&mut stream).await;
            if request_index == 1 {
                *captured_second_request.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I will inspect the image.",
                            "tool_calls": [{
                                "id": "read-image-unsupported",
                                "type": "function",
                                "function": {
                                    "name": "read_image",
                                    "arguments": serde_json::to_string(&json!({
                                        "path": "/path/that/must/not/be-read.png"
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "This model cannot inspect images." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_json_response(&mut stream, response).await;
        }
    });

    let mut input = conversation_context_input(vec![message("user", "Inspect this image")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.model_capabilities.image_input = false;
    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "This model cannot inspect images.");
    let call_index = output
        .events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolCall { call, .. } if call.id == "read-image-unsupported"))
        .expect("read_image tool call event");
    let (result_index, result) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolResult { result, .. } if result.call_id == "read-image-unsupported" => {
                Some((index, result))
            }
            _ => None,
        })
        .expect("paired read_image tool result event");
    assert!(call_index < result_index);
    assert!(!result.ok);
    assert_eq!(result.tool, "read_image");
    let structured = result
        .result
        .as_ref()
        .expect("structured capability failure");
    assert_eq!(structured["code"], "modelCapabilityUnsupported");
    assert_eq!(
        structured["errorCode"],
        "agent.model_capability_unsupported"
    );

    let second_request = second_request
        .lock()
        .unwrap()
        .clone()
        .expect("second model request");
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| {
            message["role"] == "assistant"
                && message["tool_calls"][0]["id"] == "read-image-unsupported"
        })
        .expect("assistant tool call in provider payload");
    let tool_result_index = messages
        .iter()
        .position(|message| {
            message["role"] == "tool" && message["tool_call_id"] == "read-image-unsupported"
        })
        .expect("paired tool result in provider payload");
    assert!(tool_call_index < tool_result_index);

    let request = serde_json::to_string(&second_request).unwrap();
    assert!(request.contains("modelCapabilityUnsupported"));
    assert!(request.contains("agent.model_capability_unsupported"));
    assert!(!request.contains("modelCapabilities"));
    assert!(!request.contains("image_url"));
    assert!(!request.contains("data:image/"));
}

#[tokio::test]
async fn image_capable_read_image_round_trip_is_legal_for_openai_and_anthropic() {
    use base64::Engine;
    use image::ImageEncoder;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

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
                        .unwrap_or_default();
                    let start = header_end + 4;
                    body_start = Some(start);
                    expected_length = Some(start + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                break;
            }
        }
        serde_json::from_slice(
            &request[body_start.unwrap()..expected_length.expect("content length")],
        )
        .unwrap()
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

    async fn run_case(style: crate::protocol::AgentApiStyle) {
        let fixture = tempdir().unwrap();
        let image_path = fixture.path().join("pixel.png");
        let mut image_bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut image_bytes)
            .write_image(
                &[0x10, 0x40, 0x90, 0xff],
                1,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .unwrap();
        std::fs::write(&image_path, &image_bytes).unwrap();
        let full_image_base64 = base64::engine::general_purpose::STANDARD.encode(&image_bytes);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let second_request = Arc::new(Mutex::new(None::<Value>));
        let captured_second_request = Arc::clone(&second_request);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                if request_index == 1 {
                    *captured_second_request.lock().unwrap() = Some(request);
                }
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": {
                                "role": "assistant",
                                "content": "I will inspect the image.",
                                "tool_calls": [{
                                    "id": "read-image-success",
                                    "type": "function",
                                    "function": {
                                        "name": "read_image",
                                        "arguments": "{\"path\":\"pixel.png\"}"
                                    }
                                }]
                            },
                            "finish_reason": "tool_calls"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "image inspected" },
                            "finish_reason": "stop"
                        }]
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [
                            { "type": "text", "text": "I will inspect the image." },
                            {
                                "type": "tool_use",
                                "id": "read-image-success",
                                "name": "read_image",
                                "input": { "path": "pixel.png" }
                            }
                        ],
                        "stop_reason": "tool_use"
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "image inspected" }],
                        "stop_reason": "end_turn"
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let mut input = conversation_context_input(vec![message("user", "Inspect pixel.png")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        input.stream = Some(false);
        input.model_capabilities.image_input = true;
        input.assistant_message_id = Some("assistant-image".to_string());
        input.context = Some(AgentRunContext {
            conversation_id: Some("conversation-image".to_string()),
            project_id: Some("project-image".to_string()),
            workspace: Some(AgentWorkspaceContext {
                project_id: Some("project-image".to_string()),
                display_name: Some("Image workspace".to_string()),
                root_path: Some(fixture.path().to_string_lossy().to_string()),
            }),
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default().send_chat(input).await.unwrap();
        server.await.unwrap();
        assert_eq!(output.content, "image inspected");

        let event_result = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult { result, .. } if result.call_id == "read-image-success" => {
                    Some(result)
                }
                _ => None,
            })
            .expect("read_image result event");
        let event_value = event_result.result.as_ref().unwrap();
        assert!(event_value["thumbnailDataUrl"]
            .as_str()
            .is_some_and(|value| value.starts_with("data:image/png;base64,")));
        assert!(event_value.get("image").is_none());

        let trace = output
            .conversation_turn_trace
            .as_ref()
            .expect("terminal conversation trace");
        trace.validate().unwrap();
        assert!(trace.truncated);
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolResult {
                call_id,
                truncated: true,
                ..
            } if call_id == "read-image-success"
        )));
        let durable = serde_json::to_string(trace).unwrap();
        assert!(!durable.contains(&full_image_base64));
        assert!(!durable.contains("data:image"));
        assert!(!durable.contains("thumbnailDataUrl"));
        assert!(!durable.contains("dataBase64"));

        let request = second_request
            .lock()
            .unwrap()
            .clone()
            .expect("second provider request");
        let messages = request["messages"].as_array().unwrap();
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant"
                            && message["tool_calls"][0]["id"] == "read-image-success"
                    })
                    .expect("OpenAI assistant tool call");
                let tool_result_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == "read-image-success"
                    })
                    .expect("OpenAI paired tool result");
                let image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| part["type"] == "image_url")
                            })
                    })
                    .expect("OpenAI visual input");
                assert!(tool_call_index < tool_result_index && tool_result_index < image_index);
                assert!(messages[image_index]["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|part| {
                        part["image_url"]["url"]
                            == format!("data:image/png;base64,{full_image_base64}")
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                let tool_call_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "assistant"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["type"] == "tool_use" && part["id"] == "read-image-success"
                                })
                            })
                    })
                    .expect("Anthropic assistant tool_use");
                let result_and_image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["type"] == "tool_result"
                                        && part["tool_use_id"] == "read-image-success"
                                }) && parts.iter().any(|part| {
                                    part["type"] == "image"
                                        && part["source"]["data"] == full_image_base64
                                })
                            })
                    })
                    .expect("Anthropic paired tool_result and visual input");
                assert!(tool_call_index < result_and_image_index);
            }
        }
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[test]
fn runtime_skill_script_definition_respects_the_host_permission_matrix() {
    use crate::protocol::{
        AgentCommandPermission, AgentReadPermission, AgentToolApprovalMode, AgentWorkspaceContext,
        AgentWritePermission,
    };

    let definitions = |write, command, command_safety, host_actions_available| {
        let mut input = conversation_context_input(vec![message("user", "run a Skill script")]);
        input.context = Some(AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions: crate::protocol::AgentPermissions {
                read: if command_safety == AgentCommandSafetyPolicy::FullAccess {
                    AgentReadPermission::All
                } else {
                    AgentReadPermission::WorkspaceOnly
                },
                write,
                command,
                command_safety,
                ..Default::default()
            },
        });
        prepare_runtime_capabilities(
            &input,
            "skill-script-definition",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
        .tool_definitions
    };

    let guarded = definitions(
        AgentWritePermission::WorkspaceOnly,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::Guarded,
        true,
    );
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!guarded
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let write_denied = definitions(
        AgentWritePermission::Denied,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_run_script"));
    assert!(!write_denied
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));

    let manual = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::RequireApproval,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    assert!(manual
        .iter()
        .any(|definition| definition.name == "skills_preflight_script"));
    let manual = manual
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(manual.requires_approval);
    assert_eq!(manual.approval_mode, AgentToolApprovalMode::Always);

    let automatic = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        true,
    );
    let automatic = automatic
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(automatic.requires_approval);
    assert_eq!(automatic.approval_mode, AgentToolApprovalMode::Always);

    let without_host = definitions(
        AgentWritePermission::All,
        AgentCommandPermission::AutoApprove,
        AgentCommandSafetyPolicy::FullAccess,
        false,
    );
    let without_host = without_host
        .iter()
        .find(|definition| definition.name == "skills_run_script")
        .unwrap();
    assert!(without_host.requires_approval);
    assert_eq!(without_host.approval_mode, AgentToolApprovalMode::Always);
}

fn conversation_context_trace(
    terminal_status: ConversationTurnTraceTerminalStatus,
    items: Vec<ConversationTurnTraceItem>,
) -> ConversationTurnTrace {
    ConversationTurnTrace {
        schema_version: CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
        run_id: "run-context".to_string(),
        conversation_id: "conversation-context".to_string(),
        assistant_message_id: "assistant-context".to_string(),
        terminal_status,
        terminal_error: None,
        truncated: false,
        items,
    }
}

fn traced_assistant_message(content: &str, trace: ConversationTurnTrace) -> AgentChatMessage {
    AgentChatMessage {
        message_id: Some(trace.assistant_message_id.clone()),
        role: "assistant".to_string(),
        content: content.to_string(),
        created_at: Some(2_000),
        conversation_turn_trace: Some(trace),
    }
}

fn full_conversation_context_snapshot(
    messages: Vec<AgentChatMessage>,
) -> AgentContextWindowSnapshot {
    create_conversation_context_state(conversation_context_input(messages))
        .unwrap()
        .snapshot(AgentContextWindowPhase::DurableCommit)
}

#[test]
fn conversation_context_state_incremental_updates_match_full_rebuilds() {
    let mut first_user = message("user", "Inspect the project and update src/lib.rs");
    first_user.created_at = Some(1_000);
    let mut state =
        create_conversation_context_state(conversation_context_input(vec![first_user.clone()]))
            .unwrap();

    let narration = ConversationTurnTraceItem::AssistantNarration {
        sequence: 0,
        content: "I will inspect the current implementation first.".to_string(),
        truncated: false,
    };
    let narrated_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration.clone()],
    );
    let cursor = state.append_trace_items(&narrated_trace, 0).unwrap();
    assert_eq!(cursor, 1);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message("", narrated_trace.clone()),
        ])
    );

    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: "call-context".to_string(),
        tool: "read_file".to_string(),
        operation: json!({ "path": "src/lib.rs" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: "call-context".to_string(),
        tool: "read_file".to_string(),
        status: ConversationTraceToolResultStatus::Succeeded,
        success: true,
        observation: json!({ "path": "src/lib.rs", "endLine": 40 }),
        approval_status: AgentApprovalStatus::NotRequired,
        error: None,
        truncated: false,
    };
    let closed_trace = conversation_context_trace(
        ConversationTurnTraceTerminalStatus::InProgress,
        vec![narration, call, result],
    );
    let cursor = state.append_trace_items(&closed_trace, cursor).unwrap();
    assert_eq!(cursor, 3);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message("", closed_trace.clone()),
        ])
    );

    let completed_trace = ConversationTurnTrace {
        terminal_status: ConversationTurnTraceTerminalStatus::Completed,
        ..closed_trace
    };
    let final_content = "I updated the implementation and verified the tests.";
    state
        .finalize_conversation_turn(&completed_trace, cursor, final_content, Some(2_000))
        .unwrap();
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user.clone(),
            traced_assistant_message(final_content, completed_trace.clone()),
        ])
    );

    let follow_up = "Now explain the change.";
    state
        .append_user_message(None, follow_up, Some(3_000))
        .unwrap();
    let mut follow_up_message = message("user", follow_up);
    follow_up_message.created_at = Some(3_000);
    assert_eq!(
        state.snapshot(AgentContextWindowPhase::DurableCommit),
        full_conversation_context_snapshot(vec![
            first_user,
            traced_assistant_message(final_content, completed_trace),
            follow_up_message,
        ])
    );
}

#[test]
fn runtime_shared_baseline_matches_full_context_assembly() {
    let input = conversation_context_input(vec![
        message("user", "First question"),
        message("assistant", "First answer"),
        message("user", "Current question"),
    ]);
    let capabilities =
        prepare_runtime_capabilities(&input, "baseline-test", &[], true, None).unwrap();
    let full =
        build_llm_request(input.clone(), &capabilities.tool_definitions, None, None).unwrap();
    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let baseline = durable_state.shared_baseline().unwrap();
    let shared =
        build_llm_request(input, &capabilities.tool_definitions, None, Some(baseline)).unwrap();

    assert_eq!(shared.context.to_messages(), full.context.to_messages());
}

#[test]
fn activated_skill_is_a_measured_dynamic_overlay_not_a_cache_input() {
    const INSTRUCTIONS: &str = "SKILL_DYNAMIC_MARKER: inspect evidence before editing.";
    let mut input = conversation_context_input(vec![
        message("user", "First question"),
        message("assistant", "First answer"),
        message("user", "Current question"),
    ]);
    input.skill_activation = Some(activated_skill(INSTRUCTIONS));

    let mut changed_selection = input.clone();
    changed_selection.skill_activation = Some(activated_skill(
        "SKILL_CHANGED_MARKER: use a different workflow.",
    ));
    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_selection).unwrap()
    );

    let capabilities =
        prepare_runtime_capabilities(&input, "skill-overlay", &[], true, None).unwrap();
    let mut full =
        build_llm_request(input.clone(), &capabilities.tool_definitions, None, None).unwrap();
    let manifest = full.context.manifest();
    let skill_entry = manifest
        .entries
        .iter()
        .find(|entry| entry.sources == vec!["skill_instructions"])
        .unwrap();
    assert_eq!(skill_entry.role, "user");
    assert_eq!(skill_entry.scope, "run");
    assert_eq!(skill_entry.retention, "retained");
    assert_eq!(skill_entry.origin_kind, Some("skill"));
    assert_eq!(skill_entry.origin_id, Some("workspace:workspace-1:review"));
    let messages = full.context.to_messages();
    let skill_index = messages
        .iter()
        .position(|message| message.content.contains(INSTRUCTIONS))
        .unwrap();
    let current_user_index = messages
        .iter()
        .position(|message| message.content == "Current question")
        .unwrap();
    assert!(skill_index > current_user_index);
    assert!(!messages[0].content.contains(INSTRUCTIONS));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let skill_report = detector.inspect(
        &mut full.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    assert_eq!(
        skill_report
            .usage
            .breakdown
            .run_transient
            .context_item_count,
        1
    );
    assert!(skill_report.usage.breakdown.run_transient.input_tokens > 0);

    let mut without_skill = input.clone();
    without_skill.skill_activation = None;
    let mut plain = build_llm_request(
        without_skill.clone(),
        &capabilities.tool_definitions,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_skill.context_window_tokens,
        sanitize_max_tokens(without_skill.max_tokens),
    );
    assert_eq!(
        skill_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(skill_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens());

    let plain_preview = inspect_context_window(without_skill).unwrap().unwrap();
    let skill_preview = inspect_context_window(input.clone()).unwrap().unwrap();
    assert_eq!(
        skill_preview.persistent_revision,
        plain_preview.persistent_revision
    );
    assert!(skill_preview.run_transient_input_tokens > 0);
    assert!(skill_preview.request_input_tokens > plain_preview.request_input_tokens);

    let mut durable_state = create_conversation_context_state(input.clone()).unwrap();
    let cached_plain = durable_state.snapshot(AgentContextWindowPhase::Idle);
    let cached_skill = durable_state
        .snapshot_with_skill_activation(
            AgentContextWindowPhase::Idle,
            input.skill_activation.as_ref(),
        )
        .unwrap();
    let cached_plain_after = durable_state.snapshot(AgentContextWindowPhase::Idle);
    assert_eq!(cached_plain, cached_plain_after);
    assert_eq!(
        cached_skill.persistent_revision,
        cached_plain.persistent_revision
    );
    assert!(cached_skill.run_transient_input_tokens > cached_plain.run_transient_input_tokens);
    let baseline = durable_state.shared_baseline().unwrap();
    let shared = build_llm_request(
        input.clone(),
        &capabilities.tool_definitions,
        None,
        Some(baseline),
    )
    .unwrap();
    assert_eq!(shared.context.to_messages(), full.context.to_messages());

    let debug = format!("{:?}", input.skill_activation);
    assert!(!debug.contains(INSTRUCTIONS));
}

#[test]
fn discoverable_skill_catalog_is_a_measured_dynamic_overlay_not_a_cache_input() {
    let mut input = conversation_context_input(vec![message("user", "Create a document")]);
    input.skill_discovery = Some(discoverable_skill("Create and verify Word documents."));
    let mut changed_catalog = input.clone();
    changed_catalog.skill_discovery = Some(discoverable_skill("Updated routing metadata."));

    assert_eq!(
        conversation_context_configuration_revision(&input).unwrap(),
        conversation_context_configuration_revision(&changed_catalog).unwrap()
    );

    let capabilities =
        prepare_runtime_capabilities(&input, "skill-discovery-overlay", &[], true, None).unwrap();
    let activation_ref = input.skill_discovery.as_ref().unwrap().skills[0]
        .activation_ref
        .clone();
    let mut with_catalog =
        build_llm_request(input.clone(), &capabilities.tool_definitions, None, None).unwrap();
    let catalog_entry = with_catalog
        .context
        .manifest()
        .entries
        .into_iter()
        .find(|entry| entry.sources == vec!["skill_catalog"])
        .unwrap();
    assert_eq!(catalog_entry.scope, "run");
    assert_eq!(catalog_entry.retention, "retained");

    let rendered = with_catalog
        .context
        .to_messages()
        .into_iter()
        .find(|message| message.content.contains("backend_available_skills"))
        .unwrap()
        .content;
    assert!(rendered.contains(&format!("\"ref\":\"{activation_ref}\"")));
    assert!(rendered.contains("Create and verify Word documents."));
    assert!(!rendered.contains("bundled:application:documents"));
    assert!(!rendered.contains("skill-package-sha256-v2:"));

    let detector = ContextCapacityDetector::for_model(
        &input.model,
        crate::protocol::AgentApiStyle::OpenAiCompatible,
        &capabilities.tool_definitions,
    );
    let catalog_report = detector.inspect(
        &mut with_catalog.context,
        input.context_window_tokens,
        sanitize_max_tokens(input.max_tokens),
    );
    let mut without_catalog = input.clone();
    without_catalog.skill_discovery = None;
    let mut plain = build_llm_request(
        without_catalog.clone(),
        &capabilities.tool_definitions,
        None,
        None,
    )
    .unwrap();
    let plain_report = detector.inspect(
        &mut plain.context,
        without_catalog.context_window_tokens,
        sanitize_max_tokens(without_catalog.max_tokens),
    );
    assert_eq!(
        catalog_report.usage.persistent_revision,
        plain_report.usage.persistent_revision
    );
    assert!(
        catalog_report.usage.request_input_tokens() > plain_report.usage.request_input_tokens()
    );
}

#[tokio::test]
async fn model_activation_discloses_full_skill_only_after_the_paired_tool_result() {
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
                                    "id": "read-before-skill",
                                    "type": "function",
                                    "function": {
                                        "name": "read_file",
                                        "arguments": serde_json::to_string(&json!({
                                            "path": "draft.docx"
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
            resources: Arc::new(crate::skills::SkillResourceSession::empty()),
        })
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let events_for_emitter = Arc::clone(&events);
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        events_for_emitter.lock().unwrap().push(event);
    });
    let mut input = conversation_context_input(vec![message("user", "Create a Word guide")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.skill_discovery = Some(discovery);

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-dynamic-skill".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_skill_activation_resolver(resolver)
                    .with_skill_resources(Arc::new(crate::skills::SkillResourceSession::empty())),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Skill loaded and applied.");
    assert_eq!(resolver_calls.load(Ordering::SeqCst), 1);
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    let first_serialized = serde_json::to_string(&requests[0]).unwrap();
    assert!(first_serialized.contains("backend_available_skills"));
    assert!(first_serialized.contains("skills_activate"));
    assert!(!first_serialized.contains(INSTRUCTIONS));

    let second_messages = requests[1]["messages"].as_array().unwrap();
    let tool_result_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "tool" && message["tool_call_id"] == "activate-documents"
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
    assert!(!serde_json::to_string(&requests[1])
        .unwrap()
        .contains("read-before-skill"));
    assert!(second_messages.iter().any(|message| {
        message["role"] == "system"
            && message["content"]
                .as_str()
                .is_some_and(|content| content.contains("deferred 1 tool call"))
    }));

    let events = events.lock().unwrap();
    let result_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.call_id == "activate-documents"))
        .unwrap();
    let activated_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::SkillActivated { skill, .. } if skill.id == "bundled:application:documents"))
        .unwrap();
    assert!(result_index < activated_index);
    assert!(!events.iter().any(|event| {
        matches!(event, AgentEvent::ToolCall { call, .. } if call.id == "read-before-skill")
    }));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains(INSTRUCTIONS));
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

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
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
    assert!(current < skill && skill < attachment);
}

#[test]
fn conversation_history_tool_is_registered_only_for_persisted_conversation_runs() {
    let mut input = conversation_context_input(vec![message("user", "Current question")]);
    input.context = Some(AgentRunContext {
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
    assert!(!capabilities
        .tool_definitions
        .iter()
        .any(|definition| definition.name == "conversation_history"));
}

#[tokio::test]
async fn durable_compaction_runs_before_capacity_gate_and_then_sends_rebuilt_context() {
    use crate::context::{ContextCompactionGeneration, ContextCompactionSummary};
    use crate::protocol::AgentApiStyle;
    use crate::{
        AgentUsage, ContextCompactionPrefix, ContextCompactionSourceItem,
        ContextCompactionSummaryDraft, ContextJournalCursor,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_body(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                break;
            }
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
                        .unwrap_or_default();
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
        request[body_start..].to_vec()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_bodies = Arc::new(Mutex::new(Vec::new()));
    let request_bodies_for_server = request_bodies.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let body = read_http_body(&mut stream).await;
            request_bodies_for_server.lock().unwrap().push(body);
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": null,
                            "tool_calls": [{
                                "id": "todo-after-compaction",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": serde_json::to_string(&json!({
                                        "items": [{
                                            "title": "Verify compacted context",
                                            "status": "completed"
                                        }]
                                    }))
                                    .unwrap()
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "done" },
                        "finish_reason": "stop"
                    }]
                })
            };
            let response_body = serde_json::to_vec(&response).unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                response_body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&response_body).await.unwrap();
        }
    });

    let old_user = AgentChatMessage {
        message_id: Some("user-old".to_string()),
        role: "user".to_string(),
        content: format!("OLD_USER_MARKER {}", "x".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
    };
    let old_assistant = AgentChatMessage {
        message_id: Some("assistant-old".to_string()),
        role: "assistant".to_string(),
        content: format!("OLD_ASSISTANT_MARKER {}", "y".repeat(60_000)),
        created_at: None,
        conversation_turn_trace: None,
    };
    let current_user = AgentChatMessage {
        message_id: Some("user-current".to_string()),
        role: "user".to_string(),
        content: "continue".to_string(),
        created_at: None,
        conversation_turn_trace: None,
    };
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(50_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-1".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-current".to_string()),
        context_compaction_summary: None,
        skill_activation: None,
        skill_discovery: None,
        messages: vec![old_user, old_assistant, current_user.clone()],
    };
    let durable_prefix = Arc::new(ContextCompactionPrefix {
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        covered_through: ContextJournalCursor::message("assistant-old"),
        previous_summary: None,
        source_items: vec![
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("user-old"),
                role: "user".to_string(),
                content: "old request".to_string(),
                created_at: 1,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
            ContextCompactionSourceItem::Message {
                cursor: ContextJournalCursor::message("assistant-old"),
                role: "assistant".to_string(),
                content: "old answer".to_string(),
                created_at: 2,
                status: Some("sent".to_string()),
                terminal_status: None,
                terminal_error: None,
            },
        ],
    });
    let compacted_continuity = crate::ContextContinuitySnapshot::from_prefix(&durable_prefix)
        .expect("test durable prefix should produce continuity records");
    let compacted_summary = ContextCompactionSummary {
        schema_version: crate::CONTEXT_COMPACTION_SUMMARY_SCHEMA_VERSION,
        id: "summary-runtime".to_string(),
        conversation_id: "conversation-1".to_string(),
        source_revision: "source-runtime".to_string(),
        previous_summary_id: None,
        covered_through: ContextJournalCursor::message("assistant-old"),
        content: "COMPACTED_HISTORY_MARKER: the old task was completed.".to_string(),
        continuity: compacted_continuity,
        generation: ContextCompactionGeneration::test(),
        source_input_tokens: 40_000,
        summary_input_tokens: 32,
        continuity_input_tokens: 64,
        replacement_input_tokens: 96,
        created_at: 1,
    };
    let mut compacted_state = create_conversation_context_state(AgentChatInput {
        context_compaction_summary: Some(compacted_summary),
        messages: vec![current_user],
        ..input.clone()
    })
    .unwrap();
    let compacted_baseline = compacted_state.shared_baseline().unwrap();
    let mut uncompacted_state = create_conversation_context_state(input.clone()).unwrap();
    let uncompacted_baseline = uncompacted_state.shared_baseline().unwrap();
    let prepare_count = Arc::new(AtomicUsize::new(0));
    let generate_count = Arc::new(AtomicUsize::new(0));
    let commit_count = Arc::new(AtomicUsize::new(0));
    let trace_publish_count = Arc::new(AtomicUsize::new(0));
    let post_compaction_empty_trace_publish_count = Arc::new(AtomicUsize::new(0));
    let prepare_counter = prepare_count.clone();
    let generate_counter = generate_count.clone();
    let commit_counter = commit_count.clone();
    let commit_count_for_trace = commit_count.clone();
    let trace_publish_counter = trace_publish_count.clone();
    let post_compaction_empty_trace_publish_counter =
        post_compaction_empty_trace_publish_count.clone();
    let compacted_baseline_for_commit = compacted_baseline.clone();
    let compacted_baseline_for_trace = compacted_baseline.clone();
    let durable_prefix_for_prepare = durable_prefix.clone();
    let services = AgentContextCompactionServices::new(
        move |request, _| {
            prepare_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(
                request.covered_through,
                ContextJournalCursor::message("assistant-old")
            );
            assert_eq!(request.visible_trace_item_count, 0);
            let durable_prefix = durable_prefix_for_prepare.clone();
            async move { Ok(AgentContextCompactionPrepareOutcome::Ready(durable_prefix)) }
        },
        move |request, _| {
            generate_counter.fetch_add(1, Ordering::SeqCst);
            async move {
                let observation =
                    crate::model_request_observation::ModelRequestObservationBuilder::new(
                        format!("model-request-{}", request.operation_id),
                        request.run_id.clone(),
                        Some(request.conversation_id.clone()),
                        Some(request.assistant_message_id.clone()),
                        Some(request.operation_id.clone()),
                        request.request_index,
                        crate::ModelRequestPurpose::ContextCompaction,
                        "test-model",
                        AgentApiStyle::OpenAiCompatible,
                        None,
                        1,
                    )
                    .completed(
                        Some(AgentUsage {
                            input_tokens: Some(100),
                            output_tokens: Some(20),
                            output_thinking_tokens: None,
                            total_tokens: Some(120),
                            cached_input_tokens: None,
                            cache_creation_input_tokens: None,
                            billable_request_count: Some(1),
                        }),
                        Some("stop".to_string()),
                        2,
                    )
                    .unwrap();
                Ok(AgentContextCompactionGenerationOutput {
                    draft: ContextCompactionSummaryDraft {
                        id: "summary-runtime".to_string(),
                        source_revision: request.prefix.source_revision.clone(),
                        content: "COMPACTED_HISTORY_MARKER: the old task was completed."
                            .to_string(),
                        continuity: request.continuity,
                        generation: ContextCompactionGeneration::test(),
                        source_input_tokens: request.source_input_tokens,
                        summary_input_tokens: 32,
                        continuity_input_tokens: 64,
                        replacement_input_tokens: 96,
                        created_at: 1,
                    },
                    observation,
                })
            }
        },
        move |request, _| {
            let baseline = compacted_baseline_for_commit.clone();
            commit_counter.fetch_add(1, Ordering::SeqCst);
            assert_eq!(request.draft.id, "summary-runtime");
            assert_eq!(request.visible_trace_item_count, 0);
            async move {
                Ok(AgentContextCompactionCommitOutcome::Applied {
                    summary_id: "summary-runtime".to_string(),
                    baseline: Box::new(baseline),
                })
            }
        },
        |_, _| async { Ok(()) },
    );
    let emitted_events = Arc::new(Mutex::new(Vec::new()));
    let emitted_events_for_callback = emitted_events.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        emitted_events_for_callback.lock().unwrap().push(event);
    });
    let observations = Arc::new(Mutex::new(Vec::new()));
    let observations_for_callback = observations.clone();
    let model_request_observer: AgentModelRequestObserver = Arc::new(move |observation| {
        observations_for_callback.lock().unwrap().push(observation);
    });
    let trace_observer: AgentConversationTraceObserver = Arc::new(move |snapshot| {
        trace_publish_counter.fetch_add(1, Ordering::SeqCst);
        if commit_count_for_trace.load(Ordering::SeqCst) > 0 && snapshot.items.is_empty() {
            post_compaction_empty_trace_publish_counter.fetch_add(1, Ordering::SeqCst);
        }
        let baseline = if commit_count_for_trace.load(Ordering::SeqCst) == 0 {
            uncompacted_baseline.clone()
        } else {
            compacted_baseline_for_trace.clone()
        };
        Ok(Some(baseline))
    });

    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-compaction".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(
                AgentRuntimeHostServices::new()
                    .with_context_compaction(services)
                    .with_trace_observer(trace_observer)
                    .with_model_request_observer(model_request_observer),
            ),
        )
        .await
        .unwrap();
    server.await.unwrap();
    let request_bodies = request_bodies
        .lock()
        .unwrap()
        .iter()
        .cloned()
        .map(|body| String::from_utf8(body).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(output.content, "done");
    assert_eq!(request_bodies.len(), 2);
    assert_eq!(prepare_count.load(Ordering::SeqCst), 1);
    assert_eq!(generate_count.load(Ordering::SeqCst), 1);
    assert_eq!(commit_count.load(Ordering::SeqCst), 1);
    // One empty trace publication must happen after commit and before the next tool call. Without
    // it, the first response promotes the stale pre-compaction baseline and the second request
    // immediately tries to compact the old history again.
    assert_eq!(
        post_compaction_empty_trace_publish_count.load(Ordering::SeqCst),
        1
    );
    assert_eq!(trace_publish_count.load(Ordering::SeqCst), 5);
    let usage = output.usage.as_ref().unwrap();
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.output_tokens, Some(20));
    assert_eq!(usage.billable_request_count, Some(3));
    let observations = observations.lock().unwrap();
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|observation| {
        observation.purpose == crate::ModelRequestPurpose::AgentLoop
            && observation.estimate.is_some()
    }));
    for request_body in request_bodies {
        assert!(request_body.contains("COMPACTED_HISTORY_MARKER"));
        assert!(!request_body.contains("OLD_USER_MARKER"));
        assert!(!request_body.contains("OLD_ASSISTANT_MARKER"));
    }
    let events = emitted_events.lock().unwrap();
    let compaction_events = events
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::ContextCompactionStarted { .. }
                    | AgentEvent::ContextCompactionFinished { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(compaction_events.len(), 2);
    let AgentEvent::ContextCompactionStarted { operation_id, .. } = compaction_events[0] else {
        panic!("first compaction event should start the operation");
    };
    let AgentEvent::ContextCompactionFinished {
        operation_id: finished_operation_id,
        outcome,
        ..
    } = compaction_events[1]
    else {
        panic!("second compaction event should finish the operation");
    };
    assert_eq!(finished_operation_id, operation_id);
    assert_eq!(*outcome, AgentContextCompactionEventOutcome::Applied);
}

#[tokio::test]
async fn context_capacity_guard_rejects_the_initial_request_before_network_io() {
    use tokio::net::TcpListener;
    use tokio::time::{timeout, Duration};

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(8_000),
        context_window_indicator_enabled: false,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: None,
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        skill_activation: None,
        skill_discovery: None,
        messages: vec![AgentChatMessage {
            message_id: None,
            role: "user".to_string(),
            content: "x".repeat(90_000),
            created_at: None,
            conversation_turn_trace: None,
        }],
    };

    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();

    assert_eq!(error.code(), Some("context_capacity_exceeded"));
    assert_eq!(
        error
            .details()
            .and_then(|details| details["status"].as_str()),
        Some("over_budget")
    );
    assert_eq!(
        error
            .details()
            .and_then(|details| details["usage"]["measurementMode"].as_str()),
        Some("incremental_cache")
    );
    assert!(timeout(Duration::from_millis(100), listener.accept())
        .await
        .is_err());
}

#[tokio::test]
async fn context_capacity_guard_accepts_budgeted_tool_results_for_the_next_request() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_http_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4_096];
        let mut expected_length = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
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
                        .unwrap_or_default();
                    expected_length = Some(header_end + 4 + content_length);
                }
            }
            if expected_length.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
    }

    async fn write_json_response(stream: &mut TcpStream, body: Value) {
        let body = serde_json::to_vec(&body).unwrap();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(response.as_bytes()).await.unwrap();
        stream.write_all(&body).await.unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let large_file = (0..2_000)
        .map(|_| "x".repeat(120))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(workspace.join("large.txt"), large_file).unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request_seen = Arc::new(AtomicBool::new(false));
    let second_request_seen_by_server = second_request_seen.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [{
                            "id": "read-large-file",
                            "type": "function",
                            "function": {
                                "name": "read_file",
                                "arguments": "{\"path\":\"large.txt\",\"maxLines\":2000}"
                            }
                        }]
                    },
                    "finish_reason": "tool_calls"
                }]
            }),
        )
        .await;

        let (mut stream, _) = tokio::time::timeout(Duration::from_secs(2), listener.accept())
            .await
            .expect("budgeted read_file result should permit a second model request")
            .unwrap();
        second_request_seen_by_server.store(true, Ordering::SeqCst);
        read_http_request(&mut stream).await;
        write_json_response(
            &mut stream,
            json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "summarized" },
                    "finish_reason": "stop"
                }]
            }),
        )
        .await;
    });
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(80_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-capacity".to_string()),
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
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: None,
        context_compaction_summary: None,
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "Read large.txt and summarize it")],
    };

    let output = AgentRuntime::default().send_chat(input).await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "summarized");
    assert!(second_request_seen.load(Ordering::SeqCst));
}

#[tokio::test]
async fn streams_write_file_previews_end_to_end_without_persisting_them() {
    use crate::protocol::{
        AgentCommandPermission, AgentPatchPermission, AgentPermissions, AgentReadPermission,
        AgentWritePermission,
    };
    use crate::storage::models::ChatConversationRecord;
    use crate::storage::service::StorageService;
    use std::time::Duration;
    use tempfile::tempdir;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    async fn read_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut expected_len = None;
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            if read == 0 {
                return;
            }
            request.extend_from_slice(&buffer[..read]);
            if expected_len.is_none() {
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
                    expected_len = Some(header_end + 4 + content_length);
                }
            }
            if expected_len.is_some_and(|length| request.len() >= length) {
                return;
            }
        }
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

    fn tool_completion(id: &str, arguments: Value) -> Value {
        json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": {
                            "name": "write_file",
                            "arguments": serde_json::to_string(&arguments).unwrap()
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        })
    }

    async fn write_append_stream(stream: &mut TcpStream, draft_id: &str) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .await
            .unwrap();
        let fragments = [
            format!(
                "{{\"phase\":\"append\",\"draftId\":\"{draft_id}\",\"index\":0,\"content\":\"line 1\\n"
            ),
            "line 2\\n".to_string(),
            "line 3\\n".to_string(),
            "line 4\\n\"}".to_string(),
        ];
        let tool_name_fragments = ["write_", "file", "", ""];
        for (fragment, tool_name) in fragments.into_iter().zip(tool_name_fragments) {
            let frame = json!({
                "choices": [{
                    "delta": {
                        "tool_calls": [{
                            "index": 0,
                            "id": "call-append",
                            "type": "function",
                            "function": {
                                "name": tool_name,
                                "arguments": fragment
                            }
                        }]
                    }
                }]
            });
            stream
                .write_all(format!("data: {frame}\n\n").as_bytes())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        stream
            .write_all(
                format!(
                    "data: {}\n\ndata: [DONE]\n\n",
                    json!({ "choices": [{ "delta": {}, "finish_reason": "tool_calls" }] })
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    }

    let fixture = tempdir().unwrap();
    let workspace = fixture.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let storage = Arc::new(StorageService::open(&fixture.path().join("app.db")).unwrap());
    storage
        .save_conversation(ChatConversationRecord {
            id: "conversation-preview".to_string(),
            project_id: None,
            model_id: None,
            title: "Preview".to_string(),
            messages: Vec::new(),
            created_at: 1,
            updated_at: 1,
            pinned_at: None,
            archived_at: None,
            unread_at: None,
        })
        .unwrap();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_storage = storage.clone();
    let server = tokio::spawn(async move {
        for request_index in 0..4 {
            let (mut stream, _) = listener.accept().await.unwrap();
            read_request(&mut stream).await;
            match request_index {
                0 => {
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-begin",
                            json!({
                                "phase": "begin",
                                "filePath": "preview.md",
                                "mode": "create"
                            }),
                        ),
                    )
                    .await;
                }
                1 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_append_stream(&mut stream, &draft_id).await;
                }
                2 => {
                    let draft_id = server_storage
                        .list_agent_file_drafts_for_run("run-preview")
                        .unwrap()[0]
                        .id
                        .clone();
                    write_json_response(
                        &mut stream,
                        tool_completion(
                            "call-abort",
                            json!({ "phase": "abort", "draftId": draft_id }),
                        ),
                    )
                    .await;
                }
                _ => {
                    write_json_response(
                        &mut stream,
                        json!({
                            "choices": [{
                                "message": { "role": "assistant", "content": "done" },
                                "finish_reason": "stop"
                            }]
                        }),
                    )
                    .await;
                }
            }
        }
    });

    let captured = Arc::new(std::sync::Mutex::new(Vec::<AgentEvent>::new()));
    let captured_for_emitter = captured.clone();
    let emitter: AgentEventEmitter = Arc::new(move |event| {
        captured_for_emitter.lock().unwrap().push(event);
    });
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
            conversation_id: Some("conversation-preview".to_string()),
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
            },
        }),
        search_config: None,
        prompt_preferences: None,
        approval_decision: None,
        tool_continuation: None,
        attachments: Vec::new(),
        resume_checkpoint: None,
        assistant_message_id: Some("assistant-preview".to_string()),
        context_compaction_summary: None,
        skill_activation: None,
        skill_discovery: None,
        messages: vec![message("user", "create a preview")],
    };
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            Some("run-preview".to_string()),
            Some(emitter),
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_storage(storage.clone())),
        )
        .await
        .unwrap();
    server.await.unwrap();

    let previews = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::FileWritePreviewUpdated { preview, .. } => Some(preview.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let context_snapshots = captured
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            AgentEvent::ContextWindowUpdated { snapshot, .. } => Some(snapshot.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let preview_additions = previews
        .iter()
        .map(|preview| preview.additions)
        .collect::<Vec<_>>();
    let mut preview_content = String::new();
    for preview in &previews {
        assert_eq!(preview.content_offset_bytes, preview_content.len() as u64);
        preview_content.push_str(&preview.content_delta);
    }
    assert!(preview_additions.len() >= 3, "{preview_additions:?}");
    assert_eq!(preview_additions.last().copied(), Some(4));
    assert_eq!(preview_content, "line 1\nline 2\nline 3\nline 4\n");
    assert!(context_snapshots.is_empty());
    assert!(output.events.iter().all(|event| !matches!(
        event,
        AgentEvent::FileWritePreviewUpdated { .. } | AgentEvent::FileWritePreviewCleared { .. }
    )));
    assert_eq!(output.content, "done");
    let append_event = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.id == "call-append" => Some(call),
            _ => None,
        })
        .expect("write_file append event");
    assert_eq!(append_event.args["content"], "[stored in private draft]");
    assert_eq!(append_event.args["contentBytes"], 28);

    let append_trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("durable conversation trace")
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id, operation, ..
            } if call_id == "call-append" => Some(operation),
            _ => None,
        })
        .expect("write_file append trace item");
    assert_eq!(append_trace["content"], "line 1\nline 2\nline 3\nline 4\n");
    assert!(append_trace.get("contentBytes").is_none());
    assert_eq!(
        storage
            .list_agent_file_drafts_for_run("run-preview")
            .unwrap()[0]
            .status,
        "aborted"
    );
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
                                )
                            ]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "I have the evidence and will prepare the report.",
                            "tool_calls": [
                                native_tool_call(
                                    "patch-approval",
                                    "apply_patch",
                                    json!({
                                        "operation": "create",
                                        "filePath": "report.txt",
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
                }),
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

    let base_input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(true),
        context: Some(AgentRunContext {
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
        skill_activation: Some(activated_skill("SKILL_SNAPSHOT_BEFORE_APPROVAL")),
        skill_discovery: None,
        messages: vec![message("user", "collect evidence and write report.txt")],
    };
    let waiting = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            base_input.clone(),
            Some("run-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(waiting.status, AgentRunStatus::WaitingForApproval);
    assert!(waiting.conversation_turn_trace.is_none());
    let checkpoint = waiting
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some(checkpoint.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(checkpoint.pending_tool_call_id, "patch-approval");
    assert_eq!(checkpoint.queued_tool_calls.len(), 1);
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

    let mut resume_input = base_input;
    resume_input.messages.clear();
    // The checkpoint is authoritative for the logical run; a changed resume payload must not
    // replace the frozen Skill snapshot selected before approval.
    resume_input.skill_activation = Some(activated_skill("SKILL_CHANGED_DURING_RESUME"));
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: "patch-approval".to_string(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: Some("Keep the evidence but revise the report first.".to_string()),
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: "patch-approval".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({ "operation": "create", "filePath": "report.txt" }),
            approval_status: AgentApprovalStatus::Rejected,
            reason: None,
        },
        result: AgentToolResult {
            call_id: "patch-approval".to_string(),
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
            None,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "resumed with evidence");
    assert_eq!(completed.todo.as_ref().unwrap().revision, 1);
    let request = final_request.lock().unwrap().take().unwrap();
    let messages = serde_json::to_string(&request["messages"]).unwrap();
    assert!(messages.contains("evidence-before-approval"));
    assert!(messages.contains("evidence-from-queued-tool"));
    assert!(messages.contains("user rejected"));
    assert!(messages.contains("read-before"));
    assert!(messages.contains("patch-approval"));
    assert!(messages.contains("read-queued"));
    assert!(messages.contains("SKILL_SNAPSHOT_BEFORE_APPROVAL"));
    assert!(!messages.contains("SKILL_CHANGED_DURING_RESUME"));
    let trace = completed.conversation_turn_trace.as_ref().unwrap();
    trace.validate().unwrap();
    for call_id in [
        "todo-before",
        "read-before",
        "patch-approval",
        "read-queued",
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
async fn skill_resource_text_is_live_for_the_model_but_omitted_from_approval_checkpoints() {
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

    let source_id = SkillSourceId::parse("installed:user").unwrap();
    let skill_id = SkillId::parse("installed:user:31234567-89ab-4def-8123-456789abcdef").unwrap();
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
                            "tool_calls": [native_tool_call(
                                "read-skill-resource",
                                "skills_read_resource",
                                json!({ "uri": resource_uri.as_str() })
                            )]
                        },
                        "finish_reason": "tool_calls"
                    }]
                }),
                1 => {
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
                                    "operation": "create",
                                    "filePath": "report.txt",
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
    let input = AgentChatInput {
        api_url: format!("http://{address}/v1/chat/completions"),
        api_token: "test-token".to_string(),
        model: "test-model".to_string(),
        model_capabilities: crate::ModelCapabilities::default(),
        api_style: Some(crate::protocol::AgentApiStyle::OpenAiCompatible),
        context_window_tokens: Some(128_000),
        context_window_indicator_enabled: true,
        max_tokens: Some(1_000),
        temperature: None,
        stream: Some(false),
        context: Some(AgentRunContext {
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
        skill_activation: Some(AgentSkillActivation {
            activation_revision: "activation-sha256-v1:resource-checkpoint".to_string(),
            skills: vec![AgentActivatedSkill {
                id: skill_id.as_str().to_string(),
                name: "resource-checkpoint".to_string(),
                revision: revision.as_str().to_string(),
                source: "installed:user".to_string(),
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
            AgentEvent::ApprovalRequired { checkpoint, .. } => Some(checkpoint),
            _ => None,
        })
        .unwrap();
    let checkpoint_json = serde_json::to_string(checkpoint).unwrap();
    assert!(!checkpoint_json.contains(RESOURCE_MARKER));
    assert!(checkpoint_json.contains("contentOmittedFromHistory"));
    assert!(!serde_json::to_string(&output.events)
        .unwrap()
        .contains(RESOURCE_MARKER));

    let mut resume_input = input;
    resume_input.messages.clear();
    resume_input.skill_activation = None;
    resume_input.resume_checkpoint = Some(checkpoint.clone());
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: "materialize-after-read".to_string(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: "materialize-after-read".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "operation": "create",
                "filePath": "report.txt",
                "content": "report"
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            call_id: "materialize-after-read".to_string(),
            tool: "apply_patch".to_string(),
            ok: true,
            result: Some(json!({ "status": "applied" })),
            error: None,
        },
    });

    let completed = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            resume_input,
            Some("run-skill-resource-checkpoint".to_string()),
            None,
            AgentCancellationToken::new(),
            Some(AgentRuntimeHostServices::new().with_skill_resources(resources)),
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(completed.status, AgentRunStatus::Completed);
    assert_eq!(completed.content, "continued after approval");
    let resumed_request = resumed_request.lock().unwrap().clone().unwrap();
    let resumed_messages = serde_json::to_string(&resumed_request["messages"]).unwrap();
    assert!(resumed_messages.contains("materialize-after-read"));
    assert!(resumed_messages.contains("applied"));
    assert!(!resumed_messages.contains(RESOURCE_MARKER));
}
