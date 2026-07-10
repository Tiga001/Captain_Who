// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::protocol::{
    AgentApprovalDecisionStatus, AgentInputAttachment, AgentInputAttachmentEncoding,
    AgentInputAttachmentKind, AgentRunContext, AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;

fn message(role: &str, content: &str) -> AgentChatMessage {
    AgentChatMessage {
        role: role.to_string(),
        content: content.to_string(),
    }
}

fn empty_attachment_context() -> AttachmentContext {
    AttachmentContext {
        text: String::new(),
        images: Vec::new(),
    }
}

#[test]
fn normalizes_supported_messages_and_skips_empty_content() {
    let messages = normalize_messages(vec![
        message(" user ", " hello "),
        message("assistant", " "),
        message("system", "rules"),
    ])
    .unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].role, "user");
    assert_eq!(messages[0].content, "hello");
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
    let messages = build_runtime_messages(
        vec![message("user", "Read src/main.rs")],
        empty_attachment_context(),
        Some(&context),
        None,
        None,
        None,
        &ToolRegistry::read_only_defaults_with_search(None).definitions(),
    )
    .unwrap();

    assert_eq!(messages[0].role.as_str(), "system");
    assert!(messages[0].content.contains("MyCopilot"));
    assert!(!messages[0].content.contains("/private/path"));
    assert_eq!(messages[1].role.as_str(), "user");
}

#[test]
fn runtime_messages_include_approval_decision_observation() {
    let decision = AgentApprovalDecision {
        action_id: "tool-1".to_string(),
        status: AgentApprovalDecisionStatus::Rejected,
        message: Some("不要运行安装命令，先说明替代方案。".to_string()),
    };
    let messages = build_runtime_messages(
        vec![message("user", "Run pnpm install")],
        empty_attachment_context(),
        None,
        None,
        Some(&decision),
        None,
        &ToolRegistry::read_only_defaults_with_search(None).definitions(),
    )
    .unwrap();

    assert!(messages
        .iter()
        .any(|message| message.content.contains("approval_decision")
            && message.content.contains("不要运行安装命令")));
}

#[test]
fn runtime_messages_include_text_attachment_content() {
    let messages = build_runtime_messages(
        vec![message("user", "Summarize this attachment")],
        AttachmentContext {
            text: "用户输入框附件内容如下。\n\n### notes.txt\nhello from attachment".to_string(),
            images: Vec::new(),
        },
        None,
        None,
        None,
        None,
        &ToolRegistry::read_only_defaults_with_search(None).definitions(),
    )
    .unwrap();

    assert!(messages
        .iter()
        .any(|message| message.role == LlmMessageRole::User
            && message.content.contains("hello from attachment")));
}

#[test]
fn runtime_messages_resume_with_native_tool_call_and_result() {
    let continuation = AgentToolContinuation {
        call: AgentToolCall {
            id: "patch-1".to_string(),
            tool: "apply_patch".to_string(),
            args: json!({
                "operation": "update",
                "filePath": "src/main.rs",
                "edits": [{ "kind": "append", "text": "\nfn test() {}\n" }]
            }),
            approval_status: AgentApprovalStatus::Approved,
            reason: None,
        },
        result: AgentToolResult {
            call_id: "patch-1".to_string(),
            tool: "apply_patch".to_string(),
            ok: false,
            result: None,
            error: Some("stale_file".to_string()),
        },
    };
    let messages = build_runtime_messages(
        vec![message("user", "Edit src/main.rs")],
        empty_attachment_context(),
        None,
        None,
        None,
        Some(&continuation),
        &ToolRegistry::read_only_defaults_with_search(None).definitions(),
    )
    .unwrap();

    let assistant = messages
        .iter()
        .find(|message| !message.tool_calls.is_empty())
        .unwrap();
    assert_eq!(assistant.role, LlmMessageRole::Assistant);
    assert_eq!(assistant.tool_calls[0].id, "patch-1");
    let result = messages
        .iter()
        .find(|message| message.role == LlmMessageRole::Tool)
        .unwrap();
    assert_eq!(result.tool_call_id.as_deref(), Some("patch-1"));
    assert!(result.is_error);
    assert!(result.content.contains("stale_file"));
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
            "faviconDataUrl": "data:image/x-icon;base64,AAAB",
            "sourceUrl": "https://example.com/icon.ico",
            "nested": {
                "previewDataUrl": "data:image/png;base64,BBBB",
                "items": [
                    "data:image/jpeg;base64,CCCC",
                    "plain text"
                ]
            },
            "image": {
                "mimeType": "image/png",
                "dataBase64": "YWJj"
            }
        })),
        error: None,
    };

    let redacted = redact_tool_result_for_event(&result);
    assert_eq!(
        redacted.result.as_ref().unwrap()["image"]["dataBase64"],
        "[redacted]"
    );
    assert_eq!(
        redacted.result.as_ref().unwrap()["faviconDataUrl"],
        "[redacted]"
    );
    assert_eq!(
        redacted.result.as_ref().unwrap()["nested"]["previewDataUrl"],
        "[redacted]"
    );
    assert_eq!(
        redacted.result.as_ref().unwrap()["nested"]["items"][0],
        "[redacted]"
    );
    assert_eq!(
        redacted.result.as_ref().unwrap()["sourceUrl"],
        "https://example.com/icon.ico"
    );

    let image_message = llm_image_message_from_tool_result(&result).unwrap();
    assert_eq!(image_message.role, LlmMessageRole::User);
    assert_eq!(image_message.images.len(), 1);
    assert_eq!(image_message.images[0].mime_type, "image/png");
    assert_eq!(image_message.images[0].data_base64, "YWJj");
}

#[test]
fn rejects_unknown_message_roles() {
    let error = normalize_messages(vec![message("tool", "result")]).unwrap_err();
    assert!(error.to_string().contains("不支持的消息角色"));
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
