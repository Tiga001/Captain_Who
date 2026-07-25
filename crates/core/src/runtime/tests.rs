// Tests for runtime message construction and tool-flow helpers.
use super::*;
use crate::conversation_trace::canonical_tool_result_for_context;
use crate::llm::LlmToolCall;
use crate::protocol::{
    AgentActivatedSkill, AgentAttachmentLibraryContext, AgentAttachmentReference,
    AgentInputAttachment, AgentInputAttachmentEncoding, AgentInputAttachmentKind,
    AgentPatchPermission, AgentRunContext, AgentSkillActivation, AgentWorkspaceContext,
};
use crate::runtime::tool_flow::parse_tool_call_request;
use crate::tools::{EffectiveToolSet, ToolCapabilityId, OFFICE_DOCUMENTS_CAPABILITY};
use crate::{
    ConversationTraceToolResultStatus, ConversationTurnTrace, ConversationTurnTraceItem,
    ConversationTurnTraceTerminalStatus, CONVERSATION_TURN_TRACE_SCHEMA_VERSION,
};
use base64::Engine;
use std::collections::BTreeSet;

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

fn assert_runtime_owned_tool_call_id(id: &str) {
    assert!(id.starts_with("tc1_"), "unexpected tool-call ID: {id}");
    assert_eq!(id.len(), 47, "unexpected canonical ID length: {id}");
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-')),
        "tool-call ID contains provider-unsafe characters: {id}"
    );
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
            revision: activated_skill_revision(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: None,
        }],
    }
}

fn activated_skill_revision() -> String {
    format!("skill-package-sha256-v3:{}", "d".repeat(64))
}

fn activated_skill_authority() -> Arc<crate::skills::SkillResourceSession> {
    let skill_id = crate::skills::SkillId::parse("workspace:workspace-1:review").unwrap();
    let source_id = skill_id.source_id().clone();
    Arc::new(
        crate::skills::memory_resource_session_for_test(
            skill_id,
            crate::skills::SkillRevision::parse(activated_skill_revision()).unwrap(),
            source_id,
            Vec::new(),
        )
        .unwrap(),
    )
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
            inputs: Vec::new(),
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
    let context = build_attachment_context(
        &[AgentInputAttachment {
            id: "attachment-1".to_string(),
            kind: AgentInputAttachmentKind::File,
            name: "notes.txt".to_string(),
            mime_type: Some("text/plain".to_string()),
            size_bytes: 16,
            encoding: AgentInputAttachmentEncoding::Utf8,
            data: "hello from file".to_string(),
            truncated: None,
        }],
        None,
    )
    .unwrap();

    assert!(context.text.contains("读取工具：read_file"));
    assert!(context.text.contains("hello from file"));
}

fn runtime_test_zip(entries: &[(&str, &str)]) -> Vec<u8> {
    use std::io::Write;

    let mut output = std::io::Cursor::new(Vec::new());
    {
        let mut archive = zip::ZipWriter::new(&mut output);
        for (path, content) in entries {
            archive
                .start_file(*path, zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(content.as_bytes()).unwrap();
        }
        archive.finish().unwrap();
    }
    output.into_inner()
}

fn runtime_test_pdf(text: &str) -> Vec<u8> {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({text}) Tj ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /Resources << /Font << /F1 4 0 R >> >> /MediaBox [0 0 612 792] /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{stream}\nendstream", stream.len()),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0_usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets.into_iter().skip(1) {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn runtime_binary_attachment(
    id: &str,
    name: &str,
    mime_type: &str,
    bytes: Vec<u8>,
) -> AgentInputAttachment {
    AgentInputAttachment {
        id: id.to_string(),
        kind: AgentInputAttachmentKind::File,
        name: name.to_string(),
        mime_type: Some(mime_type.to_string()),
        size_bytes: bytes.len() as u64,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
        truncated: None,
    }
}

fn runtime_attachment_library(
    attachments: &[AgentInputAttachment],
) -> AgentAttachmentLibraryContext {
    AgentAttachmentLibraryContext {
        root_path: None,
        conversation_id: Some("conversation-attachments".to_string()),
        project_id: None,
        conversation_attachments: attachments
            .iter()
            .map(|attachment| AgentAttachmentReference {
                id: attachment.id.clone(),
                conversation_id: "conversation-attachments".to_string(),
                message_id: "message-attachments".to_string(),
                project_id: None,
                kind: attachment.kind,
                name: attachment.name.clone(),
                mime_type: attachment.mime_type.clone(),
                size_bytes: attachment.size_bytes,
                read_path: format!("@attachments/{}/{}", attachment.id, attachment.name),
                storage_rel_path: format!(
                    "conversations/conversation-attachments/message-attachments/{}/{}",
                    attachment.id, attachment.name
                ),
                created_at: 1,
            })
            .collect(),
        project_attachments: Vec::new(),
    }
}

#[test]
fn steer_attachment_library_updates_runtime_overlay_without_granting_conversation_identity() {
    let attachment =
        runtime_binary_attachment("steer-image", "steer.png", "image/png", vec![1, 2, 3]);
    let library = runtime_attachment_library(&[attachment]);
    let mut run_context = None;
    let mut tool_context = ToolExecutionContext::from_run_context(None);

    replace_runtime_attachment_library(&mut run_context, &mut tool_context, library.clone());

    let run_context = run_context.expect("steer library should create runtime projection");
    assert_eq!(run_context.conversation_id, None);
    assert_eq!(run_context.project_id, None);
    assert_eq!(run_context.attachment_library, Some(library));
    let overlay = crate::prompts::build_runtime_context_overlay(Some(&run_context));
    assert!(overlay.contains("\"attachmentLibraryAvailable\":true"));
    assert!(overlay.contains("\"conversationAttachmentCount\":1"));
    assert!(overlay.contains("\"conversationAvailable\":false"));
}

#[test]
fn attachment_context_extracts_stable_pdf_but_defers_skill_gated_office_files() {
    let attachments = vec![
        runtime_binary_attachment(
            "attachment-pdf",
            "paper.pdf",
            "application/pdf",
            runtime_test_pdf("Guidance PDF"),
        ),
        runtime_binary_attachment(
            "attachment-docx",
            "document.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            runtime_test_zip(&[(
                "word/document.xml",
                r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Guidance DOCX</w:t></w:r></w:p></w:body></w:document>"#,
            )]),
        ),
        runtime_binary_attachment(
            "attachment-pptx",
            "slides.pptx",
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            runtime_test_zip(&[(
                "ppt/slides/slide1.xml",
                r#"<p:sld xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><a:t xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">Guidance PPTX</a:t></p:spTree></p:cSld></p:sld>"#,
            )]),
        ),
        runtime_binary_attachment(
            "attachment-xlsx",
            "table.xlsx",
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            runtime_test_zip(&[
                (
                    "xl/sharedStrings.xml",
                    r#"<sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><si><t>Guidance XLSX</t></si></sst>"#,
                ),
                (
                    "xl/worksheets/sheet1.xml",
                    r#"<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="s"><v>0</v></c></row></sheetData></worksheet>"#,
                ),
            ]),
        ),
        runtime_binary_attachment(
            "attachment-csv",
            "table.csv",
            "text/csv",
            b"name,value\nGuidance CSV,1\n".to_vec(),
        ),
        runtime_binary_attachment(
            "attachment-tsv",
            "table.tsv",
            "text/tab-separated-values",
            b"name\tvalue\nGuidance TSV\t1\n".to_vec(),
        ),
        runtime_binary_attachment(
            "attachment-csv-mime",
            "renamed-table.txt",
            "text/csv",
            b"name,value\nGuidance MIME CSV,1\n".to_vec(),
        ),
    ];

    let library = runtime_attachment_library(&attachments);
    let context = build_attachment_context(&attachments, Some(&library)).unwrap();

    assert!(context.text.contains("Guidance PDF"));
    for hidden_office_content in [
        "Guidance DOCX",
        "Guidance PPTX",
        "Guidance XLSX",
        "Guidance CSV",
        "Guidance TSV",
        "Guidance MIME CSV",
    ] {
        assert!(
            !context.text.contains(hidden_office_content),
            "skill-gated Office content leaked during attachment preprocessing: {hidden_office_content}\n{}",
            context.text
        );
    }
    for attachment_id in [
        "attachment-docx",
        "attachment-pptx",
        "attachment-xlsx",
        "attachment-csv",
        "attachment-tsv",
        "attachment-csv-mime",
    ] {
        assert!(
            context
                .text
                .contains(&format!("@attachments/{attachment_id}/")),
            "missing authoritative readPath for {attachment_id}\n{}",
            context.text
        );
    }
    assert_eq!(
        context
            .text
            .matches("正文未读取；先激活匹配该文件类型的 Skill")
            .count(),
        6
    );
    for hidden_contract_detail in [
        "read_word",
        "read_presentation",
        "read_spreadsheet",
        "office.documents",
        "office.presentations",
        "office.spreadsheets",
    ] {
        assert!(
            !context.text.contains(hidden_contract_detail),
            "dynamic Tool contract leaked before Skill activation: {hidden_contract_detail}"
        );
    }
}

#[test]
fn attachment_context_keeps_image_visual_input_and_authoritative_read_path() {
    let attachment = AgentInputAttachment {
        id: "attachment-image".to_string(),
        kind: AgentInputAttachmentKind::Image,
        name: "pixel.png".to_string(),
        mime_type: Some("image/png".to_string()),
        size_bytes: 3,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: base64::engine::general_purpose::STANDARD.encode(b"png"),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert_eq!(context.images.len(), 1);
    assert!(context.text.contains("已作为视觉输入发送给模型"));
    assert!(context
        .text
        .contains("@attachments/attachment-image/pixel.png"));
}

#[test]
fn attachment_context_does_not_decode_skill_gated_office_payloads() {
    let attachment = AgentInputAttachment {
        id: "attachment-docx-invalid-payload".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "document.docx".to_string(),
        mime_type: Some(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        ),
        size_bytes: 10,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "not-base64".to_string(),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert!(context.text.contains("正文未读取"));
    assert!(context
        .text
        .contains("@attachments/attachment-docx-invalid-payload/document.docx"));
}

#[test]
fn activated_document_reader_can_read_the_same_authoritative_attachment_path() {
    let fixture = tempfile::tempdir().unwrap();
    let storage_rel_path = "conversations/conversation-1/message-1/attachment-docx/document.docx";
    let attachment_path = fixture.path().join(storage_rel_path);
    std::fs::create_dir_all(attachment_path.parent().unwrap()).unwrap();
    let bytes = runtime_test_zip(&[(
        "word/document.xml",
        r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Visible after activation</w:t></w:r></w:p></w:body></w:document>"#,
    )]);
    std::fs::write(&attachment_path, &bytes).unwrap();
    let read_path = "@attachments/attachment-docx/document.docx";
    let library = AgentAttachmentLibraryContext {
        root_path: Some(fixture.path().to_string_lossy().to_string()),
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        conversation_attachments: vec![AgentAttachmentReference {
            id: "attachment-docx".to_string(),
            conversation_id: "conversation-1".to_string(),
            message_id: "message-1".to_string(),
            project_id: None,
            kind: AgentInputAttachmentKind::File,
            name: "document.docx".to_string(),
            mime_type: Some(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                    .to_string(),
            ),
            size_bytes: u64::try_from(bytes.len()).unwrap(),
            read_path: read_path.to_string(),
            storage_rel_path: storage_rel_path.to_string(),
            created_at: 1,
        }],
        project_attachments: Vec::new(),
    };
    let registry = ToolRegistry::defaults_with_search(None);
    let active_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    let effective_tool_set = EffectiveToolSet::from_permitted_definitions(
        &registry,
        registry.definitions(),
        &active_capabilities,
    )
    .unwrap();
    assert!(effective_tool_set.contains("read_word"));

    let tool_context = ToolExecutionContext::from_run_context(Some(&AgentRunContext {
        conversation_id: Some("conversation-1".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: Some(library),
        permissions: Default::default(),
    }));
    let result = registry.execute(
        &tool_context,
        &AgentToolCall {
            id: "call-read-word".to_string(),
            tool: "read_word".to_string(),
            args: json!({ "path": read_path }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: Some("读取已激活文档技能可访问的附件".to_string()),
        },
    );

    assert!(result.ok, "{:?}", result.error);
    assert!(result
        .result
        .as_ref()
        .and_then(|value| value.get("text"))
        .and_then(Value::as_str)
        .is_some_and(|text| text.contains("Visible after activation")));
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

async fn read_runtime_test_json_request(stream: &mut tokio::net::TcpStream) -> Value {
    use tokio::io::AsyncReadExt;

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
    serde_json::from_slice(&request[body_start.unwrap()..expected_length.expect("content length")])
        .unwrap()
}

async fn write_runtime_test_json_response(stream: &mut tokio::net::TcpStream, body: Value) {
    use tokio::io::AsyncWriteExt;

    let body = serde_json::to_vec(&body).unwrap();
    let headers = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    stream.write_all(&body).await.unwrap();
}

fn runtime_steer_input(
    guidance_id: &str,
    client_message_id: &str,
    content: &str,
) -> crate::AgentSteerInput {
    crate::AgentSteerInput {
        guidance_id: guidance_id.to_string(),
        client_message_id: client_message_id.to_string(),
        content: content.to_string(),
        attachments: Vec::new(),
        attachment_library: None,
        created_at: 42,
    }
}

#[tokio::test]
async fn concurrent_steer_during_sampling_is_fifo_and_turns_a_terminal_response_into_narration() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            }
            let content = if request_index == 0 {
                "Initial answer before guidance."
            } else {
                "Final answer after guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-before-stream",
                "client-before-stream",
                "Include the pre-stream constraint."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let mut input = conversation_context_input(vec![message("user", "Start the task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-steer".to_string());
    input.context = Some(AgentRunContext {
        conversation_id: Some("conversation-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-1",
                "client-1",
                "Also include the migration risk."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-2",
                "client-2",
                "Keep the rollout steps in chronological order."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final answer after guidance.");
    let requests = requests.lock().unwrap();
    let second_messages = requests[1]["messages"].as_array().unwrap();
    let intermediate_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "assistant"
                && message["content"] == "Initial answer before guidance."
        })
        .unwrap();
    let before_stream_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Include the pre-stream constraint."
        })
        .unwrap();
    let first_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user" && message["content"] == "Also include the migration risk."
        })
        .unwrap();
    let second_guidance_index = second_messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "Keep the rollout steps in chronological order."
        })
        .unwrap();
    assert!(intermediate_index < before_stream_guidance_index);
    assert!(before_stream_guidance_index < first_guidance_index);
    assert!(first_guidance_index < second_guidance_index);
    drop(requests);

    let trace = output.conversation_turn_trace.as_ref().unwrap();
    assert!(matches!(
        &trace.items[..],
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id,
                client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: second_guidance_id,
                client_message_id: second_client_message_id,
                ..
            },
            ConversationTurnTraceItem::UserGuidance {
                guidance_id: third_guidance_id,
                client_message_id: third_client_message_id,
                ..
            }
        ] if content == "Initial answer before guidance."
            && guidance_id == "guidance-before-stream"
            && client_message_id == "client-before-stream"
            && second_guidance_id == "guidance-1"
            && second_client_message_id == "client-1"
            && third_guidance_id == "guidance-2"
            && third_client_message_id == "client-2"
    ));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 1,
            ..
        } if guidance_id == "guidance-before-stream"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 2,
            ..
        } if guidance_id == "guidance-1"
    )));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied {
            guidance_id,
            sequence: 3,
            ..
        } if guidance_id == "guidance-2"
    )));
    assert!(!queue.is_accepting());
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-late",
                "client-late",
                "too late"
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Closed
    );
}

#[tokio::test]
async fn steer_accepted_during_transport_retry_is_applied_after_the_retried_response() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
    let requests_for_server = Arc::clone(&requests);
    let (retry_started_tx, retry_started_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut retry_started_tx = Some(retry_started_tx);
        for connection_index in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            requests_for_server.lock().unwrap().push(request);
            if connection_index == 0 {
                let body = b"temporary upstream failure";
                stream
                    .write_all(
                        format!(
                            "HTTP/1.1 503 Service Unavailable\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
                stream.write_all(body).await.unwrap();
                retry_started_tx.take().unwrap().send(()).unwrap();
                continue;
            }
            let content = if connection_index == 1 {
                "Response after retry."
            } else {
                "Final response after retry guidance."
            };
            write_runtime_test_json_response(
                &mut stream,
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": content },
                        "finish_reason": "stop"
                    }]
                }),
            )
            .await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "Start the retry task.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-retry-steer".to_string());
    input.context = Some(AgentRunContext {
        conversation_id: Some("conversation-retry-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-retry-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    retry_started_rx.await.unwrap();
    assert_eq!(
        queue
            .enqueue(runtime_steer_input(
                "guidance-retry",
                "client-retry",
                "Apply this only after the retry response."
            ))
            .unwrap(),
        AgentSteerEnqueueOutcome::Queued
    );
    let output = runtime.await.unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "Final response after retry guidance.");
    let requests = requests.lock().unwrap();
    assert_eq!(requests.len(), 3);
    for request in requests.iter() {
        let serialized = serde_json::to_string(request).unwrap();
        assert!(serialized.contains("<backend_runtime_context>"));
        assert!(serialized.contains("\\\"write\\\":\\\"denied\\\""));
    }
    assert!(!serde_json::to_string(&requests[1])
        .unwrap()
        .contains("Apply this only after the retry response."));
    assert!(serde_json::to_string(&requests[2])
        .unwrap()
        .contains("Apply this only after the retry response."));
    let trace = output.conversation_turn_trace.unwrap();
    assert!(matches!(
        trace.items.as_slice(),
        [
            ConversationTurnTraceItem::AssistantNarration { content, .. },
            ConversationTurnTraceItem::UserGuidance { guidance_id, .. }
        ] if content == "Response after retry." && guidance_id == "guidance-retry"
    ));
}

#[tokio::test]
async fn steer_waits_until_a_complete_multi_tool_exchange_before_next_sampling() {
    use tokio::net::TcpListener;
    use tokio::sync::oneshot;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let second_request = Arc::new(Mutex::new(None::<Value>));
    let second_request_for_server = Arc::clone(&second_request);
    let (first_request_seen_tx, first_request_seen_rx) = oneshot::channel();
    let (release_first_response_tx, release_first_response_rx) = oneshot::channel();
    let server = tokio::spawn(async move {
        let mut first_request_seen_tx = Some(first_request_seen_tx);
        let mut release_first_response_rx = Some(release_first_response_rx);
        for request_index in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let request = read_runtime_test_json_request(&mut stream).await;
            if request_index == 0 {
                first_request_seen_tx.take().unwrap().send(()).unwrap();
                release_first_response_rx.take().unwrap().await.unwrap();
            } else {
                *second_request_for_server.lock().unwrap() = Some(request);
            }
            let response = if request_index == 0 {
                json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "tool_calls": [{
                                "id": "provider-call-1",
                                "type": "function",
                                "function": {
                                    "name": "attachments_list",
                                    "arguments": "{}"
                                }
                            }, {
                                "id": "provider-call-2",
                                "type": "function",
                                "function": {
                                    "name": "todo_update",
                                    "arguments": "{\"items\":[{\"title\":\"Verify ordering\",\"status\":\"completed\"}]}"
                                }
                            }]
                        },
                        "finish_reason": "tool_calls"
                    }]
                })
            } else {
                json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "Done after the tool." },
                        "finish_reason": "stop"
                    }]
                })
            };
            write_runtime_test_json_response(&mut stream, response).await;
        }
    });

    let queue = AgentSteerInputQueue::new();
    let mut input = conversation_context_input(vec![message("user", "List attachments.")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    input.assistant_message_id = Some("assistant-tool-steer".to_string());
    input.context = Some(AgentRunContext {
        conversation_id: Some("conversation-tool-steer".to_string()),
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: Default::default(),
    });
    let runtime_queue = queue.clone();
    let runtime = tokio::spawn(async move {
        AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-tool-steer".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_steer_input(runtime_queue)),
            )
            .await
            .unwrap()
    });

    first_request_seen_rx.await.unwrap();
    queue
        .enqueue(runtime_steer_input(
            "guidance-tool",
            "client-tool",
            "After the tool, summarize the count.",
        ))
        .unwrap();
    release_first_response_tx.send(()).unwrap();
    let output = runtime.await.unwrap();
    server.await.unwrap();

    let second_request = second_request.lock().unwrap().take().unwrap();
    let messages = second_request["messages"].as_array().unwrap();
    let tool_call_index = messages
        .iter()
        .position(|message| message["role"] == "assistant" && message["tool_calls"].is_array())
        .unwrap();
    let tool_result_indices = messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| (message["role"] == "tool").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(tool_result_indices.len(), 2);
    let guidance_index = messages
        .iter()
        .position(|message| {
            message["role"] == "user"
                && message["content"] == "After the tool, summarize the count."
        })
        .unwrap();
    assert!(tool_result_indices
        .iter()
        .all(|tool_result_index| tool_call_index < *tool_result_index
            && *tool_result_index < guidance_index));

    let trace = output.conversation_turn_trace.unwrap();
    trace.validate().unwrap();
    assert!(matches!(
        trace.items.last(),
        Some(ConversationTurnTraceItem::UserGuidance {
            guidance_id,
            ..
        }) if guidance_id == "guidance-tool"
    ));
    let tool_call_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolCall { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    let tool_result_indices = trace
        .items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            matches!(item, ConversationTurnTraceItem::ToolResult { .. }).then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_call_indices.len(), 2);
    assert_eq!(tool_result_indices.len(), 2);
    let exchange_start = *tool_call_indices.first().unwrap();
    let exchange_end = *tool_result_indices.last().unwrap();
    assert!(trace.items[exchange_start..=exchange_end]
        .iter()
        .all(|item| !matches!(item, ConversationTurnTraceItem::UserGuidance { .. })));
    assert!(exchange_end < trace.items.len() - 1);
}

#[tokio::test]
async fn empty_normal_completion_is_repaired_once_for_openai_and_anthropic() {
    use crate::model_request_observation::ModelRequestObservationStatus;
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
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(Vec::<Value>::new()));
        let captured_for_server = Arc::clone(&captured);
        let server = tokio::spawn(async move {
            for request_index in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let request = read_json_request(&mut stream).await;
                captured_for_server.lock().unwrap().push(request);
                let response = match (style, request_index) {
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, 0) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 11,
                            "completion_tokens": 3,
                            "total_tokens": 14
                        }
                    }),
                    (crate::protocol::AgentApiStyle::OpenAiCompatible, _) => json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "recovered response" },
                            "finish_reason": "stop"
                        }],
                        "usage": {
                            "prompt_tokens": 12,
                            "completion_tokens": 4,
                            "total_tokens": 16
                        }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, 0) => json!({
                        "content": [],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 11, "output_tokens": 3 }
                    }),
                    (crate::protocol::AgentApiStyle::AnthropicCompatible, _) => json!({
                        "content": [{ "type": "text", "text": "recovered response" }],
                        "stop_reason": "end_turn",
                        "usage": { "input_tokens": 12, "output_tokens": 4 }
                    }),
                };
                write_json_response(&mut stream, response).await;
            }
        });

        let observations = Arc::new(Mutex::new(Vec::new()));
        let observations_for_host = Arc::clone(&observations);
        let observer: AgentModelRequestObserver = Arc::new(move |observation| {
            observations_for_host.lock().unwrap().push(observation);
        });
        let mut input = conversation_context_input(vec![message("user", "complete the task")]);
        input.api_url = format!("http://{address}/v1/messages");
        input.api_token = "test-token".to_string();
        input.api_style = Some(style);
        input.stream = Some(false);
        input.assistant_message_id = Some("assistant-empty-repair".to_string());
        input.context = Some(AgentRunContext {
            conversation_id: Some("conversation-empty-repair".to_string()),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: Default::default(),
        });

        let output = AgentRuntime::default()
            .send_chat_with_events_and_cancellation(
                input,
                Some("run-empty-repair".to_string()),
                None,
                AgentCancellationToken::new(),
                Some(AgentRuntimeHostServices::new().with_model_request_observer(observer)),
            )
            .await
            .unwrap();
        server.await.unwrap();

        assert_eq!(output.content, "recovered response");
        assert_eq!(
            output
                .usage
                .as_ref()
                .and_then(|usage| usage.billable_request_count),
            Some(2)
        );
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        let first = serde_json::to_string(&requests[0]).unwrap();
        let second = serde_json::to_string(&requests[1]).unwrap();
        assert!(!first.contains("preceding model response ended normally"));
        assert!(second.contains("preceding model response ended normally"));
        match style {
            crate::protocol::AgentApiStyle::OpenAiCompatible => {
                assert!(requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| {
                        message["role"] == "system"
                            && message["content"]
                                .as_str()
                                .is_some_and(|content| content.contains("do not repeat"))
                    }));
            }
            crate::protocol::AgentApiStyle::AnthropicCompatible => {
                assert!(requests[1]["system"]
                    .as_str()
                    .is_some_and(|system| system.contains("do not repeat")));
                assert!(!requests[1]["messages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|message| message["role"] == "system"));
            }
        }
        drop(requests);

        let observations = observations.lock().unwrap();
        assert_eq!(observations.len(), 2);
        assert_eq!(observations[0].request_index, 1);
        assert_eq!(
            observations[0].status,
            ModelRequestObservationStatus::Failed
        );
        assert_eq!(
            observations[0].error_code.as_deref(),
            Some("agent.empty_model_action")
        );
        assert_eq!(observations[1].request_index, 2);
        assert_eq!(
            observations[1].status,
            ModelRequestObservationStatus::Completed
        );
        let trace = serde_json::to_string(
            output
                .conversation_turn_trace
                .as_ref()
                .expect("completed trace"),
        )
        .unwrap();
        assert!(!trace.contains("preceding model response ended normally"));
    }

    run_case(crate::protocol::AgentApiStyle::OpenAiCompatible).await;
    run_case(crate::protocol::AgentApiStyle::AnthropicCompatible).await;
}

#[tokio::test]
async fn empty_model_action_repair_stops_after_the_second_empty_response() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_server = Arc::clone(&request_count);
    let server = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2_048];
            let expected_length = loop {
                let read = stream.read(&mut buffer).await.unwrap();
                assert!(read > 0);
                request.extend_from_slice(&buffer[..read]);
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
                    let expected = header_end + 4 + content_length;
                    if request.len() >= expected {
                        break expected;
                    }
                }
            };
            assert!(request.len() >= expected_length);
            request_count_for_server.fetch_add(1, Ordering::SeqCst);
            let body = serde_json::to_vec(&json!({
                "choices": [{
                    "message": { "role": "assistant", "content": "" },
                    "finish_reason": "stop"
                }],
                "usage": {
                    "prompt_tokens": 5,
                    "completion_tokens": 1,
                    "total_tokens": 6
                }
            }))
            .unwrap();
            let headers = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(headers.as_bytes()).await.unwrap();
            stream.write_all(&body).await.unwrap();
        }
    });

    let mut input = conversation_context_input(vec![message("user", "complete the task")]);
    input.api_url = format!("http://{address}/v1/chat/completions");
    input.api_token = "test-token".to_string();
    input.stream = Some(false);
    let error = AgentRuntime::default().send_chat(input).await.unwrap_err();
    server.await.unwrap();

    assert_eq!(error.code(), Some("agent.empty_model_action"));
    assert_eq!(
        error.usage().and_then(|usage| usage.billable_request_count),
        Some(2)
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 2);
}

#[test]
fn runtime_command_definition_is_fixed_while_dispatch_uses_current_permissions() {
    let definitions = [
        AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::AutoApprove,
        },
        AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        },
    ]
    .into_iter()
    .map(|permissions| {
        let mut input = conversation_context_input(vec![message("user", "run a command")]);
        input.context = Some(AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions,
        });
        let capabilities =
            prepare_runtime_capabilities(&input, "command-definition", &[], true, None).unwrap();
        capabilities
            .initial_tool_set
            .stable_definitions()
            .iter()
            .find(|definition| definition.name == "run_command")
            .cloned()
            .expect("stable run_command definition")
    })
    .collect::<Vec<_>>();

    assert!(definitions[0].requires_approval);
    assert_eq!(
        definitions[0].approval_mode,
        crate::protocol::AgentToolApprovalMode::Always
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[1]).unwrap()
    );
    assert_eq!(
        serde_json::to_value(&definitions[0]).unwrap(),
        serde_json::to_value(&definitions[2]).unwrap()
    );
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
fn run_context_changes_only_the_dynamic_overlay_while_prompt_preferences_change_configuration() {
    let mut baseline = conversation_context_input(vec![message("user", "Inspect the project")]);
    baseline.context = Some(AgentRunContext {
        conversation_id: None,
        project_id: None,
        workspace: None,
        attachment_library: None,
        permissions: AgentPermissions {
            read: AgentReadPermission::WorkspaceOnly,
            write: AgentWritePermission::WorkspaceOnly,
            command: AgentCommandPermission::RequireApproval,
            command_safety: AgentCommandSafetyPolicy::Guarded,
            patch: AgentPatchPermission::RequireApproval,
        },
    });
    let baseline_revision = conversation_context_configuration_revision(&baseline).unwrap();

    let mut changed_runtime = baseline.clone();
    changed_runtime.context = Some(AgentRunContext {
        conversation_id: Some("conversation-private".to_string()),
        project_id: Some("project-private".to_string()),
        workspace: Some(AgentWorkspaceContext {
            project_id: Some("project-private".to_string()),
            display_name: Some("Runtime Workspace".to_string()),
            root_path: Some("/Users/example/runtime-workspace".to_string()),
        }),
        attachment_library: Some(AgentAttachmentLibraryContext {
            root_path: Some("/Users/example/runtime-attachments".to_string()),
            conversation_id: Some("conversation-private".to_string()),
            project_id: Some("project-private".to_string()),
            conversation_attachments: Vec::new(),
            project_attachments: Vec::new(),
        }),
        permissions: AgentPermissions {
            read: AgentReadPermission::All,
            write: AgentWritePermission::All,
            command: AgentCommandPermission::AutoApprove,
            command_safety: AgentCommandSafetyPolicy::FullAccess,
            patch: AgentPatchPermission::AutoApprove,
        },
    });
    assert_eq!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let mut cached_state = create_conversation_context_state(baseline.clone()).unwrap();
    let baseline_snapshot = cached_state
        .snapshot_with_run_overlays(
            AgentContextWindowPhase::Idle,
            baseline.context.as_ref(),
            None,
            None,
        )
        .unwrap();
    let changed_snapshot = cached_state
        .snapshot_with_run_overlays(
            AgentContextWindowPhase::Idle,
            changed_runtime.context.as_ref(),
            None,
            None,
        )
        .unwrap();
    assert_eq!(
        baseline_snapshot.persistent_revision,
        changed_snapshot.persistent_revision
    );
    assert_eq!(baseline_snapshot.run_transient_input_tokens, 0);
    assert_eq!(changed_snapshot.run_transient_input_tokens, 0);
    assert_ne!(
        baseline_snapshot.request_input_tokens,
        changed_snapshot.request_input_tokens
    );

    changed_runtime.prompt_preferences = Some(AgentPromptPreferences {
        work_mode: Some(crate::protocol::AgentPromptWorkMode::General),
        tone: Some(crate::protocol::AgentPromptTone::Friendly),
        detail_level: None,
        custom_instructions: Some("Use concise domain terminology.".to_string()),
        updated_at: Some(10),
    });
    assert_ne!(
        baseline_revision,
        conversation_context_configuration_revision(&changed_runtime).unwrap()
    );

    let mut timestamp_only_change = changed_runtime.clone();
    timestamp_only_change
        .prompt_preferences
        .as_mut()
        .expect("prompt preferences")
        .updated_at = Some(11);
    assert_eq!(
        conversation_context_configuration_revision(&changed_runtime).unwrap(),
        conversation_context_configuration_revision(&timestamp_only_change).unwrap(),
        "presentation-only settings timestamps must not open a new configuration epoch"
    );
}

#[test]
fn settings_capability_changes_open_a_stable_epoch_but_secret_rotation_does_not() {
    let input_with_search = |mode, key: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "Find current evidence")]);
        input.search_config = Some(crate::protocol::AgentSearchConfig {
            mode,
            tavily_api_key: key.map(str::to_string),
        });
        input
    };
    let disabled_input = input_with_search(
        crate::protocol::AgentSearchMode::Disabled,
        Some("tvly-disabled"),
    );
    let enabled_a_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-a"),
    );
    let enabled_b_input = input_with_search(
        crate::protocol::AgentSearchMode::Tavily,
        Some("tvly-secret-b"),
    );

    let prepare = |input: &AgentChatInput, host_actions_available| {
        prepare_runtime_capabilities(
            input,
            "settings-capability-epoch",
            &[],
            host_actions_available,
            None,
        )
        .unwrap()
    };
    let disabled = prepare(&disabled_input, true);
    let enabled_a = prepare(&enabled_a_input, true);
    let enabled_b = prepare(&enabled_b_input, false);

    assert!(!disabled.initial_tool_set.contains("web_search"));
    assert!(!disabled.initial_tool_set.contains("web_fetch"));
    assert!(enabled_a.initial_tool_set.contains("web_search"));
    assert!(enabled_a.initial_tool_set.contains("web_fetch"));
    assert_ne!(
        disabled.initial_tool_set.stable_revision(),
        enabled_a.initial_tool_set.stable_revision(),
        "enabling a model-visible settings capability must open a new stable epoch"
    );
    assert_ne!(
        conversation_context_configuration_revision(&disabled_input).unwrap(),
        conversation_context_configuration_revision(&enabled_a_input).unwrap()
    );

    assert_eq!(
        serde_json::to_vec(enabled_a.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(enabled_b.initial_tool_set.stable_definitions()).unwrap(),
        "rotating a ready capability secret must not rewrite the model-visible contract"
    );
    assert_eq!(
        enabled_a.initial_tool_set.stable_revision(),
        enabled_b.initial_tool_set.stable_revision(),
        "neither secret rotation nor Host executor availability may perturb stable tools"
    );
    assert_eq!(
        conversation_context_configuration_revision(&enabled_a_input).unwrap(),
        conversation_context_configuration_revision(&enabled_b_input).unwrap()
    );
    let serialized_definitions =
        serde_json::to_string(enabled_a.initial_tool_set.stable_definitions()).unwrap();
    assert!(!serialized_definitions.contains("tvly-secret-a"));
    assert!(!serialized_definitions.contains("tvly-secret-b"));
}

#[test]
fn composer_permissions_do_not_change_stable_tools_but_denied_writes_still_fail() {
    let input_with_permissions = |permissions| {
        let mut input = conversation_context_input(vec![message("user", "edit a file")]);
        input.context = Some(AgentRunContext {
            conversation_id: None,
            project_id: None,
            workspace: Some(AgentWorkspaceContext {
                project_id: None,
                display_name: Some("workspace".to_string()),
                root_path: Some("/tmp/workspace".to_string()),
            }),
            attachment_library: None,
            permissions,
        });
        input
    };
    let default_permissions = AgentPermissions {
        read: AgentReadPermission::WorkspaceOnly,
        write: AgentWritePermission::WorkspaceOnly,
        command: AgentCommandPermission::RequireApproval,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::RequireApproval,
    };
    let full_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::All,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::FullAccess,
        patch: AgentPatchPermission::AutoApprove,
    };
    let custom_permissions = AgentPermissions {
        read: AgentReadPermission::All,
        write: AgentWritePermission::Denied,
        command: AgentCommandPermission::AutoApprove,
        command_safety: AgentCommandSafetyPolicy::Guarded,
        patch: AgentPatchPermission::AutoApprove,
    };

    let default = prepare_runtime_capabilities(
        &input_with_permissions(default_permissions),
        "stable-default",
        &[],
        true,
        None,
    )
    .unwrap();
    let full = prepare_runtime_capabilities(
        &input_with_permissions(full_permissions),
        "stable-full",
        &[],
        true,
        None,
    )
    .unwrap();
    let denied_input = input_with_permissions(custom_permissions);
    let custom =
        prepare_runtime_capabilities(&denied_input, "stable-custom", &[], true, None).unwrap();

    let default_bytes = serde_json::to_vec(default.initial_tool_set.stable_definitions()).unwrap();
    assert_eq!(
        default_bytes,
        serde_json::to_vec(full.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default_bytes,
        serde_json::to_vec(custom.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        full.initial_tool_set.stable_revision()
    );
    assert_eq!(
        default.initial_tool_set.stable_revision(),
        custom.initial_tool_set.stable_revision()
    );
    for name in ["apply_patch", "write_file", "run_command"] {
        assert!(
            custom.initial_tool_set.contains(name),
            "{name} must remain in the stable prefix"
        );
    }

    let error = custom
        .tool_registry
        .proposed_action(
            &ToolExecutionContext::from_run_context(denied_input.context.as_ref()),
            &AgentToolCall {
                id: "denied-stable-write".to_string(),
                tool: "apply_patch".to_string(),
                args: json!({
                    "operation": "create",
                    "filePath": "denied.txt",
                    "content": "must not be written"
                }),
                approval_status: AgentApprovalStatus::NotRequired,
                reason: None,
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("denied"));
}

#[test]
fn conversation_identity_does_not_change_the_stable_history_tool() {
    let input_with_conversation = |conversation_id: Option<&str>| {
        let mut input = conversation_context_input(vec![message("user", "find earlier evidence")]);
        input.context = Some(AgentRunContext {
            conversation_id: conversation_id.map(str::to_string),
            project_id: None,
            workspace: None,
            attachment_library: None,
            permissions: AgentPermissions {
                read: AgentReadPermission::WorkspaceOnly,
                write: AgentWritePermission::WorkspaceOnly,
                command: AgentCommandPermission::RequireApproval,
                command_safety: AgentCommandSafetyPolicy::Guarded,
                patch: AgentPatchPermission::RequireApproval,
            },
        });
        input
    };
    let without_input = input_with_conversation(None);
    let without = prepare_runtime_capabilities(
        &without_input,
        "stable-without-conversation",
        &[],
        true,
        None,
    )
    .unwrap();
    let with_input = input_with_conversation(Some("conversation-1"));
    let with =
        prepare_runtime_capabilities(&with_input, "stable-with-conversation", &[], true, None)
            .unwrap();

    assert!(without.initial_tool_set.contains("conversation_history"));
    assert!(with.initial_tool_set.contains("conversation_history"));
    assert_eq!(
        serde_json::to_vec(without.initial_tool_set.stable_definitions()).unwrap(),
        serde_json::to_vec(with.initial_tool_set.stable_definitions()).unwrap()
    );
    assert_eq!(
        without.initial_tool_set.stable_revision(),
        with.initial_tool_set.stable_revision()
    );

    let result = without.tool_registry.execute(
        &ToolExecutionContext::from_run_context(without_input.context.as_ref()),
        &AgentToolCall {
            id: "history-without-conversation".to_string(),
            tool: "conversation_history".to_string(),
            args: json!({ "action": "search", "query": "evidence" }),
            approval_status: AgentApprovalStatus::NotRequired,
            reason: None,
        },
    );
    assert!(!result.ok);
    assert!(result.error.is_some());
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

    for name in ["apply_patch", "write_file"] {
        let manual = manual
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        let automatic = automatic
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        let without_host = without_host
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert!(manual.requires_approval);
        assert_eq!(
            serde_json::to_value(manual).unwrap(),
            serde_json::to_value(automatic).unwrap()
        );
        assert_eq!(
            serde_json::to_value(manual).unwrap(),
            serde_json::to_value(without_host).unwrap()
        );
    }

    for name in [
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
        let automatic = automatic
            .iter()
            .find(|definition| definition.name == name)
            .unwrap();
        assert!(!automatic.requires_approval);
        assert_eq!(
            automatic.approval_mode,
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
fn write_denied_keeps_stable_writers_but_filters_dynamic_write_only_tools() {
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

    for name in ["apply_patch", "write_file"] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
    assert!(!definitions
        .iter()
        .any(|definition| definition.name == "skills_materialize_resource"));
    for name in [
        "office_document",
        "office_spreadsheet",
        "office_presentation",
    ] {
        assert!(definitions.iter().any(|definition| definition.name == name));
    }
}

#[test]
fn unavailable_tool_errors_distinguish_activation_permissions_and_runtime_capabilities() {
    let registry = ToolRegistry::defaults_with_search(None);
    let definitions = registry.definitions();

    let inactive = registry
        .effective_tool_set(definitions.clone(), &BTreeSet::new())
        .unwrap();
    let activation_error = unavailable_tool_error(&inactive, "office_document");
    assert_eq!(
        activation_error.code(),
        Some("agent.tool_requires_skill_activation")
    );
    assert_eq!(
        activation_error.details().unwrap()["recovery"],
        "activateSkill"
    );

    let document_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::OFFICE_DOCUMENTS_CAPABILITY,
    )]);
    let active_without_engine = registry
        .effective_tool_set(definitions.clone(), &document_capabilities)
        .unwrap();
    let runtime_error = unavailable_tool_error(&active_without_engine, "office_document");
    assert_eq!(
        runtime_error.code(),
        Some("agent.tool_runtime_capability_unavailable")
    );
    assert_eq!(
        runtime_error.details().unwrap()["code"],
        "toolRuntimeCapabilityUnavailable"
    );
    assert_eq!(
        runtime_error.details().unwrap()["recovery"],
        "configureCapability"
    );

    let permitted_without_script_execution = definitions
        .into_iter()
        .filter(|definition| definition.name != "skills_run_script")
        .collect::<Vec<_>>();
    let script_capabilities = BTreeSet::from([ToolCapabilityId::application_owned(
        crate::tools::SKILL_SCRIPTS_CAPABILITY,
    )]);
    let permission_filtered = registry
        .effective_tool_set(permitted_without_script_execution, &script_capabilities)
        .unwrap();
    let permission_error = unavailable_tool_error(&permission_filtered, "skills_run_script");
    assert_eq!(
        permission_error.code(),
        Some("agent.tool_blocked_by_permissions")
    );
    assert_eq!(
        permission_error.details().unwrap()["recovery"],
        "changePermissions"
    );
    assert_eq!(permission_error.details().unwrap()["bypassAllowed"], false);

    let unknown_error = unavailable_tool_error(&inactive, "invented_tool");
    assert_eq!(unknown_error.code(), Some("agent.tool_not_registered"));
    assert_eq!(
        unknown_error.details().unwrap()["recovery"],
        "useAvailableTool"
    );
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
    let output = AgentRuntime::default()
        .send_chat_with_events_and_cancellation(
            input,
            None,
            None,
            AgentCancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    server.await.unwrap();

    assert_eq!(output.content, "done");
    let call = output
        .events
        .iter()
        .find_map(|event| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "skills_preflight_script" => {
                Some(call)
            }
            _ => None,
        })
        .expect("the unavailable tool call remains observable");
    assert_runtime_owned_tool_call_id(&call.id);
    assert_eq!(call.reason, None);
    let request = String::from_utf8(second_request.lock().unwrap().clone()).unwrap();
    assert!(request.contains(&call.id));
    assert!(!request.contains("hidden-tool-call"));
    assert!(request.contains("agent.tool_requires_skill_activation"));
    assert!(request.contains("toolRequiresSkillActivation"));
    assert!(request.contains("activateSkill"));
    assert!(request.contains("skill.scripts"));
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
    let (call_index, call_id) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                Some((index, call.id.clone()))
            }
            _ => None,
        })
        .expect("read_image tool call event");
    assert_runtime_owned_tool_call_id(&call_id);
    let (result_index, result) = output
        .events
        .iter()
        .enumerate()
        .find_map(|(index, event)| match event {
            AgentEvent::ToolResult { result, .. } if result.call_id == call_id => {
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
            message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
        })
        .expect("assistant tool call in provider payload");
    let tool_result_index = messages
        .iter()
        .position(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        .expect("paired tool result in provider payload");
    assert!(tool_call_index < tool_result_index);
    assert!(!serde_json::to_string(messages)
        .unwrap()
        .contains("read-image-unsupported"));

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

        let call_id = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolCall { call, .. } if call.tool == "read_image" => {
                    Some(call.id.clone())
                }
                _ => None,
            })
            .expect("read_image call event");
        assert_runtime_owned_tool_call_id(&call_id);
        let event_result = output
            .events
            .iter()
            .find_map(|event| match event {
                AgentEvent::ToolResult { result, .. } if result.call_id == call_id => Some(result),
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
                call_id: trace_call_id,
                truncated: true,
                ..
            } if trace_call_id == &call_id
        )));
        assert!(trace.items.iter().any(|item| matches!(
            item,
            ConversationTurnTraceItem::ToolCall {
                call_id: trace_call_id,
                ..
            } if trace_call_id == &call_id
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
                        message["role"] == "assistant" && message["tool_calls"][0]["id"] == call_id
                    })
                    .expect("OpenAI assistant tool call");
                let tool_result_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "tool" && message["tool_call_id"] == call_id
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
                                parts
                                    .iter()
                                    .any(|part| part["type"] == "tool_use" && part["id"] == call_id)
                            })
                    })
                    .expect("Anthropic assistant tool_use");
                let result_and_image_index = messages
                    .iter()
                    .position(|message| {
                        message["role"] == "user"
                            && message["content"].as_array().is_some_and(|parts| {
                                parts.iter().any(|part| {
                                    part["type"] == "tool_result" && part["tool_use_id"] == call_id
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
        assert!(!serde_json::to_string(messages)
            .unwrap()
            .contains("read-image-success"));
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

    let context_call_id =
        crate::llm::model_response_tool_call_id("run-context", 0, 0, "call-context");
    assert_runtime_owned_tool_call_id(&context_call_id);
    let call = ConversationTurnTraceItem::ToolCall {
        sequence: 1,
        call_id: context_call_id.clone(),
        tool: "read_file".to_string(),
        operation: json!({ "path": "src/lib.rs" }),
        approval_status: AgentApprovalStatus::NotRequired,
        truncated: false,
    };
    let result = ConversationTurnTraceItem::ToolResult {
        sequence: 2,
        call_id: context_call_id,
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

    let mut stable_input = input.clone();
    stable_input.skill_activation = None;
    stable_input.skill_discovery = None;
    let capabilities =
        prepare_runtime_capabilities(&stable_input, "skill-overlay", &[], true, None).unwrap();
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
    let mut dynamic_tool = capabilities
        .tool_definitions
        .iter()
        .find(|definition| definition.name == "read_file")
        .cloned()
        .unwrap();
    dynamic_tool.name = "skill_dynamic_test_tool".to_string();
    dynamic_tool.description =
        "A deliberately verbose Skill-gated Tool schema used for context capacity testing."
            .to_string();
    let dynamic_projection = AgentContextWindowToolProjection::new(
        "stable-test-revision".to_string(),
        "dynamic-test-revision".to_string(),
        "effective-test-revision".to_string(),
        vec![dynamic_tool.clone()],
    );
    let dynamic_preview =
        inspect_context_window_with_tool_projection(input.clone(), &dynamic_projection)
            .unwrap()
            .unwrap();
    assert_eq!(
        dynamic_preview.persistent_revision,
        skill_preview.persistent_revision
    );
    assert!(dynamic_preview.run_transient_input_tokens > skill_preview.run_transient_input_tokens);
    assert!(dynamic_preview.request_input_tokens > skill_preview.request_input_tokens);

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
    let cached_dynamic_skill = durable_state
        .snapshot_with_skill_overlays_and_tool_projection(
            AgentContextWindowPhase::Idle,
            input.skill_discovery.as_ref(),
            input.skill_activation.as_ref(),
            &dynamic_projection,
        )
        .unwrap();
    assert_eq!(
        cached_dynamic_skill.persistent_revision,
        cached_plain.persistent_revision
    );
    assert!(
        cached_dynamic_skill.run_transient_input_tokens > cached_skill.run_transient_input_tokens
    );
    assert!(cached_dynamic_skill.request_input_tokens > cached_skill.request_input_tokens);
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
fn context_preview_counts_only_host_verified_initial_dynamic_tools() {
    use crate::skills::{
        memory_resource_session_for_test, SkillId, SkillPackageUri, SkillResourceKind,
        SkillRevision, SkillSourceId,
    };

    let skill_id = SkillId::parse("workspace:workspace-1:preview-resources").unwrap();
    let revision =
        SkillRevision::parse(format!("skill-package-sha256-v3:{}", "d".repeat(64))).unwrap();
    let source_id = SkillSourceId::parse("workspace:workspace-1").unwrap();
    let resources = Arc::new(
        memory_resource_session_for_test(
            skill_id.clone(),
            revision.clone(),
            source_id,
            vec![(
                "references/guide.md".to_string(),
                SkillResourceKind::Reference,
                b"verified reference".to_vec(),
            )],
        )
        .unwrap(),
    );
    let package = SkillPackageUri::new(skill_id.clone(), revision.clone());
    let instructions = "Read the verified reference before answering.";
    let mut input = conversation_context_input(vec![message("user", "Inspect the reference")]);
    input.skill_activation = Some(AgentSkillActivation {
        activation_revision: "activation-sha256-v1:preview-resources".to_string(),
        skills: vec![AgentActivatedSkill {
            id: skill_id.to_string(),
            name: "preview-resources".to_string(),
            revision: revision.to_string(),
            source: "workspace:workspace-1".to_string(),
            instructions: instructions.to_string(),
            source_bytes: u64::try_from(instructions.len()).unwrap(),
            resources: Some(crate::protocol::AgentActivatedSkillResources {
                root_uri: package.to_string(),
                resource_count: 1,
                kinds: vec!["reference".to_string()],
            }),
        }],
    });

    let without_authority =
        prepare_context_window_tool_projection(&input, &AgentRuntimeHostServices::new(), true)
            .unwrap_err();
    assert!(without_authority
        .to_string()
        .contains("no exact Host package authority"));

    let host_services =
        AgentRuntimeHostServices::new().with_skill_resources(Arc::clone(&resources));
    let projection = prepare_context_window_tool_projection(&input, &host_services, true).unwrap();
    let names = projection
        .dynamic_definitions()
        .iter()
        .map(|definition| definition.name.as_str())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["skills_list_resources", "skills_read_resource"]);

    let conservative = inspect_context_window(input.clone()).unwrap().unwrap();
    let exact = inspect_context_window_with_tool_projection(input, &projection)
        .unwrap()
        .unwrap();
    assert_eq!(exact.persistent_revision, conservative.persistent_revision);
    assert!(exact.run_transient_input_tokens > conservative.run_transient_input_tokens);
    assert!(exact.request_input_tokens > conservative.request_input_tokens);
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
    assert_eq!(
        context_window_snapshots[0].persistent_revision,
        context_window_snapshots[1].persistent_revision,
        "model activation is a run overlay and must not mutate the cache-stable durable prefix"
    );
    assert!(
        context_window_snapshots[1].run_transient_input_tokens
            > context_window_snapshots[0].run_transient_input_tokens,
        "the post-activation request must account for the paired ToolResult, full Skill instructions, dynamic availability notice and unlocked Tool schemas"
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
        .position(|event| matches!(event, AgentEvent::ToolResult { result, .. } if result.call_id == activation_call_id))
        .unwrap();
    let activated_index = events
        .iter()
        .position(|event| matches!(event, AgentEvent::SkillActivated { skill, .. } if skill.id == "bundled:application:documents"))
        .unwrap();
    assert!(result_index < activated_index);
    assert!(!events.iter().any(|event| {
        matches!(event, AgentEvent::ToolCall { call, .. } if call.tool == "read_file")
    }));
    assert!(!serde_json::to_string(&*events)
        .unwrap()
        .contains("activate-documents"));
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
    let steer_input = AgentSteerInputQueue::new();
    let steer_input_during_compaction = steer_input.clone();
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
            let large_attachment_text =
                format!("LARGE_GUIDANCE_ATTACHMENT_MARKER {}", "z".repeat(20_000));
            assert_eq!(
                steer_input_during_compaction
                    .enqueue(crate::AgentSteerInput {
                        guidance_id: "guidance-during-compaction".to_string(),
                        client_message_id: "client-during-compaction".to_string(),
                        content: "Preserve this constraint across compaction.".to_string(),
                        attachments: vec![AgentInputAttachment {
                            id: "attachment-during-compaction".to_string(),
                            kind: AgentInputAttachmentKind::File,
                            name: "large-guidance.txt".to_string(),
                            mime_type: Some("text/plain".to_string()),
                            size_bytes: large_attachment_text.len() as u64,
                            encoding: AgentInputAttachmentEncoding::Utf8,
                            data: large_attachment_text,
                            truncated: None,
                        }],
                        attachment_library: Some(crate::AgentAttachmentLibraryContext {
                            root_path: None,
                            conversation_id: Some("conversation-1".to_string()),
                            project_id: None,
                            conversation_attachments: Vec::new(),
                            project_attachments: Vec::new(),
                        }),
                        created_at: 42,
                    })
                    .unwrap(),
                AgentSteerEnqueueOutcome::Queued
            );
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
                    .with_model_request_observer(model_request_observer)
                    .with_steer_input(steer_input),
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
    assert_eq!(trace_publish_count.load(Ordering::SeqCst), 6);
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
    for request_body in &request_bodies {
        assert!(request_body.contains("COMPACTED_HISTORY_MARKER"));
        assert!(!request_body.contains("OLD_USER_MARKER"));
        assert!(!request_body.contains("OLD_ASSISTANT_MARKER"));
    }
    assert!(!request_bodies[0].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("Preserve this constraint across compaction."));
    assert!(request_bodies[1].contains("LARGE_GUIDANCE_ATTACHMENT_MARKER"));
    assert!(output.events.iter().any(|event| matches!(
        event,
        AgentEvent::GuidanceApplied { guidance_id, .. }
            if guidance_id == "guidance-during-compaction"
    )));
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
            AgentEvent::ToolCall { call, .. }
                if call.tool == "write_file" && call.args["phase"] == "append" =>
            {
                Some(call)
            }
            _ => None,
        })
        .expect("write_file append event");
    assert_runtime_owned_tool_call_id(&append_event.id);
    assert_eq!(append_event.args["content"], "[stored in private draft]");
    assert_eq!(append_event.args["contentBytes"], 28);
    let append_call_id = append_event.id.clone();

    let append_trace = output
        .conversation_turn_trace
        .as_ref()
        .expect("durable conversation trace")
        .items
        .iter()
        .find_map(|item| match item {
            ConversationTurnTraceItem::ToolCall {
                call_id, operation, ..
            } if call_id == &append_call_id => Some(operation),
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
            Some(AgentRuntimeHostServices::new().with_skill_resources(activated_skill_authority())),
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
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
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
        &pending_call_id,
        &queued_call_id,
    ] {
        assert_runtime_owned_tool_call_id(call_id);
    }
    assert_eq!(
        [
            &todo_call_id,
            &read_before_call_id,
            &pending_call_id,
            &queued_call_id,
        ]
        .into_iter()
        .collect::<std::collections::HashSet<_>>()
        .len(),
        4
    );
    for completed_call_id in [&todo_call_id, &read_before_call_id] {
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

    let mut resume_input = base_input;
    resume_input.messages.clear();
    // The checkpoint is authoritative for the logical run; a changed resume payload must not
    // replace the frozen Skill snapshot selected before approval.
    resume_input.skill_activation = Some(activated_skill("SKILL_CHANGED_DURING_RESUME"));
    resume_input.resume_checkpoint = Some(checkpoint);
    resume_input.approval_decision = Some(AgentApprovalDecision {
        action_id: pending_call_id.clone(),
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
    assert_eq!(completed.todo.as_ref().unwrap().revision, 1);
    let request = final_request.lock().unwrap().take().unwrap();
    let messages = serde_json::to_string(&request["messages"]).unwrap();
    assert!(messages.contains("evidence-before-approval"));
    assert!(messages.contains("evidence-from-queued-tool"));
    assert!(messages.contains("user rejected"));
    for call_id in [
        &todo_call_id,
        &read_before_call_id,
        &pending_call_id,
        &queued_call_id,
    ] {
        assert!(messages.contains(call_id.as_str()));
    }
    for provider_call_id in [
        "todo-before",
        "read-before",
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
    let pending_call_id = checkpoint.pending_tool_call_id.clone();
    assert_runtime_owned_tool_call_id(&resource_read_call_id);
    assert_runtime_owned_tool_call_id(&pending_call_id);
    assert_ne!(resource_read_call_id, pending_call_id);
    let model_request_messages = serde_json::to_string(&model_request["messages"]).unwrap();
    assert!(model_request_messages.contains(&resource_read_call_id));
    assert!(!model_request_messages.contains("read-skill-resource"));
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
        action_id: pending_call_id.clone(),
        status: AgentApprovalDecisionStatus::Approved,
        message: None,
    });
    resume_input.tool_continuation = Some(AgentToolContinuation {
        call: AgentToolCall {
            id: pending_call_id.clone(),
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
            call_id: pending_call_id.clone(),
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
    assert!(resumed_messages.contains(&resource_read_call_id));
    assert!(resumed_messages.contains(&pending_call_id));
    assert!(!resumed_messages.contains("read-skill-resource"));
    assert!(!resumed_messages.contains("materialize-after-read"));
    assert!(resumed_messages.contains("applied"));
    assert!(!resumed_messages.contains(RESOURCE_MARKER));
    let trace = completed
        .conversation_turn_trace
        .as_ref()
        .expect("terminal conversation trace");
    for call_id in [&resource_read_call_id, &pending_call_id] {
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
