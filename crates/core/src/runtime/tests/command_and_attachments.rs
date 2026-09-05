use super::*;

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
            runtime_binding: None,
            managed_office_script: None,
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
        collaboration_identity: None,
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

    assert_eq!(messages[0].role().as_str(), "system");
    assert!(messages[0].content().contains("Captain（船长）"));
    assert!(!messages[0].content().contains("image_generation"));
    assert!(!messages[0].content().contains("图片生成"));
    assert!(!messages[0].content().contains("/private/path"));
    assert_eq!(messages[1].role().as_str(), "user");
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
        .any(|message| message.role() == LlmMessageRole::User
            && message.content().contains("hello from attachment")));
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
fn steer_attachment_library_updates_run_world_state_without_granting_conversation_identity() {
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
    let mut input = conversation_context_input(vec![message("user", "inspect attachment")]);
    input.context = Some(run_context);
    let capabilities =
        prepare_runtime_capabilities(&input, "steer-attachment-world-state", &[], true, None)
            .unwrap();
    let tracker = RunWorldStateTracker::new(
        "steer-attachment-world-state",
        &input,
        &capabilities.initial_tool_set,
    )
    .unwrap();
    let rendered = tracker
        .snapshot()
        .model_projection(WorldStateLifetime::Run)
        .unwrap()
        .render_sanitized_text();
    assert!(rendered.contains("\"conversationAttachmentCount\":1"));
    assert!(rendered.contains("\"projectAttachmentCount\":0"));
    assert!(!rendered.contains("conversationAvailable"));
}

#[test]
fn attachment_context_defers_pdf_and_office_files_to_matching_skills() {
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

    for hidden_content in [
        "Guidance PDF",
        "Guidance DOCX",
        "Guidance PPTX",
        "Guidance XLSX",
        "Guidance CSV",
        "Guidance TSV",
        "Guidance MIME CSV",
    ] {
        assert!(
            !context.text.contains(hidden_content),
            "skill-gated document content leaked during attachment preprocessing: {hidden_content}\n{}",
            context.text
        );
    }
    for attachment_id in [
        "attachment-pdf",
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
        7
    );
    assert!(context.text.contains("bundled:application:pdf"));
    assert!(context.text.contains("run_command.inputs"));
    assert!(context.text.contains("read_image"));
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
fn attachment_context_routes_pdf_without_decoding_its_payload() {
    let attachment = AgentInputAttachment {
        id: "attachment-pdf-invalid-payload".to_string(),
        kind: AgentInputAttachmentKind::File,
        name: "manual.pdf".to_string(),
        mime_type: Some("application/pdf".to_string()),
        size_bytes: 10,
        encoding: AgentInputAttachmentEncoding::Base64,
        data: "not-base64".to_string(),
        truncated: None,
    };
    let library = runtime_attachment_library(std::slice::from_ref(&attachment));

    let context = build_attachment_context(&[attachment], Some(&library)).unwrap();

    assert!(context.text.contains("正文未读取"));
    assert!(context.text.contains("bundled:application:pdf"));
    assert!(context.text.contains("run_command.inputs"));
    assert!(context
        .text
        .contains("@attachments/attachment-pdf-invalid-payload/manual.pdf"));
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
        collaboration_identity: None,
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
        exact_archive_file: None,
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
    assert_eq!(image_message.role(), LlmMessageRole::User);
    assert_eq!(image_message.images().len(), 1);
    assert_eq!(image_message.images()[0].mime_type, "image/png");
    assert_eq!(image_message.images()[0].data_base64, "YWJj");
}

#[test]
fn failed_read_image_capability_result_never_creates_visual_input() {
    let result = AgentToolResult {
        exact_archive_file: None,
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
        exact_archive_file: None,
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
    assert_eq!(image_message.role(), LlmMessageRole::User);
    assert!(image_message
        .content()
        .contains("/managed/generated-image.png"));
    assert_eq!(image_message.images().len(), 1);
    assert_eq!(image_message.images()[0].mime_type, "image/png");
    assert_eq!(image_message.images()[0].data_base64, "YWJj");
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
        tool: "apply_patch".to_string(),
        received_bytes: 128,
    });

    assert_eq!(captured.lock().unwrap().len(), 1);
    assert!(stream.into_events().is_empty());
}

#[test]
fn context_window_preview_is_available_independently_of_indicator_events() {
    let mut input = serde_json::from_value::<AgentChatInput>(serde_json::json!({
        "apiUrl": "https://example.test/v1/chat/completions",
        "apiToken": "",
        "model": "test-model",
        "modelCapabilities": { "imageInput": false },
        "contextWindowTokens": 128000,
        "contextWindowIndicatorEnabled": false,
        "messages": []
    }))
    .unwrap();
    input.provider_profile_config = Some(ProviderProfileConfig::generic_for_dialect(
        ProviderProtocolDialect::OpenAiChatCompletions,
    ));

    assert!(inspect_context_window(input).unwrap().is_some());
}
